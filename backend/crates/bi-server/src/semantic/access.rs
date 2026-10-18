//! Who may use what (ADR 0010 and 0012). Roles come from the project's
//! `policies/members.yaml`, never from the request; policies from
//! `policies/*.cedar`, or the base roles when there are none; resources from
//! the governance CSMIR keeps on each object. A query's uses are decided
//! twice on the same snapshot: what it asks for, before validation, and
//! everything its plan reads, after planning.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use sef_authz::{
    AttrValue, PolicySnapshot, PolicySource, Principal, Resource, ResourceKind, Usage, Use,
};
use sef_exec_ir::SemanticPlan;
use sef_model_ir::identity::StableId;
use sef_model_ir::model::{Classification, Lifecycle, ModelDefinition, ObjectMetadata};
use sef_query_ir::query::{
    FilterExpr, FilterTarget, QualifiedDimensionRef, SelectionItem, SemanticQueryBlock,
};
use serde::Deserialize;

use crate::models::{project_subdir, read_file};

/// The edge's fragment of the Cedar schema (ADR 0012 §7): users in roles,
/// no request attributes yet, and the audit trail with the one action that
/// reads it (#158), applicable to nothing else.
pub const PLATFORM_SCHEMA: &str = "\
type RequestContext = {};
entity Role;
entity User in [Role];
entity Dashboard = { classification: String };
entity Source;
entity AuditLog;
action \"view_audit\" appliesTo {
  principal: [User],
  resource: [AuditLog],
  context: RequestContext,
};
";

const MAX_POLICY_FILES: usize = 100;
const MAX_ENTRIES: usize = 4096;
const MAX_POLICY_BYTES: usize = 8 * 1024 * 1024;

/// Policies and role membership of one project, read together.
pub struct Access {
    pub snapshot: PolicySnapshot,
    pub members: Members,
}

/// Why the project's access configuration cannot be used: the semantic
/// path stays closed, never open.
#[derive(Debug)]
pub enum AccessError {
    Unreadable,
    Invalid(Vec<String>),
}

/// `policies/members.yaml`: verified identities to roles. Groups grant
/// nothing by themselves; only what this file declares.
#[derive(Debug, Default)]
pub struct Members {
    users: BTreeMap<String, BTreeSet<String>>,
    groups: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MembersFile {
    version: u32,
    #[serde(default)]
    users: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    groups: BTreeMap<String, Vec<String>>,
}

impl Members {
    fn parse(text: &str) -> Result<Self, String> {
        let file: MembersFile = serde_saphyr::from_str(text)
            .map_err(|_| "policies/members.yaml no es un documento válido.".to_owned())?;
        if file.version != 1 {
            return Err("policies/members.yaml declara una versión desconocida.".to_owned());
        }
        let roles = |entries: BTreeMap<String, Vec<String>>, what: &str| {
            entries
                .into_iter()
                .map(|(name, roles)| {
                    if name.is_empty() || name.len() > 255 {
                        return Err(format!("Un {what} de members.yaml tiene un nombre vacío o demasiado largo."));
                    }
                    let roles = roles
                        .into_iter()
                        .map(|role| {
                            valid_role(&role).then_some(role).ok_or_else(|| {
                                "Rol no válido en members.yaml: usa minúsculas, cifras y guiones bajos.".to_owned()
                            })
                        })
                        .collect::<Result<BTreeSet<_>, _>>()?;
                    Ok((name, roles))
                })
                .collect::<Result<BTreeMap<_, _>, String>>()
        };
        Ok(Self {
            users: roles(file.users, "usuario")?,
            groups: roles(file.groups, "grupo")?,
        })
    }

