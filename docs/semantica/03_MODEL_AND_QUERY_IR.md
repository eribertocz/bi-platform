# Model and Query IR

This document is the detailed, engine-neutral contract for CSMIR and SQIR. It preserves the full semantic detail required for implementation; historical stage reviews and implementation-status notes were intentionally removed.

---

# Part I — CSMIR

Status: **CSMIR canonical design v0.1 — implementation handoff approved**  
Date: **2026-09-27**

## 1. CSMIR purpose

CSMIR is SEF's canonical, engine-neutral representation of **compiled semantic truth**.

It exists between external semantic formats and query planning:

```text
Ossie / dbt / Cube / future formats
              ↓
         adapters
              ↓
            CSMIR
              ↓
     Semantic Intelligence Core
              ↓
            SEIR
```

CSMIR is **not**:

- a public semantic interchange specification;
- a SQL AST;
- a DataFusion logical plan;
- a DuckDB catalog;
- a query request;
- a physical execution plan.

External formats may change. CSMIR semantics must remain stable unless SEF deliberately changes its own semantic contract.

---

## 2. CSMIR boundary

CSMIR models semantic meaning only.

Allowed:

- identity;
- datasets and business entities;
- typed fields and dimensions;
- primitive measures;
- business metrics;
- relationships and cardinality;
- grain metadata;
- aggregation/reaggregation semantics;
- time semantics;
- units;
- row-policy definitions;
- deterministic semantic metadata.

Forbidden in CSMIR:

- SQL strings as semantic definitions;
- engine-specific functions;
- optimizer cost estimates;
- backend capabilities;
- materialization storage details;
- cache state;
- physical partitions;
- DataFusion/DuckDB/ClickHouse types as canonical domain types;
- runtime user identity values;
- LLM prompts or agent state.

---

## 3. Top-level model

Conceptual shape:

```text
CompiledSemanticModel
├── metadata
├── namespaces[]
├── source_bindings[]
├── datasets[]
├── entities[]
├── dimensions[]
├── measures[]
├── metrics[]
├── relationships[]
├── bridges[]
├── policies[]
├── calendars[]
├── unit_conversions[]
└── extensions
```

The Rust representation is deliberately deferred until the semantic model is approved.

---

## 4. Identity model

Every first-class semantic object has three distinct identity concepts:

```text
stable_id
semantic_version
content_hash
```

### 4.1 `stable_id`

Logical identity that survives compatible edits.

Conceptual form:

```text
<namespace>/<kind>/<name>
```

Examples:

```text
commercial/metric/revenue
commercial/entity/customer
commercial/dimension/billing_region
```

The internal representation MUST NOT use a database table name as object identity.

### 4.2 `semantic_version`

Represents contract evolution. Version semantics are independent from package/application versioning.

### 4.3 `content_hash`

Deterministic fingerprint of the exact semantic definition after normalization.

Uses:

- reproducibility;
- semantic diff;
- cache/materialization invalidation;
- provenance;
- equivalence bookkeeping.

The hash algorithm and canonical byte encoding are versioned infrastructure decisions and are not fixed by CSMIR.

---

## 5. Semantic dataset

A **SemanticDataset** is a row-bearing semantic relation from which dimensions/measures can be defined.

Conceptual model:

```text
SemanticDataset {
    id
    source_binding
    observation_grain
    fields[]
    key_bindings[]
    time_roles[]
    policy_refs[]
}
```

Examples:

```text
orders
order_items
balances
customer_history
```

A dataset is not assumed to be a physical SQL table. An adapter may bind it to a table, view, Parquet dataset, Iceberg table, remote source or another supported relation.

### Invariants

1. Observation grain MUST be explicit.
2. A dataset used as the `one` side of a declared cardinality MUST expose a key/uniqueness contract sufficient to justify it.
3. Source binding identifies data location/schema mapping but does not inject engine semantics into CSMIR.

---

## 6. Source binding

CSMIR needs to know **what semantic fields are bound to**, but not how a particular engine executes them.

Conceptual model:

```text
SourceBinding {
    source_id
    relation_ref
    field_bindings[]
}
```

`relation_ref` and field bindings are opaque logical handles owned by the relevant adapter contract.

Forbidden:

```text
"SELECT ..."
DuckDBTableFunction
DataFusionExpr
SnowflakeVariant
```

inside canonical semantic objects.

Backend capability metadata belongs to engine adapters / physical planning, not CSMIR.

---

## 7. Business entity

An **Entity** represents a stable business identity independently of a specific dataset.

Examples:

```text
customer
order
invoice
account
campaign
```

Conceptual model:

```text
Entity {
    id
    keys[]
}

EntityKey {
    id
    value_type
    uniqueness_scope
}
```

Keys may be composite.

An entity can appear in multiple datasets, e.g. `customer` in current customer data and customer history.

---

## 8. Fields, dimensions and conformance

A **Field** is a typed value available from one semantic dataset.

A **Dimension** is a canonical queryable business attribute independent of any one physical/source dataset. A dimension can have one or more dataset-specific bindings.

Conceptual model:

```text
Field {
    id
    dataset
    value_type
    nullable
    binding
}

Dimension {
    id
    value_type
    semantic_role?
    unit?
    time_semantics?
    bindings[]
}

DimensionBinding {
    dimension
    dataset
    expression
    conformance_contract
}
```

