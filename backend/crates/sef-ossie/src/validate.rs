//! The checks of the official validator (`validation/validate.py`) that a
//! typed parse cannot express: schema constraints (`const`, `minLength`,
//! `minItems`), unique names, relationship references, column arity and key
//! coverage. Expression syntax is checked when expressions are compiled.
//!
//! One deliberate difference: names are compared as **normalized
//! identifiers** (Ossie: unquoted identifiers are case-insensitive), so
//! `orders` and `Orders` collide here although `validate.py` compares raw
//! strings. Both would resolve to the same dataset in an expression.

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostic::{Code, Diagnostic};
use crate::document::{Expression, SPEC_VERSION, SemanticModel};

/// Every error and warning of `model`, errors first in document order.
#[must_use]
pub fn validate(model: &SemanticModel) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    if model.version != SPEC_VERSION {
        found.push(Diagnostic::error(
            Code::UnsupportedVersion,
            "/version",
            format!(
                "Versión {:?} no admitida; se espera {SPEC_VERSION:?}.",
                model.version
            ),
        ));
    }
    non_empty(&mut found, "/name", &model.name, "el nombre del modelo");
    if model.datasets.is_empty() {
        found.push(Diagnostic::error(
            Code::EmptyList,
            "/datasets",
            "El modelo necesita al menos un dataset.",
        ));
    }
    for (i, dataset) in model.datasets.iter().enumerate() {
        let base = format!("/datasets/{i}");
        non_empty(
            &mut found,
            &format!("{base}/name"),
            &dataset.name,
            "el nombre del dataset",
        );
        non_empty(
            &mut found,
            &format!("{base}/source"),
            &dataset.source,
            "el origen del dataset",
        );
        for (j, field) in dataset.fields.iter().enumerate() {
            let path = format!("{base}/fields/{j}");
            non_empty(
                &mut found,
                &format!("{path}/name"),
                &field.name,
                "el nombre del campo",
            );
            expression(&mut found, &format!("{path}/expression"), &field.expression);
        }
        duplicates(
            &mut found,
            dataset.fields.iter().map(|field| field.name.as_str()),
            &format!("{base}/fields"),
            &format!("campo del dataset {:?}", dataset.name),
        );
    }
    duplicates(
        &mut found,
        model.datasets.iter().map(|dataset| dataset.name.as_str()),
        "/datasets",
        "dataset",
    );
    for (i, metric) in model.metrics.iter().enumerate() {
        let path = format!("/metrics/{i}");
        non_empty(
            &mut found,
            &format!("{path}/name"),
            &metric.name,
            "el nombre de la métrica",
        );
        expression(
            &mut found,
            &format!("{path}/expression"),
            &metric.expression,
        );
    }
    duplicates(
        &mut found,
        model.metrics.iter().map(|metric| metric.name.as_str()),
        "/metrics",
        "métrica",
    );
    relationships(&mut found, model);
    found.sort_by_key(|diagnostic| !diagnostic.is_error());
    found
}

/// Ossie's normalized identifier: an unquoted identifier is upper-cased; a
/// quoted one (`"…"`) loses its quotes and has `""` unescaped, keeping case.
#[must_use]
pub fn normalize_identifier(identifier: &str) -> String {
    match identifier
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        Some(quoted) => quoted.replace("\"\"", "\""),
        None => identifier.to_uppercase(),
    }
}

fn non_empty(found: &mut Vec<Diagnostic>, path: &str, value: &str, what: &str) {
    if value.is_empty() {
        found.push(Diagnostic::error(
            Code::EmptyValue,
            path,
            format!("Falta {what}: no puede estar vacío."),
        ));
    }
}

fn expression(found: &mut Vec<Diagnostic>, path: &str, expression: &Expression) {
    if expression.dialects.is_empty() {
        found.push(Diagnostic::error(
            Code::EmptyList,
            format!("{path}/dialects"),
            "La expresión necesita al menos un dialecto.",
        ));
    }
}