    /// The roles of a user and the verified groups they belong to.
    #[must_use]
    pub fn roles(&self, user: &str, groups: &[String]) -> BTreeSet<String> {
        let mut roles = self.users.get(user).cloned().unwrap_or_default();
        for group in groups {
            roles.extend(self.groups.get(group).into_iter().flatten().cloned());
        }
        roles
    }
}

fn valid_role(role: &str) -> bool {
    let mut chars = role.chars();
    role.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Reads `policies/` of the project (blocking: call off the async executor).
///
/// # Errors
/// An unreadable directory or file, or an invalid policy set or members file.
pub fn load(project: &Path) -> Result<Access, AccessError> {
    let (sources, members) = match project_subdir(project, "policies") {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (Vec::new(), None),
        Err(_) => return Err(AccessError::Unreadable),
        Ok(dir) => read_policies(&dir)?,
    };
    let sources = if sources.is_empty() {
        vec![PolicySource {
            path: "policies/base.cedar".to_owned(),
            text: sef_authz::BASE_POLICIES.to_owned(),
        }]
    } else {
        sources
    };
    let members = members
        .map(|text| Members::parse(&text))
        .transpose()
        .map_err(|message| AccessError::Invalid(vec![message]))?
        .unwrap_or_default();
    let (snapshot, _warnings) =
        PolicySnapshot::load(PLATFORM_SCHEMA, &sources).map_err(|problems| {
            AccessError::Invalid(
                problems
                    .into_iter()
                    .map(|problem| format!("{}: {}", problem.path, problem.message))
                    .collect(),
            )
        })?;
    Ok(Access { snapshot, members })
}

fn read_policies(
    dir: &cap_std::fs::Dir,
) -> Result<(Vec<PolicySource>, Option<String>), AccessError> {
    let mut names = Vec::new();
    for (i, entry) in dir
        .entries()
        .map_err(|_| AccessError::Unreadable)?
        .enumerate()
    {
        if i >= MAX_ENTRIES {
            return Err(AccessError::Invalid(vec![
                "La carpeta policies tiene demasiadas entradas.".to_owned(),
            ]));
        }
        let name = entry.map_err(|_| AccessError::Unreadable)?.file_name();
        let Some(text) = name.to_str() else { continue };
        let policy = Path::new(text)
            .extension()
            .is_some_and(|extension| extension == "cedar");
        if policy || text == "members.yaml" {
            names.push(text.to_owned());
        }
    }
    if names.len() > MAX_POLICY_FILES {
        return Err(AccessError::Invalid(vec![format!(
            "La carpeta policies supera {MAX_POLICY_FILES} archivos."
        )]));
    }
    names.sort();
    let mut total = 0;
    let mut sources = Vec::new();
    let mut members = None;
    for name in names {
        let text = read_file(dir, std::ffi::OsStr::new(&name)).map_err(|diagnostic| {
            AccessError::Invalid(vec![format!("policies/{name}: {}", diagnostic.message)])
        })?;
        total += text.len();
        if total > MAX_POLICY_BYTES {
            return Err(AccessError::Invalid(vec![
                "Las políticas superan 8 MiB en conjunto.".to_owned(),
            ]));
        }
        if name == "members.yaml" {
            members = Some(text);
        } else {
            sources.push(PolicySource {
                path: format!("policies/{name}"),
                text,
            });
        }
    }
    Ok((sources, members))
}

/// The Cedar principal of a verified session: its roles come from the
/// project, never from the request.
#[must_use]
pub fn principal(members: &Members, user: &str, groups: &[String]) -> Principal {
    Principal {
        id: user.to_owned(),
        roles: members.roles(user, groups),
        attributes: BTreeMap::new(),
    }
}

/// The resources a decision can be about: the model and its governed
/// datasets, metrics and dimensions.
pub struct Resources {
    by_id: BTreeMap<StableId, Resource>,
}

const fn classification(value: Classification) -> &'static str {
    match value {
        Classification::Public => "public",
        Classification::Internal => "internal",
        Classification::Confidential => "confidential",
        Classification::Restricted => "restricted",
    }
}

impl Resources {
    #[must_use]
    pub fn of(model: &ModelDefinition) -> Self {
        let root = Resource {
            kind: ResourceKind::Model,
            id: model.metadata.id().as_str().to_owned(),
            attributes: BTreeMap::from([(
                "classification".to_owned(),
                AttrValue::String(
                    classification(model.metadata.governance.classification).to_owned(),
                ),
            )]),
            parents: Vec::new(),
        };
        let governed = |kind, metadata: &ObjectMetadata| {
            let governance = &metadata.governance;
            let resource = Resource {
                kind,
                id: metadata.id().as_str().to_owned(),
                attributes: BTreeMap::from([
                    (
                        "classification".to_owned(),
                        AttrValue::String(classification(governance.classification).to_owned()),
                    ),
                    (
                        "tags".to_owned(),
                        AttrValue::Set(
                            governance
                                .tags
                                .iter()
                                .map(|tag| AttrValue::String(tag.clone()))
                                .collect(),
                        ),
                    ),
                    (
                        "deprecated".to_owned(),
                        AttrValue::Bool(matches!(
                            governance.lifecycle,
                            Lifecycle::Deprecated { .. }
                        )),
                    ),
                ]),
                parents: vec![root.clone()],
            };
            (metadata.id().clone(), resource)
        };
        let by_id = model
            .datasets
            .iter()
            .map(|d| governed(ResourceKind::Dataset, &d.metadata))
            .chain(
                model
                    .metrics
                    .iter()
                    .map(|m| governed(ResourceKind::Metric, &m.metadata)),
            )
            .chain(
                model
                    .dimensions
                    .iter()
                    .map(|d| governed(ResourceKind::Dimension, &d.metadata)),
            )
            .collect();
        Self { by_id }
    }

