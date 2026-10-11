# Correctness and Semantic Planning

This document defines the Semantic Intelligence Core: correctness analysis, semantic facts/obligations, deterministic planning, SEIR operators and obligation discharge. It intentionally excludes physical engine choices.

---

# Part I — Semantic Correctness

Status: **Correctness Engine canonical design v0.1**  
Date: **2026-09-27**

## 1. Correctness purpose

The Semantic Correctness Engine is the first executable part of the **Semantic Intelligence Core**.

Its job is not to generate SQL and not to choose an execution strategy. Its job is to decide, deterministically, whether a bound semantic request is:

- semantically valid;
- semantically invalid;
- ambiguous;
- valid only if specific semantic obligations are satisfied by the planner.

Canonical boundary:

```text
CSMIR + bound SQIR + runtime policy context
                 ↓
        Semantic Correctness Engine
                 ↓
ValidatedSemanticQuerySet
+ SemanticFacts
+ SemanticObligations
                 ↓
        Semantic Planner Semantic Planner
```

The engine MUST NOT depend on DuckDB, DataFusion, ClickHouse, SQL dialects, Apache Ossie DTOs, Substrait, MCP, HTTP or any physical execution API.

---

## 2. Hard separation: correctness vs planning

Correctness Engine answers:

```text
Is the requested meaning well-defined?
Is it legal?
What must be preserved or recomputed for the result to remain correct?
```

Semantic Planner answers:

```text
Which semantic operations satisfy those obligations?
In what deterministic semantic order?
```

Examples:

- Correctness Engine may emit `PreserveMetricGrain(order_revenue, order)`.
- Semantic Planner may satisfy it by independently aggregating orders before aligning with item metrics.

- Correctness Engine may emit `RecomputeAtTargetGrain(average_order_value, [revenue, order_count])`.
- Semantic Planner chooses the semantic plan that aggregates the components and divides afterward.

The correctness engine MUST NOT silently choose a physical or relational workaround.

---

## 3. Correctness inputs

### 3.1 Required inputs

```text
CorrectnessInput {
    model: CSMIR
    query: BoundSemanticQuerySet   // canonical SQIR
    runtime_semantic_context
}
```

`runtime_semantic_context` contains only runtime values that affect semantic validity, primarily mandatory policy bindings and declared runtime semantic parameters.

It does **not** contain:

- engine statistics;
- latency/cost SLAs;
- cache state;
- materialization state;
- backend capabilities.

Those belong to later stages.

### 3.2 Preconditions

Before Correctness Engine begins:

- all SQIR names are bound to stable CSMIR IDs;
- typed literals are canonicalized;
- CSMIR structural compilation has succeeded.

Correctness Engine may still report model-invalidity if a deeper invariant is discovered, but unresolved user-facing names should normally have failed at the SQIR layer binding boundary.

---

## 4. Correctness outputs

A successful validation returns a canonical object conceptually equivalent to:

```text
ValidatedSemanticQuerySet {
    query
    block_facts[]
    obligations[]
    policy_attachments[]
    diagnostics[]
}
```

### 4.1 Semantic facts

Facts are proven properties, not planner choices.

Examples:

```text
MetricNativeGrain(metric, grain)
RequestedGrain(block, grain)
RelationshipPathUnique(dimension, role, path_class)
MetricIsRecomputable(metric, components)
MetricIsSemiAdditive(metric, forbidden_axis=time)
UnitOf(metric, BOB)
TemporalRole(metric, snapshot_time)
PolicyRequired(dataset, policy)
```

### 4.2 Semantic obligations

Obligations are constraints every valid Semantic Planner plan MUST satisfy.

Minimum v0.1 families:

```text
PreserveMetricGrain
AggregateBeforeAlignment
RecomputeAtTargetGrain
DoNotReaggregate
ApplyPeriodEndSelection
ApplyOrderedCumulative
UseTemporalRelationship
UseDeclaredRelationshipRole
ApplyBridgeAllocation
ValidateAllocationNormalization
ApplyMandatoryPolicy
ConvertTimezoneBeforeBucketing
PreserveExactDecimalSemantics
PreserveDeclaredNullSemantics
```

An obligation describes **what correctness requires**, never how a backend implements it.

### 4.3 Rejection

Invalid requests return a structured `SemanticError` and produce no semantic plan.

No "best effort" fallback is allowed for semantic ambiguity.

---

## 5. Validation pipeline

The canonical Correctness Engine pipeline is finite and ordered:

```text
1. model invariant validation
2. query semantic validation
3. type + unit analysis
4. grain analysis
5. relationship/path analysis
6. fanout + multi-fact analysis
7. metric algebra validation
8. temporal semantics validation
9. policy attachment validation
10. obligation synthesis
```

Later steps may depend on facts established by earlier steps.

The pipeline is deterministic for identical normalized CSMIR + SQIR + semantic runtime context.

---

# Part I — Model correctness

## 6. Structural model invariants

The compiled model MUST satisfy at minimum:

- unique stable IDs within their semantic namespace;
- all references resolve;
- metric dependencies are acyclic unless a future construct explicitly defines recursion;
- relationship endpoint datasets and key bindings exist;
- declared keys have compatible semantic types;
- conformed-dimension bindings are complete for every declared participating dataset;
- temporal relationships declare the required event/snapshot/validity roles;
- many-to-many attribution requiring allocation declares an allocation contract;
- policy references resolve;
- unit-conversion references resolve;
- metric decomposition references resolve.

Invalid model semantics never reach query planning.

---

## 7. Semantic type system

Physical storage types and semantic types are separate concerns.

Correctness Engine reasons over canonical semantic types such as:

```text
Boolean
Integer
Decimal(precision, scale)
String
Date
Timestamp
TimestampTZ
Duration
EntityKey(entity)
Quantity(number_type, unit)
```

The exact Rust enum layout is an implementation decision, but the semantic distinctions are normative.

### 7.1 Numeric promotion

Numeric promotion MUST be explicit and deterministic.

For exact business metrics, decimal arithmetic is preferred over implicit binary floating-point promotion.

No backend may redefine the semantic result type.

### 7.2 Unit typing

A unit is part of semantic typing.

Examples:

```text
Decimal(18,2)[BOB]
Decimal(18,2)[USD]
Decimal(18,3)[kWh]
```

Numerically compatible values are not necessarily semantically compatible.