Examples:

```text
region
product_category
invoice_date
snapshot_date
```

### 8.1 Conformed dimensions

A shared dimension identity is the explicit proof that independently sourced facts may align on the same semantic axis.

Example:

```text
Dimension: commercial/dimension/region

Bindings:
  invoices.region_code  -> region
  targets.region_code   -> region
  refunds.region_code   -> region
```

The planner MUST NOT infer conformance merely because two columns share the same name or type.

A conformance contract may state that bindings share the same key domain directly, or that an explicit deterministic normalization/mapping is required.

Minimum conceptual modes:

```text
ExactDomain       # values already belong to the same canonical domain
NormalizedDomain  # deterministic normalization produces the canonical domain
ExplicitMapping   # conformance requires a declared mapping relation/contract
```

The core never assumes conformance from field name, physical type or coincidental value overlap.

This construct is required for safe multi-fact alignment in CASE-009 and CASE-010.

A dimension identity also does not erase relationship roles. `billing_region` and `shipping_region` may intentionally be distinct semantic dimensions even when they ultimately reference the same geographic domain.

---

## 9. Canonical value type system

CSMIR owns a semantic/logical type system independent of SQL and Arrow concrete type enums.

Minimum v0.1 types:

```text
Boolean
Integer { signed, bits? }
Decimal { precision, scale }
Float { bits }
Utf8
Binary
Date
Time
Timestamp { timezone_semantics }
Duration
Uuid
Identifier { entity_key }
Struct
List
OpaqueExtension { namespace, type_name }
```

### Value-type rules

1. Exact business amounts SHOULD use decimal semantics, not binary floating point.
2. Numeric compatibility does not imply unit compatibility.
3. Engine adapters are responsible for proving lossless/loss-acceptable mappings to backend types.
4. Unknown vendor types cannot silently become strings.

### 9.1 Exact numeric semantics

Backend-native arithmetic is not automatically canonical arithmetic.

For exact decimal operations, the compiled semantic expression must have a determined result type and must not rely on an engine choosing precision/scale implicitly. Lossy scale reduction requires an explicit rounding rule. Integer overflow/wraparound is never silently accepted as semantic truth.

CSMIR does not freeze one global rounding mode for every operation; it requires the compiler to make any lossy numeric behavior explicit before physical execution.

### 9.2 Canonical null/boolean profile

Scalar null propagation and boolean predicates use a SQL-compatible three-valued semantic profile unless an operator explicitly defines otherwise. Adapters must emulate that profile rather than inheriting incompatible backend defaults.

---

## 10. Semantic units

Physical type and semantic unit are separate.

Examples:

```text
Decimal(18,2) + ISO4217:BOB
Decimal(18,2) + ISO4217:USD
Decimal(18,3) + UCUM:kW
Decimal(18,3) + UCUM:kW.h
Ratio
Percent
Count
```

Preferred standard profiles:

- currencies: **ISO 4217** codes;
- physical/scientific units: **UCUM** codes;
- dimensionless ratio/percent: native semantic unit kinds.

Conceptual model:

```text
Unit {
    kind:
      Currency { iso4217 }
      | Physical { ucum }
      | Ratio
      | Percent
      | Count
      | Custom { namespace, code }
}
```

### Unit rules

- `USD + BOB` is invalid without an explicit conversion.
- `kW + kWh` is invalid.
- Compatible UCUM units may be explicitly converted.
- FX conversion is never implicit; it requires an explicit conversion semantic with rate/time context.

---

## 11. Expression model

CSMIR expressions are semantic ASTs, not SQL fragments.

### 11.1 Scalar expression family

Minimum concepts:

```text
Literal
FieldRef
DimensionRef
Unary
Arithmetic
Comparison
BooleanLogic
Case
NullTest
ExplicitCast
TemporalDerivation
UnitConversion
```

Core operators have SEF semantics. Adapters may later render them to engine-specific syntax.

### 11.2 Aggregate expression family

Minimum concepts:

```text
Sum
CountRows
CountNonNull
CountDistinct
Min
Max
```

More aggregate families may be added only when their cross-engine semantic contract is explicit.

### 11.3 Metric expression family

Minimum concepts:

```text
MeasureRef
MetricRef
Arithmetic
Ratio
Window
```

A metric expression may reference metrics only through an acyclic dependency graph.

---

## 12. Primitive measure

A **Measure** is the lowest certified aggregation primitive over a semantic dataset.

Conceptual model:

```text
Measure {
    id
    dataset
    scalar_expression?
    aggregation
    intrinsic_filter?
    value_type
    unit?
    input_grain
    aggregation_contract
    null_semantics
}
```

Examples:

```text
revenue_amount = SUM(order_total)
item_count     = COUNT(item_id)
active_users   = COUNT_DISTINCT(customer_id)
```

A measure must preserve enough information for the planner to reason about legal reaggregation.

### Measure locality law

A primitive measure's scalar expression is local to its declared dataset. It may not contain hidden cross-dataset lookups or joins. Cross-dataset semantics must remain explicit through relationships, conformed dimensions and metric/planner composition so fanout/cardinality analysis cannot be bypassed.

---

## 13. Business metric

A **Metric** is a governed business computation over measures and/or other metrics.

Conceptual model:

```text
Metric {
    id
    expression
    intrinsic_filter?
    value_type
    unit?
    native_grain
    aggregation_contract
    time_behavior?
    window_behavior?
    dependencies[]
    policy_refs[]
}
```

Examples:

```text
revenue
average_order_value
attainment
month_end_balance
cumulative_revenue
```

A metric is not required to map one-to-one to a SQL aggregate. Intrinsic filters are part of metric meaning and therefore belong to CSMIR rather than SQIR.

---

## 14. Grain model

Grain is a first-class semantic type-like concept.

### 14.1 Distinct grain concepts

CSMIR preserves the vocabulary established in normative corpus:

- observation grain;
- metric native grain;
- requested grain (belongs to SQIR, not CSMIR);
- output grain (derived by planning, not stored as query state in CSMIR).

### 14.2 Grain atoms

Conceptual primitives:

```text
GrainAtom =
    EntityKeyRef
  | DimensionRef
  | TimeBucketRef
```

A grain is a normalized set/product of grain atoms with explicit derivation semantics where needed.

Examples:

```text
order_id
account_id × snapshot_day
region × calendar_month
```

### 14.3 Grain laws

CSMIR records enough metadata to later determine whether a transition is legal.

CSMIR MUST NOT assume that dropping a grain atom is always legal.

Examples:

- dropping `order_id` for additive revenue can be legal;
- dropping `region` for a distinct count may require recomputation;
- dropping `month` for snapshot balance may be illegal without a temporal rule.

The actual grain algebra algorithm belongs to the Semantic Correctness Engine (Correctness Engine).

---

## 15. Aggregation contract

A boolean `additive` flag is forbidden.

Conceptual model:

```text
AggregationContract {
    classification:
        FullyAdditive
      | SemiAdditive
      | NonAdditive
      | Recomputable

    axis_rules[]
    decomposition?
}
```

### 15.1 Axis rule

A semantic axis may declare behavior such as:

```text
Additive
Recompute
DistinctRecompute
PeriodEnd
PeriodStart
Forbidden
WindowDependent
```

### 15.2 Decomposition

A recomputable metric may declare sufficient components and recomposition semantics.

Example:

```text
average_order_value
  components: revenue, order_count
  recomposition: revenue / order_count
```

This is what prevents average-of-averages.

---

## 16. Null semantics

Null behavior must be explicit at the semantic operation level.

Examples:

```text
SUM(amount)       → ignore null input values
COUNT(amount)     → count non-null values
COUNT(*)          → count rows
```

Implicit `NULL → 0` coercion is forbidden unless explicitly modeled.

This contract is necessary for cross-engine conformance.

---

## 17. Relationship model

A **Relationship** is a semantic traversal contract, not merely a pair of join columns.

Conceptual model:

```text
Relationship {
    id
    from_dataset
    to_dataset
    role
    cardinality
    key_pairs[]
    optionality
    temporal_constraint?
}
```

Cardinality in the declared direction:

```text
OneToOne
ManyToOne
OneToMany
ManyToMany
```

### Required invariants

1. Cardinality is a correctness assertion.
2. Relationship role is mandatory when multiple paths are semantically distinct.
3. Planner must not guess between equally valid roles.
4. `one`-side uniqueness must be backed by a semantic key/uniqueness contract.
5. Relationship key pairs must be type-compatible and semantically bind compatible entity/key domains.
6. A raw `ManyToMany` traversal cannot carry additive measures across the relationship unless an explicit safe semantic rule (for example a bridge allocation) proves how values are attributed.

---

## 18. Temporal relationship

Temporal/as-of joins are first-class relationship semantics.

Conceptual model:

```text
TemporalConstraint {
    event_time
    valid_from
    valid_to?
    interval_boundary
    match_cardinality
}
```

Example:

```text
valid_from <= sale_time < valid_to
```

The model must be able to require exactly one valid target version for a fact observation.

---

## 19. Many-to-many bridge and allocation

Many-to-many attribution is not represented as an ordinary unqualified join.

Conceptual model:

```text
Bridge {
    id
    left_dataset
    bridge_dataset
    right_dataset
    left_relationship
    right_relationship
    allocation?
}

Allocation {
    weight_expression
    normalization_scope
    required_total
    tolerance
}
```

For `order → campaign`, a declared allocation weight may be required before an order-level additive metric can be distributed across campaigns.

A bridge without sufficient allocation semantics cannot silently duplicate additive measures.

---

## 20. Time semantics

Time semantics are modeled independently from engine date functions.

### 20.1 Time role

```text
EventTime
SnapshotTime
ValidityStart
ValidityEnd
ProcessingTime
```

### 20.2 Timezone

Canonical timezone identifiers use **IANA TZDB** zone IDs such as:

```text
America/La_Paz
UTC
```

The timezone database version is runtime provenance, not embedded into semantic object identity unless a definition explicitly pins it.

### 20.3 Calendar

Minimum calendar profiles:

```text
Gregorian
ISOWeek
Custom/Fiscal { id }
```

### 20.4 Time grain

Minimum standard grains:

```text
second
minute
hour
day
week
month
quarter
year
```

A time bucket is defined by time source + timezone + calendar + grain.

Timezone conversion and calendar bucketing are distinct operations.

---

## 21. Snapshot/semi-additive time behavior

Snapshot metrics can declare a temporal aggregation rule.

Example:

```text
balance
  additive across account
  period-end across snapshot time
```