    /// The governed object `id` names, if it is one.
    #[must_use]
    pub fn get(&self, id: &StableId) -> Option<&Resource> {
        self.by_id.get(id)
    }
}

/// Uses in request order, each object and usage once.
#[derive(Default)]
pub struct Uses {
    seen: BTreeSet<(String, Usage)>,
    /// The objects the request itself named (selections and filter
    /// targets): the only ones an answer may call by name.
    named: BTreeSet<String>,
    pub list: Vec<Use>,
}

impl Uses {
    fn add(&mut self, resources: &Resources, id: &StableId, usage: Usage) {
        if let Some(resource) = resources.get(id)
            && self.seen.insert((id.as_str().to_owned(), usage))
        {
            self.list.push(Use {
                resource: resource.clone(),
                usage,
            });
        }
    }

    fn contains(&self, id: &StableId) -> bool {
        self.seen.iter().any(|(seen, _)| seen == id.as_str())
    }

    fn name(&mut self, id: &StableId) {
        self.named.insert(id.as_str().to_owned());
    }

    /// Whether the request named this object: a denial or a diagnostic may
    /// call it by name only then.
    #[must_use]
    pub fn is_named(&self, id: &str) -> bool {
        self.named.contains(id)
    }

    /// The identifiers the request named.
    pub fn named_ids(&self) -> impl Iterator<Item = &str> {
        self.named.iter().map(String::as_str)
    }

    /// Every metric `metric` is computed from, transitively. Deriving
    /// reveals values, so each needs its own `Lineage` use, also when the
    /// same metric is selected and shown masked (ADR 0012 §5).
    fn derivations(&mut self, resources: &Resources, model: &ModelDefinition, metric: &StableId) {
        let mut found = BTreeSet::new();
        let mut pending = vec![metric.clone()];
        while let Some(id) = pending.pop() {
            let Some(definition) = model.metrics.iter().find(|m| m.metadata.id() == &id) else {
                continue;
            };
            for dependency in definition.expression.metric_refs() {
                if found.insert(dependency.clone()) {
                    self.add(resources, &dependency, Usage::Lineage);
                    pending.push(dependency);
                }
            }
        }
    }