Operations:

```text
BOB + BOB  -> valid
USD + BOB  -> invalid unless an explicit conversion semantic exists
kWh + kW   -> invalid
BOB / count -> valid derived unit according to declared metric semantics
```

Implicit currency conversion is forbidden.

---

# Part II — Grain algebra

## 8. Grain representation

A grain is a normalized semantic grouping identity, not a SQL `GROUP BY` string.

Conceptually:

```text
Grain {
    entity_key_atoms[]
    dimension_atoms[]
    time_atoms[]
    role_qualifiers[]
}
```

The implementation may use a more compact canonical representation, but equivalence must be deterministic.

### 8.1 Grain refinement relation

Define:

```text
G_fine refines G_coarse
```

when every group at `G_fine` belongs to exactly one group at `G_coarse` under the semantic model.

Examples:

```text
invoice_id          refines month × region
month × region      refines month
month               refines global
```

This is a semantic partial order, not a statement about row count.

### 8.2 Legal transitions

A request may require one of four transitions:

```text
same grain
roll-up to coarser grain
refinement to finer grain
grain alignment across independent facts
```

The engine MUST distinguish them.

A roll-up is legal only if the metric's aggregation semantics permit it.

A refinement is never invented from a coarser fact. It requires source semantics that genuinely support the finer grain or an explicit allocation/distribution rule.

### 8.3 Output grain

For each query block, Correctness Engine proves a valid requested/output grain contract or rejects the query.

It never infers output grain from generated SQL.

---

## 9. Aggregation semantics

Do not represent metric behavior with `additive: bool`.

Correctness Engine distinguishes at minimum:

```text
FullyAdditive
SemiAdditive
NonAdditive
Recomputable
OrderedWindow
```

These semantic classes are independent from the primitive aggregate implementation.

### 9.1 Fully additive

Can be rolled up across all declared legal grouping axes.

Example: revenue.

### 9.2 Semi-additive

Can be aggregated over some axes but requires a special rule over another axis.

Example: account balance is additive across accounts but period-end over time.

### 9.3 Non-additive

Already aggregated values cannot generally be summed.

Example: exact distinct customer count across overlapping regions.

### 9.4 Recomputable

Can be computed at a requested grain from sufficient components.

Example:

```text
average_order_value = revenue / order_count
```

The correctness engine emits a recomputation obligation rather than permitting aggregation of lower-grain ratios.

### 9.5 Ordered/window semantic

Requires an explicitly ordered semantic operation after a prerequisite aggregation grain.

Example: cumulative revenue after daily revenue aggregation.

---

# Part III — Relationship correctness

## 10. Relationship graph

CSMIR relationships form a typed semantic graph.

Every traversable edge carries correctness metadata including:

- direction;
- source/target datasets or entities;
- key bindings;
- cardinality;
- semantic role;
- temporal contract where applicable;
- optional bridge/allocation semantics.

Relationship cardinality is a correctness contract, not merely optimizer statistics.

---

## 11. Path resolution

For every selected or filtered dimension not local to a metric's source, Correctness Engine resolves semantic reachability.

A path is accepted only when:

- endpoints are semantically compatible;
- roles match explicit SQIR qualification when present;
- no equally valid role/path remains ambiguous;
- temporal requirements are satisfiable;
- bridge requirements are declared.

If two distinct semantic roles are valid and the caller did not disambiguate, the query is rejected.

This is CASE-016 behavior.

---

## 12. Cardinality proof model

For a metric anchored at dataset `F`, every relationship traversal is classified relative to the metric observation grain.

Minimum classes:

```text
AtMostOneMatchPerSourceObservation
MayMultiplySourceObservation
RequiresTemporalSingleMatch
RequiresAllocation
UnknownOrUnproven
```

A declared many-to-one path with a unique target key can prove `AtMostOneMatchPerSourceObservation`.

A one-to-many traversal can create `MayMultiplySourceObservation` and therefore cannot be treated as harmless for an additive metric at the source grain.

Unknown cardinality never silently becomes many-to-one.

---

## 13. Fanout reasoning

Fanout is analyzed per **metric lineage**, not merely per query graph.

For each requested metric, Correctness Engine determines whether any required dimension/path can multiply observations before that metric reaches a safe aggregation boundary.

Outcomes:

```text
FanoutSafe
FanoutRequiresIsolation
FanoutRequiresAllocation
FanoutRejected
```

### 13.1 Isolation obligation

CASE-004 requires:

```text
order_revenue  -> PreserveMetricGrain(order)
item_count     -> PreserveMetricGrain(order_item)

then:
AggregateBeforeAlignment(region)
```

Correctness Engine does not choose subqueries or CTEs. It proves that independent semantic computation is required.

### 13.2 Many-to-many

A many-to-many path is not automatically invalid.

It is valid only when the requested metric semantics define how attribution works.

CASE-011 requires explicit allocation weights and a normalization contract.

Without an allocation or another explicit many-to-many semantic, duplication is not resolved by guessing.

---

## 14. Multi-fact correctness

Independent facts MUST NOT be raw-joined merely because they share dimension names.

Correctness Engine requires conformed semantic dimensions and determines a legal common alignment grain.

For each metric/fact:

```text
native grain
→ legal aggregation/recomputation
→ alignment grain
```

The alignment grain must be justified by conformed dimensions/time semantics.

Examples:

- CASE-009: both facts align at `month × region`.
- CASE-010: invoices first roll up to `month × region`; targets already exist at that grain.

Correctness Engine emits `AggregateBeforeAlignment` obligations for facts whose native grain is finer than the common grain.

If no semantically valid common alignment exists, the query is rejected.

---

# Part IV — Metric algebra

## 15. Primitive aggregate semantics

Primitive measures declare enough information to reason about safe aggregation independently of a backend function name.

At minimum Correctness Engine understands semantic behavior equivalent to:

```text
sum-like additive state
count-like additive state
min/max-like distributive state
exact distinct state
recomputable algebraic state
time-selection state
ordered-window state
```

The external/source expression syntax is not the semantic classification.

### 15.1 Exact distinct

Exact distinct count is non-additive across overlapping groups.

CASE-008 therefore requires separate computation at each requested grain.

A future sketch/approximate distinct implementation is a different semantic contract and cannot silently replace exact distinct.

### 15.2 Ratios and algebraic metrics

A recomputable metric declares sufficient components and recomposition rule.

