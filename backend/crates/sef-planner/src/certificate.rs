//! The plan certificate (§4.1–4.2): every obligation of the correctness
//! engine paired with the SEIR evidence that discharges it. An auditable
//! compiler artifact, not a proof-system claim.

use std::fmt;

use sef_correctness::Obligation;
use sef_exec_ir::NodeId;
use sef_model_ir::canonical::Fingerprint;

/// How an obligation is witnessed in the plan.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Evidence {
    /// A node whose operator performs what the obligation requires.
    Node(NodeId),
    /// A property checked over the whole graph (for example, that a
    /// distinct result is never aggregated again).
    Invariant(&'static str),
    /// A typed annotation the plan carries through to execution.
    Annotation(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discharge {
    pub block_id: String,
    pub obligation: Obligation,
    pub evidence: Vec<Evidence>,
}

/// Identities of what was planned and how every obligation is met.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Certificate {
    pub model_hash: Fingerprint,
    pub query_hash: Fingerprint,
    pub seir_hash: Fingerprint,
    pub discharges: Vec<Discharge>,
}

impl fmt::Display for Evidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Node(id) => write!(f, "node {id}"),
            Self::Invariant(name) => write!(f, "invariant {name}"),
            Self::Annotation(text) => write!(f, "annotation {text}"),
        }
    }
}

impl fmt::Display for Certificate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "model {}", self.model_hash)?;
        writeln!(f, "query {}", self.query_hash)?;
        writeln!(f, "plan  {}", self.seir_hash)?;
        for discharge in &self.discharges {
            let evidence: Vec<String> =
                discharge.evidence.iter().map(ToString::to_string).collect();
            writeln!(
                f,
                "[{}] {} <= {}",
                discharge.block_id,
                discharge.obligation,
                evidence.join(" + ")
            )?;
        }
        Ok(())
    }
}
