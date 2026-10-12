//! Immutable policy snapshots (ADR 0012 §3 and §8): the composed schema and
//! the validated policy files of a project, with a version that identifies
//! both. A set that fails strict validation is never loaded; the caller
//! keeps serving the previous snapshot and shows the problems.

use std::collections::BTreeMap;
use std::str::FromStr;

use cedar_policy::{PolicyId, PolicySet, Schema, SchemaFragment, ValidationMode, Validator};

use crate::decision::Encoder;
use crate::schema::{SEMANTIC_SCHEMA, SEMANTIC_SCHEMA_VERSION};

/// One policy file, as stored in the project (`policies/*.cedar`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicySource {
    pub path: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
}

/// A diagnostic for the Problems panel: where, how serious and what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub path: String,
    pub severity: Severity,
    pub message: String,
}

impl Problem {
    fn error(path: &str, message: impl Into<String>) -> Self {
        Self {
            path: path.to_owned(),
            severity: Severity::Error,
            message: message.into(),
        }
    }
}

/// A validated, immutable set of policies and the schema they were
/// validated against. Every decision names the version it was taken with.
#[derive(Debug)]
pub struct PolicySnapshot {
    version: String,
    schema: Schema,
    policies: PolicySet,
}

/// The path used for problems in the platform schema fragment.
const PLATFORM_SCHEMA_PATH: &str = "policies/platform.cedarschema";

impl PolicySnapshot {
    /// Composes the semantic schema with the edge's platform fragment,
    /// parses every policy file and validates the whole set in strict mode.
    ///
    /// Policy ids are the `@id("…")` annotation of each policy, or
    /// `<path>#<position>`; they must be unique across files.
    ///
    /// # Errors
    /// Every error found (schema, syntax, templates, duplicate ids, strict
    /// validation), so the author sees them all at once.
    pub fn load(
        platform_schema: &str,
        sources: &[PolicySource],
    ) -> Result<(Self, Vec<Problem>), Vec<Problem>> {
        let mut problems = Vec::new();
        let schema = compose(platform_schema, &mut problems);
        let (policies, origins) = parse(sources, &mut problems);
        if let Some(schema) = &schema {
            let result = Validator::new(schema.clone()).validate(&policies, ValidationMode::Strict);
            let origin = |id: &PolicyId| {
                origins
                    .get(AsRef::<str>::as_ref(id))
                    .cloned()
                    .unwrap_or_else(|| id.to_string())
            };
            for error in result.validation_errors() {
                problems.push(Problem::error(
                    &origin(error.policy_id()),
                    error.to_string(),
                ));
            }
            for warning in result.validation_warnings() {
                problems.push(Problem {
                    path: origin(warning.policy_id()),
                    severity: Severity::Warning,
                    message: warning.to_string(),
                });
            }
        }
        match schema {
            Some(schema) if !problems.iter().any(|p| p.severity == Severity::Error) => Ok((
                Self {
                    version: version(platform_schema, sources),
                    schema,
                    policies,
                },
                problems,
            )),
            _ => Err(problems),
        }
    }

    /// Identifies the semantic schema, the platform fragment and the
    /// policy files: any change to them is a new version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    pub(crate) const fn schema(&self) -> &Schema {
        &self.schema
    }

    pub(crate) const fn policies(&self) -> &PolicySet {
        &self.policies
    }
}

fn compose(platform: &str, problems: &mut Vec<Problem>) -> Option<Schema> {
    let mut fragments = Vec::new();
    for (path, text) in [
        ("sef-authz/semantic.cedarschema", SEMANTIC_SCHEMA),
        (PLATFORM_SCHEMA_PATH, platform),
    ] {
        match SchemaFragment::from_cedarschema_str(text) {
            Ok((fragment, warnings)) => {
                fragments.push(fragment);
                problems.extend(warnings.map(|warning| Problem {
                    path: path.to_owned(),
                    severity: Severity::Warning,
                    message: warning.to_string(),
                }));
            }
            Err(error) => problems.push(Problem::error(path, error.to_string())),
        }
    }
    if fragments.len() < 2 {
        return None;
    }
    Schema::from_schema_fragments(fragments)
        .map_err(|error| problems.push(Problem::error(PLATFORM_SCHEMA_PATH, error.to_string())))
        .ok()
}

/// All policies of all files under their stable ids, and the file each one
/// came from.
fn parse(
    sources: &[PolicySource],
    problems: &mut Vec<Problem>,
) -> (PolicySet, BTreeMap<String, String>) {
    let mut policies = PolicySet::new();
    let mut origins = BTreeMap::new();
    for source in sources {
        let parsed = match PolicySet::from_str(&source.text) {
            Ok(parsed) => parsed,
            Err(error) => {
                problems.push(Problem::error(&source.path, error.to_string()));
                continue;
            }
        };
        if parsed.templates().next().is_some() {
            problems.push(Problem::error(
                &source.path,
                "Las plantillas de políticas no están admitidas: escribe políticas completas.",
            ));
        }
        for policy in parsed.policies() {
            let id = policy.annotation("id").map_or_else(
                || format!("{}#{}", source.path, policy.id()),
                ToOwned::to_owned,
            );
            if origins.contains_key(&id) {
                problems.push(Problem::error(
                    &source.path,
                    format!("El id de política {id:?} ya existe."),
                ));
                continue;
            }
            let renamed = policy.new_id(PolicyId::new(&id));
            if let Err(error) = policies.add(renamed) {
                problems.push(Problem::error(&source.path, error.to_string()));
                continue;
            }
            origins.insert(id, source.path.clone());
        }
    }
    (policies, origins)
}

fn version(platform: &str, sources: &[PolicySource]) -> String {
    let mut ordered: Vec<&PolicySource> = sources.iter().collect();
    ordered.sort_by(|a, b| a.path.cmp(&b.path));
    let mut encoder = Encoder::new("sef-authz/snapshot");
    encoder.text(SEMANTIC_SCHEMA_VERSION);
    encoder.text(SEMANTIC_SCHEMA);
    encoder.text(platform);
    for source in ordered {
        encoder.text(&source.path);
        encoder.text(&source.text);
    }
    encoder.finish().to_string()
}