For:

```text
average_order_value = revenue / order_count
```

Correctness Engine proves:

```text
revenue can be produced at target grain
order_count can be produced at target grain
units/types are compatible with division
```

Then emits:

```text
RecomputeAtTargetGrain(average_order_value)
```

It never permits sum-of-ratios or average-of-averages by accident.

---

## 16. Expression correctness

All semantic expressions are type-checked and unit-checked.

Required invariants include:

- operand type compatibility;
- deterministic result type;
- explicit null behavior;
- deterministic decimal behavior;
- valid metric dependency graph;
- unit compatibility;
- no hidden cross-dataset join inside a primitive measure expression.

Metric definitions with intrinsic filters preserve those filters as part of metric identity and correctness.

---

# Part V — Time correctness

## 17. Time semantic roles

Correctness Engine distinguishes:

```text
EventTime
SnapshotTime
ValidityInterval
BusinessTimezone
BusinessCalendar
RequestedTimeBucket
WindowOrder
```

These roles are not interchangeable.

### 17.1 Timezone before calendar bucketing

When business semantics are timezone-relative:

```text
instant
→ business timezone conversion
→ business calendar bucketing
```

The order is normative.

CASE-019 depends on this invariant.

### 17.2 Period-end semantics

For semi-additive snapshot metrics, the model must define the valid temporal selection rule.

CASE-007 requires:

```text
within account × month:
select latest valid snapshot
then aggregate across accounts
```

The correctness engine emits `ApplyPeriodEndSelection` before cross-account aggregation.

### 17.3 Illegal temporal reaggregation

If no legal temporal rule exists, Correctness Engine rejects the query.

CASE-020 therefore returns `ILLEGAL_TEMPORAL_REAGGREGATION` instead of summing monthly balances.

### 17.4 Temporal relationships

CASE-013 requires exactly the historical dimension version valid at event time.

Correctness Engine validates the temporal contract and emits `UseTemporalRelationship`.

The planner later chooses a relational implementation.

### 17.5 Cumulative metrics

Cumulative semantics require:

1. prerequisite aggregation at the declared base time grain;
2. deterministic order;
3. declared partition scope;
4. cumulative operation after base aggregation.

CASE-021 cannot be treated as an ordinary additive metric over dates.

---

# Part VI — Null, decimal and deterministic semantics

## 18. Null semantics

Null handling is semantic metadata, not an engine default.

At minimum v0.1 must preserve the corpus behavior:

```text
SUM(value)     -> null-elision semantics as declared
COUNT(value)   -> counts non-null values
COUNT(*)       -> counts rows
```

No adapter may rewrite null values to zero unless the metric definition explicitly says so.

---

## 19. Decimal semantics

Exact business metrics declare canonical decimal precision/scale behavior.

Correctness Engine validates result-type compatibility and emits `PreserveExactDecimalSemantics` where required.

Backend-specific decimal limitations are engine-adapter layer/I capability-conformance concerns; they do not weaken the canonical semantic contract.

---

# Part VII — Policy correctness

## 20. Mandatory policy attachment

Policies are semantic constraints attached to governed model objects.

Correctness Engine validates that every mandatory policy required by the selected semantic lineage is bound to a runtime semantic context.

The caller may further restrict a mandatory policy but cannot remove or broaden it.

CASE-015 requires tenant filtering before unsafe exposure or aggregation that could leak cross-tenant information.

Correctness Engine emits `ApplyMandatoryPolicy` obligations.

### 20.1 Policy monotonicity

A caller predicate may narrow an authorized row set.

It may not widen the row set defined by mandatory policy.

Any future policy composition system must preserve this invariant.

---

# Part VIII — Structured semantic errors

## 21. Error architecture

Errors are stable semantic concepts, not backend error strings.

Conceptual shape:

```text
SemanticError {
    code
    phase
    severity
    object_refs[]
    query_refs[]
    message
    evidence[]
    suggestions[]
}
```

`suggestions` may help a caller disambiguate but MUST NOT silently mutate the query.

### 21.1 Error domains

```text
MODEL
QUERY
TYPE_UNIT
GRAIN
RELATIONSHIP
FANOUT
METRIC
TIME
POLICY
UNSUPPORTED
```

### 21.2 Required v0.1 codes

The exact Rust enum names may use idiomatic casing, but stable external codes include at minimum:

```text
INVALID_MODEL_REFERENCE
METRIC_DEPENDENCY_CYCLE
INVALID_RELATIONSHIP_CONTRACT
INVALID_TEMPORAL_CONTRACT
INVALID_ALLOCATION_CONTRACT

AMBIGUOUS_RELATIONSHIP_PATH
INVALID_RELATIONSHIP_ROLE
UNPROVEN_CARDINALITY
UNRESOLVED_FANOUT

INVALID_GRAIN_TRANSITION
GRAIN_REFINEMENT_UNSUPPORTED
ILLEGAL_REAGGREGATION
ILLEGAL_TEMPORAL_REAGGREGATION

INCOMPATIBLE_TYPES
INCOMPATIBLE_UNITS
MISSING_UNIT_CONVERSION

INVALID_METRIC_RECOMPOSITION
INVALID_WINDOW_SEMANTICS

POLICY_CONTEXT_MISSING
POLICY_VIOLATION

UNSUPPORTED_SEMANTIC_CONSTRUCT
```

The three normative corpus normative rejection codes remain exact:

```text
AMBIGUOUS_RELATIONSHIP_PATH
INCOMPATIBLE_UNITS
ILLEGAL_TEMPORAL_REAGGREGATION
```

---

# Part IX — Conformance strategy

## 22. Test pyramid

```text
fuzz/property tests
        ↑
cross-engine equivalence
        ↑
semantic golden corpus
        ↑
correctness-rule unit tests
        ↑
IR/compiler invariant tests
```

No performance benchmark can override a failed semantic test.

---

## 23. Property-based correctness

Properties are stronger than individual examples.

Required families include:

### Additive partition property

For a fully additive metric over a complete disjoint partition:

```text
m(all) == Σ m(partition_i)
```

### Recomputable ratio property

```text
ratio(target) == aggregate(numerator, target)
                 / aggregate(denominator, target)
```

not aggregation of child ratios.

### Fanout invariance property

Adding an unrelated one-to-many traversal MUST NOT change a metric whose semantic grain is outside that detail relation.

