//! Neutral contracts the edges supply (ADR 0012 §7): who asks, what they
//! want to do and on which object, with typed attributes. Nothing here knows
//! HTTP, storage or the modules that own these objects.

use std::collections::{BTreeMap, BTreeSet};

/// A value a policy can compare. Only Cedar's core types: no extension
/// function is needed to read them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AttrValue {
    Bool(bool),
    Long(i64),
    String(String),
    Set(BTreeSet<AttrValue>),
}

/// Request attributes, typed by the schema's `RequestContext`.
pub type Context = BTreeMap<String, AttrValue>;

/// The kinds of objects a decision is about: the semantic ones (owned by
/// SEF) and the platform ones (declared by the edge's schema fragment).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResourceKind {
    Model,
    Dataset,
    Metric,
    Dimension,
    Dashboard,
    Source,
    User,
    Role,
    /// The platform's audit trail, as one object: reading it is
    /// [`Action::ViewAudit`], never a semantic `view`.
    AuditLog,
}

impl ResourceKind {
    /// The Cedar entity type.
    #[must_use]
    pub const fn entity_type(self) -> &'static str {
        match self {
            Self::Model => "Model",
            Self::Dataset => "Dataset",
            Self::Metric => "Metric",
            Self::Dimension => "Dimension",
            Self::Dashboard => "Dashboard",
            Self::Source => "Source",
            Self::User => "User",
            Self::Role => "Role",
            Self::AuditLog => "AuditLog",
        }
    }
}

/// The actions of the owner's security model (document 6, ADR 0012 §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    View,
    ViewMasked,
    UseInDashboard,
    Edit,
    Share,
    Delete,
    ProposeChange,
    Approve,
    Publish,
    Connect,
    Configure,
    Manage,
    /// Reading the audit trail. Its own action, so that no rule written for
    /// the semantic ones (a modeler's `view` on any resource) reaches it.
    ViewAudit,
}

impl Action {
    pub const ALL: [Self; 13] = [
        Self::View,
        Self::ViewMasked,
        Self::UseInDashboard,
        Self::Edit,
        Self::Share,
        Self::Delete,
        Self::ProposeChange,
        Self::Approve,
        Self::Publish,
        Self::Connect,
        Self::Configure,
        Self::Manage,
        Self::ViewAudit,
    ];

    /// The Cedar action identifier.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::ViewMasked => "view_masked",
            Self::UseInDashboard => "use_in_dashboard",
            Self::Edit => "edit",
            Self::Share => "share",
            Self::Delete => "delete",
            Self::ProposeChange => "propose_change",
            Self::Approve => "approve",
            Self::Publish => "publish",
            Self::Connect => "connect",
            Self::Configure => "configure",
            Self::Manage => "manage",
            Self::ViewAudit => "view_audit",
        }
    }
}

/// Who asks: a verified identity (OIDC, #70), its roles and the attributes
/// the edge selected. Never text of the query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Principal {
    pub id: String,
    pub roles: BTreeSet<String>,
    pub attributes: BTreeMap<String, AttrValue>,
}

/// The object of a decision, with the objects that contain it (a metric's
/// model, for example) so policies can grant by containment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub kind: ResourceKind,
    pub id: String,
    pub attributes: BTreeMap<String, AttrValue>,
    pub parents: Vec<Resource>,
}
