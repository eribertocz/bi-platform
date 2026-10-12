//! Reading and writing Ossie documents. YAML 1.2 is a superset of JSON, so
//! one reader serves `.yaml`, `.yml` and `.json`.
//!
//! The reader is strict on purpose: a duplicate key would otherwise keep
//! one value silently and change a metric without warning; YAML merge keys
//! and ambiguous YAML 1.1 booleans (`yes`, `on`) are rejected; and resource
//! budgets bound what a hostile document can cost.

use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_saphyr::{DuplicateKeyPolicy, MergeKeyPolicy};

use crate::diagnostic::{Code, Diagnostic};
use crate::document::SemanticModel;
use crate::validate;

/// Largest document accepted, in bytes.
pub const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;

/// A document that passed every check, with the warnings it raised.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub model: SemanticModel,
    pub warnings: Vec<Diagnostic>,
}

/// Parses and validates one document.
///
/// # Errors
/// Every error found (syntax first; then all validation errors at once),
/// followed by the warnings.
pub fn load(text: &str) -> Result<Loaded, Vec<Diagnostic>> {
    let model = parse(text).map_err(|diagnostic| vec![diagnostic])?;
    let diagnostics = validate::validate(&model);
    if diagnostics.iter().any(Diagnostic::is_error) {
        return Err(diagnostics);
    }
    Ok(Loaded {
        model,
        warnings: diagnostics,
    })
}

/// Parses one document into the typed model without the semantic checks.
///
/// # Errors
/// A single [`Code::Syntax`], [`Code::TooLarge`] or [`Code::LegacyWrapper`].
pub fn parse(text: &str) -> Result<SemanticModel, Diagnostic> {
    if text.len() > MAX_DOCUMENT_BYTES {
        return Err(Diagnostic::error(
            Code::TooLarge,
            "",
            format!("El documento supera {MAX_DOCUMENT_BYTES} bytes."),
        ));
    }
    if is_legacy_wrapper(text) {
        return Err(Diagnostic::error(
            Code::LegacyWrapper,
            "/semantic_model",
            "Forma anterior a 0.2.0: mueve cada modelo de `semantic_model` a la raíz de su propio \
             documento (un modelo por archivo) y usa `version: 0.2.0.dev0`.",
        ));
    }
    serde_saphyr::from_str_with_options(text, options()).map_err(|error| {
        let mut diagnostic = Diagnostic::error(Code::Syntax, "", error.to_string());
        diagnostic.location = error
            .location()
            .map(|location| (location.line(), location.column()));
        diagnostic
    })
}

/// The document as YAML that reads back to exactly the same model.
///
/// # Errors
/// Only if a value cannot be represented, which the typed model prevents.
pub fn to_yaml(model: &SemanticModel) -> Result<String, serde_saphyr::ser::Error> {
    let plain = serde_saphyr::to_string(model)?;
    // serde-saphyr 1.3 leaves strings unquoted when they start or end with
    // a non-ASCII space (U+2000…), which its own reader then trims: the
    // text would change on the way back. Plain style when it reads back
    // identically; every string quoted otherwise.
    if parse(&plain).is_ok_and(|read| &read == model) {
        return Ok(plain);
    }
    let mut quoted = serde_saphyr::SerializerOptions::default();
    quoted.quote_all = true;
    serde_saphyr::to_string_with_options(model, quoted)
}

/// The document as pretty-printed JSON.
///
/// # Errors
/// Only if a value cannot be represented, which the typed model prevents.
pub fn to_json(model: &SemanticModel) -> serde_json::Result<String> {
    serde_json::to_string_pretty(model)
}

fn options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        duplicate_keys: DuplicateKeyPolicy::Error,
        merge_keys: MergeKeyPolicy::Error,
        strict_booleans: true,
        budget: serde_saphyr::budget! {
            max_documents: 1,
            max_depth: 64,
            max_anchors: 64,
            max_aliases: 256,
        },
    }
}

/// Whether the root is the legacy `{semantic_model: [...]}` wrapper, so it
/// gets its migration message instead of a bare "unknown field".
fn is_legacy_wrapper(text: &str) -> bool {
    #[derive(Deserialize)]
    struct Probe {
        #[serde(default)]
        semantic_model: Option<IgnoredAny>,
    }
    serde_saphyr::from_str_with_options::<Probe>(text, options())
        .is_ok_and(|probe| probe.semantic_model.is_some())
}