### Policy non-broadening property

Adding caller filters cannot increase the row set authorized by a mandatory policy.

### Conformed-dimension alignment property

Independent facts aligned through a conformed dimension must produce the same grouping identity regardless of source-local field names.

### Temporal selection property

For period-end semantics, adding earlier snapshots inside a period must not change the selected end-state if the latest snapshot remains unchanged.

---

## 24. Runtime contract validation

Some semantic declarations are compile-time contracts whose truth can be checked against data only at runtime or in validation jobs.

Examples:

- target key uniqueness;
- bridge weights sum to one per allocation scope;
- temporal relationship has at most one valid target when required;
- conformed key domain assumptions.

These are **data contract checks**, not reasons to move semantic correctness into the physical engine.

Violation must surface as normalized semantic/data-contract errors rather than silently returning wrong numbers.

---

## 25. normative corpus coverage matrix

Correctness Engine v0.1 covers the correctness requirement of every normative case:

| Case | Correctness Engine responsibility |
|---|---|
| 001 | additive grain legality |
| 002 | safe many-to-one dimensional traversal |
| 003 | cardinality proof |
| 004 | fanout isolation obligations |
| 005 | unique multi-hop semantic path |
| 006 | ratio decomposition/recomputation |
| 007 | semi-additive period-end semantics |
| 008 | exact distinct non-additivity |
| 009 | same-grain multi-fact alignment |
| 010 | different-grain multi-fact alignment |
| 011 | bridge allocation requirement |
| 012 | deterministic time bucketing contract |
| 013 | temporal/as-of relationship contract |
| 014 | null semantics |
| 015 | mandatory policy attachment |
| 016 | `AMBIGUOUS_RELATIONSHIP_PATH` rejection |
| 017 | `INCOMPATIBLE_UNITS` rejection |
| 018 | base semantic equivalence facts; materialization selection remains Semantic Optimizer |
| 019 | exact decimal + timezone-before-bucketing semantics |
| 020 | `ILLEGAL_TEMPORAL_REAGGREGATION` rejection |
| 021 | ordered cumulative obligation |

Correctness Engine does not select a materialization, backend or physical operator.

---

---

## 26. Post-aggregate metric-filter correctness

A SQIR `post_aggregate_filter` is valid only when every target resolves to a metric selection in the same query block and the comparison is type/unit compatible. Its evaluation grain is the block output grain after the referenced metric has been aggregated, recomposed and/or windowed as required. Correctness MUST reject any formulation whose truth would depend on evaluating a derived metric before its semantic definition is complete.

The baseline planner records an obligation to evaluate the predicate at that semantic phase; it is never treated as an ordinary source-row filter.

# Part II — Semantic Planner and SEIR

Status: **Semantic Planner canonical design v0.1**  
Date: **2026-09-27**

## 1. Planner purpose

Semantic Planner converts a semantically valid request into one deterministic, engine-neutral **semantic execution plan**.

Canonical boundary:

```text
ValidatedSemanticQuerySet
+ SemanticFacts
+ SemanticObligations
        ↓
   Semantic Planner
        ↓
SemanticPlanSet (SEIR DAGs)
+ ObligationDischargeMap
+ SemanticPlanCertificate
        ↓
engine-adapter layer+ physical adapters/planning
```

Semantic Planner answers:

```text
What semantic operations must occur,
and in what semantic dependency order,
so that every Correctness Engine obligation is satisfied?
```

Semantic Planner does **not** answer:

```text
Which engine should run the query?
Which join algorithm should be used?
Should a materialization/cache be used?
What SQL dialect should be emitted?
What is the cheapest plan?
How many partitions/threads should be used?
```

Those are later-stage concerns.

---

## 2. Core law: SEIR is not abstract SQL

SEIR is a semantic plan IR, not a relational/SQL AST and not a physical plan.

Forbidden as the canonical representation:

- SQL text;
- SQL AST nodes as semantic truth;
- DataFusion `LogicalPlan`;
- Substrait plans;
- DuckDB/ClickHouse/Snowflake logical plans;
- engine statistics;
- materialization IDs;
- cache keys;
- physical join algorithms;
- partitioning/exchange operators.

SEIR may later be **lowered** into any of those representations by adapters or physical planning layers.

The reason is architectural: relational algebra can represent how rows are manipulated, but it does not by itself preserve all semantic obligations such as metric native grain, ratio recomposition, temporal snapshot selection, many-to-many allocation, conformed-dimension alignment or policy provenance.

---

## 3. Planner inputs

The planner consumes only Correctness Engine output and canonical model/query identities.

Conceptual input:

```text
PlannerInput {
    model: CSMIR
    validated_query_set: ValidatedSemanticQuerySet
}
```

`ValidatedSemanticQuerySet` already contains:

- bound SQIR;
- proven semantic facts;
- semantic obligations;
- policy attachments;
- normalized semantic types/units/time roles.

The planner MUST NOT repeat Correctness Engine by silently changing semantic validity decisions.

If Correctness Engine accepted a request but the current planner implementation cannot construct a plan, the result is an explicit **planner capability failure**, not a different semantic interpretation.

---

## 4. Planner outputs

A successful Semantic Planner result is conceptually:

```text
SemanticPlanSet {
    blocks[]: SemanticPlanBlock
    certificate: SemanticPlanCertificate
}

SemanticPlanBlock {
    block_id
    root_node
    output_contract
}

SemanticPlanCertificate {
    model_hash
    query_hash
    seir_hash
    obligation_discharge_map[]
}
```

### 4.1 Obligation discharge

Every Correctness Engine obligation MUST be discharged explicitly by inspectable SEIR evidence. Evidence may be a concrete witness node, a graph invariant (for negative obligations such as "do not reaggregate"), or a typed semantic annotation whose preservation is checked by the planner.

Conceptual record:

```text
ObligationDischarge {
    obligation_id
    evidence[]   // NodeWitness | GraphInvariant | AnnotationWitness
    status: satisfied
}
```

Examples:

```text
PreserveMetricGrain(order_revenue, order)
    -> witnessed by AggregateAtGrain(node_17, grain=order)

RecomputeAtTargetGrain(avg_order_value, [revenue, order_count])
    -> witnessed by AggregateAtGrain(revenue)
     + AggregateAtGrain(order_count)
     + RecomposeMetric(avg_order_value)

UseTemporalRelationship(customer_history_as_of)
    -> witnessed by TemporalAsOf(node_31)
```

