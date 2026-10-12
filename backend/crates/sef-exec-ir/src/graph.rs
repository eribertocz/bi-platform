//! The SEIR DAG (§5): every node is identified by its canonical content,
//! never by creation order or memory address, so the same semantic plan
//! always has the same identity (§5.1) and identical subplans are shared
//! (§5.2) whatever order they were built in.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

use sef_model_ir::canonical::{
    Canonical, CanonicalError, CanonicalValue as C, Domain, Fingerprint, fingerprint,
};

use crate::operator::Operator;

/// A node's identity: the SEIR fingerprint of its operator, whose inputs
/// are themselves node identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(Fingerprint);

impl NodeId {
    #[must_use]
    pub const fn fingerprint(&self) -> &Fingerprint {
        &self.0
    }
}

impl Canonical for NodeId {
    fn canonical_value(&self) -> C {
        C::Bytes(self.0.as_bytes().to_vec())
    }
}

impl fmt::Display for NodeId {
    /// The first 12 hex digits: enough to read an explanation.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0.as_bytes()[..6] {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: NodeId,
    pub operator: Operator,
}

/// Why a node cannot be added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphError {
    /// An input is not a node of this graph: plans are built bottom-up.
    UnknownInput(NodeId),
    Encoding(CanonicalError),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownInput(id) => write!(f, "input node {id} is not in the plan"),
            Self::Encoding(error) => write!(f, "canonical encoding failed: {error:?}"),
        }
    }
}

impl std::error::Error for GraphError {}

/// An acyclic graph by construction: a node can only read nodes that
/// already exist, and its identity covers its inputs'.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SeirGraph {
    nodes: BTreeMap<NodeId, Operator>,
}

impl SeirGraph {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an operator and returns its identity; an identical operator
    /// already present is shared, not duplicated.
    ///
    /// # Errors
    /// [`GraphError::UnknownInput`] if an input is not in the graph;
    /// [`GraphError::Encoding`] if canonical encoding exceeds its budget.
    pub fn add(&mut self, mut operator: Operator) -> Result<NodeId, GraphError> {
        operator.normalize();
        if let Some(missing) = operator
            .inputs()
            .into_iter()
            .find(|input| !self.nodes.contains_key(input))
        {
            return Err(GraphError::UnknownInput(missing));
        }
        let id = NodeId(fingerprint(Domain::Seir, &operator).map_err(GraphError::Encoding)?);
        self.nodes.entry(id).or_insert(operator);
        Ok(id)
    }

    #[must_use]
    pub fn operator(&self, id: &NodeId) -> Option<&Operator> {
        self.nodes.get(id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Every node `root` depends on, itself included.
    #[must_use]
    pub fn reachable(&self, roots: &[NodeId]) -> BTreeSet<NodeId> {
        let mut seen = BTreeSet::new();
        let mut pending: Vec<NodeId> = roots.to_vec();
        while let Some(id) = pending.pop() {
            if seen.insert(id)
                && let Some(operator) = self.nodes.get(&id)
            {
                pending.extend(operator.inputs());
            }
        }
        seen
    }

    /// Nodes reachable from `roots`, inputs before the nodes that read
    /// them; ties broken by identity, so the order is canonical.
    #[must_use]
    pub fn topological(&self, roots: &[NodeId]) -> Vec<Node> {
        let reachable = self.reachable(roots);
        let mut placed: BTreeSet<NodeId> = BTreeSet::new();
        let mut order = Vec::with_capacity(reachable.len());
        while placed.len() < reachable.len() {
            let ready: Vec<NodeId> = reachable
                .iter()
                .filter(|id| !placed.contains(*id))
                .filter(|id| {
                    self.nodes.get(*id).is_some_and(|operator| {
                        operator.inputs().iter().all(|i| placed.contains(i))
                    })
                })
                .copied()
                .collect();
            if ready.is_empty() {
                // Unreachable by construction: inputs always exist first.
                break;
            }
            for id in ready {
                if let Some(operator) = self.nodes.get(&id) {
                    order.push(Node {
                        id,
                        operator: operator.clone(),
                    });
                }
                placed.insert(id);
            }
        }
        order
    }
}

/// One query block's plan: its result node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanBlock {
    pub block_id: String,
    pub root: NodeId,
}

/// The canonical semantic plan of a query set (§4): one DAG, one root per
/// block. Its fingerprint covers exactly the nodes the blocks reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticPlan {
    pub graph: SeirGraph,
    pub blocks: Vec<PlanBlock>,
}

impl SemanticPlan {
    fn roots(&self) -> Vec<NodeId> {
        self.blocks.iter().map(|block| block.root).collect()
    }

    /// The plan's semantic fingerprint (domain `SEIR`, §24): reachable
    /// nodes and block roots only, so construction leftovers never count.
    ///
    /// # Errors
    /// Only if canonical encoding exceeds its resource budget.
    pub fn fingerprint(&self) -> Result<Fingerprint, CanonicalError> {
        fingerprint(Domain::Seir, self)
    }

    /// Stable human-readable explanation: one line per node, inputs first.
    #[must_use]
    pub fn explain(&self) -> String {
        let mut text = String::new();
        for node in self.graph.topological(&self.roots()) {
            let inputs: Vec<String> = node
                .operator
                .inputs()
                .iter()
                .map(ToString::to_string)
                .collect();
            // Writing into a String cannot fail.
            let _ = write!(
                text,
                "{} {} {}",
                node.id,
                node.operator.name(),
                node.operator.describe()
            );
            if !inputs.is_empty() {
                let _ = write!(text, " <- {}", inputs.join(", "));
            }
            text.push('\n');
        }
        for block in &self.blocks {
            let _ = writeln!(text, "block {} = {}", block.block_id, block.root);
        }
        text
    }
}

impl Canonical for SemanticPlan {
    fn canonical_value(&self) -> C {
        let reachable = self.graph.reachable(&self.roots());
        C::record(
            "semantic-plan",
            [
                ("seir", C::text("0.1")),
                ("nodes", C::set(reachable)),
                (
                    "blocks",
                    C::Sequence(
                        self.blocks
                            .iter()
                            .map(|block| {
                                C::record(
                                    "plan-block",
                                    [
                                        ("block_id", C::text(block.block_id.clone())),
                                        ("root", block.root.canonical_value()),
                                    ],
                                )
                            })
                            .collect(),
                    ),
                ),
            ],
        )
    }
}
