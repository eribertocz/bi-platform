//! The answer of an execution (#146): the exact result of #136, a receipt
//! anyone can recompute from what it shows, and the lifecycle notices, all
//! from one real execution of one [`Prepared`] query.
//!
//! Only an execution that ended in [`Event::Complete`] is assembled: a
//! failure, a stream that closes without completing, or anything after the
//! completion leaves no receipt of success. The receipt shows what this
//! session may see, never an identity, a row-policy value, SQL, a physical
//! relation or a hidden object; the rest of the evidence is bound by an
//! opaque commitment under the instance's key (#204), so a substitution of
//! it changes the receipt without revealing it.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::time::SystemTime;

use sef_adapter_duckdb::execution::BlockResult;
use sef_adapter_duckdb::provenance::ResultProvenance;
use sef_exec_ir::{ObjectRef, Operator};
use sef_model_ir::canonical::{Canonical, CanonicalValue as C};
use sef_model_ir::identity::StableId;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use super::context::{ContextKey, Scope};
use super::prepare::Prepared;
use super::result::{self, CodecError, MAX_RESULT_BYTES, SemanticResult};
use crate::semantic_execution::{Event, Failure};

/// Names this receipt's form; part of what its ID covers.
pub const RECEIPT_VERSION: &str = "bi-run-receipt/1";

/// The answer of one execution: rows, receipt and notices, as one JSON
/// body of at most 4 MiB.
#[derive(Debug, PartialEq, Serialize, ToSchema)]
pub struct SemanticRun {
    pub result: SemanticResult,
    pub receipt: SemanticRunReceipt,
    pub notices: Vec<SemanticRunNotice>,
}

/// What the answer depended on, as far as this session may see it. `id` is
/// SHA-256 of the RFC 8785 (JCS) form of this object without `id`,
/// `read_started_at` and `read_finished_at`: anyone recomputes it from what
/// is shown. It identifies that content; it is no signature, and proves
/// neither who issued it nor that whoever handed it over changed nothing.
#[derive(Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct SemanticRunReceipt {
    /// `run:` and 64 hexadecimal digits.
    pub id: String,
    pub version: String,
    pub query: SemanticRunQuery,
    pub fingerprints: SemanticRunFingerprints,
    /// The selected metrics and dimensions this session may see, with the
    /// fingerprint of their content. The projection may be incomplete:
    /// reproducing the answer needs authorized access to all its evidence.
    pub definitions: Vec<SemanticRunDefinition>,
    pub versions: SemanticRunVersions,
    /// The datasets read that this session may see.
    pub sources: Vec<SemanticRunSource>,
    /// Opaque commitment to the evidence not shown, under this instance's
    /// key and for this principal: comparable within one instance and for
    /// one principal, never across restarts.
    pub evidence: String,
    /// When this read began (RFC 3339, UTC): the time of this execution,
    /// never of a refresh of its sources.
    pub read_started_at: String,
    /// When this read ended (RFC 3339, UTC).
    pub read_finished_at: String,
}

