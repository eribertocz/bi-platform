//! A platform schema fragment like the one an edge declares, the base
//! policies, and builders for principals and semantic objects.
#![allow(clippy::expect_used, clippy::panic, dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use sef_authz::{
    AttrValue, BASE_POLICIES, Context, PolicySnapshot, PolicySource, Principal, Resource,
    ResourceKind,
};

pub const PLATFORM: &str = r"
type RequestContext = {
  risk?: Long,
};
entity Role;
entity User in [Role] = {
  department?: String,
};
entity Dashboard = {
  classification: String,
};
entity Source;
";

pub fn source(path: &str, text: &str) -> PolicySource {
    PolicySource {
        path: path.to_owned(),
        text: text.to_owned(),
    }
}

/// The base roles plus `extra` policies.
pub fn snapshot(extra: &str) -> PolicySnapshot {
    let mut sources = vec![source("policies/base.cedar", BASE_POLICIES)];
    if !extra.is_empty() {
        sources.push(source("policies/extra.cedar", extra));
    }
    PolicySnapshot::load(PLATFORM, &sources)
        .unwrap_or_else(|problems| panic!("policies must load: {problems:#?}"))
        .0
}

pub fn principal(roles: &[&str]) -> Principal {
    Principal {
        id: "ana".to_owned(),
        roles: roles.iter().map(|role| (*role).to_owned()).collect(),
        attributes: BTreeMap::new(),
    }
}

pub fn text(value: &str) -> AttrValue {
    AttrValue::String(value.to_owned())
}

pub fn model(id: &str) -> Resource {
    Resource {
        kind: ResourceKind::Model,
        id: id.to_owned(),
        attributes: BTreeMap::from([("classification".to_owned(), text("internal"))]),
        parents: Vec::new(),
    }
}

/// A governed semantic object inside model `sales`.
pub fn governed(kind: ResourceKind, id: &str, classification: &str, tags: &[&str]) -> Resource {
    Resource {
        kind,
        id: id.to_owned(),
        attributes: BTreeMap::from([
            ("classification".to_owned(), text(classification)),
            (
                "tags".to_owned(),
                AttrValue::Set(tags.iter().map(|tag| text(tag)).collect::<BTreeSet<_>>()),
            ),
            ("deprecated".to_owned(), AttrValue::Bool(false)),
        ]),
        parents: vec![model("sales")],
    }
}

pub fn metric(id: &str, classification: &str) -> Resource {
    governed(ResourceKind::Metric, id, classification, &[])
}

pub fn none() -> Context {
    Context::new()
}

pub fn risk(value: i64) -> Context {
    Context::from([("risk".to_owned(), AttrValue::Long(value))])
}

/// What the base roles allow, written independently of the policy text.
pub fn expected(roles: &BTreeSet<String>, classification: &str, action: &str) -> bool {
    let has = |role: &str| roles.contains(role);
    let open = matches!(classification, "public" | "internal");
    match action {
        "view" | "use_in_dashboard" => {
            has("admin")
                || has("modeler")
                || (has("analyst") && classification != "restricted")
                || (has("viewer") && open)
        }
        "view_masked" => {
            has("admin")
                || (has("analyst") && classification == "restricted")
                || (has("viewer") && classification == "confidential")
        }
        other => panic!("unexpected action {other}"),
    }
}