A plan is not valid merely because it can be lowered to SQL. It is valid only if all obligations are discharged.

### 4.2 Plan certificate scope

The certificate is an auditable compiler artifact, not a mathematical proof system claim. It provides deterministic evidence that the baseline planner accounted for every correctness obligation produced by Correctness Engine.

---

## 5. SEIR graph model

SEIR is a typed directed acyclic graph (DAG).

Each node conceptually carries:

```text
SEIRNode {
    id
    operator
    inputs[]

    output_grain
    output_dimensions[]
    output_metrics[]
    semantic_types[]
    units[]

    metric_lineage
    dimension_lineage
    policy_state
    temporal_state

    discharged_obligations[]
}
```

Not every field must exist literally on every Rust struct. These are the normative semantic properties that must be derivable/inspectable.

### 5.1 Deterministic node identity

Node identity is derived from canonical normalized semantic content, not memory addresses or creation order.

Equivalent canonical Semantic Planner inputs MUST produce the same canonical plan hash under the same planner/IR version.

### 5.2 Structural sharing

Structurally identical semantic subplans MAY be interned/shared by content hash.

This is canonicalization, not cost optimization. Semantic Planner does not choose sharing based on execution cost or backend characteristics.

---

# Part I — Canonical SEIR operators

## 6. Design rule for operators

An operator belongs in SEIR only when it represents a semantic operation or correctness-preserving semantic boundary that must survive translation across execution engines.

A physical implementation detail does not belong in SEIR.

Example:

```text
AggregateAtGrain(month, region)
```

belongs in SEIR.

```text
HashAggregate(partitions=16)
```

does not.

---

## 7. Source and governance operators

### 7.1 `ReadSemanticDataset`

References a CSMIR semantic dataset and the semantic fields required by the baseline plan.

Conceptual form:

```text
ReadSemanticDataset {
    dataset_id
    required_fields[]
}
```

It does not contain SQL/table-function syntax.

### 7.2 `PolicyGuard`

Represents mandatory row-policy enforcement bound from Correctness Engine.

```text
PolicyGuard {
    input
    policy_id
    bound_parameters
}
```

Law:

> For every protected dataset branch, `PolicyGuard` MUST dominate every node that can derive a metric/result from those rows.

A later optimizer may move/push the physical predicate only if this security invariant remains provably preserved.

### 7.3 `ContractGuard`

Represents a data-contract requirement whose truth is necessary for the semantic plan to be trusted.

```text
ContractGuard {
    input
    contract_id
    required_evidence
}
```

Examples:

- declared target-key uniqueness for a many-to-one relationship;
- exactly-one-match requirement for a temporal/as-of relationship;
- bridge allocation completeness/normalization.

`ContractGuard` does not dictate how evidence is produced. A later runtime may satisfy it using a fresh validated contract certificate or by executing a validation check. Execution must not silently assume a required contract that governance marks unverified/invalid.

### 7.4 `ApplySemanticFilter`

Represents an explicit filter whose semantic phase has already been classified.

Minimum phases:

```text
RowFilter
PostAggregateFilter
```

Intrinsic metric filters come from the metric definition and are attached to the metric branch at the semantically correct phase; they are not silently rewritten as caller filters.

Semantic Planner uses canonical placement. Aggressive filter pushdown belongs to Semantic Optimizer/physical lowering and must preserve meaning.

---

## 8. Dimension and relationship operators

### 8.1 `AttachDimension`

Attaches a requested dimension through a Correctness Engine validated relationship role/path.

```text
AttachDimension {
    input
    dimension_id
    relationship_path[]
    relationship_role?
    grain_contract
}
```

It says **which semantic relationship** is used, not which physical join algorithm executes it.

It MUST preserve the grain behavior proven by Correctness Engine.

### 8.2 `NormalizeConformedDimension`

Produces a canonical conformed-dimension domain value from a dataset-specific binding when the conformance contract requires normalization or mapping.

```text
NormalizeConformedDimension {
    input
    dimension_id
    binding_id
    conformance_mode
}
```

No two facts are aligned merely because source columns share a name/type.

### 8.3 `TemporalAsOf`

Represents a temporal relationship such as slowly changing dimension resolution.

```text
TemporalAsOf {
    input
    temporal_relationship_id
    event_time
    validity_start
    validity_end
    boundary_semantics
}
```

The operator owns semantic as-of meaning; the physical adapter later chooses the relational strategy.

### 8.4 `BridgeAllocate`

Represents explicit allocation through an approved many-to-many bridge.

```text
BridgeAllocate {
    input
    bridge_id
    target_dimension
    allocation_expression
    normalization_contract
}
```

This operator may expand row/dimensional grain while preserving a declared allocation invariant such as total amount conservation.

A raw many-to-many join without declared allocation semantics is never synthesized as a substitute.

---

## 9. Time and calendar operators

### 9.1 `NormalizeTime`

Canonicalizes a timestamp into the declared semantic timezone/calendar context before bucketing when required.

```text
NormalizeTime {
    input
    time_dimension
    timezone
    calendar
}
```

### 9.2 `TimeBucket`

Maps a canonical time value to the requested semantic grain.

```text
TimeBucket {
    input
    time_dimension
    grain
    calendar
}
```

Law:

> When timezone conversion affects bucket membership, `NormalizeTime` MUST occur before `TimeBucket`.

### 9.3 `SelectPeriodEnd`

Represents semi-additive period-end selection.

```text
SelectPeriodEnd {
    input
    metric_or_measure
    period_grain
    selection_time
    selection_scope_grain
    tie_break_semantics
}
```

Example: choose each account's final valid balance observation inside the month before aggregating balances across accounts.

---

## 10. Metric and aggregation operators

### 10.1 `AggregateAtGrain`

The core grain-changing semantic operator.

```text
AggregateAtGrain {
    input
    target_grain
    aggregations[]
}
```

Each aggregation references canonical metric/measure semantics, not backend aggregate syntax.

The planner may insert this operator only when Correctness Engine established the required aggregation/reaggregation is legal.

### 10.2 `DistinctAggregate`

Represents non-additive distinct aggregation over a declared semantic identity.

```text
DistinctAggregate {
    input
    entity_key
    target_grain
    result_metric
}
```