Conceptual behavior:

```text
TemporalAggregation {
    axis
    rule: PeriodEnd | PeriodStart | ...
    order_by
    selection_scope
}
```

`selection_scope` identifies the grain preserved while selecting the temporal observation (for example account). If no valid rule exists, temporal reaggregation can be forbidden.

---

## 22. Window metric semantics

Windowed metrics are distinct from ordinary aggregation.

Minimum v0.1 window behavior required by the corpus:

```text
Cumulative {
    base_metric
    order_by
    partition_by[]
    frame: unbounded_preceding_to_current
}
```

For cumulative revenue:

1. compute revenue at daily grain;
2. apply cumulative window over ordered days.

The resulting cumulative metric is not treated as a base additive measure.

---

## 23. Policy model

CSMIR stores certified policy definitions, not current user/session values.

Conceptual model:

```text
Policy {
    id
    scope
    enforcement: Mandatory | OptionalGoverned
    predicate
    context_slots[]
}
```

Example:

```text
tenant_id = context.tenant_id
```

Rules:

1. Mandatory policy cannot be removed by SQIR callers.
2. Runtime identity values are bound after model compilation.
3. Policies must participate in provenance.
4. Policy expressions use the semantic expression system, not string injection.

---

## 24. Extensions

CSMIR supports namespaced extension metadata, but extension handling is constrained.

```text
Extension {
    namespace
    key
    payload
}
```

Rules:

- opaque metadata may be ignored by the core;
- anything that changes certified semantic meaning must have an explicit semantic contract;
- vendor extensions cannot silently override core semantics;
- widely needed semantic constructs should graduate into canonical CSMIR rather than remain hidden vendor payloads.

---

## 25. Compilation states

External input does not become valid CSMIR merely because it parsed.

Conceptual state progression:

```text
ExternalModel
   ↓ adapter
NormalizedSemanticModel
   ↓ resolution/type checking
ValidatedSemanticModel
   ↓ derivation/canonicalization
CompiledSemanticModel (CSMIR)
```

Only `CompiledSemanticModel` may enter the Semantic Intelligence Core planner.

Invalid or unresolved definitions cannot cross that boundary except through explicit test-only facilities.

---

## 26. Determinism requirements

For the same normalized semantic meaning and compiler version/profile, CSMIR must be deterministic with respect to:

- object identity;
- dependency ordering after canonicalization;
- semantic fingerprinting;
- equivalent normalized expressions;
- diagnostic location mapping where source mappings exist.

Determinism is required for reproducibility and semantic diff.

---

## 27. normative corpus coverage matrix

| Corpus case | CSMIR concepts required |
|---|---|
| CASE-001 additive metric | dataset, measure, fully additive contract |
| CASE-002 fact + dimensions | dimensions, many-to-one relationships |
| CASE-003 cardinality | entity keys, uniqueness, many-to-one |
| CASE-004 fanout trap | independent measure grains, cardinality metadata |
| CASE-005 multi-hop | relationship graph + roles/cardinality |
| CASE-006 ratio | metric dependency graph + decomposition/recompute |
| CASE-007 period-end balance | snapshot time + semi-additive temporal rule |
| CASE-008 distinct count | non-additive/recompute semantics |
| CASE-009 multi-fact same grain | independent datasets + conformed dimensions + compatible grain descriptors |
| CASE-010 multi-fact different grain | conformed dimensions + explicit native grains + ratio metric |
| CASE-011 many-to-many | bridge + allocation contract |
| CASE-012 time conversion | time dimension + timezone/calendar/grain |
| CASE-013 SCD/as-of | temporal relationship |
| CASE-014 nulls | explicit aggregate null semantics |
| CASE-015 row policy | mandatory policy + context slot |
| CASE-016 ambiguous roles | relationship roles |
| CASE-017 incompatible units | semantic units + typed arithmetic |
| CASE-018 materialization equivalence | metric identity, grain and semantic fingerprint; physical materialization metadata belongs later |
| CASE-019 decimal/timezone | exact decimal + IANA timezone semantics |
| CASE-020 illegal temporal reaggregation | forbidden temporal transition metadata |
| CASE-021 cumulative | window metric semantics |

---

## 28. CSMIR invariants

The CSMIR design is accepted only if all of these remain true:

1. No external Ossie type is a canonical domain type.
2. No engine type is a canonical domain type.
3. No SQL text is required to express semantic meaning.
4. Observation grain is explicit for every row-bearing dataset.
5. Cardinality is explicit for every relationship.
6. Relationship role disambiguation is representable.
7. Metric reaggregation behavior cannot collapse to `additive: bool`.
8. Exact decimal, unit and timezone semantics are representable.
9. Temporal/as-of relationships are representable.
10. Many-to-many allocation is representable without raw-join ambiguity.
11. Mandatory row policy is representable without runtime string injection.
12. Window metrics can express cumulative-after-aggregation semantics.
13. Multi-fact alignment requires explicit conformed-dimension identity; name matching is never sufficient.
14. All 21 normative corpus cases have sufficient semantic representation.
15. Primitive measures are dataset-local and cannot hide joins.
16. Exact numeric behavior cannot depend silently on backend precision/rounding defaults.
17. Physical optimizer/materialization state remains outside CSMIR.

---

## 29. Explicitly deferred from CSMIR v0.1

