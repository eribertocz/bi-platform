//! Every diagnostic points at the text (#106): line and column by JSON
//! pointer, in YAML and JSON, and names quoted as their author wrote them.
#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use proptest::prelude::*;
use sef_model_ir::identity::SourceHandle;
use sef_model_ir::types::{DecimalType, ValueType};
use sef_ossie::catalog::{Catalog, Column, Relation};
use sef_ossie::csmir::{Options, to_csmir};
use sef_ossie::{Code, Diagnostic, Positions};

/// Line numbers matter below: keep the layout.
const SALES: &str = "\
version: '0.2.0.dev0'
name: Ventas
datasets:
  - name: Sales
    source: sales
    fields:
      - name: amount
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: amount
      - name: zona
        expression:
          dialects:
            - dialect: OSSIE_SQL_2026
              expression: region
        dimension: {}
metrics:
  - name: total
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(Sales.Importe)
  - name: otro
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(Ventas.amount)
  - name: suelto
    expression:
      dialects:
        - dialect: OSSIE_SQL_2026
          expression: SUM(Importe)
";

fn catalog() -> Catalog {
    let column = |name: &str, value_type| Column {
        name: name.to_owned(),
        handle: SourceHandle::new(name).expect("handle"),
        value_type,
        nullable: true,
    };
    let relation = Relation {
        source: SourceHandle::new("test").expect("handle"),
        relation: SourceHandle::new("sales").expect("handle"),
        columns: vec![
            column(
                "amount",
                ValueType::Decimal(DecimalType::new(18, 2).expect("decimal")),
            ),
            column("region", ValueType::Utf8),
        ],
    };
    Catalog {
        relations: BTreeMap::from([("sales".to_owned(), relation)]),
    }
}

/// What the Models screen gets: conversion errors located in the text.
fn conversion_errors(text: &str) -> Vec<Diagnostic> {
    let loaded = sef_ossie::load(text).expect("loads");
    let options = Options {
        namespace: "test".to_owned(),
        model: "ventas".to_owned(),
    };
    let mut errors = to_csmir(&loaded.model, &catalog(), &options).expect_err("must fail");
    Positions::of(text).fill(&mut errors);
    errors
}

fn find<'d>(errors: &'d [Diagnostic], needle: &str) -> &'d Diagnostic {
    errors
        .iter()
        .find(|e| e.message.contains(needle))
        .unwrap_or_else(|| panic!("no diagnostic with {needle:?}: {errors:#?}"))
}