/// The query, as its canonical intent with typed literals.
#[derive(Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct SemanticRunQuery {
    /// `sef-canonical-cbor-v1`: deterministic CBOR (RFC 8949 §4.2) of SQIR.
    pub encoding: String,
    /// The encoded query, in hexadecimal.
    pub sqir: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct SemanticRunFingerprints {
    pub query: String,
    pub model: String,
    pub plan: String,
    /// The decision's fingerprint, protected like `context`: the plain one
    /// covers the principal and its roles.
    pub decision: String,
    pub policies: String,
    /// The protected fingerprint of the row-policy values (#204).
    pub context: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct SemanticRunDefinition {
    pub id: String,
    pub content: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct SemanticRunVersions {
    pub engine: String,
    pub adapter: String,
    pub capability_profile: String,
    pub tzdb: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct SemanticRunSource {
    /// The dataset's stable identifier; never its physical relation.
    pub dataset: String,
    /// The DuckLake snapshot observed in the transaction that read the
    /// rows, as decimal text; `null` for a source with no persistent
    /// version, which cannot be read again as it was. Observed, not
    /// retained: no promise that it can still be read.
    #[schema(required = true)]
    pub data_snapshot: Option<String>,
    /// When the source itself was last refreshed; `null` while unknown.
    #[schema(required = true)]
    pub source_refreshed_at: Option<String>,
}

/// A selected metric that is deprecated, as its authorized definition
/// declares it.
#[derive(Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct SemanticRunNotice {
    pub metric: String,
    #[schema(required = true)]
    pub sunset: Option<String>,
    /// Only a replacement this session may see.
    #[schema(required = true)]
    pub replacement: Option<String>,
}

/// Why no answer was assembled. None of it carries a receipt.
#[derive(Debug, PartialEq, Eq)]
pub enum RunError {
    /// The execution failed, before or after any batch.
    Failed(Failure),
    /// The stream ended without its completion.
    Incomplete,
    /// The evidence does not belong to this preparation, or something
    /// followed the completion.
    Evidence(&'static str),
    /// The rows do not fit their contract or the body's budget.
    Codec(CodecError),
    /// No key or no canonical form: nothing can be bound.
    Unavailable,
}

/// Assembles the answer from every event of one execution of `prepared`,
/// in the order they were delivered.
///
/// Trust boundary: `events` must be those of one stream, as it delivered
/// them; the caller that owns the stream (#137) guarantees it. Nothing in
/// the events binds the completion to the batches beyond the certificate,
/// the definitions and the row count, so two executions of the same
/// preparation are told apart only by that ownership. The blocks gathered
/// here are bounded by the executor's limits before the codec's budget.
///
/// # Errors
/// A [`RunError`] for anything but a complete, coherent execution whose
/// answer fits the body's budget.
pub fn assemble(
    prepared: &Prepared,
    events: impl IntoIterator<Item = Event>,
    key: &ContextKey,
    scope: &Scope<'_>,
) -> Result<SemanticRun, RunError> {
    let mut blocks = Vec::new();
    let mut complete = None;
    for event in events {
        if complete.is_some() {
            return Err(RunError::Evidence("after_complete"));
        }
        match event {
            Event::Batch(block) => blocks.push(block),
            Event::Complete(provenance) => complete = Some(provenance),
            Event::Failed(failure) => return Err(RunError::Failed(failure)),
        }
    }
    let provenance = complete.ok_or(RunError::Incomplete)?;
    let datasets = coherent(prepared, &provenance, &blocks)?;
    let receipt = receipt(prepared, &provenance, &datasets, key, scope)?;
    let notices = prepared
        .notices()
        .iter()
        .map(|notice| SemanticRunNotice {
            metric: notice.metric.as_str().to_owned(),
            sunset: notice.sunset.clone(),
            replacement: notice.replacement.as_ref().map(|id| id.as_str().to_owned()),
        })
        .collect();
    // The body without rows first; the rows get exactly what is left.
    let mut run = SemanticRun {
        result: SemanticResult { blocks: Vec::new() },
        receipt,
        notices,
    };
    let overhead = json_len(&run)? - json_len(&run.result)?;
    let budget = MAX_RESULT_BYTES
        .checked_sub(overhead)
        .ok_or(RunError::Codec(CodecError::ResultTooLarge))?;
    run.result = result::encode_within(prepared, &blocks, budget).map_err(RunError::Codec)?;
    Ok(run)
}

/// What the plan of `prepared` reads, as its datasets.
fn read_datasets(prepared: &Prepared) -> BTreeSet<StableId> {
    nodes(prepared)
        .filter_map(|operator| match operator {
            Operator::ReadSemanticDataset { dataset, .. } => Some(dataset.id),
            _ => None,
        })
        .collect()
}

fn planned_definitions(prepared: &Prepared) -> BTreeSet<ObjectRef> {
    nodes(prepared)
        .filter_map(|operator| match operator {
            Operator::ProjectSemanticOutput { definitions, .. } => Some(definitions),
            _ => None,
        })
        .flatten()
        .collect()
}

fn nodes(prepared: &Prepared) -> impl Iterator<Item = Operator> {
    let plan = prepared.plan();
    let roots: Vec<_> = plan.blocks.iter().map(|block| block.root).collect();
    plan.graph
        .topological(&roots)
        .into_iter()
        .map(|node| node.operator)
}

/// The evidence is this preparation's: its certificate, its definitions,
/// the rows delivered, and a dataset of the plan behind every source.
/// Returns, per source, the datasets it is the relation of.
fn coherent(
    prepared: &Prepared,
    provenance: &ResultProvenance,
    blocks: &[BlockResult],
) -> Result<Vec<Vec<StableId>>, RunError> {
    if provenance.certificate != *prepared.certificate() {
        return Err(RunError::Evidence("certificate"));
    }
    let delivered: BTreeSet<ObjectRef> = provenance.definitions.iter().cloned().collect();
    if delivered != planned_definitions(prepared) {
        return Err(RunError::Evidence("definitions"));
    }
    let rows: usize = blocks
        .iter()
        .flat_map(|block| &block.batches)
        .map(bi_runtime::duckdb::arrow::record_batch::RecordBatch::num_rows)
        .sum();
    if rows != provenance.rows {
        return Err(RunError::Evidence("rows"));
    }
    let definition = prepared.model().definition();
    let read = read_datasets(prepared);
    let mut covered = BTreeSet::new();
    let mut datasets = Vec::with_capacity(provenance.sources.len());
    for source in &provenance.sources {
        let of_source: Vec<StableId> = definition
            .source_bindings
            .iter()
            .filter(|binding| {
                binding.source_id.as_str() == source.source
                    && binding.relation_ref.as_str() == source.relation
            })
            .flat_map(|binding| {
                definition
                    .datasets
                    .iter()
                    .filter(move |d| &d.source_binding == binding.metadata.id())
                    .map(|d| d.metadata.id().clone())
            })
            .filter(|id| read.contains(id))
            .collect();
        if of_source.is_empty() {
            return Err(RunError::Evidence("source"));
        }
        covered.extend(of_source.iter().cloned());
        datasets.push(of_source);
    }
    if covered != read {
        return Err(RunError::Evidence("source"));
    }
    Ok(datasets)
}

fn receipt(
    prepared: &Prepared,
    provenance: &ResultProvenance,
    datasets: &[Vec<StableId>],
    key: &ContextKey,
    scope: &Scope<'_>,
) -> Result<SemanticRunReceipt, RunError> {
    let sqir = prepared
        .query()
        .canonical_value()
        .encode()
        .map_err(|_| RunError::Unavailable)?;
    let context = key
        .fingerprint(scope, prepared.context())
        .ok_or(RunError::Unavailable)?;
    let decision = prepared.decision().fingerprint.to_string();
    let protected_decision = key
        .decision(scope, &decision)
        .ok_or(RunError::Unavailable)?;
    let shows = |id: &StableId| prepared.shows(id);
    let definitions = shown_definitions(provenance, shows);
    let sources = shown_sources(provenance, datasets, shows);
    let evidence = key
        .commit(scope, private_evidence(provenance, &decision))
        .ok_or(RunError::Unavailable)?;
    let mut receipt = SemanticRunReceipt {
        id: String::new(),
        version: RECEIPT_VERSION.to_owned(),
        query: SemanticRunQuery {
            encoding: "sef-canonical-cbor-v1".to_owned(),
            sqir: hex(&sqir),
        },
        fingerprints: SemanticRunFingerprints {
            query: prepared.query_fingerprint().to_owned(),
            model: prepared.model().content_hash().to_string(),
            plan: prepared.plan_fingerprint().to_owned(),
            decision: protected_decision,
            policies: prepared.policies().to_owned(),
            context,
        },
        definitions,
        versions: SemanticRunVersions {
            engine: provenance.engine_version.clone(),
            adapter: provenance.adapter_version.to_owned(),
            capability_profile: provenance.capability_profile.to_owned(),
            tzdb: provenance.tzdb_version.to_owned(),
        },
        sources,
        evidence,
        read_started_at: instant(provenance.started_at),
        read_finished_at: instant(provenance.finished_at),
    };
    receipt.id = receipt_id(&receipt)?;
    Ok(receipt)
}

/// The definitions `shows` admits, in identifier order.
fn shown_definitions(
    provenance: &ResultProvenance,
    shows: impl Fn(&StableId) -> bool,
) -> Vec<SemanticRunDefinition> {
    let mut definitions: Vec<SemanticRunDefinition> = provenance
        .definitions
        .iter()
        .filter(|object| shows(&object.id))
        .map(|object| SemanticRunDefinition {
            id: object.id.as_str().to_owned(),
            content: object.content.to_string(),
        })
        .collect();
    definitions.sort_by(|a, b| a.id.cmp(&b.id));
    definitions.dedup();
    definitions
}

/// Each source as the datasets of it that `shows` admits.
fn shown_sources(
    provenance: &ResultProvenance,
    datasets: &[Vec<StableId>],
    shows: impl Fn(&StableId) -> bool,
) -> Vec<SemanticRunSource> {
    let mut sources: Vec<SemanticRunSource> = provenance
        .sources
        .iter()
        .zip(datasets)
        .flat_map(|(source, datasets)| {
            datasets
                .iter()
                .filter(|id| shows(id))
                .map(|id| SemanticRunSource {
                    dataset: id.as_str().to_owned(),
                    data_snapshot: source.data_snapshot.map(|s| s.to_string()),
                    source_refreshed_at: None,
                })
        })
        .collect();
    sources.sort_by(|a, b| a.dataset.cmp(&b.dataset));
    sources.dedup();
    sources
}

/// Everything the execution proved that the receipt does not show: every
/// definition, every source with its relation, schema and snapshot, the
/// certificate, the output grains and the plain decision fingerprint.
fn private_evidence(provenance: &ResultProvenance, decision: &str) -> C {
    let definitions = provenance
        .definitions
        .iter()
        .map(|object| {
            C::Map(vec![
                ("id".to_owned(), C::text(object.id.as_str())),
                ("content".to_owned(), C::text(object.content.to_string())),
            ])
        })
        .collect();
    let sources = provenance
        .sources
        .iter()
        .map(|source| {
            C::Map(vec![
                ("source".to_owned(), C::text(&source.source)),
                ("relation".to_owned(), C::text(&source.relation)),
                ("schema".to_owned(), C::text(&source.schema_fingerprint)),
                (
                    "snapshot".to_owned(),
                    source.data_snapshot.map_or(C::Absent, C::Signed),
                ),
            ])
        })
        .collect();
    let certificate = &provenance.certificate;
    C::Map(vec![
        ("definitions".to_owned(), C::Set(definitions)),
        ("sources".to_owned(), C::Set(sources)),
        (
            "certificate".to_owned(),
            C::Map(vec![
                (
                    "model".to_owned(),
                    C::text(certificate.model_hash.to_string()),
                ),
                (
                    "query".to_owned(),
                    C::text(certificate.query_hash.to_string()),
                ),
                (
                    "seir".to_owned(),
                    C::text(certificate.seir_hash.to_string()),
                ),
            ]),
        ),
        (
            "checked_contracts".to_owned(),
            C::Unsigned(u64::try_from(provenance.checked_contracts).unwrap_or(u64::MAX)),
        ),
        (
            "output_grains".to_owned(),
            C::Map(
                provenance
                    .output_grains
                    .iter()
                    .map(|(block, atoms)| (block.clone(), C::sequence(atoms)))
                    .collect(),
            ),
        ),
        (
            "schema_version".to_owned(),
            C::text(provenance.schema_version),
        ),
        ("decision".to_owned(), C::text(decision)),
    ])
}

/// SHA-256 of the receipt's JCS form, without the ID and the read times.
fn receipt_id(receipt: &SemanticRunReceipt) -> Result<String, RunError> {
    id_of(serde_json::to_value(receipt).map_err(|_| RunError::Unavailable)?)
}

/// The ID of a receipt in its JSON form, as a third party computes it.
fn id_of(mut receipt: Value) -> Result<String, RunError> {
    let object = receipt.as_object_mut().ok_or(RunError::Unavailable)?;
    for field in ["id", "read_started_at", "read_finished_at"] {
        object.remove(field);
    }
    let mut canonical = String::new();
    jcs(&receipt, &mut canonical)?;
    Ok(format!(
        "run:{}",
        hex(&Sha256::digest(canonical.as_bytes()))
    ))
}

/// RFC 8785 for what a receipt holds: objects with keys in code-unit
/// order, strings as JSON escapes them, `null` and booleans. A number
/// (none is published) is refused rather than written in another form.
fn jcs(value: &Value, out: &mut String) -> Result<(), RunError> {
    match value {
        Value::Null | Value::Bool(_) | Value::String(_) => {
            out.push_str(&serde_json::to_string(value).map_err(|_| RunError::Unavailable)?);
        }
        Value::Number(_) => return Err(RunError::Unavailable),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                jcs(item, out)?;
            }
            out.push(']');
        }
        Value::Object(fields) => {
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).map_err(|_| RunError::Unavailable)?);
                out.push(':');
                jcs(&fields[key], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn instant(at: SystemTime) -> String {
    let at: chrono::DateTime<chrono::Utc> = at.into();
    at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
}

fn json_len(value: &impl Serialize) -> Result<usize, RunError> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|_| RunError::Unavailable)
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

#[cfg(test)]
mod tests;
