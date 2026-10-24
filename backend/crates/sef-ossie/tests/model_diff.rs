//! What changes between two versions of a model (#99), from real Ossie
//! documents: meaning, presentation and governance told apart, renames
//! recognised by meaning, and what depends on a change.
#![allow(clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use proptest::prelude::*;
use sef_model_ir::diff::{Aspect, Change, ModelDiff, diff};
use sef_model_ir::identity::{SourceHandle, StableId};
use sef_model_ir::model::{CompiledSemanticModel, ModelDefinition};
use sef_model_ir::types::{DecimalType, ValueType};
use sef_ossie::catalog::{Catalog, Column, Relation};
use sef_ossie::csmir::{Options, to_csmir};

const SALES: &str = r"
version: '0.2.0.dev0'
name: Ventas
datasets:
  - name: sales
    source: sales
    primary_key: [sale_id]
    fields:
      - name: sale_id
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: sale_id
      - name: amount
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: amount
      - name: qty
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: qty
      - name: region
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: region
        dimension: {}
metrics:
  - name: importe
    description: Suma del importe.
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(sales.amount)
  - name: unidades
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(sales.qty)
  - name: doble
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: importe + importe
";

fn decimal(precision: u8, scale: u8) -> ValueType {
    ValueType::Decimal(DecimalType::new(precision, scale).expect("decimal"))
}

fn sales_catalog(amount: ValueType) -> Catalog {
    let column = |name: &str, value_type| Column {
        name: name.to_owned(),
        handle: SourceHandle::new(name).expect("handle"),
        value_type,
        nullable: false,
    };
    let int64 = ValueType::Integer {
        signed: true,
        bits: Some(64),
    };
    Catalog {
        relations: BTreeMap::from([(
            "sales".to_owned(),
            Relation {
                source: SourceHandle::new("test").expect("handle"),
                relation: SourceHandle::new("sales").expect("handle"),
                columns: vec![
                    column("sale_id", int64.clone()),
                    column("amount", amount),
                    column("qty", int64),
                    column("region", ValueType::Utf8),
                ],
            },
        )]),
    }
}

fn options() -> Options {
    Options {
        namespace: "test".to_owned(),
        model: "ventas".to_owned(),
    }
}

fn definition(text: &str, catalog: &Catalog) -> ModelDefinition {
    let model = sef_ossie::load(text).expect("loads").model;
    to_csmir(&model, catalog, &options()).unwrap_or_else(|errors| panic!("converts: {errors:#?}"))
}

fn model(text: &str) -> CompiledSemanticModel {
    compiled(definition(text, &sales_catalog(decimal(18, 2))))
}

fn compiled(definition: ModelDefinition) -> CompiledSemanticModel {
    definition
        .compile()
        .unwrap_or_else(|errors| panic!("compiles: {errors:#?}"))
}

/// The change of the object whose identifier ends in `/<kind>/<name>`.
fn change<'d>(diff: &'d ModelDiff, kind: &str, name: &str) -> &'d Change {
    let suffix = format!("/{kind}/{name}");
    &diff
        .changes
        .iter()
        .find(|entry| entry.id.as_str().ends_with(&suffix))
        .unwrap_or_else(|| panic!("no change for {suffix}: {diff:#?}"))
        .change
}

fn meaning(change: &Change) -> BTreeSet<Aspect> {
    match change {
        Change::Modified { meaning, .. } => meaning.clone(),
        other => panic!("expected a modification, got {other:?}"),
    }
}