    fn join(&mut self, resources: &Resources, model: &ModelDefinition, role: &str) {
        for relationship in model.relationships.iter().filter(|r| r.role == role) {
            self.add(resources, &relationship.from_dataset, Usage::Join);
            self.add(resources, &relationship.to_dataset, Usage::Join);
        }
    }

    fn dimension(
        &mut self,
        resources: &Resources,
        model: &ModelDefinition,
        dimension: &QualifiedDimensionRef,
        usage: Usage,
    ) {
        self.name(&dimension.dimension);
        self.add(resources, &dimension.dimension, usage);
        for role in &dimension.roles {
            self.join(resources, model, role);
        }
    }

    fn filter(
        &mut self,
        resources: &Resources,
        model: &ModelDefinition,
        block: &SemanticQueryBlock,
        filter: &FilterExpr,
    ) {
        let mut pending = vec![filter];
        while let Some(filter) = pending.pop() {
            let target = match filter {
                FilterExpr::Compare { target, .. }
                | FilterExpr::Between { target, .. }
                | FilterExpr::In { target, .. }
                | FilterExpr::IsNull(target)
                | FilterExpr::IsNotNull(target) => target,
                FilterExpr::And(filters) | FilterExpr::Or(filters) => {
                    pending.extend(filters);
                    continue;
                }
                FilterExpr::Not(filter) => {
                    pending.push(filter);
                    continue;
                }
            };
            match target {
                FilterTarget::Dimension(dimension) => {
                    self.dimension(resources, model, dimension, Usage::Filter);
                }
                FilterTarget::Metric(selection) => {
                    if let Some(metric) = block.metric_of(selection) {
                        self.add(resources, metric, Usage::Filter);
                    }
                }
            }
        }
    }
}

/// What a bound block asks for: projections and grouping of its
/// selections, filters, ordering and the relationships its roles name.
#[must_use]
pub fn requested(
    resources: &Resources,
    model: &ModelDefinition,
    block: &SemanticQueryBlock,
) -> Uses {
    let mut uses = Uses::default();
    for selection in &block.selections {
        match &selection.item {
            SelectionItem::Metric(metric) => {
                uses.name(metric);
                uses.add(resources, metric, Usage::Project);
            }
            SelectionItem::Dimension(dimension) => {
                uses.dimension(resources, model, dimension, Usage::Project);
                uses.dimension(resources, model, dimension, Usage::Group);
            }
            SelectionItem::TimeDimension(time) => {
                uses.dimension(resources, model, &time.dimension, Usage::Project);
                uses.dimension(resources, model, &time.dimension, Usage::Group);
            }
        }
    }
    // What the selected metrics are derived from, before validating: a
    // masked metric shown next to one derived from it does not waive it.
    for selection in &block.selections {
        if let SelectionItem::Metric(metric) = &selection.item {
            uses.derivations(resources, model, metric);
        }
    }
    for filter in block.filter.iter().chain(&block.post_aggregate_filter) {
        uses.filter(resources, model, block, filter);
    }
    for order in &block.order_by {
        let item = block
            .selections
            .iter()
            .find(|selection| selection.selection_id == order.selection_id)
            .map(|selection| &selection.item);
        match item {
            Some(SelectionItem::Metric(metric)) => uses.add(resources, metric, Usage::Order),
            Some(SelectionItem::Dimension(dimension)) => {
                uses.dimension(resources, model, dimension, Usage::Order);
            }
            Some(SelectionItem::TimeDimension(time)) => {
                uses.dimension(resources, model, &time.dimension, Usage::Order);
            }
            None => {}
        }
    }
    uses
}

/// Adds everything the plan reads beyond what was asked: the datasets it
/// scans, the metrics and dimensions it derives from, and the datasets its
/// relationships and bridges join.
pub fn read_by_plan(
    uses: &mut Uses,
    resources: &Resources,
    model: &ModelDefinition,
    plan: &SemanticPlan,
) {
    let roots: Vec<_> = plan.blocks.iter().map(|block| block.root).collect();
    for node in plan.graph.topological(&roots) {
        for object in node.operator.objects() {
            let id = &object.id;
            // Looked up in the model, never inferred from the identifier.
            if let Some(resource) = resources.get(id) {
                let derived = matches!(
                    resource.kind,
                    ResourceKind::Metric | ResourceKind::Dimension
                );
                if resource.kind == ResourceKind::Dataset || (derived && !uses.contains(id)) {
                    uses.add(resources, id, Usage::Lineage);
                }
            } else if let Some(measure) = model.measures.iter().find(|m| m.metadata.id() == id) {
                uses.add(resources, &measure.dataset, Usage::Lineage);
            } else if let Some(r) = model.relationships.iter().find(|r| r.metadata.id() == id) {
                uses.add(resources, &r.from_dataset, Usage::Join);
                uses.add(resources, &r.to_dataset, Usage::Join);
            } else if let Some(b) = model.bridges.iter().find(|b| b.metadata.id() == id) {
                for dataset in [&b.left_dataset, &b.bridge_dataset, &b.right_dataset] {
                    uses.add(resources, dataset, Usage::Join);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proptest::prelude::*;
    use sef_model_ir::expressions::ComparisonOp;
    use sef_model_ir::identity::SourceHandle;
    use sef_model_ir::time::SortDirection;
    use sef_model_ir::types::{DecimalType, Literal, ValueType};
    use sef_ossie::catalog::{Catalog, Column, Relation};
    use sef_query_ir::query::{NullOrdering, OrderTerm, Selection};

    use super::*;

    const MODEL: &str = r#"
version: '0.2.0.dev0'
name: Ventas
datasets:
  - name: sales
    source: sales
    primary_key: [order_id]
    fields:
      - name: order_id
        expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: order_id}]}
      - name: region
        expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: region}]}
        dimension: {}
      - name: channel
        expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: channel}]}
        dimension: {}
      - name: amount
        expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: amount}]}