#[test]
fn conversion_errors_quote_names_as_written_and_point_at_the_expression() {
    // A field stops the conversion before metrics: one document each.
    let columns = conversion_errors(&SALES.replace("expression: region", "expression: Regionx"));
    let column = find(&columns, r#"El origen no tiene la columna "Regionx"."#);
    assert_eq!(column.code, Code::UnknownColumn);
    assert_eq!(column.location, Some((16, 27)));
    let errors = conversion_errors(SALES);
    let field = find(
        &errors,
        r#"El dataset "Sales" no tiene el campo "Importe"."#,
    );
    assert_eq!(field.code, Code::UnknownReference);
    assert_eq!(field.location, Some((23, 23)));
    let dataset = find(&errors, r#"No existe el dataset "Ventas"."#);
    assert_eq!(dataset.location, Some((28, 23)));
    let anywhere = find(&errors, r#"Ningún dataset tiene el campo "Importe"."#);
    assert_eq!(anywhere.location, Some((33, 23)));
    // Nothing upper-cased by the comparison leaks into a message.
    for upper in ["REGIONX", "IMPORTE", "SALES", "VENTAS"] {
        assert!(
            errors
                .iter()
                .chain(&columns)
                .all(|e| !e.message.contains(upper)),
            "{upper} in {errors:#?}"
        );
    }
}

#[test]
fn validation_diagnostics_are_located_when_loading() {
    let text = SALES.replace("- name: zona", "- name: AMOUNT");
    let errors = sef_ossie::load(&text).expect_err("duplicate field");
    let duplicate = errors
        .iter()
        .find(|e| e.code == Code::DuplicateName)
        .expect("duplicate name");
    assert_eq!(duplicate.path, "/datasets/0/fields/1/name");
    assert_eq!(duplicate.location, Some((12, 15)));
}

#[test]
fn the_same_document_in_json_is_located_too() {
    let model = sef_ossie::load(SALES).expect("loads").model;
    let json = sef_ossie::to_json(&model).expect("json");
    let errors = conversion_errors(&json);
    let field = find(
        &errors,
        r#"El dataset "Sales" no tiene el campo "Importe"."#,
    );
    let (line, column) = field.location.expect("located");
    let text = json
        .lines()
        .nth(usize::try_from(line - 1).expect("line"))
        .expect("line");
    let at: String = text
        .chars()
        .skip(usize::try_from(column - 1).expect("column"))
        .collect();
    assert!(at.starts_with("\"SUM(Sales.Importe)\""), "{at:?}");
}

#[test]
fn a_missing_member_is_located_at_its_nearest_ancestor() {
    let positions = Positions::of(SALES);
    assert_eq!(positions.locate("/datasets/0/source"), Some((5, 13)));
    // `primary_key` is absent: the dataset itself.
    assert_eq!(positions.locate("/datasets/0/primary_key"), Some((4, 5)));
    assert_eq!(positions.locate(""), Some((1, 1)));
    assert_eq!(positions.locate("/no/such/path"), Some((1, 1)));
}

#[test]
fn text_that_does_not_parse_locates_nothing() {
    let positions = Positions::of("datasets: [unclosed");
    assert_eq!(positions.locate(""), None);
    assert_eq!(positions.locate("/datasets/0"), None);
}

#[test]
fn member_names_are_escaped_as_json_pointers() {
    let positions = Positions::of("a/b:\n  c~d: 1\n");
    assert_eq!(positions.locate("/a~1b/c~0d"), Some((2, 8)));
    assert_eq!(positions.locate("/a~1b"), Some((2, 3)));
}

/// JSON values, with member names a pointer must escape.
fn value() -> impl Strategy<Value = serde_json::Value> {
    let text = "[a-zA-Z0-9 :#/~ñé\"'-]{0,12}";
    let leaf = prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::Bool),
        any::<i64>().prop_map(serde_json::Value::from),
        text.prop_map(serde_json::Value::String),
    ];
    leaf.prop_recursive(4, 48, 6, move |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(serde_json::Value::Array),
            prop::collection::btree_map("[a-z/~]{1,6}", inner, 0..6)
                .prop_map(|map| serde_json::Value::Object(map.into_iter().collect())),
        ]
    })
}

/// Every pointer of `value` with its node.
fn pointers(value: &serde_json::Value, pointer: &str, out: &mut Vec<(String, serde_json::Value)>) {
    out.push((pointer.to_owned(), value.clone()));
    match value {
        serde_json::Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                pointers(item, &format!("{pointer}/{i}"), out);
            }
        }
        serde_json::Value::Object(members) => {
            for (key, item) in members {
                let key = key.replace('~', "~0").replace('/', "~1");
                pointers(item, &format!("{pointer}/{key}"), out);
            }
        }
        _ => {}
    }
}

fn text_at(text: &str, (line, column): (u64, u64)) -> Option<String> {
    let line = text
        .lines()
        .nth(usize::try_from(line).ok()?.checked_sub(1)?)?;
    let skip = usize::try_from(column).ok()?.checked_sub(1)?;
    (skip <= line.chars().count()).then(|| line.chars().skip(skip).collect())
}

proptest! {
    #[test]
    fn every_node_is_located_inside_the_document_at_its_own_text(root in value()) {
        let yaml = serde_saphyr::to_string(&root).expect("yaml");
        let json = serde_json::to_string_pretty(&root).expect("json");
        let mut nodes = Vec::new();
        pointers(&root, "", &mut nodes);
        for text in [&yaml, &json] {
            let positions = Positions::of(text);
            for (pointer, node) in &nodes {
                let position = positions.locate(pointer);
                prop_assert!(position.is_some(), "{pointer:?} unlocated in {text}");
                let at = text_at(text, position.expect("located"));
                prop_assert!(at.is_some(), "{pointer:?} outside {text}");
                // A text value starts where it was located: bare, quoted or
                // as a block scalar.
                if let serde_json::Value::String(value) = node {
                    let at = at.expect("inside");
                    let first = value.chars().next();
                    prop_assert!(
                        at.starts_with(['"', '\'', '|', '>'])
                            || first.is_some_and(|c| at.starts_with(c)),
                        "{pointer:?} = {value:?} located at {at:?} in {text}"
                    );
                }
            }
        }
    }
}
