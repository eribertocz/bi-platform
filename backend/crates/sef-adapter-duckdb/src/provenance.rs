//! Execution receipts contain semantic identities and evidence, never bindings.
use sef_exec_ir::ObjectRef;
use sef_planner::Certificate;
use sef_query_ir::query::RequestedAtom;
use std::collections::BTreeMap;
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct SourceSnapshot {
    pub source: String,
    pub relation: String,
    pub schema_fingerprint: String,
    /// Local in-memory data have no persistent version. DuckLake has a
    /// snapshot ID observed in the very transaction that read the result.
    pub data_snapshot: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct ResultProvenance {
    pub schema_version: &'static str,
    pub certificate: Certificate,
    pub definitions: Vec<ObjectRef>,
    pub output_grains: BTreeMap<String, Vec<RequestedAtom>>,
    pub context_binding_fingerprint: String,
    pub sources: Vec<SourceSnapshot>,
    pub checked_contracts: usize,
    pub engine_version: String,
    pub adapter_version: &'static str,
    pub capability_profile: &'static str,
    pub tzdb_version: &'static str,
    pub started_at: SystemTime,
    pub finished_at: SystemTime,
    pub rows: usize,
    pub arrow_bytes: usize,
}