Its output cannot later be summed as if it were a fully additive count unless the metric contract explicitly permits a valid recomputation strategy.

### 10.3 `ComputeSemanticExpression`

Computes a row/grain-local semantic expression while preserving CSMIR type, null and exact-decimal semantics.

It is not a general escape hatch for hidden joins or backend-specific functions.

### 10.4 `RecomposeMetric`

Builds a recomputable metric from sufficient components **after** those components have been computed at the correct target grain.

```text
RecomposeMetric {
    inputs[]
    metric_id
    expression
    target_grain
}
```

Canonical example:

```text
SUM(revenue) / SUM(order_count)
```

not:

```text
AVG(precomputed_ratio)
```

when the metric contract defines ratio recomposition.

### 10.5 `ConvertUnit`

Performs only an explicitly declared semantic conversion.

```text
ConvertUnit {
    input
    from_unit
    to_unit
    conversion_semantic_id
}
```

FX conversions require their declared rate/time context and never appear implicitly because two values happen to be numeric.

---

## 11. Multi-fact alignment operators

### 11.1 `AlignSemanticBranches`

Aligns independently correct metric branches at a validated common semantic grain/domain.

```text
AlignSemanticBranches {
    inputs[]
    target_grain
    conformed_dimensions[]
    alignment_domain
}
```

This is a semantic alignment operator, not a physical join type.

The inputs MUST already satisfy their native-grain/fanout obligations before alignment when required.

Canonical pattern:

```text
Fact A branch ── aggregate safely ──┐
                                    ├─ AlignSemanticBranches ── ...
Fact B branch ── aggregate safely ──┘
```

not:

```text
raw Fact A × raw Fact B -> aggregate
```

unless the model explicitly proves that raw alignment is semantically safe.

### 11.2 Alignment domain

The set of dimensional keys represented in a multi-fact output is semantic, not a physical `INNER/FULL/LEFT JOIN` accident.

Semantic Planner MUST use the alignment-domain semantics established by the validated query/model. If the domain is not semantically determined, Correctness Engine should reject the request rather than Semantic Planner inventing a join-side default.

Physical lowering may realize the domain through different relational strategies as long as the resulting semantic domain is equivalent.

---

## 12. Window and ordered metric operators

### 12.1 `ComputeWindowMetric`

Represents a semantic windowed/cumulative metric after its base metric has been computed at the required grain.

```text
ComputeWindowMetric {
    input
    metric_id
    partition_grain[]
    order_axis
    frame
    operation
}
```

Canonical cumulative example:

```text
1. revenue at day × region
2. order by day inside region
3. cumulative sum
```

The planner MUST NOT move the cumulative operation before the required base aggregation.

---

### 12.2 `ApplyPostAggregateFilter`

```text
ApplyPostAggregateFilter {
    input
    predicate
    evaluation_grain
    referenced_metric_selections[]
}
```

It executes after the referenced governed metrics have reached their required semantic state. Physical/optimizer pushdown is allowed only when equivalence is proven; the deterministic baseline keeps the operator at the canonical post-aggregate phase.

## 13. Result-shaping operators

Result shape is part of query intent even though it is not metric algebra.

Minimum terminal operators may include:

```text
ProjectSemanticOutput
OrderSemanticOutput
LimitSemanticOutput
```

These encode SQIR-visible result semantics only.

Backend pagination protocols, streaming chunk sizes and presentation formatting remain outside SEIR.

---

# Part II — Planner construction algorithm

## 14. Canonical planning pipeline

The baseline planner is deterministic and uses the following ordered phases:

```text
1. decompose QuerySet into independent query blocks
2. build metric dependency requirements
3. create protected semantic source branches
4. attach intrinsic/request row semantics at canonical phases
5. resolve validated dimension roles/relationship paths
6. normalize time/conformed dimensions where required
7. apply temporal/bridge semantics where required
8. construct metric branches at safe/native grains
9. satisfy pre-alignment aggregation obligations
10. align multi-fact branches at the validated common grain/domain
11. recompute derived/recomputable metrics at target grain
12. apply semi-additive/window semantics in required order
13. apply post-aggregate filters and result shaping
14. verify every obligation is discharged
15. canonicalize + hash the SEIR DAG
```

A phase may be a no-op for simple queries.

---

## 15. Metric branch construction

Every requested metric is first reasoned about through its dependency graph.

For each primitive dependency, the planner establishes:

- source dataset;
- native/observation grain;
- required intrinsic filters;
- required policy guard;
- requested dimensions relevant to that branch;
- relationship/temporal/allocation obligations;
- legal target grain transitions.

Derived metrics do not force their components into one raw rowset.

This is essential for fanout safety and multi-fact planning.

---

## 16. Fanout-safe planning law

When combining branches whose row multiplication could change a metric, the baseline planner MUST satisfy the metric's grain obligation before unsafe alignment.

Canonical pattern:

```text
orders                    order_items
  │                           │
AggregateAtGrain(order)   AggregateAtGrain(order)
  │                           │
  └────────── AlignSemanticBranches ──────────┘
```

The exact target grain depends on the validated request, but the planner never relies on `SUM(DISTINCT ...)`, accidental key uniqueness or engine-specific behavior as a generic fanout fix.

---

## 17. Derived/recomputable metric law

A metric declared recomputable is planned from its sufficient components.

For metric:

```text
average_order_value = revenue / order_count
```

requested at `month × region`, the baseline semantic plan is:

```text
revenue branch      -> AggregateAtGrain(month, region)
order_count branch  -> AggregateAtGrain(month, region)
                      ↓
               AlignSemanticBranches
                      ↓
               RecomposeMetric
```

It is forbidden to aggregate a lower-grain ratio unless the metric explicitly declares such reaggregation as semantically valid.

---

## 18. Semi-additive metric law

A semi-additive temporal metric follows its declared selection semantics before aggregation across otherwise additive axes.

Period-end balance example:

```text
balance observations
      ↓
SelectPeriodEnd(scope=account, period=month)
      ↓
AggregateAtGrain(month, region)
```

The planner cannot replace this with `MAX(balance)` or `SUM(all snapshots)` unless the metric semantics explicitly say so.

---

## 19. Distinct metric law

Distinct-count semantics are computed over the declared semantic identity at the target grain.

Example:

```text
DistinctAggregate(customer_key, month × region)
```