/// One error per repeated name, at its first repetition.
fn duplicates<'a>(
    found: &mut Vec<Diagnostic>,
    names: impl Iterator<Item = &'a str>,
    path: &str,
    what: &str,
) {
    let mut seen = BTreeSet::new();
    let mut reported = BTreeSet::new();
    for (i, name) in names.enumerate() {
        let normalized = normalize_identifier(name);
        if !seen.insert(normalized.clone()) && reported.insert(normalized) {
            found.push(Diagnostic::error(
                Code::DuplicateName,
                format!("{path}/{i}/name"),
                format!("Nombre de {what} repetido: {name:?}."),
            ));
        }
    }
}

fn relationships(found: &mut Vec<Diagnostic>, model: &SemanticModel) {
    let datasets: BTreeMap<String, _> = model
        .datasets
        .iter()
        .map(|dataset| (normalize_identifier(&dataset.name), dataset))
        .collect();
    for (i, relationship) in model.relationships.iter().enumerate() {
        let base = format!("/relationships/{i}");
        non_empty(
            found,
            &format!("{base}/name"),
            &relationship.name,
            "el nombre de la relación",
        );
        for (end, dataset) in [("from", &relationship.from), ("to", &relationship.to)] {
            non_empty(
                found,
                &format!("{base}/{end}"),
                dataset,
                "el dataset de la relación",
            );
            if !dataset.is_empty() && !datasets.contains_key(&normalize_identifier(dataset)) {
                found.push(Diagnostic::error(
                    Code::UnknownDataset,
                    format!("{base}/{end}"),
                    format!(
                        "La relación {:?} usa el dataset {dataset:?}, que el modelo no declara.",
                        relationship.name
                    ),
                ));
            }
        }
        for (side, columns) in [
            ("from_columns", &relationship.from_columns),
            ("to_columns", &relationship.to_columns),
        ] {
            if columns.is_empty() {
                found.push(Diagnostic::error(
                    Code::EmptyList,
                    format!("{base}/{side}"),
                    "La relación necesita al menos una columna en cada lado.",
                ));
            }
        }
        let (from, to) = (&relationship.from_columns, &relationship.to_columns);
        if !from.is_empty() && !to.is_empty() && from.len() != to.len() {
            found.push(Diagnostic::error(
                Code::ColumnArity,
                base.clone(),
                format!(
                    "La relación {:?} empareja {} columnas con {}.",
                    relationship.name,
                    from.len(),
                    to.len()
                ),
            ));
        }
        if let Some(target) = datasets.get(&normalize_identifier(&relationship.to)) {
            key_coverage(found, &base, &relationship.name, target, to);
        }
    }
    duplicates(
        found,
        model
            .relationships
            .iter()
            .map(|relationship| relationship.name.as_str()),
        "/relationships",
        "relación",
    );
}

/// A warning, as in `validate.py`: `to_columns` should cover a declared key
/// of the `to` dataset (a superset still guarantees many-to-one). Datasets
/// without declared keys are skipped.
fn key_coverage(
    found: &mut Vec<Diagnostic>,
    base: &str,
    name: &str,
    target: &crate::document::Dataset,
    to_columns: &[String],
) {
    let covered: BTreeSet<String> = to_columns.iter().map(|c| normalize_identifier(c)).collect();
    let keys: Vec<&Vec<String>> = target
        .primary_key
        .iter()
        .chain(target.unique_keys.iter().flatten())
        .filter(|key| !key.is_empty())
        .collect();
    let any_covered = keys.iter().any(|key| {
        key.iter()
            .all(|column| covered.contains(&normalize_identifier(column)))
    });
    if !keys.is_empty() && !to_columns.is_empty() && !any_covered {
        found.push(Diagnostic::warning(
            Code::KeyNotCovered,
            format!("{base}/to_columns"),
            format!(
                "La relación {name:?} no cubre ninguna clave declarada de {:?}: \
                 no se puede garantizar que sea muchos-a-uno.",
                target.name
            ),
        ));
    }
}