Do not expand CSMIR merely to anticipate every future feature.

Deferred until driven by corpus/evidence:

- fiscal calendar implementation details;
- hierarchy/recursive entity algebra;
- cohort semantics;
- period-over-period operators;
- approximate/sketch metrics;
- differential privacy;
- probabilistic result contracts;
- bitemporal semantics beyond the current temporal relationship primitive;
- semantic write/update operations;
- physical materialization descriptors;
- engine capability negotiation.

The semantic core may evolve, but only from concrete requirements and new normative corpus cases.

---

---

# Part II — SQIR

Status: **SQIR canonical design v0.1**  
Date: **2026-09-27**

## 1. SQIR purpose

SQIR is SEF's canonical, typed, engine-neutral representation of **what a caller is asking for semantically**.

It sits between public/front-end request syntax and semantic correctness/planning:

```text
HTTP / Python / CLI / BI / MCP / future SQL frontend
                    ↓
            request translation
                    ↓
               binding
                    ↓
                  SQIR
                    ↓
      Semantic Intelligence Core
                    ↓
                  SEIR
```

SQIR is **not**:

- SQL or a SQL AST;
- an Ossie query format;
- an execution plan;
- a DataFusion logical plan;
- a physical target request;
- a cache/materialization hint;
- a user/session identity object;
- an optimizer cost model.

---

## 2. SQIR boundary

SQIR represents query intent only.

Allowed:

- selected certified metrics;
- selected canonical dimensions;
- time bucketing intent;
- semantic role qualification;
- caller filters;
- result ordering;
- deterministic result limits/pagination;
- output labels/presentation aliases that do not change meaning;
- multiple independent query blocks when one request needs multiple grains.

Forbidden in SQIR:

- raw SQL;
- engine-specific expressions/functions;
- backend selection;
- materialization selection;
- latency/cost/freshness requirements;
- runtime identity values;
- mandatory policy predicates injected by the system;
- join algorithms;
- physical join paths;
- optimizer statistics;
- LLM prompts/agent state.

---

## 3. Canonical request states

External requests are not SQIR merely because they parsed.

```text
ExternalQueryInput
      ↓ translation
UnboundSemanticQuery
      ↓ binding against CSMIR
BoundSemanticQuerySet  = SQIR
```

Only **bound** queries enter the Semantic Intelligence Core.

### 3.1 Unbound query

May contain:

- names;
- aliases;
- frontend-local identifiers;
- typed literal syntax requiring canonicalization.

It is a boundary representation, not a canonical IR.

### 3.2 Bound SQIR

Contains only:

- resolved CSMIR stable IDs;
- canonical typed literals;
- explicit semantic role qualifiers where supplied;
- normalized query structure.

Unknown or ambiguously bound names never cross into SQIR.

---

## 4. Top-level query set

One request may need results at more than one requested grain.

Instead of making one query block carry ambiguous grouping-set semantics, SQIR v0.1 models this explicitly:

```text
SemanticQuerySet {
    blocks[]
}
```

Each block has **one unambiguous requested grain**.

Example: CASE-008 asks for active customers by region **and** active customers overall.

That is represented as two semantic query blocks:

```text
block A: active_customers by region
block B: active_customers at global grain
```

The optimizer may later share work between blocks, but semantic meaning remains independent.

---

## 5. Semantic query block

Conceptual model:

```text
SemanticQueryBlock {
    block_id
    selections[]
    filter?                 # pre-aggregate dimension predicate
    post_aggregate_filter?  # predicate over selected governed metrics
    order_by[]
    limit?
    offset?
}
```

A block MUST contain at least one metric selection.

Dimensions define the requested grouping/detail grain.

No explicit `GROUP BY` list exists separately; selected dimensions are the semantic grouping contract.

---

## 6. Selection model

Every selected output has a query-local `selection_id`.

```text
Selection {
    selection_id
    item
    output_label?
}
```

`selection_id` exists only inside the query and is used by ordering/output references.

`output_label` is presentation metadata. It does not change semantic identity and is excluded from semantic equivalence.

### 6.1 Metric selection

```text
MetricSelection {
    metric: StableMetricId
}
```

SQIR v0.1 selects governed CSMIR metrics, not arbitrary caller-defined aggregate expressions.

Primitive CSMIR measures remain an implementation/semantic-model concern unless deliberately exposed as certified metrics.

### 6.2 Dimension selection

```text
DimensionSelection {
    dimension: QualifiedDimensionRef
}
```

### 6.3 Time dimension selection

Time bucketing is query meaning and therefore may be represented explicitly:

```text
TimeDimensionSelection {
    dimension: QualifiedDimensionRef
    grain: TimeGrain
    timezone?: IanaZoneId
    calendar?: CalendarId
}
```

A query may request, for example:

```text
event_time bucketed by day in America/La_Paz
```

Whether a requested override is legal for that governed dimension is validated in Correctness Engine.

---

## 7. Qualified semantic references

A canonical dimension can be reachable through multiple semantic relationship roles.

SQIR must be able to state caller intent without prescribing a physical join plan.

```text
QualifiedDimensionRef {
    dimension_id
    role_qualifier?
}
```

Conceptual role qualifier:

```text
RoleQualifier {
    semantic_roles[]
}
```

Example:

```text
customer_country via billing_customer
```

versus:

```text
customer_country via shipping_customer
```

This is a **semantic role choice**, not a physical path or join algorithm.

If the caller omits a required role and multiple semantic paths remain valid, Correctness Engine rejects the query as ambiguous. CASE-016 exists specifically for this behavior.

---

## 8. Requested grain

Requested grain is derived from a query block's selected dimensions after binding.

Conceptually:

```text
RequestedGrain = normalize(
    selected canonical dimensions
    + requested time buckets
    + semantic role qualification where grain identity depends on role
)
```

No caller-supplied free-form grain string exists.

Examples:

```text
[]                         -> global/scalar grain
[region]                   -> region
[region, calendar_month]   -> region × calendar_month
```

SQIR records query intent. Whether every requested metric can legally produce that grain is a Correctness Engine decision.

---

## 9. Caller filter model

Caller filters are typed semantic predicates, never SQL fragments.

Minimum v0.1 expression family:

```text
FilterExpr =
    Compare
  | Between
  | In
  | IsNull
  | IsNotNull
  | And
  | Or
  | Not
```

### 9.1 Filter target

SQIR distinguishes **pre-aggregate dimension predicates** from **post-aggregate metric predicates**.

```text
DimensionFilterRef {
    dimension: QualifiedDimensionRef
}

MetricFilterRef {
    selection_id  # must reference a metric selection in the same query block
}
```

A `post_aggregate_filter` uses the same typed boolean expression family but may target only governed metric selections already computed at the query block's requested grain. It is semantic HAVING, not arbitrary SQL. The Correctness Engine type-checks value type/unit compatibility. The planner applies it only after the referenced metric is correctly aggregated/recomposed/windowed for that block; an optimizer may push an equivalent predicate earlier only through the semantic-equivalence gate.

### 9.2 Literals

Filter literals are canonical typed values after binding:

```text
Boolean
Integer
Decimal
Utf8
Date
Timestamp
Uuid
...
```

No backend literal syntax is preserved.

### 9.3 Null semantics

`= NULL` is not a canonical filter operation.

Null intent is explicit:

```text
IsNull
IsNotNull
```

This avoids engine/dialect ambiguity.

---

## 10. Intrinsic filters, caller filters and policies are different

These three concepts MUST NOT collapse into one generic predicate bag.

### Intrinsic metric filter

Lives in **CSMIR** and is part of metric meaning.

Example:

```text
completed_revenue
  intrinsic: order_status = completed
```

### Caller filter

Lives in **SQIR** and expresses the caller's requested restriction.

Example:

```text
region IN [North, South]
```

### Mandatory policy predicate

Lives in **CSMIR policy definitions** and is bound using runtime context during Correctness Engine/runtime request processing.

Example:

```text
tenant_id = context.tenant_id
```

A caller can further restrict a mandatory policy but can never remove it.

---

## 11. Ordering

Ordering references query-local selections, not raw names:

```text
OrderTerm {
    selection_id
    direction: Asc | Desc
    nulls: First | Last | SemanticDefault
}
```

The query layer must never rely on a backend's undocumented/default null ordering for a certified deterministic result when ordering matters.

Result order is unspecified when `order_by` is empty.

---

## 12. Limit and pagination

Conceptual model:

```text
limit: NonNegativeInteger?
offset: NonNegativeInteger?
```

Rules:

1. `offset` requires `limit`.
2. `limit`/`offset` that determine a subset of rows require explicit deterministic ordering.
3. A backend-specific cursor/token is not part of SQIR.
4. Future cursor pagination belongs to the API/request layer and must compile to deterministic semantic ordering.

This prevents accidental dependence on engine row order.

---

## 13. Runtime request envelope is outside SQIR

The complete runtime request contains information that affects execution or policy binding but is **not query meaning**.

Conceptual separation:

```text
SemanticRequestEnvelope {
    query: SQIR
    runtime_context
    execution_requirements?
    request_metadata?
}
```

### Runtime context

Examples:

```text
tenant_id = A
user_region = North
role = analyst
```

These values bind CSMIR policy context slots. They are not embedded into SQIR.

### Execution requirements

Examples:

```text
max_latency
max_cost
freshness
preferred locality
```

These belong to later physical/adaptive planning, not semantic intent.

### Request metadata

Examples:

```text
trace id
client id
request id
```

These are operational, not semantic.

This separation allows the same SQIR to be planned under different authorized contexts and physical SLAs without changing the canonical query representation.

---

## 14. Binding rules

Binding converts external query input into canonical SQIR.

Minimum rules:

1. Metric names resolve to exactly one certified CSMIR metric stable ID.
2. Dimension names resolve to exactly one canonical CSMIR dimension stable ID.
3. Explicit stable IDs bypass name lookup but still require existence/type validation.
4. Aliases never become semantic identity.
5. Role qualifiers resolve to declared semantic relationship roles.
6. Dimension-filter literals canonicalize to the referenced dimension type where lossless/valid.
7. A post-aggregate metric-filter target must resolve to a metric `selection_id` in the same block; its literal canonicalizes against that metric's value type/unit contract.
8. Unknown names fail binding.
9. Ambiguous names fail binding.
10. Ambiguous **relationship semantics after successful object binding** are deferred to Correctness Engine, because that requires graph correctness analysis.
11. No backend object/table/column name is exposed as canonical semantic identity.

---