metrics:
  - name: importe
    expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: SUM(sales.amount)}]}
  - name: margen
    custom_extensions:
      - vendor_name: bi-platform
        data: '{"version": 1, "sensitivity": "confidential"}'
    expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: SUM(sales.amount)}]}
  - name: doble
    expression: {dialects: [{dialect: OSSIE_SQL_2026, expression: importe + importe}]}
"#;

    fn model() -> ModelDefinition {
        let column = |name: &str, value_type| Column {
            name: name.to_owned(),
            handle: SourceHandle::new(name).expect("handle"),
            value_type,
            nullable: true,
        };
        let catalog = Catalog {
            relations: BTreeMap::from([(
                "sales".to_owned(),
                Relation {
                    source: SourceHandle::new("test").expect("handle"),
                    relation: SourceHandle::new("sales").expect("handle"),
                    columns: vec![
                        column(
                            "order_id",
                            ValueType::Integer {
                                signed: true,
                                bits: Some(64),
                            },
                        ),
                        column("region", ValueType::Utf8),
                        column("channel", ValueType::Utf8),
                        column(
                            "amount",
                            ValueType::Decimal(DecimalType::new(18, 2).expect("decimal")),
                        ),
                    ],
                },
            )]),
        };
        let document = sef_ossie::load(MODEL).expect("loads").model;
        sef_ossie::csmir::to_csmir(
            &document,
            &catalog,
            &sef_ossie::csmir::Options {
                namespace: "test".to_owned(),
                model: "ventas".to_owned(),
            },
        )
        .expect("converts")
    }

    #[test]
    fn members_grant_only_what_the_file_declares() {
        let members = Members::parse(
            "version: 1\nusers:\n  ana: [viewer]\ngroups:\n  finanzas: [analyst, viewer]\n",
        )
        .expect("valid");
        let roles = |user, groups: &[&str]| {
            let groups: Vec<String> = groups.iter().map(|g| (*g).to_owned()).collect();
            members.roles(user, &groups).into_iter().collect::<Vec<_>>()
        };
        assert_eq!(roles("ana", &[]), ["viewer"]);
        assert_eq!(roles("ana", &["finanzas"]), ["analyst", "viewer"]);
        // A group or a user the file does not mention grants nothing.
        assert_eq!(roles("luis", &["otro"]), Vec::<String>::new());
        assert_eq!(roles("luis", &[]), Vec::<String>::new());
    }

    #[test]
    fn an_invalid_members_file_is_refused() {
        for text in [
            "version: 2\n",
            "version: 1\nusers:\n  ana: [Admin]\n",
            "version: 1\nusers:\n  ana: ['']\n",
            "version: 1\nroles: [admin]\n",
            "users: {ana: [viewer]}\n",
            "version: 1\nusers:\n  '': [viewer]\n",
        ] {
            assert!(Members::parse(text).is_err(), "{text:?}");
        }
    }

    fn dimension(model: &ModelDefinition, name: &str) -> QualifiedDimensionRef {
        let dimension = model
            .dimensions
            .iter()
            .find(|d| d.metadata.id().as_str().ends_with(name))
            .expect("dimension");
        QualifiedDimensionRef {
            dimension: dimension.metadata.id().clone(),
            roles: Vec::new(),
        }
    }

    fn metric(model: &ModelDefinition, name: &str) -> StableId {
        model
            .metrics
            .iter()
            .find(|m| m.metadata.id().as_str().ends_with(&format!("/{name}")))
            .expect("metric")
            .metadata
            .id()
            .clone()
    }

    /// A block over the test model: which metrics and dimensions it
    /// selects, which dimensions it filters, whether it filters the first
    /// metric after aggregating, and which selections it orders by.
    #[derive(Debug, Clone)]
    struct Shape {
        metrics: Vec<bool>,
        dimensions: Vec<bool>,
        filtered: Vec<bool>,
        having: bool,
        ordered: Vec<bool>,
    }

    fn shape() -> impl Strategy<Value = Shape> {
        (
            prop::collection::vec(any::<bool>(), 3),
            prop::collection::vec(any::<bool>(), 2),
            prop::collection::vec(any::<bool>(), 2),
            any::<bool>(),
            prop::collection::vec(any::<bool>(), 5),
        )
            .prop_map(|(metrics, dimensions, filtered, having, ordered)| Shape {
                metrics,
                dimensions,
                filtered,
                having,
                ordered,
            })
    }

    fn block(model: &ModelDefinition, shape: &Shape) -> SemanticQueryBlock {
        let metrics = ["importe", "margen", "doble"].map(|name| metric(model, name));
        let dimensions = ["region", "channel"].map(|name| dimension(model, name));
        let mut selections = Vec::new();
        for (i, metric) in metrics.iter().enumerate() {
            if shape.metrics[i] {
                selections.push(Selection {
                    selection_id: format!("m{i}"),
                    item: SelectionItem::Metric(metric.clone()),
                    output_label: None,
                });
            }
        }
        for (i, dimension) in dimensions.iter().enumerate() {
            if shape.dimensions[i] {
                selections.push(Selection {
                    selection_id: format!("d{i}"),
                    item: SelectionItem::Dimension(dimension.clone()),
                    output_label: None,
                });
            }
        }
        let filters: Vec<FilterExpr> = dimensions
            .iter()
            .zip(&shape.filtered)
            .filter(|(_, on)| **on)
            .map(|(dimension, _)| FilterExpr::Compare {
                target: FilterTarget::Dimension(dimension.clone()),
                op: ComparisonOp::Equal,
                value: Literal::Utf8("Norte".to_owned()),
            })
            .collect();
        let having = (shape.having && shape.metrics[0])
            .then(|| FilterExpr::IsNotNull(FilterTarget::Metric("m0".to_owned())));
        let order_by = selections
            .iter()
            .zip(&shape.ordered)
            .filter(|(_, on)| **on)
            .map(|(selection, _)| OrderTerm {
                selection_id: selection.selection_id.clone(),
                direction: SortDirection::Descending,
                nulls: NullOrdering::Last,
            })
            .collect();
        SemanticQueryBlock {
            block_id: "query".to_owned(),
            selections,
            filter: (!filters.is_empty()).then_some(FilterExpr::And(filters)),
            post_aggregate_filter: having,
            order_by,
            limit: None,
            offset: None,
        }
    }

    fn object(item: &SelectionItem) -> String {
        match item {
            SelectionItem::Metric(metric) => metric.as_str().to_owned(),
            SelectionItem::Dimension(dimension) => dimension.dimension.as_str().to_owned(),
            SelectionItem::TimeDimension(time) => time.dimension.dimension.as_str().to_owned(),
        }
    }

    /// What the uses must be, written independently of `requested`.
    fn expected(block: &SemanticQueryBlock) -> BTreeSet<(String, Usage)> {
        let mut expected = BTreeSet::new();
        for selection in &block.selections {
            let id = object(&selection.item);
            expected.insert((id.clone(), Usage::Project));
            if !matches!(selection.item, SelectionItem::Metric(_)) {
                expected.insert((id.clone(), Usage::Group));
            }
            // `doble = importe + importe` reads `importe`.
            if id.ends_with("/doble") {
                expected.insert((id.replace("/doble", "/importe"), Usage::Lineage));
            }
        }
        if let Some(FilterExpr::And(filters)) = &block.filter {
            for filter in filters {
                if let FilterExpr::Compare {
                    target: FilterTarget::Dimension(dimension),
                    ..
                } = filter
                {
                    expected.insert((dimension.dimension.as_str().to_owned(), Usage::Filter));
                }
            }
        }
        if block.post_aggregate_filter.is_some()
            && let Some(metric) = block.metric_of("m0")
        {
            expected.insert((metric.as_str().to_owned(), Usage::Filter));
        }
        for order in &block.order_by {
            let selection = block
                .selections
                .iter()
                .find(|s| s.selection_id == order.selection_id)
                .expect("ordered selection");
            expected.insert((object(&selection.item), Usage::Order));
        }
        expected
    }

    proptest! {
        #[test]
        fn every_use_a_query_asks_for_is_listed_exactly_once(shape in shape()) {
            let model = model();
            let resources = Resources::of(&model);
            let block = block(&model, &shape);
            let uses = requested(&resources, &model, &block);
            let listed: Vec<(String, Usage)> = uses
                .list
                .iter()
                .map(|u| (u.resource.id.clone(), u.usage))
                .collect();
            let unique: BTreeSet<_> = listed.iter().cloned().collect();
            prop_assert_eq!(unique.len(), listed.len(), "duplicated uses: {:?}", listed);
            prop_assert_eq!(unique, expected(&block));
            // Every use is about a governed object, with its model as parent.
            for used in &uses.list {
                prop_assert_eq!(used.resource.parents.len(), 1);
                prop_assert_eq!(used.resource.parents[0].kind, ResourceKind::Model);
            }
        }
    }

    #[test]
    fn governance_reaches_the_resources() {
        let model = model();
        let resources = Resources::of(&model);
        let margen = resources.get(&metric(&model, "margen")).expect("resource");
        assert_eq!(
            margen.attributes.get("classification"),
            Some(&AttrValue::String("confidential".to_owned()))
        );
        let importe = resources.get(&metric(&model, "importe")).expect("resource");
        assert_eq!(
            importe.attributes.get("classification"),
            Some(&AttrValue::String("internal".to_owned()))
        );
        assert_eq!(
            importe.attributes.get("deprecated"),
            Some(&AttrValue::Bool(false))
        );
    }
}
