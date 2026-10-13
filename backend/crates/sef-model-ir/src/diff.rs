//! What changes between two versions of a model, by meaning rather than by
//! text (#99). Every object with an identity is compared through its
//! canonical value, part by part:
//! - a change of expression, contract, policy, source or version is a change
//!   of **meaning**; label and description are **presentation**; owner, tags,
//!   lifecycle and classification are **governance** (neither enters the
//!   canonical value);
//! - an object that disappears while another of the same kind appears with
//!   exactly the same meaning is a **rename**, and references to it do not
//!   count as changes of the objects that make them;
//! - an object whose own definition is unchanged but that depends,
//!   directly or not, on a change of meaning is **affected**.
//!
//! It feeds the change plan of the ELT and "what do I break?" in the Studio.

use std::collections::{BTreeMap, BTreeSet};

use crate::canonical::CanonicalValue as C;
use crate::identity::StableId;
use crate::model::{CompiledSemanticModel, Governance};

/// Which part of an object's meaning changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Aspect {
    /// What it computes: expressions, aggregations, filters, dimension bindings.
    Expression,
    /// What it promises: types, units, grains, additivity, nulls, time behaviour.
    Contract,
    /// Who sees what: row policies and the references to them.
    Policy,
    /// Where the data comes from: source bindings and column handles.
    Source,
    /// The declared semantic version.
    Version,
    /// Any other definition: relationships, bridges, entities, calendars…
    Definition,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Added,
    Removed,
    /// The same object, possibly under a new identifier.
    Modified {
        renamed_from: Option<StableId>,
        /// Empty when the meaning is the same.
        meaning: BTreeSet<Aspect>,
        /// Label or description.
        presentation: bool,
        governance: bool,
    },
    /// Unchanged itself, but it depends on these changes of meaning.
    Affected {
        through: BTreeSet<StableId>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectChange {
    /// The identifier in the new version; the old one when removed.
    pub id: StableId,
    pub change: Change,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelDiff {
    /// One entry per changed object, in identifier order.
    pub changes: Vec<ObjectChange>,
}

impl ModelDiff {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// The change of the object an old reader knew as `old_id`: its own
    /// entry, or the one it was renamed into.
    #[must_use]
    pub fn of(&self, old_id: &StableId) -> Option<&ObjectChange> {
        self.changes.iter().find(|entry| match &entry.change {
            Change::Modified {
                renamed_from: Some(from),
                ..
            } => from == old_id,
            _ => &entry.id == old_id,
        })
    }
}

/// One object as the comparison sees it.
struct Entry {
    id: StableId,
    tag: String,
    /// Canonical parts, without the objects it contains (listed on their own).
    parts: Vec<(String, C)>,
    label: Option<String>,
    description: Option<String>,
    governance: Governance,
}

type Entries = BTreeMap<String, Entry>;
type Renames = BTreeMap<String, String>;

/// Compares two versions of a model.
#[must_use]
pub fn diff(old: &CompiledSemanticModel, new: &CompiledSemanticModel) -> ModelDiff {
    let before = entries(old);
    let after = entries(new);
    let renames = renames(&before, &after);
    let origins: BTreeMap<&str, &str> = renames
        .iter()
        .map(|(from, to)| (to.as_str(), from.as_str()))
        .collect();
    let mut changes: BTreeMap<StableId, Change> = BTreeMap::new();
    let mut changed_meaning = BTreeSet::new();
    for (key, entry) in &after {
        let old_key = origins.get(key.as_str()).copied().unwrap_or(key);
        let Some(previous) = before.get(old_key) else {
            changes.insert(entry.id.clone(), Change::Added);
            continue;
        };
        let meaning = aspects(previous, entry, &renames);
        let presentation =
            previous.label != entry.label || previous.description != entry.description;
        let governance = previous.governance != entry.governance;
        let from = (old_key != key).then(|| previous.id.clone());
        if !meaning.is_empty() {
            changed_meaning.insert(key.as_str());
        }
        if from.is_some() || !meaning.is_empty() || presentation || governance {
            changes.insert(
                entry.id.clone(),
                Change::Modified {
                    renamed_from: from,
                    meaning,
                    presentation,
                    governance,
                },
            );
        }
    }
    for (key, entry) in &before {
        if !after.contains_key(key) && !renames.contains_key(key) {
            changes.insert(entry.id.clone(), Change::Removed);
        }
    }
    for (id, through) in affected(&after, &changed_meaning) {
        changes.entry(id).or_insert(Change::Affected { through });
    }
    ModelDiff {
        changes: changes
            .into_iter()
            .map(|(id, change)| ObjectChange { id, change })
            .collect(),
    }
}

fn entries(model: &CompiledSemanticModel) -> Entries {
    model
        .definition()
        .semantic_objects()
        .into_iter()
        .filter_map(|(metadata, value)| {
            let C::Variant { tag, fields } = value else {
                return None;
            };
            let parts = fields
                .into_iter()
                .filter(|(field, _)| !contained(&tag, field))
                .collect();
            let id = metadata.id().clone();
            Some((
                id.as_str().to_owned(),
                Entry {
                    id,
                    tag,
                    parts,
                    label: metadata.label.clone(),
                    description: metadata.description.clone(),
                    governance: metadata.governance.clone(),
                },
            ))
        })
        .collect()
}

/// Members that hold objects compared on their own.
fn contained(tag: &str, field: &str) -> bool {
    match tag {
        "compiled-semantic-model" => matches!(
            field,
            "namespaces"
                | "source_bindings"
                | "datasets"
                | "entities"
                | "dimensions"
                | "measures"
                | "metrics"
                | "relationships"
                | "bridges"
                | "policies"
                | "calendars"
                | "unit_conversions"
        ),
        "semantic-dataset" => field == "fields",
        "entity" => field == "keys",
        _ => false,
    }
}

fn aspect(tag: &str, field: &str) -> Aspect {
    match (tag, field) {
        // The identity, once renames are mapped, differs only by version.
        (_, "metadata") => Aspect::Version,
        (_, "policy_refs") | ("policy", _) => Aspect::Policy,
        ("metric", "expression" | "intrinsic_filter" | "dependencies")
        | ("measure", "scalar_expression" | "aggregation" | "intrinsic_filter" | "dataset")
        | ("dimension", "bindings" | "participating_datasets") => Aspect::Expression,
        ("semantic-dataset", "source_binding")
        | ("source-binding", _)
        | ("field", "binding" | "dataset") => Aspect::Source,
        ("metric" | "measure" | "dimension" | "field" | "semantic-dataset" | "entity-key", _) => {
            Aspect::Contract
        }
        _ => Aspect::Definition,
    }
}

/// The aspects in which `before`, read with `renames`, differs from `after`.
fn aspects(before: &Entry, after: &Entry, renames: &Renames) -> BTreeSet<Aspect> {
    let mut found = BTreeSet::new();
    if before.tag != after.tag {
        found.insert(Aspect::Definition);
        return found;
    }
    let old: BTreeMap<&str, &C> = before.parts.iter().map(|(k, v)| (k.as_str(), v)).collect();
    let new: BTreeMap<&str, &C> = after.parts.iter().map(|(k, v)| (k.as_str(), v)).collect();
    let fields: BTreeSet<&str> = old.keys().chain(new.keys()).copied().collect();
    for field in fields {
        let (Some(a), Some(b)) = (old.get(field), new.get(field)) else {
            found.insert(aspect(&after.tag, field));
            continue;
        };
        let (a, b) = (normalized(a, renames), normalized(b, &Renames::new()));
        if a == b {
            continue;
        }
        // Expression nodes carry their type: an expression that only
        // changed types (a wider column) computes the same thing under
        // another contract.
        let aspect = match aspect(&after.tag, field) {
            Aspect::Expression if untyped(&a) == untyped(&b) => Aspect::Contract,
            aspect => aspect,
        };
        found.insert(aspect);
    }
    found
}

/// `value` without the types, units and arithmetic contracts of its
/// expression nodes.
fn untyped(value: &C) -> C {
    match value {
        C::Variant { tag, fields } => {
            let expression = tag.starts_with("scalar-expr") || tag.starts_with("metric-expr");
            C::Variant {
                tag: tag.clone(),
                fields: fields
                    .iter()
                    .filter(|(field, _)| {
                        !(expression
                            && matches!(field.as_str(), "value_type" | "unit" | "contract"))
                    })
                    .map(|(field, item)| (field.clone(), untyped(item)))
                    .collect(),
            }
        }
        C::Sequence(items) => C::Sequence(items.iter().map(untyped).collect()),
        C::Set(items) => C::Set(items.iter().map(untyped).collect()),
        C::Map(items) => C::Map(items.iter().map(|(k, v)| (k.clone(), untyped(v))).collect()),
        C::Present(inner) => C::Present(Box::new(untyped(inner))),
        other => other.clone(),
    }
}

/// `value` with every identifier in `renames` replaced by its new one and
/// every set in one order: a set keeps its insertion order until it is
/// encoded, and a rename may move an element.
fn normalized(value: &C, renames: &Renames) -> C {
    let parts = |items: &[(String, C)]| {
        items
            .iter()
            .map(|(k, v)| (k.clone(), normalized(v, renames)))
            .collect()
    };
    match value {
        C::Text(text) => C::Text(renames.get(text).cloned().unwrap_or_else(|| text.clone())),
        C::Sequence(items) => C::Sequence(items.iter().map(|v| normalized(v, renames)).collect()),
        C::Set(items) => {
            let mut items: Vec<(String, C)> = items
                .iter()
                .map(|v| {
                    let v = normalized(v, renames);
                    (format!("{v:?}"), v)
                })
                .collect();
            items.sort_by(|a, b| a.0.cmp(&b.0));
            C::Set(items.into_iter().map(|(_, v)| v).collect())
        }
        C::Map(items) => C::Map(parts(items)),
        C::Variant { tag, fields } => C::Variant {
            tag: tag.clone(),
            fields: parts(fields),
        },
        C::Present(inner) => C::Present(Box::new(normalized(inner, renames))),
        other => other.clone(),
    }
}

/// Old identifier → new one, for objects that changed only their
/// identifier. A pair is accepted only when each side has exactly one
/// candidate; references between renamed objects resolve over rounds.
fn renames(before: &Entries, after: &Entries) -> Renames {
    let removed: Vec<&Entry> = before
        .iter()
        .filter(|(key, _)| !after.contains_key(*key))
        .map(|(_, entry)| entry)
        .collect();
    let added: Vec<&Entry> = after
        .iter()
        .filter(|(key, _)| !before.contains_key(*key))
        .map(|(_, entry)| entry)
        .collect();
    let mut renames = Renames::new();
    loop {
        let taken: BTreeSet<&str> = renames.values().map(String::as_str).collect();
        let mut candidates: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut claims: BTreeMap<&str, usize> = BTreeMap::new();
        for from in removed
            .iter()
            .filter(|entry| !renames.contains_key(entry.id.as_str()))
        {
            for to in added
                .iter()
                .filter(|entry| !taken.contains(entry.id.as_str()))
            {
                if from.id.kind() != to.id.kind() {
                    continue;
                }
                let mut trial = renames.clone();
                trial.insert(from.id.as_str().to_owned(), to.id.as_str().to_owned());
                if aspects(from, to, &trial).is_empty() {
                    candidates
                        .entry(from.id.as_str())
                        .or_default()
                        .push(to.id.as_str());
                    *claims.entry(to.id.as_str()).or_default() += 1;
                }
            }
        }
        let accepted: Vec<(String, String)> = candidates
            .into_iter()
            .filter_map(|(from, tos)| match tos.as_slice() {
                [to] if claims.get(to) == Some(&1) => Some((from.to_owned(), (*to).to_owned())),
                _ => None,
            })
            .collect();
        if accepted.is_empty() {
            return renames;
        }
        renames.extend(accepted);
    }
}

/// Objects with no change of meaning of their own that depend, transitively,
/// on one, with the changes they reach.
fn affected(after: &Entries, changed: &BTreeSet<&str>) -> BTreeMap<StableId, BTreeSet<StableId>> {
    let references: BTreeMap<&str, BTreeSet<&str>> = after
        .iter()
        .map(|(key, entry)| {
            let mut found = BTreeSet::new();
            for (field, value) in &entry.parts {
                if reads(&entry.tag, field) {
                    mentions(value, after, &mut found);
                }
            }
            found.remove(key.as_str());
            (key.as_str(), found)
        })
        .collect();
    let mut result = BTreeMap::new();
    for (key, entry) in after {
        if changed.contains(key.as_str()) {
            continue;
        }
        let mut reached = BTreeSet::new();
        let mut seen = BTreeSet::from([key.as_str()]);
        let mut pending: Vec<&str> = references
            .get(key.as_str())
            .into_iter()
            .flatten()
            .copied()
            .collect();
        while let Some(next) = pending.pop() {
            if !seen.insert(next) {
                continue;
            }
            if changed.contains(next)
                && let Some(target) = after.get(next)
            {
                reached.insert(target.id.clone());
            }
            pending.extend(references.get(next).into_iter().flatten().copied());
        }
        if !reached.is_empty() {
            result.insert(entry.id.clone(), reached);
        }
    }
    result
}

/// Whether a part names what the object reads, rather than what it
/// describes: a source binding lists the fields bound to it and a dataset
/// its keys and time roles, but those fields depend on them, not the other
/// way round; following such parts would make every object of a dataset
/// depend on every field.
fn reads(tag: &str, field: &str) -> bool {
    !matches!(
        (tag, field),
        (_, "metadata")
            | ("source-binding", "field_bindings")
            | ("semantic-dataset", "key_bindings" | "time_roles")
    )
}

/// The identifiers of `entries` that `value` mentions.
fn mentions<'a>(value: &C, entries: &'a Entries, found: &mut BTreeSet<&'a str>) {
    match value {
        C::Text(text) => {
            if let Some((key, _)) = entries.get_key_value(text.as_str()) {
                found.insert(key.as_str());
            }
        }
        C::Sequence(items) | C::Set(items) => {
            for item in items {
                mentions(item, entries, found);
            }
        }
        C::Map(items) | C::Variant { fields: items, .. } => {
            for (_, item) in items {
                mentions(item, entries, found);
            }
        }
        C::Present(inner) => mentions(inner, entries, found),
        _ => {}
    }
}