## 15. Canonicalization and determinism

For semantically equivalent bound input under the same SQIR profile, canonical SQIR must normalize:

- stable semantic references;
- pre-aggregate and post-aggregate filter trees where normalization is semantics-preserving;
- typed literals;
- time grain/calendar/timezone identifiers;
- role qualifiers;
- local selection references.

Presentation-only labels do not affect semantic equivalence.

Selection ordering may affect output column order and is therefore preserved in the output contract even when it does not change metric meaning.

A future query fingerprint can be derived from canonical SQIR, but the exact serialization/hash algorithm is not fixed by SQIR.

---

## 16. What SQIR rejects vs what Correctness Engine rejects

### SQIR / binding rejection

Examples:

- unknown metric;
- unknown dimension;
- ambiguous name/alias;
- invalid typed literal;
- unknown semantic role name;
- duplicate query-local selection ID;
- post-aggregate filter referencing a missing or non-metric selection;
- invalid pagination shape.

### Correctness Engine / semantic correctness rejection

Examples:

- illegal grain transition;
- fanout-unsafe metric combination;
- ambiguous relationship path after semantic graph analysis;
- incompatible units in a governed metric;
- illegal temporal reaggregation;
- invalid time override;
- policy binding failure;
- metric not valid at requested grain.

SQIR must not attempt to solve Correctness Engine's problems.

---

## 17. Corpus coverage matrix

| Corpus case | SQIR v0.1 representation |
|---|---|
| CASE-001 | metric `revenue`; dimension `region` |
| CASE-002 | metric `revenue`; dimensions `customer_region`, `product_category` |
| CASE-003 | metric `billed_amount`; dimension `customer_segment` |
| CASE-004 | metrics `order_revenue`, `item_count`; dimension `customer_region` |
| CASE-005 | metric `revenue`; dimension `region_name` |
| CASE-006 | metric `average_order_value`; global grain |
| CASE-007 | metric `balance`; dimensions `region` + month time selection |
| CASE-008 | two blocks: by-region + global `active_customers` |
| CASE-009 | metrics `gross_revenue`, `refunds`, `net_revenue`; month + region |
| CASE-010 | metrics `actual_revenue`, `target_revenue`, `attainment`; month + region |
| CASE-011 | metric `allocated_revenue`; dimension `campaign` |
| CASE-012 | metric `revenue`; month time selection |
| CASE-013 | metric `revenue`; historical `customer_region` dimension |
| CASE-014 | metrics `amount_sum`, `amount_count`, `row_count`; global grain |
| CASE-015 | SQIR: `revenue` by region; envelope context: `tenant_id=A` |
| CASE-016 | `revenue` + customer country without required role qualifier; Correctness Engine ambiguity rejection |
| CASE-017 | select governed `combined_revenue`; invalid model/unit semantics rejected before execution |
| CASE-018 | `revenue` by month + category; materialization is invisible to SQIR |
| CASE-019 | `revenue`; event time bucketed to day with `America/La_Paz` |
| CASE-020 | `balance` at global/year-collapsed grain; Correctness Engine temporal rejection |
| CASE-021 | metrics `daily_revenue`, `cumulative_revenue`; date dimension |

All 21 normative corpus requests are representable without SQL or engine concepts.

---

## 18. SQIR invariants

SQIR v0.1 is accepted only if all remain true:

1. SQIR contains only caller semantic query intent.
2. All semantic object references are bound CSMIR stable IDs.
3. No raw SQL or backend identifier is required.
4. Selected dimensions determine requested grain; no duplicate free-form `group_by` source exists.
5. Semantic role qualification is expressible without encoding a physical join path.
6. Caller dimension filters, post-aggregate metric filters, intrinsic metric filters and mandatory policies remain separate concepts.
7. Runtime identity/context values remain outside SQIR.
8. Cost/latency/freshness requirements remain outside SQIR.
9. Materializations/caches remain invisible to SQIR.
10. Query blocks have one unambiguous requested grain.
11. Multi-grain requests use multiple blocks rather than implicit reaggregation.
12. Limit/pagination cannot rely on undefined backend row order.
13. Every normative corpus request is representable.
14. No Correctness Engine algorithm is smuggled into SQIR.

---

## 19. Explicitly deferred from SQIR v0.1

Deferred until a concrete corpus/product requirement exists:

- grouping sets / rollup / cube syntax inside one block;
- arbitrary caller-defined aggregate expressions;
- unrestricted ad-hoc semantic metrics;
- cohort query primitives;
- period-over-period query operators;
- approximate-result requests;
- probabilistic/DP query contracts;
- write/update semantics;
- backend hints;
- materialization hints;
- distributed execution hints;
- query-language text grammar.

A frontend may add syntax later only if it compiles losslessly to canonical SQIR.

---


---

# Part III — Implementation-critical semantic profile v0.1

This section freezes semantics that an implementation MUST NOT invent differently.

## A. IR/profile versioning

Every canonical artifact carries an internal profile identifier:

```text
sef-semantic-profile: 0.1
csmir: 0.1
sqir: 0.1
seir: 0.1
```

Internal Rust types may evolve before a public 1.0 API, but a change that alters canonical meaning, fingerprinting, type behavior, grain proof, null behavior, policy behavior or planner equivalence MUST increment the affected IR/profile version.

