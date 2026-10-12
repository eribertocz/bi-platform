//! Writing a model as YAML never changes its text (#91): strings that start
//! or end with a non-ASCII space read back exactly, while ordinary models
//! keep the readable plain style.
#![allow(clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use sef_ossie::document::{AiContext, SemanticModel, StructuredAiContext};

fn corpus() -> SemanticModel {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/CASE-001.yaml");
    sef_ossie::load(&std::fs::read_to_string(path).expect("fixture"))
        .expect("corpus document")
        .model
}

fn round_trip(model: &SemanticModel) -> SemanticModel {
    let text = sef_ossie::to_yaml(model).expect("serializes");
    sef_ossie::parse(&text).unwrap_or_else(|error| panic!("reads back: {error}\n{text}"))
}

#[test]
fn edge_unicode_spaces_survive_the_round_trip() {
    let mut model = corpus();
    // The case the conformance property found, then other edge spaces.
    model.datasets[0].ai_context = Some(AiContext::Structured(StructuredAiContext {
        examples: Some(vec!["\u{2000}7".to_owned(), "x\u{3000}".to_owned()]),
        ..StructuredAiContext::default()
    }));
    model.datasets[0].description = Some("\u{00a0}true".to_owned());
    model.metrics[0].description = Some("\u{2028}12.5\u{2009}".to_owned());
    assert_eq!(round_trip(&model), model);
}

#[test]
fn ordinary_models_keep_the_plain_style() {
    let model = corpus();
    let text = sef_ossie::to_yaml(&model).expect("serializes");
    assert!(text.contains("name: case-001"), "{text}");
    assert_eq!(round_trip(&model), model);
}

proptest! {
    /// Any description, edge spaces included, reads back unchanged.
    #[test]
    fn descriptions_read_back_unchanged(
        lead in proptest::sample::select(vec!["", " ", "\u{2000}", "\u{00a0}", "\u{3000}", "\t"]),
        body in "[a-z0-9 .:#-]{0,12}",
        tail in proptest::sample::select(vec!["", " ", "\u{2001}", "\u{205f}"]),
    ) {
        let mut model = corpus();
        model.metrics[0].description = Some(format!("{lead}{body}{tail}"));
        prop_assert_eq!(round_trip(&model), model);
    }
}
