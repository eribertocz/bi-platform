# Execution and Adapters

## 1. Purpose

This layer converts a semantically valid SEIR plan into concrete execution without allowing backend behavior to redefine semantic meaning.

```text
SEIR + SemanticPlanCertificate
        ↓
Physical planner
        ↓
Capability negotiation
        ↓
Adapter-owned EngineExecutionPlan
        ↓
Engine adapter
        ↓
Backend
        ↓
Arrow result + ResultProvenance
```

## 2. Ownership boundary

`EngineExecutionPlan` is **opaque outside the adapter that owns it**. SEF does not create a second universal physical-plan IR in v0.1.

The semantic core never imports DuckDB/DataFusion/ClickHouse types. Adapters depend inward on SEIR and engine contracts; the reverse dependency is forbidden.

## 3. Engine capability algebra

Capability support is not boolean:

```text
SupportLevel =
    NativeExact
  | EmulatedExact
  | Unsupported
```

`Approximate` is intentionally absent in v0.1. Approximation requires an explicit future semantic metric contract and cannot substitute for exact semantics.

Every adapter publishes a versioned `CapabilityProfile` covering at minimum:

- canonical scalar/value types and maximum exact decimal precision/scale;
- null and boolean semantics;
- string equality/ordering/collation behavior;
- timezone conversion and calendar bucketing behavior;
- aggregates, exact distinct and window semantics;
- temporal/as-of support;
- parameter binding;
- temporary/intermediate relations;
- Arrow import/export and streaming abilities;
- transaction/snapshot guarantees relevant to a query;
- contract-validation abilities;
- supported pushdowns;
- resource/cancellation hooks when available.

An `EmulatedExact` capability must name the lowering strategy and pass the same conformance tests as native execution.

## 4. Adapter contract

Conceptually:

```text
capabilities() -> CapabilityProfile
lower(seir, certificate, execution_requirements) -> EngineExecutionPlan
validate_plan(plan, certificate) -> AdapterValidation
execute(plan, bound_parameters, runtime_context) -> ResultEnvelope
explain(plan) -> PhysicalExplain
```

Optional later hooks may expose engine statistics to the physical optimizer.

`validate_plan` MUST prove that every SEIR semantic operator and certificate obligation has an exact lowering. Unsupported lowering fails before execution.

## 5. Source catalog boundary

CSMIR contains only opaque source handles. The runtime/adapter source catalog maps those handles to physical resources:

```text
source_id -> connection/engine
relation_ref -> table/view/file/query-backed relation
field_binding -> physical expression/column mapping
schema_snapshot -> canonical referenced-field/type/key fingerprint + freshness/catalog evidence
```

Credentials and executable source SQL never enter CSMIR hashes or semantic provenance. A query-backed external relation may exist in administrator-owned adapter configuration, but it is treated as a physical source definition, not as semantic AST. Governed query callers cannot supply arbitrary URLs/paths/connection strings; connector egress/path policy is enforced at source registration/resolution.

## 6. Contract enforcement

Certified execution defaults to **Strict** contract mode.

Before an operation relies on a uniqueness, temporal-single-match or bridge-normalization assertion, each `ContractGuard` must be discharged by:

- valid cached/certified evidence tied to a source snapshot; or
- an exact runtime validation executed by the adapter/runtime.

Development mode may allow declared/unverified contracts for experimentation, but the result MUST be marked `uncertified` and cannot populate certified caches/materializations.

## 7. DuckDB v0.1 baseline

DuckDB is the first execution target because it is embedded, analytical, columnar/vectorized, open source and strong for Parquet/Arrow/local execution. It proves end-to-end semantics; it does not define SEF architecture.

Initial lowering may use parameterized DuckDB SQL.

Required rules:

- caller values are always bound parameters, never interpolated SQL text;
- physical identifiers come only from trusted source-catalog bindings and are deterministically quoted/validated;
- raw caller SQL is never accepted by the semantic path;
- decimal lowering preserves the canonical numeric contract or capability-rejects;
- timezone/calendar behavior matches the canonical time profile;
- string/null/boolean semantics match the certified profile;
- aggregate-before-align multi-fact plans lower without raw fact multiplication;
- temporal/as-of, period-end and bridge-allocation obligations preserve SEIR order;
- mandatory policies execute before protected contribution can leak into metrics/results;
- contract validation can be executed when required;
- results convert to Arrow-compatible batches with semantic schema metadata;
- adapter/lowering/backend failures remain distinct from semantic diagnostics.

For one DuckDB execution request, all source reads that participate in the result should observe one transaction/snapshot when the backend/source topology makes this possible. If a future source cannot provide a coherent snapshot, that consistency limitation must be explicit in provenance/capabilities.

## 8. DataFusion role

DataFusion is optional Rust-native execution infrastructure. Its plans remain behind the execution boundary. SEIR never becomes a DataFusion `LogicalPlan` wrapper.

## 9. ADBC and remote connectivity

ADBC is preferred where a mature driver preserves Arrow-native interoperability. It is a connectivity boundary, not a mandatory internal abstraction.