The resulting scalar is not treated as an additive primitive for later rollup unless a valid decomposition/sketch contract exists in a future metric type.

---

## 20. Many-to-many allocation law

A many-to-many attribution that requires allocation is planned as explicit allocation before target-level aggregation.

```text
source metric branch
      ↓
BridgeAllocate(weight contract)
      ↓
AggregateAtGrain(target dimensions)
```

If the allocation contract requires normalization validation, the plan certificate records the corresponding obligation witness.

---

## 21. Temporal relationship law

As-of semantics are explicit and precede use of the temporally resolved dimension.

```text
fact event
   ↓
TemporalAsOf(customer_history, event_time)
   ↓
Attach/use resolved dimension
   ↓
AggregateAtGrain(...)
```

A normal equality relationship cannot silently substitute for an as-of relationship.

---

## 22. Exact time and decimal law

Time and exact numeric semantics remain visible through SEIR annotations/operators.

For timezone-sensitive bucketing:

```text
NormalizeTime(timezone)
      ↓
TimeBucket(month/day/...)
```

For exact decimal business metrics, expression/aggregation nodes carry canonical result type/rounding requirements established by Correctness Engine/CSMIR. Backend default precision rules are not semantic authority.

---

# Part III — Determinism and canonical baseline

## 23. One canonical reference plan

Semantic Planner v0.1 deliberately produces a **single canonical baseline semantic plan**.

It does not enumerate alternatives and does not search for the cheapest plan.

Why:

- correctness becomes testable independently of optimization;
- differential engine tests share one semantic reference plan;
- regressions are explainable;
- Semantic Optimizer can later prove rewrites equivalent to this baseline.

### 23.1 Canonical tie-breaking

Where semantically equivalent construction order exists, Semantic Planner uses deterministic ordering based on canonical stable IDs and normalized expression hashes.

It MUST NOT use:

- table size;
- row-count statistics;
- engine latency;
- adapter capabilities;
- materialization availability;
- current cache state.

Those belong later.

---

## 24. SEIR semantic fingerprint

The canonical SEIR plan has a deterministic semantic fingerprint derived from:

- SEIR version;
- CSMIR semantic/content hashes referenced by the plan;
- normalized SQIR meaning;
- canonical operator DAG;
- semantic operator parameters;
- policy semantic bindings that affect result meaning.

Operational metadata such as trace IDs, timestamps or engine statistics MUST NOT enter the semantic hash.

Uses:

- reproducibility;
- differential testing;
- provenance;
- optimizer equivalence checking;
- semantic cache/materialization identity later.

---

# Part IV — Failure taxonomy

## 25. Semantic invalidity vs planner incompleteness

Semantic Planner must distinguish two classes clearly.

### 25.1 Semantic errors

These come from Correctness Engine and should not normally be invented by Semantic Planner.

Examples:

```text
AMBIGUOUS_RELATIONSHIP_PATH
INCOMPATIBLE_UNITS
ILLEGAL_TEMPORAL_REAGGREGATION
```

### 25.2 Planner capability errors

The request is semantically valid, but this planner version lacks an implementation path.

Stable family:

```text
UNSUPPORTED_SEMANTIC_CONSTRUCT
PLANNER_OBLIGATION_UNSATISFIED
PLANNER_INTERNAL_INVARIANT_VIOLATION
```

`UNSUPPORTED_SEMANTIC_CONSTRUCT` is a product capability limitation, not a claim that the user's semantics are wrong.

`PLANNER_OBLIGATION_UNSATISFIED` indicates a correctness bug/incomplete implementation if Correctness Engine supplied a valid obligation that Semantic Planner claims to support.

No planner capability failure may silently degrade to an approximate or differently defined metric.

---

# Part V — Explainability

## 26. Explain contract

Every SEIR plan must be inspectable in a stable human-readable form.

Minimum explain output:

```text
requested metrics/dimensions
resolved metric dependencies
source semantic datasets
native/requested/output grains
relationship roles/paths
policy guards
fanout protection boundaries
multi-fact alignment grain/domain
metric recompositions
period-end/window ordering
obligations -> witness nodes
semantic plan hash
```

This explain surface is part of the trust model and will later feed APIs/agents/provenance.

It MUST NOT require the user to read generated SQL to understand why the number is correct.

---

# Part VI — Corpus coverage

## 27. normative corpus case mapping

Semantic Planner v0.1 must be able to construct a deterministic plan for every **valid** normative corpus case and must never receive a plan request for Correctness Engine-invalid cases.

| Case | Required Semantic Planner semantic behavior |
|---|---|
| CASE-001 additive metric | `AggregateAtGrain` |
| CASE-002 fact + dimension | `AttachDimension` + aggregate |
| CASE-003 many-to-one | relationship attachment preserving fact grain + required uniqueness `ContractGuard` where validation is not already certified |
| CASE-004 fanout trap | independent safe aggregation before alignment |
| CASE-005 multi-hop | explicit validated relationship path |
| CASE-006 ratio | component aggregation + `RecomposeMetric` |
| CASE-007 period-end | `SelectPeriodEnd` before legal aggregation |
| CASE-008 distinct count | `DistinctAggregate` |
| CASE-009 multi-fact same grain | independent branches + conformed alignment |
| CASE-010 multi-fact different grain | safe branch aggregation + conformed alignment + recomposition as needed |
| CASE-011 many-to-many | allocation `ContractGuard` + `BridgeAllocate` |
| CASE-012 time conversion | `NormalizeTime`/`TimeBucket` in semantic order |
| CASE-013 SCD/as-of | temporal single-match `ContractGuard` + `TemporalAsOf` |
| CASE-014 null semantics | canonical expression/null semantics retained |
| CASE-015 policy | dominating `PolicyGuard` |
| CASE-016 ambiguous path | rejected in Correctness Engine; no SEIR |
| CASE-017 incompatible units | rejected in Correctness Engine; no SEIR |
| CASE-018 materialization equivalence | baseline raw semantic plan; substitution deferred to Semantic Optimizer/K |
| CASE-019 decimal/timezone | exact decimal annotations + timezone before bucketing |
| CASE-020 illegal temporal reaggregation | rejected in Correctness Engine; no SEIR |
| CASE-021 cumulative metric | base aggregation before `ComputeWindowMetric` |

---

## 27.1 Correctness Engine obligation coverage

Every v0.1 Correctness Engine obligation family has a defined Semantic Planner discharge mechanism:

| Correctness Engine obligation | Semantic Planner discharge evidence |
|---|---|
| `PreserveMetricGrain` | `AggregateAtGrain` / branch grain annotation + invariant |
| `AggregateBeforeAlignment` | `AggregateAtGrain` predecessor(s) to `AlignSemanticBranches` |
| `RecomputeAtTargetGrain` | component aggregations + `RecomposeMetric` |
| `DoNotReaggregate` | graph invariant proving absence of forbidden grain transition |
| `ApplyPeriodEndSelection` | `SelectPeriodEnd` |
| `ApplyOrderedCumulative` | `ComputeWindowMetric` with required order/frame |
| `UseTemporalRelationship` | `TemporalAsOf` |
| `UseDeclaredRelationshipRole` | `AttachDimension` carrying declared role/path |
| `ApplyBridgeAllocation` | `BridgeAllocate` |
| `ValidateAllocationNormalization` | `ContractGuard` for allocation normalization/completeness + `BridgeAllocate` |
| `ApplyMandatoryPolicy` | dominating `PolicyGuard` |
| `ConvertTimezoneBeforeBucketing` | graph order `NormalizeTime -> TimeBucket` |
| `PreserveExactDecimalSemantics` | typed decimal annotation + graph invariant |
| `PreserveDeclaredNullSemantics` | expression/operator null-profile annotation + invariant |

If a new correctness obligation family is added later, Semantic Planner is incomplete until its discharge mechanism is specified.

---

# Part VII — Semantic Planner non-goals

## 28. Explicitly deferred

Semantic Planner does not include:

- DuckDB SQL generation;
- DataFusion lowering;
- ClickHouse/Snowflake/PostgreSQL dialects;
- physical join choice;
- statistics collection;
- cost estimation;
- materialization matching/substitution;
- cache lookup;
- distributed scheduling;
- partition selection;
- query runtime feedback;
- MCP/HTTP/Python surfaces.

Those would pollute the semantic baseline.

---


---

# Part VIII — Implementation closure for correctness and planning

## 29. Exact grain-proof algorithm v0.1

Grain refinement is proven through a deterministic **functional-dependency closure**, never through name matching or row-count guesses.

The correctness engine constructs proven dependencies from:

1. dataset observation grain -> every dataset-local field/dimension binding;
2. declared unique/entity keys;
3. validated many-to-one / one-to-one relationships whose target uniqueness contract is available;
4. conformed-dimension normalization contracts;
5. declared time-bucket hierarchy inside the same timezone/calendar profile;
6. explicit allocation/distribution rules when refinement would otherwise be impossible.

For grains `Gfine` and `Gcoarse`:

```text
Gfine refines Gcoarse
```

iff every atom in `Gcoarse` belongs to the functional-dependency closure of `Gfine` under the validated semantic model.

Rules:

- same normalized atoms => same grain;
- requested grain is the alignment target for a query block;
- each metric branch must independently prove it can produce that requested grain;
- coarser facts cannot be refined to a finer requested grain without an explicit semantic distribution/allocation contract;
- unknown/unproven dependency => no proof, never optimistic acceptance;
- time-bucket refinement uses the declared calendar hierarchy only; no implicit cross-calendar inference.

This algorithm is the baseline. Later optimization may discover equivalent plans but may not weaken the proof.

## 30. Deterministic temporal tie rules

`SelectPeriodEnd`, `SelectPeriodStart` and temporal as-of matching must be single-valued at the declared selection scope.

If multiple rows remain tied at the same semantic selection instant, the model MUST declare a deterministic secondary tie-break key. Otherwise correctness rejects the request/model with:

```text
AMBIGUOUS_TEMPORAL_SELECTION
```

For temporal relationships, more than one valid target match where `match_cardinality` requires one is a **data contract violation**, not a random join choice.

## 31. Data-contract evidence model

Cardinality, uniqueness, temporal-single-match and bridge-normalization assertions distinguish declaration from evidence:

```text
ContractEvidenceState =
    Verified { evidence_id, observed_at, source_snapshot }
  | DeclaredUnverified
  | Invalid { evidence_id, observed_at }
```

Runtime modes:

```text
Strict      # default for certified results
Development # may execute with declared/unverified contracts but result is not certified
```

In `Strict` mode a `ContractGuard` must be satisfied by fresh `Verified` evidence or by an exact runtime validation before protected metric computation is accepted. `Invalid` always fails. The acceptable evidence freshness is part of runtime configuration/provenance, not semantic meaning.

Required contract-violation codes include:

```text
UNIQUENESS_CONTRACT_VIOLATION
TEMPORAL_MATCH_CONTRACT_VIOLATION
ALLOCATION_CONTRACT_VIOLATION
```

## 32. Stable diagnostic contract v0.1

A public semantic diagnostic has this minimum stable shape:

```text
SemanticDiagnostic {
    schema_version: "0.1"
    code
    phase
    severity
    primary_ref?
    related_refs[]
    query_refs[]
    evidence[]
    suggestion_codes[]
    message
    source_span?
}
```

Stability contract:

- `schema_version`, `code`, `phase`, semantic refs and structured evidence are machine-stable;
- `message` is human-readable and is **not** a machine contract;
- tests MUST assert structured fields rather than English message fragments;
- backend error text is never promoted to a semantic error code;
- suggestions never mutate a query automatically.

Additional required codes introduced by the v1.1 audit:

```text
AMBIGUOUS_TEMPORAL_SELECTION
DIVISION_BY_ZERO
NUMERIC_OVERFLOW
UNIQUENESS_CONTRACT_VIOLATION
TEMPORAL_MATCH_CONTRACT_VIOLATION
ALLOCATION_CONTRACT_VIOLATION
```

## 33. Planner proof-completeness gate

A baseline SEIR plan is valid only when:

1. every correctness obligation has exactly one explicit discharge entry or a justified shared discharge;
2. every SEIR node is reachable from a result root;
3. every protected source branch is dominated by the required `PolicyGuard`;
4. every required `ContractGuard` is represented;
5. output grain, value types, units and selected columns equal bound SQIR expectations;
6. canonical SEIR fingerprinting succeeds under `sef-dcbor-sha256-v1`;
7. no engine capability or physical-cost decision appears in the proof.

Failure is `PLANNER_INCOMPLETE`, never a silently degraded plan.