`semantic_version` for governed semantic objects follows **Semantic Versioning 2.0.0**. `stable_id` is a logical identity; `content_hash` identifies one exact canonical definition.

Stable IDs use lowercase ASCII path segments:

```text
<namespace>/<kind>/<name>
```

Each segment MUST match:

```text
[a-z0-9][a-z0-9._-]*
```

Human labels remain separate and may use Unicode.

## B. Canonical fingerprint profile

SEF v0.1 uses the profile:

```text
sef-dcbor-sha256-v1
```

Algorithm:

1. Convert the semantic object into SEF's typed canonical value tree.
2. Remove non-semantic fields: descriptions, source spans, UI labels, trace/request IDs, telemetry and other operational metadata.
3. Preserve every field that can change result meaning, policy meaning, grain, units, types, time semantics, dependencies or relationship semantics.
4. Canonically order set-like collections by stable ID / canonical child hash. Preserve sequence order where order is semantically meaningful.
5. Encode using **RFC 8949 deterministic CBOR** requirements.
6. Hash the resulting bytes with **SHA-256**.
7. Render as:

```text
sef-sha256-v1:<64 lowercase hex chars>
```

CSMIR, canonical SQIR and canonical SEIR use the same fingerprint profile with distinct domain separators (`CSMIR`, `SQIR`, `SEIR`) included in the hashed canonical value tree. This prevents cross-kind hash ambiguity.

## C. Canonical scalar semantics

### C.1 Strings

The default certified string profile is `Utf8Exact`:

- valid UTF-8 only;
- equality is exact, case-sensitive and locale-independent;
- no implicit Unicode normalization or case folding;
- default deterministic ordering, when required, is binary UTF-8 lexicographic order;
- locale/collation-aware semantics require an explicit future collation contract and cannot be inherited silently from a backend.

### C.2 Boolean and nulls

- predicates use SQL-compatible three-valued logic;
- `NULL` never equals another value under ordinary equality;
- grouping places null dimension values into one semantic null group;
- `COUNT(*)`, `COUNT(x)` and `SUM(x)` follow the explicit rules already defined in CSMIR;
- an ordered result MUST state null placement.

### C.3 Decimal and exact numeric behavior

Canonical decimal supports precision `1..76` and scale `0..precision` in v0.1. Precision above an engine's exact capability causes capability rejection or an exact emulation; it is never silently narrowed.

Every compiled arithmetic expression carries a resolved result type. Rules for exact decimal operands are:

```text
ADD/SUB:
  scale = max(s1, s2)
  integer_digits = max(p1-s1, p2-s2) + 1
  precision = integer_digits + scale

MUL:
  scale = s1 + s2
  precision = p1 + p2 + 1
```

If a derived precision exceeds 76, compilation requires an explicit narrowing/cast contract or rejects the expression.

Exact decimal **division** requires an explicit target decimal type and a `ZeroDivisionPolicy`. Any scale reduction requires an explicit rounding mode. Supported rounding modes in v0.1 are:

```text
HalfEven | HalfUp | TowardZero | Floor | Ceiling
```

Overflow is an error (`NUMERIC_OVERFLOW`); wraparound is forbidden.

### C.4 Ratio zero-denominator behavior

A `Ratio` semantic MUST declare:

```text
zero_denominator: Null | Error
```

There is no hidden backend default. `Null` produces semantic null; `Error` produces `DIVISION_BY_ZERO`. A future non-zero substitute policy would be a distinct semantic contract.

### C.5 Floating point

Float values are approximate. Certified v0.1 float semantics accept only finite values. NaN and positive/negative infinity are rejected at certified boundaries unless a future approximate-metric contract explicitly defines them.

## D. Canonical time profile

A timestamp used for certified time semantics is one of:

```text
Instant                  # absolute instant
LocalDateTime { source_timezone, local_time_resolution }
```

`source_timezone` uses an IANA TZDB zone ID.

For a local civil time that is ambiguous or nonexistent because of DST, `local_time_resolution` MUST be explicit:

```text
Reject | Earlier | Later | ShiftForward
```

`Reject` is the default when a local timestamp definition omits a resolution policy. No engine-specific DST choice is inherited silently.

Time bucketing order is always:

```text
source instant/local resolution
→ business timezone
→ business calendar
→ requested bucket
```

`week` requires an explicit calendar profile. `ISOWeek` is portable; custom/fiscal week semantics require a declared calendar definition. SEF does not assume that ISO week-year and Gregorian year are interchangeable.

## E. Policy context contract

Policy `context_slots` are typed declarations, not arbitrary key/value strings:

```text
ContextSlot {
    name
    value_type
    required
    sensitivity: Public | Sensitive | Secret
}
```

All mandatory policies applicable to a protected branch combine by logical **AND**. Caller filters also combine by AND with mandatory policy predicates and can only narrow the authorized row set.

The request edge/runtime is responsible for producing authenticated `runtime_context`. SQIR callers cannot populate reserved policy slots directly. Sensitive/secret slot values MUST NOT appear in logs, diagnostics or provenance; only stable policy IDs and a non-reversible binding fingerprint may appear.

## F. Source-handle rule

`SourceBinding.source_id`, `relation_ref` and field bindings are logical handles. Physical URIs, connection secrets, SQL queries and engine objects live in the adapter/runtime source catalog. CSMIR may reference those handles but never owns credentials or executable source text.
