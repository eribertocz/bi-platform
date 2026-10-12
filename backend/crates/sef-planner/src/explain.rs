//! The readable explanation of a plan (§26): its SEIR steps, then the
//! contract of every measure, metric and dimension it reads or returns —
//! exact type and unit — resolved against the compiled model, so what the
//! plan fingerprints can also be inspected.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use sef_exec_ir::SemanticPlan;
use sef_model_ir::identity::StableId;
use sef_model_ir::model::CompiledSemanticModel;
use sef_model_ir::time::TimestampSemantics;
use sef_model_ir::types::ValueType;
use sef_model_ir::units::Unit;

/// The plan's steps followed by one `contract` line per measure, metric
/// and dimension it uses, in canonical order.
#[must_use]
pub fn explain(model: &CompiledSemanticModel, plan: &SemanticPlan) -> String {
    let mut text = plan.explain();
    let roots: Vec<_> = plan.blocks.iter().map(|block| block.root).collect();
    let objects: BTreeSet<StableId> = plan
        .graph
        .topological(&roots)
        .iter()
        .flat_map(|node| node.operator.objects())
        .map(|object| object.id.clone())
        .collect();
    for id in objects {
        if let Some((value_type, unit)) = contract(model, &id) {
            let unit = unit.map_or_else(|| "sin unidad".to_owned(), unit_text);
            // Writing into a String cannot fail.
            let _ = writeln!(text, "contract {id} {} {unit}", type_text(value_type));
        }
    }
    text
}

fn contract<'m>(
    model: &'m CompiledSemanticModel,
    id: &StableId,
) -> Option<(&'m ValueType, Option<&'m Unit>)> {
    let definition = model.definition();
    definition
        .measures
        .iter()
        .find(|measure| measure.metadata.id() == id)
        .map(|measure| (&measure.value_type, measure.unit.as_ref()))
        .or_else(|| {
            definition
                .metrics
                .iter()
                .find(|metric| metric.metadata.id() == id)
                .map(|metric| (&metric.value_type, metric.unit.as_ref()))
        })
        .or_else(|| {
            definition
                .dimensions
                .iter()
                .find(|dimension| dimension.metadata.id() == id)
                .map(|dimension| (&dimension.value_type, dimension.unit.as_ref()))
        })
}

/// The SQL-style name of an exact type, as a reader expects it.
fn type_text(value_type: &ValueType) -> String {
    match value_type {
        ValueType::Boolean => "BOOLEAN".to_owned(),
        ValueType::Integer {
            signed,
            bits: Some(bits),
        } => format!("{}INT{bits}", if *signed { "" } else { "U" }),
        ValueType::Integer { bits: None, .. } => "INTEGER".to_owned(),
        ValueType::Decimal(decimal) => {
            format!("DECIMAL({},{})", decimal.precision(), decimal.scale())
        }
        ValueType::Float { bits } => format!("FLOAT{bits}"),
        ValueType::Utf8 => "VARCHAR".to_owned(),
        ValueType::Binary => "BINARY".to_owned(),
        ValueType::Date => "DATE".to_owned(),
        ValueType::Time => "TIME".to_owned(),
        ValueType::Timestamp {
            timezone_semantics: TimestampSemantics::Instant,
        } => "TIMESTAMPTZ".to_owned(),
        ValueType::Timestamp { .. } => "TIMESTAMP".to_owned(),
        ValueType::Duration => "INTERVAL".to_owned(),
        ValueType::Uuid => "UUID".to_owned(),
        ValueType::Identifier { entity_key } => format!("IDENTIFIER({entity_key})"),
        ValueType::Struct(fields) => format!("STRUCT[{}]", fields.len()),
        ValueType::List(item) => format!("LIST({})", type_text(item)),
        ValueType::OpaqueExtension {
            namespace,
            type_name,
        } => format!("{namespace}.{type_name}"),
    }
}

fn unit_text(unit: &Unit) -> String {
    match unit {
        Unit::Currency { iso4217 } => iso4217.clone(),
        Unit::Physical { ucum } => ucum.clone(),
        Unit::Ratio => "ratio".to_owned(),
        Unit::Percent => "%".to_owned(),
        Unit::Count => "count".to_owned(),
        Unit::Custom { namespace, code } => format!("{namespace}:{code}"),
    }
}