Remote engines (ClickHouse, PostgreSQL, Snowflake, Databricks, BigQuery, etc.) are added only through adapters with capability and conformance matrices.

## 10. Ossie compatibility adapter

Apache Ossie is an **input compatibility boundary**, not an execution engine.

Because `0.2.0.dev0` is mutable, an implementation MUST pin:

```text
Ossie git commit SHA
+ ossie-schema.json SHA-256
+ adapter compatibility profile version
```

A version string such as `0.2.0.dev0` alone is insufficient for reproducible builds.

Minimum mapping intent:

| Ossie | SEF |
|---|---|
| model | adapter input container / namespace context |
| dataset | `SemanticDataset` |
| source | adapter-owned `SourceBinding` handle |
| primary/unique keys | entity/uniqueness contracts |
| field | typed `Field`; dimension metadata may create `DimensionBinding` |
| relationship | validated many-to-one relationship when target uniqueness proves it |
| metric | parsed/normalized measure/metric expression |
| custom extension | opaque extension unless a registered SEF semantic extension exists |

Ossie expressions are never copied into CSMIR as SQL strings. The adapter parses only a declared supported expression profile (prefer `OSSIE_SQL_2026`/portable ANSI subset where semantics are known) into the CSMIR semantic AST. Unsupported or ambiguous dialect constructs fail with explicit adapter diagnostics.

Current Ossie core does not by itself encode all SEF concepts such as rich grain algebra, temporal relationships, allocation or mandatory policy semantics. Such information must be either:

- representable through a registered, namespaced **SEF Ossie Compatibility Profile** at the compatibility boundary; or
- reported as unsupported/lossy.

Each compatibility profile is versioned and machine-readable and declares the mappings/extensions it understands. Phase 3 must publish the profile schema alongside the pinned Ossie commit/schema hash.

The adapter supports both directions for the representable subset:

- **import:** Ossie/profile -> validated CSMIR construction;
- **export:** governed SEF model -> Ossie/profile plus an explicit loss/extension report.

Round-trip equivalence is tested for the supported subset. The adapter must never weaken CSMIR merely to accept or export an Ossie document.

## 11. Materializations and acceleration

Materialization remains outside CSMIR/SQIR. A materialization can replace baseline work only when semantic equivalence is proven for:

- metric definitions and versions;
- grain/domain;
- intrinsic and caller filters;
- mandatory policy scope/binding partition;
- exactness/numeric semantics;
- time/calendar semantics;
- freshness requirement;
- source/model compatibility.

Uncertified results can never seed certified materializations.

## 12. Result envelope and provenance v0.1

```text
ResultEnvelope {
    data: Arrow-compatible batches/stream
    schema
    provenance: ResultProvenance
    certification: Certified | Uncertified
}

ResultProvenance {
    schema_version: "0.1"
    model_hash
    query_hash
    semantic_plan_hash
    metric_refs[] { stable_id, semantic_version, content_hash }
    output_grain
    effective_filter_fingerprint
    policy_refs[] { policy_id, binding_fingerprint }
    contract_evidence_ids[]
    engine { name, version, adapter_version, capability_profile_version }
    source_snapshots[] { source_id, schema_fingerprint, data_snapshot_or_freshness_token?, observed_at }
    materialization_refs[]
    timezone_database_version?
    semantic_profile_version
    execution_started_at
    execution_finished_at
    telemetry_refs[]
}
```

Sensitive policy/context values are never emitted. `binding_fingerprint` is non-reversible and scoped so it cannot be used to recover a secret.

Arrow fields SHOULD carry namespaced metadata sufficient to preserve semantic identity at the result boundary:

```text
sef.selection_id
sef.metric_id / sef.dimension_id
sef.value_type
sef.unit
sef.semantic_version
```

## 13. Failure classes

Keep failures distinct:

- **semantic rejection** — query/model is semantically invalid;
- **capability rejection** — chosen engine cannot preserve required semantics exactly;
- **contract violation** — source data violates a relied-on semantic contract;
- **lowering error** — adapter cannot correctly lower a supposedly supported plan;
- **execution error** — backend/runtime failure;
- **conformance mismatch** — backend result differs from normative semantics;
- **consistency limitation** — backend/source cannot provide requested snapshot/freshness guarantees.

No failure class may be translated into a plausible-looking partial result silently.

## 14. Adaptive execution (later)

Only after baseline correctness is proven, physical selection may optimize latency, freshness, cost, concurrency, locality/egress, materialization build cost and engine load. Optimization changes physical work, never metric meaning.

## 15. Query runtime lifecycle

A query has an explicit lifecycle:

```text
Accepted
  → Validating
  → Planning
  → Admitted
  → Executing
  → Streaming
  → Completed

terminal alternatives:
Rejected | TimedOut | Cancelled | Failed
```

`Completed` is the only success terminal state. Cancellation/timeouts must propagate to the active engine adapter where supported and must close result streams/resources deterministically.

## 16. Admission, backpressure and resource budgets

The runtime must protect itself before an engine is saturated.

Per request/tenant/project, configurable budgets may include:

- maximum concurrent queries;
- planning deadline and execution deadline;
- maximum queued duration;
- result-row/byte ceiling for materialized responses;
- streaming buffer bounds;
- engine-specific memory/time settings where safely enforceable;
- materialization/job concurrency limits.

Admission is explicit. Unbounded queues/channels are forbidden in server mode. Overload produces a structured retryable rejection rather than process exhaustion.

## 17. Execution handle

The engine-neutral runtime contract exposes conceptually:

```text
ExecutionHandle {
  query_id
  state()
  cancel()
  schema()
  stream_batches()
  provenance()
}
```

The concrete adapter may use SQL, Substrait or an engine API internally, but those details do not leak into the semantic core or public product contracts.

## 18. Retry and failure policy

Retries are conservative:

- semantic/capability/contract rejections are never retried;
- planning is deterministic and can be recomputed;
- read-only engine work may be retried only when the adapter can prove the operation is idempotent and the required source snapshot/freshness semantics are preserved;
- partial streamed results are never silently concatenated across an unsafe retry;
- cancellation never triggers an automatic retry;
- retry count/backoff is bounded and observable.

## 19. Engine registry and capability snapshots

An `EngineRegistry` binds an engine instance to:

- adapter kind/version;
- engine version;
- connection/source reference;
- health state;
- versioned `CapabilityProfile`;
- optional cost/freshness/statistics provider.

Capability profiles are immutable snapshots for a planning/execution attempt. A query cannot begin under one capability profile and silently finish under another semantic profile.

## 20. Cache architecture

Caches are distinct products with distinct correctness keys. Storage is behind a runtime `CacheBackend` contract; local mode may use bounded in-memory storage and later shared/distributed implementations cannot change cache-key semantics.

- **model/compilation cache** — canonical semantic artifacts by content hash;
- **plan cache** — validated/planned SEIR by model+query+semantic-profile hash;
- **result cache** — output by semantic plan + effective filters + policy scope + source snapshot/freshness contract;
- **metadata cache** — non-semantic control-plane reads with bounded TTL/invalidation.

No cache may weaken policy, source snapshot or semantic version boundaries. Cache hit/miss is provenance/telemetry, not semantic meaning.

## 21. Materialization manager

Materialization is managed by a dedicated runtime service with:

```text
MaterializationSpec
MaterializationInstance
RefreshPlan
RefreshRun
FreshnessState
DependencySet
RetentionPolicy
```

Responsibilities:

- equivalence certification against baseline semantics;
- partition/refresh planning;
- incremental refresh when exact semantics permit it;
- dependency-driven invalidation;
- atomic publish/swap of a refreshed instance;
- stale/failed state handling;
- retention/cleanup;
- usage and cost telemetry.

A failed refresh never corrupts the last certified instance.

## 22. Jobs and scheduler

SEF schedules only SEF-owned background responsibilities:

- materialization refresh;
- source/metadata synchronization;
- semantic contract validation;
- statistics collection;
- retention/cleanup;
- optional Ossie import/sync.

`JobStore` persists desired/run state. The first scheduler is in-process and durable through the metadata store. Distributed scheduling is deferred behind the same interface.

Job semantics require:

- idempotency key where applicable;
- lease/heartbeat for recoverable work;
- bounded retries with categorized failure;
- cancellation;
- next-attempt scheduling;
- structured run provenance.

## 23. Streaming and transport

Internally, analytical results use Arrow-compatible record batches. Product APIs choose transport by workload:

- small metadata/control responses: JSON;
- small analytical result: JSON may be allowed for ergonomics;
- large analytical result: Arrow IPC stream/file path;
- progress/job status: SSE where useful.

The transport layer cannot coerce exact decimal/time values into lossy browser/JSON types without an explicit representation rule.

Streamed analytical batches are **provisional** until the query reaches `Completed`. Final certification/provenance is committed only for a completed stream. If execution terminates as `Failed`, `Cancelled` or `TimedOut`, clients must receive the terminal state and MUST NOT treat previously received batches as a complete certified result.

## 24. Source drift, history and retention

Source metadata synchronization computes/refreshes `schema_fingerprint` and detects drift against active bindings. A referenced incompatible drift marks dependent contract evidence/materializations stale and prevents certified cache reuse until revalidation succeeds.

Operational persistence follows data-minimization defaults:

- query history records IDs, state/timings, hashes and redacted/fingerprinted parameters, not raw sensitive filter/policy values by default;
- audit records remain append-oriented and retention-controlled;
- cleanup jobs enforce query/job/audit retention independently from semantic model retention;
- telemetry sampling/retention is an operations profile and never changes semantic results.

## 25. Health and graceful lifecycle

Adapters/services expose health independently:

- **liveness** — process/event loop is alive;
- **readiness** — required control-plane dependencies are available;
- **engine health** — advisory per registered engine/source;
- **degraded** — product can serve limited functionality but a noncritical dependency is unavailable.

Shutdown sequence stops admission, cancels/drains within a deadline, persists scheduler leases/state, flushes telemetry and closes stores/adapters.