fn names(ids: &BTreeSet<StableId>) -> BTreeSet<String> {
    ids.iter()
        .map(|id| {
            id.as_str()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

#[test]
fn identical_versions_have_no_differences() {
    assert!(diff(&model(SALES), &model(SALES)).is_empty());
    // Reordering the document changes no meaning either.
    let importe = "  - name: importe\n    description: Suma del importe.\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: SUM(sales.amount)\n";
    let unidades = "  - name: unidades\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: SUM(sales.qty)\n";
    let reordered = SALES.replace(
        &format!("{importe}{unidades}"),
        &format!("{unidades}{importe}"),
    );
    assert_ne!(reordered, SALES);
    assert!(diff(&model(SALES), &model(&reordered)).is_empty());
}

#[test]
fn an_expression_change_is_one_of_meaning_and_reaches_what_depends_on_it() {
    let changed = diff(
        &model(SALES),
        &model(&SALES.replace("SUM(sales.amount)", "MAX(sales.amount)")),
    );
    // The aggregation lives in the measure; the metric on it now has
    // another contract (MAX is not additive).
    assert!(
        meaning(change(&changed, "measure", "importe")).contains(&Aspect::Expression),
        "{changed:#?}"
    );
    assert!(
        meaning(change(&changed, "metric", "importe")).contains(&Aspect::Contract),
        "{changed:#?}"
    );
    // `doble` is defined on `importe`. MAX keeps the field's type while SUM
    // widens it (#261), so its own derived type follows: a contract change.
    assert_eq!(
        meaning(change(&changed, "metric", "doble")),
        BTreeSet::from([Aspect::Contract]),
        "{changed:#?}"
    );
    // An expression change that keeps every type only affects `doble`.
    let same_type = diff(
        &model(SALES),
        &model(&SALES.replace("SUM(sales.amount)", "SUM(sales.amount + sales.amount)")),
    );
    let Change::Affected { through } = change(&same_type, "metric", "doble") else {
        panic!("doble: {same_type:#?}");
    };
    assert!(names(through).contains("importe"), "{same_type:#?}");
    // What does not read `amount` is untouched.
    assert!(
        changed
            .changes
            .iter()
            .all(|entry| !entry.id.as_str().ends_with("/unidades")),
        "{changed:#?}"
    );
}

#[test]
fn a_contract_change_is_told_apart_from_an_expression_change() {
    let before = model(SALES);
    let after = compiled(definition(SALES, &sales_catalog(decimal(20, 4))));
    let changed = diff(&before, &after);
    let amount = change(&changed, "field", "sales.amount");
    assert_eq!(meaning(amount), BTreeSet::from([Aspect::Contract]));
    // The metric computes the same thing on a wider type.
    for (kind, name) in [
        ("measure", "importe"),
        ("metric", "importe"),
        ("metric", "doble"),
    ] {
        assert_eq!(
            meaning(change(&changed, kind, name)),
            BTreeSet::from([Aspect::Contract]),
            "{kind} {name}: {changed:#?}"
        );
    }
    // `unidades` reads `qty`, not `amount`.
    assert!(
        changed
            .changes
            .iter()
            .all(|entry| !entry.id.as_str().ends_with("/unidades")),
        "{changed:#?}"
    );
}

#[test]
fn a_description_change_is_presentation_only() {
    let changed = diff(
        &model(SALES),
        &model(&SALES.replace("Suma del importe.", "Importe vendido.")),
    );
    // The metric and the measure it compiles to share the description.
    let cosmetic = Change::Modified {
        renamed_from: None,
        meaning: BTreeSet::new(),
        presentation: true,
        governance: false,
    };
    assert_eq!(*change(&changed, "metric", "importe"), cosmetic);
    assert!(
        changed.changes.iter().all(|entry| entry.change == cosmetic),
        "{changed:#?}"
    );
}

#[test]
fn a_governance_change_is_neither_meaning_nor_presentation() {
    let governed = SALES.replace(
        "    description: Suma del importe.\n",
        "    description: Suma del importe.\n    custom_extensions:\n      - vendor_name: bi-platform\n        data: '{\"version\": 1, \"owner\": \"finanzas\", \"sensitivity\": \"confidential\"}'\n",
    );
    let changed = diff(&model(SALES), &model(&governed));
    assert_eq!(changed.changes.len(), 1, "{changed:#?}");
    assert_eq!(
        *change(&changed, "metric", "importe"),
        Change::Modified {
            renamed_from: None,
            meaning: BTreeSet::new(),
            presentation: false,
            governance: true,
        }
    );
}

#[test]
fn a_rename_without_change_of_meaning_is_recognised() {
    let renamed = SALES
        .replace("  - name: importe\n", "  - name: ventas\n")
        .replace("importe + importe", "ventas + ventas");
    let changed = diff(&model(SALES), &model(&renamed));
    let ventas = change(&changed, "metric", "ventas");
    let Change::Modified {
        renamed_from: Some(from),
        meaning,
        ..
    } = ventas
    else {
        panic!("ventas: {changed:#?}");
    };
    assert!(from.as_str().ends_with("/importe"), "{from}");
    assert!(meaning.is_empty(), "{changed:#?}");
    // `doble` now reads `ventas`: the same thing, so no change at all.
    assert!(
        changed
            .changes
            .iter()
            .all(|entry| !entry.id.as_str().ends_with("/doble")),
        "{changed:#?}"
    );
    // An old reader of `importe` finds where it went.
    let old = StableId::new(from.as_str()).expect("id");
    assert!(
        changed
            .of(&old)
            .is_some_and(|entry| entry.id == *ventas_id(&changed))
    );
}

fn ventas_id(diff: &ModelDiff) -> &StableId {
    &diff
        .changes
        .iter()
        .find(|entry| entry.id.as_str().ends_with("/metric/ventas"))
        .expect("ventas")
        .id
}

#[test]
fn an_ambiguous_rename_is_never_guessed() {
    // `importe` disappears and two identical metrics appear: which one it
    // became cannot be known, so neither is claimed.
    let twins = SALES
        .replace("  - name: importe\n", "  - name: gemelo_a\n")
        .replace(
            "  - name: unidades\n",
            "  - name: gemelo_b\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: SUM(sales.amount)\n  - name: unidades\n",
        )
        .replace("importe + importe", "gemelo_a + gemelo_a")
        .replace("    description: Suma del importe.\n", "");
    let changed = diff(&model(SALES), &model(&twins));
    assert_eq!(
        *change(&changed, "metric", "importe"),
        Change::Removed,
        "{changed:#?}"
    );
    assert_eq!(
        *change(&changed, "metric", "gemelo_a"),
        Change::Added,
        "{changed:#?}"
    );
    assert_eq!(
        *change(&changed, "metric", "gemelo_b"),
        Change::Added,
        "{changed:#?}"
    );
}

#[test]
fn removed_and_added_objects_are_reported() {
    let without = SALES.replace(
        "  - name: unidades\n    expression:\n      dialects:\n        - dialect: OSSIE_SQL_2026\n          expression: SUM(sales.qty)\n",
        "",
    );
    let removed = diff(&model(SALES), &model(&without));
    assert_eq!(
        *change(&removed, "metric", "unidades"),
        Change::Removed,
        "{removed:#?}"
    );
    let added = diff(&model(&without), &model(SALES));
    assert_eq!(
        *change(&added, "metric", "unidades"),
        Change::Added,
        "{added:#?}"
    );
}

// ---- References are typed: handles and literals are never identifiers ---------

#[test]
fn a_source_handle_equal_to_an_identifier_is_no_reference() {
    // `qty` is read through a handle spelled like the `amount` field's ID.
    let mut before = definition(SALES, &sales_catalog(decimal(18, 2)));
    let amount = before.datasets[0].fields[1].metadata.id().clone();
    let qty = before.datasets[0].fields[2].metadata.id().clone();
    let handle = SourceHandle::new(amount.as_str()).expect("handle");
    before.datasets[0].fields[2].binding = handle.clone();
    for binding in &mut before.source_bindings[0].field_bindings {
        if binding.field == qty {
            binding.handle = handle.clone();
        }
    }
    let mut after = before.clone();
    after.datasets[0].fields[1].nullable = true;
    let changed = diff(&compiled(before), &compiled(after));
    for untouched in ["/field/sales.qty", "/measure/unidades", "/metric/unidades"] {
        assert!(
            changed
                .changes
                .iter()
                .all(|entry| !entry.id.as_str().ends_with(untouched)),
            "{untouched}: {changed:#?}"
        );
    }
}

#[test]
fn a_rename_never_rewrites_a_source_handle() {
    // The source's ID and its connection handle change together: that is a
    // new connection, not the same source under another name.
    let mut before = definition(SALES, &sales_catalog(decimal(18, 2)));
    let old = before.source_bindings[0].metadata.id().clone();
    before.source_bindings[0].source_id = SourceHandle::new(old.as_str()).expect("handle");
    let mut after = before.clone();
    let new = StableId::new(format!("{}/source/another", old.namespace())).expect("id");
    after.source_bindings[0].metadata.identity.stable_id = new.clone();
    after.source_bindings[0].source_id = SourceHandle::new(new.as_str()).expect("handle");
    after.datasets[0].source_binding = new;
    let changed = diff(&compiled(before), &compiled(after));
    assert_eq!(
        changed.of(&old).expect("change").change,
        Change::Removed,
        "{changed:#?}"
    );
}

#[test]
fn a_literal_equal_to_an_identifier_is_no_reference() {
    let labelled = SALES.replace(
        "      - name: region\n",
        "      - name: etiqueta\n        expression:\n          dialects:\n            - dialect: OSSIE_SQL_2026\n              expression: \"'test/field/sales.amount'\"\n        dimension: {}\n      - name: region\n",
    );
    let before = definition(&labelled, &sales_catalog(decimal(18, 2)));
    let mut after = before.clone();
    after.datasets[0].fields[1].nullable = true;
    let changed = diff(&compiled(before), &compiled(after));
    assert!(
        changed
            .changes
            .iter()
            .any(|entry| entry.id.as_str().ends_with("/field/sales.amount")),
        "{changed:#?}"
    );
    assert!(
        changed
            .changes
            .iter()
            .all(|entry| !entry.id.as_str().ends_with("etiqueta")),
        "{changed:#?}"
    );
}

// ---- Policies, from the normative corpus (CASE-015) ----------------------------

fn corpus(file: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(file);
    std::fs::read_to_string(path).expect("fixture")
}

/// A corpus column type: `int64`, `utf8`, `date`, `timestamptz` or
/// `decimal(p,s)`, optionally followed by ` null`.
fn column_type(spec: &str) -> (ValueType, bool) {
    let (spec, nullable) = spec
        .strip_suffix(" null")
        .map_or((spec, false), |spec| (spec, true));
    let value_type = match spec {
        "int64" => ValueType::Integer {
            signed: true,
            bits: Some(64),
        },
        "utf8" => ValueType::Utf8,
        "date" => ValueType::Date,
        "timestamptz" => ValueType::Timestamp {
            timezone_semantics: sef_model_ir::time::TimestampSemantics::Instant,
        },
        other => {
            let (p, s) = other
                .strip_prefix("decimal(")
                .and_then(|rest| rest.strip_suffix(')'))
                .and_then(|rest| rest.split_once(','))
                .unwrap_or_else(|| panic!("type {other}"));
            decimal(p.parse().expect("p"), s.parse().expect("s"))
        }
    };
    (value_type, nullable)
}

fn corpus_catalog(case: u8) -> Catalog {
    let relations: BTreeMap<String, BTreeMap<String, String>> =
        serde_saphyr::from_str(&corpus(&format!("CASE-{case:03}.catalog.yaml"))).expect("catalog");
    Catalog {
        relations: relations
            .into_iter()
            .map(|(name, columns)| {
                let relation = Relation {
                    source: SourceHandle::new("corpus").expect("handle"),
                    relation: SourceHandle::new(name.as_str()).expect("handle"),
                    columns: columns
                        .into_iter()
                        .map(|(column, spec)| {
                            let (value_type, nullable) = column_type(&spec);
                            Column {
                                handle: SourceHandle::new(column.as_str()).expect("handle"),
                                name: column,
                                value_type,
                                nullable,
                            }
                        })
                        .collect(),
                };
                (name, relation)
            })
            .collect(),
    }
}

#[test]
fn a_policy_change_is_one_of_policy() {
    let text = corpus("CASE-015.yaml");
    let catalog = corpus_catalog(15);
    let before = compiled(definition(&text, &catalog));
    let after = compiled(definition(
        &text.replace("tenant_id = :tenant_id", "tenant_id <> :tenant_id"),
        &catalog,
    ));
    let changed = diff(&before, &after);
    let policy = changed
        .changes
        .iter()
        .find(|entry| entry.id.kind() == "policy")
        .unwrap_or_else(|| panic!("no policy change: {changed:#?}"));
    assert_eq!(meaning(&policy.change), BTreeSet::from([Aspect::Policy]));
    // Nothing changed its own expression or contract: the rest is affected.
    for entry in &changed.changes {
        if entry.id != policy.id {
            assert!(
                matches!(entry.change, Change::Affected { .. }),
                "{changed:#?}"
            );
        }
    }
}

// ---- Property: presentation and governance never change meaning --------------

/// The corpus models that convert, as CSMIR definitions.
fn corpus_definitions() -> &'static [ModelDefinition] {
    static DEFINITIONS: std::sync::OnceLock<Vec<ModelDefinition>> = std::sync::OnceLock::new();
    DEFINITIONS.get_or_init(|| {
        (1_u8..=21)
            .filter_map(|case| {
                let text = corpus(&format!("CASE-{case:03}.yaml"));
                let model = sef_ossie::load(&text).ok()?.model;
                let options = Options {
                    namespace: "corpus".to_owned(),
                    model: format!("case-{case:03}"),
                };
                to_csmir(&model, &corpus_catalog(case), &options).ok()
            })
            .collect()
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn labels_descriptions_and_governance_never_change_meaning(
        pick in any::<prop::sample::Index>(),
        label in proptest::option::of("[A-Za-zñ ]{1,12}"),
        description in proptest::option::of("[A-Za-z .]{0,24}"),
        owner in proptest::option::of("[a-z]{1,8}"),
        tags in prop::collection::btree_set("[a-z]{1,6}", 0..3),
        which in any::<prop::sample::Index>(),
    ) {
        let definitions = corpus_definitions();
        prop_assume!(!definitions.is_empty());
        let original = definitions[pick.index(definitions.len())].clone();
        prop_assert!(definitions.len() >= 10, "corpus models: {}", definitions.len());
        let before = compiled(original.clone());
        prop_assert!(diff(&before, &before).is_empty());
        let mut edited = original;
        let metadata = match which.index(3) {
            0 if !edited.metrics.is_empty() => {
                let i = which.index(edited.metrics.len());
                &mut edited.metrics[i].metadata
            }
            1 if !edited.dimensions.is_empty() => {
                let i = which.index(edited.dimensions.len());
                &mut edited.dimensions[i].metadata
            }
            _ => &mut edited.metadata,
        };
        metadata.label = label;
        metadata.description = description;
        metadata.governance.owner = owner;
        metadata.governance.tags = tags.into_iter().collect();
        let after = compiled(edited);
        prop_assert_eq!(before.content_hash(), after.content_hash());
        for entry in &diff(&before, &after).changes {
            let is_cosmetic = matches!(
                &entry.change,
                Change::Modified { renamed_from: None, meaning, .. } if meaning.is_empty()
            );
            prop_assert!(is_cosmetic, "{:?}", entry);
        }
    }
}
