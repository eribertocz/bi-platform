# Architecture

## 1. Reference architecture

```text
Authoring / external models
(Ossie, future dbt/Cube converters, UI/SDK)
                    │
                    ▼
          Compatibility adapters
                    │
                    ▼
                 CSMIR
                    │
          SQIR ─────┤
                    ▼
        Semantic Intelligence Core
      correctness + semantic planning
                    │
                    ▼
                  SEIR
                    │
                    ▼
        Physical execution planner
                    │
          ┌─────────┼─────────┐
          ▼         ▼         ▼
       DuckDB   DataFusion   remote engines
          │         │         │
          └─────────┼─────────┘
                    ▼
           Arrow-compatible result
                    +
          provenance / telemetry
```

## 2. Ownership zones

### Stable SEF core

- CSMIR
- SQIR
- semantic correctness rules
- semantic facts and obligations
- SEIR
- semantic plan certificate
- semantic error taxonomy

### Semi-stable runtime

- physical planner contracts
- capability model
- materialization metadata
- provenance envelope
- optimizer rules

### Volatile edges

- Apache Ossie adapter
- DuckDB/DataFusion/ClickHouse/etc. adapters
- ADBC/Flight/HTTP/MCP interfaces
- Python/JS SDKs
- UI

## 3. Three-IR law

SEF permanently separates:

**CSMIR — Canonical Semantic Model IR:** what the business model means.  
**SQIR — Semantic Query IR:** what semantic result the caller requests.  
**SEIR — Semantic Execution IR:** how to compute that request correctly at semantic level.

None of these is a SQL AST, Substrait plan, DataFusion `LogicalPlan`, DuckDB plan or Ossie DTO graph.

## 4. Request envelope

Runtime identity and execution SLAs are not part of SQIR:

```text
SemanticRequestEnvelope
├── sqir
├── runtime_context          # tenant/user/policy bindings
├── execution_requirements   # latency/freshness/cost
└── request_metadata         # trace/client/request ids
```

This allows the same semantic query to be reused under different identities and physical conditions without changing its meaning.

## 5. Hard dependency laws

- semantic model/query/correctness/planner modules MUST NOT depend on DuckDB, DataFusion, Ossie DTOs, Substrait or network/UI frameworks;
- engine adapters MAY depend inward on SEIR/capability contracts;
- compatibility adapters MAY depend inward on CSMIR construction APIs;
- product/API surfaces MAY depend inward on query/runtime APIs;
- dependencies point toward stable semantics, never from the semantic core toward volatile integrations.

## 6. Semantic vs physical planning

The semantic planner answers **what operations are required to preserve meaning**. The physical planner answers **where and how to execute them efficiently**.

Semantic planning may require aggregation-before-alignment, period-end selection, temporal as-of matching, allocation, recomposition or policy guards. It must not choose hash join vs merge join, warehouse size, partitions, materializations or engine-specific functions.

## 7. Deployment separation

Code architecture is the same in embedded and service modes. Deployment topology must not change semantic behavior.

## 8. Canonical module map for implementation

```text
sef-model-ir          CSMIR
sef-query-ir          SQIR and binding
sef-correctness       validation, facts, obligations, diagnostics
sef-exec-ir           SEIR types
sef-planner           deterministic semantic planning
sef-runtime           request envelope/orchestration
sef-adapter-ossie     Ossie -> CSMIR
sef-adapter-duckdb    SEIR -> DuckDB execution
sef-engine-api        engine capability/execution interfaces
sef-provenance        result/plan provenance
```

Additional adapters are separate crates/modules and cannot become dependencies of the first five semantic modules.

## 9. Architecture decisions preserved

### ADR: open-standard boundaries

Use established standards at external boundaries whenever sufficiently mature; keep immature standards isolated behind adapters.

### ADR: three internal IRs

Do not collapse CSMIR, SQIR and SEIR. Their separation protects meaning from query syntax and physical execution details.

### ADR: no engine as semantic architecture

DataFusion and DuckDB are execution technology choices, not semantic object models.

## 10. Canonical terminology

- **grain:** semantic key set at which a value is valid;
- **observation grain:** natural row/observation granularity of a semantic dataset;
- **metric native grain:** finest grain at which a metric is semantically defined;
- **requested grain:** grain implied by query dimensions;
- **output grain:** grain delivered by a semantic plan;
- **fanout:** multiplicative duplication caused by a relationship traversal;
- **conformed dimension:** one business dimension with explicit bindings across multiple datasets;
- **semantic obligation:** condition that any valid semantic plan must satisfy;
- **plan certificate:** deterministic evidence linking obligations to SEIR discharge mechanisms;
- **physical plan:** engine-oriented execution plan produced after semantic planning;
- **capability model:** declared features/semantics an engine adapter can execute safely.


## 11. Trust boundaries

- The product edge authenticates the caller and constructs trusted `runtime_context`; raw caller payloads cannot claim reserved policy-context values.
- The semantic core trusts compiled CSMIR contracts, but certified execution additionally requires the contract-evidence rules defined in correctness/execution docs.
- The source catalog owns credentials and physical source definitions; semantic IRs hold only opaque handles.
- SEF governs requests that pass through SEF. It cannot prevent an independently authorized user from bypassing SEF and querying a backend directly; backend access control remains part of deployment security.
- LLM/agent text is untrusted input. It must bind to SQIR/tools and cannot bypass correctness/policy layers.

## 12. v0.1 state model

For the first implementation, the compiled semantic model is an immutable in-memory snapshot identified by its model hash. A request references one model snapshot for its full semantic planning/execution lifecycle. Hot reload and multi-version serving enter through the Phase 3 control plane; distributed registry/execution is optional later. None may change the meaning of an already-started request.

## 13. Semantic Console architecture

The canonical frontend is **Svelte 5 + SvelteKit** and remains a volatile product edge.

```text
Browser
  │
  ▼
SvelteKit Semantic Console
  │
  ├── HTTPS JSON control/query APIs
  ├── SSE for long-running progress/events when needed
  └── Arrow IPC/stream response path for large analytical results when justified
  │
  ▼
SEF product API / runtime
  │
  ├── model/query binding
  ├── correctness diagnostics
  ├── SEIR + certificate/provenance
  └── engine execution
```

### Frontend dependency law

- `apps/console` may depend on generated/public API contracts, never on Rust semantic implementation types directly;
- the semantic core MUST NOT depend on Svelte, Node.js, browser packages or UI schemas;
- browser-side validation improves UX but never replaces server-side semantic validation;
- the browser never receives database credentials and never connects directly to governed analytical sources;
- runtime identity is established at the trusted product edge and converted into reserved `runtime_context` server-side;
- UI-only labels/layout state never enter canonical CSMIR/SQIR/SEIR fingerprints;
- large result rendering uses server-side filtering/pagination and virtualization; the browser is not a second analytical engine.

### Console package boundary

Target repository location when product surfaces begin:

```text
apps/
└── console/               # SvelteKit
    ├── src/lib/api/       # generated/thin SEF API client
    ├── src/lib/components/
    ├── src/lib/features/
    │   ├── model/
    │   ├── query/
    │   ├── diagnostics/
    │   ├── plan/
    │   ├── lineage/
    │   ├── results/
    │   └── runtime/
    └── src/routes/
```

Feature modules depend on a small frontend API layer, not on one another's internals. Shared design-system components remain presentation-only.

### Authentication boundary

Enterprise deployments should prefer standards-based identity (OIDC/OAuth-family flows) at the product edge. The SvelteKit server/session layer must keep backend credentials/tokens out of client JavaScript wherever deployment topology allows it. Authentication identifies the caller; semantic authorization remains enforced by backend policy compilation/binding.

## 14. Complete product architecture

```text
                           PRODUCT SURFACES
       Svelte Console | CLI | SDK | MCP | HTTP | Semantic SQL(later)
                                   │
                                   ▼
                         PRODUCT API / EDGE
                      authn | request shaping
                                   │
              ┌────────────────────┴────────────────────┐
              ▼                                         ▼
        CONTROL PLANE                              DATA PLANE
 registry/versioning                         Semantic Intelligence Core
 environments                               CSMIR + SQIR + correctness
 sources/engines                                      ↓
 jobs/audit                                        SEIR
 policy bindings                                      ↓
 metadata store                              Execution Runtime
              │                                      ↓
              │                            engine/source adapters
              │                                      ↓
              └──────────────► provenance ◄──── DuckDB / ...
                                   │
                                   ▼
                            OPERATIONS PLANE
                telemetry | health | migrations | backup
                     packaging | upgrades | release trust
```

The control plane can evolve independently of the data plane. Product surfaces depend only on versioned public contracts. Operations observe/control lifecycle state but never alter semantic meaning.

## 15. Control-plane ports

The service layer owns interfaces, not concrete products:

```text
MetadataStore
AuditStore
JobStore
SemanticRegistry
SourceRegistry
EngineRegistry
SecretProvider
IdentityProvider
AuthorizationProvider
ConfigurationProvider
EventPublisher
Clock
IdGenerator
```

Rules:

- local mode may bind several ports to one SQLite database/process;
- server mode may use PostgreSQL and external identity/secret systems;
- the Semantic Intelligence Core imports none of these interfaces;
- adapters translate persisted/control-plane state into immutable request/model snapshots before semantic planning;
- no credential material enters CSMIR, SQIR, SEIR, logs or provenance.

## 16. Tenancy and environment isolation

Every control-plane object is scoped explicitly. The minimum authorization tuple is:

```text
principal × organization × workspace × project × environment × action × resource
```

Isolation rules:

- IDs are globally opaque; authorization never relies on guess-resistant IDs alone;
- every metadata query is scope-constrained server-side;
- cross-project/environment access requires an explicit policy grant;
- source credentials are referenced indirectly through `SecretProvider` handles;
- cache/materialization keys include the semantic/policy scope required to prevent cross-tenant leakage;
- policy context is constructed by the trusted backend, never accepted wholesale from the browser/agent.

## 17. Security architecture

Security is layered:

```text
Authentication      Who is the principal?
Product Authorization  May the principal perform this product action?
Semantic Policy     Which governed data is visible/allowed?
Engine Credentials  How does SEF access a physical source?
```

The layers are intentionally independent.

### Authentication

SEF does not store user passwords in the default architecture. Server deployments integrate with standards-based OIDC/OAuth identity providers. Browser sessions should prefer authorization-code flow with PKCE and server-side session/token handling.

Machine/service clients use standards-based workload identity or OAuth client-credentials where available. Long-lived API keys are an optional compatibility fallback only: they are scoped, rotatable, revocable, stored hashed rather than plaintext, and never used to bypass product/semantic authorization.

### Product authorization

Product RBAC/ABAC is evaluated through `AuthorizationProvider`. Cedar is the preferred first adapter because its policy language is designed for analyzable RBAC/ABAC; the application contract remains provider-neutral.

### Semantic authorization

Semantic policies are compiled/bound by the backend semantic path. Product authorization may grant access to a metric/model, but cannot weaken semantic row/data policies.

### Engine access

Engine/source credentials are isolated in `SecretProvider`; adapters receive short-lived/resolved credentials only at the execution boundary where practical.

## 18. Metadata persistence and transactions

`MetadataStore` is the durable control-plane source of truth, not the analytical warehouse.

Required semantics:

- ACID transaction boundary for model release/promotion, bindings and migrations;
- optimistic concurrency/version checks for administrative mutations;
- append-only audit records for security/administrative events;
- schema version table and deterministic migrations;
- transactional outbox pattern when durable external event publication is introduced;
- backup/restore must cover metadata plus secret references/configuration metadata, never assume analytical source data is backed up by SEF.

Local default: SQLite. Server default: PostgreSQL stable release. Both are adapters behind the same repository/transaction interfaces.

## 19. Runtime/process topology

Start as a modular single service/binary with explicit internal boundaries. Scale by separation only when measurements require it.

```text
sef-server
├── API/edge
├── control-plane services
├── semantic core
├── execution runtime
├── job scheduler
└── telemetry
```

Later, execution workers may become separate processes while retaining the same contracts. The architecture MUST NOT require distributed deployment for local mode.

## 20. Semantic diff and promotion-safety contract

Semantic change analysis is a control-plane service over canonical releases; it does not redefine CSMIR semantics.

At minimum a diff compares objects by `stable_id`, `semantic_version` and `content_hash` and classifies:

```text
Unchanged | Added | Removed | Modified
```

For `Modified`, impact analysis records which semantic contracts changed (type/unit/grain/definition/relationship/policy/time semantics) and walks the canonical dependency graph to identify affected metrics, materializations and registered consumer contracts. The system may label a change `Compatible` or `Breaking` only from deterministic rules; unknown compatibility is reported as `Unknown`, never guessed.

Environment promotion can enforce policy such as "no Breaking/Unknown impact on protected consumers without explicit approval". Rollback rebinds an environment to a prior immutable release; it does not mutate history.

A registered external dependency is represented minimally as:

```text
ConsumerContract {
    consumer_id
    environment_id
    semantic_refs[] { stable_id, accepted_version_range? | pinned_content_hash? }
    enforcement: Inform | Block
}
```

This is optional for consumers that do not register; internal dependency impact is always available.

## 21. Configuration and request-stability contract

Configuration is typed, versioned and validated before use. Canonical precedence is:

```text
built-in safe defaults < config file < environment < CLI/explicit deployment overrides
```

Secret values are never ordinary configuration fields; configuration carries `SecretProvider` references. Unknown/invalid security- or semantics-affecting configuration fails closed instead of silently falling back.

A request captures immutable snapshots of the model, runtime/capability profile and relevant configuration at admission. Dynamic administrative changes affect new work; they MUST NOT silently change an already-admitted query.

## 22. Source snapshot and schema-drift contract

A certified source observation carries at minimum:

```text
SourceSnapshot {
    source_id
    schema_fingerprint
    data_snapshot_or_freshness_token?
    observed_at
}
```

Source adapters may additionally expose catalog/version/watermark metadata. Incompatible drift of a referenced field/type/key/relationship invalidates affected contract evidence, result/materialization cache eligibility and certified execution until the model/source binding is revalidated. Compatible unrelated additions do not invalidate semantics merely because the physical schema grew.

Caller-controlled arbitrary network/file locations are not accepted as source identities in the governed query path; source locations are administrator-registered and pass connector egress/path policy.

## 23. Availability, retention and privacy boundaries

- embedded/SQLite mode is single-instance and has no HA promise;
- service mode may run multiple API/control-plane replicas over PostgreSQL; scheduler/job leases must prevent duplicate ownership;
- distributed query workers remain optional and are not required for HA of the control/API plane;
- query history stores semantic IDs, timings and redacted/fingerprinted parameters by default rather than raw sensitive values;
- audit/query/job retention is configurable per deployment profile and enforceable by cleanup jobs;
- metadata/backups use deployment-provided encryption at rest; SEF uses TLS for network paths and does not invent a proprietary encryption system.

## 24. Public contract boundary

Public product contracts are versioned independently from internal Rust types.

- JSON control-plane/query APIs use generated OpenAPI contracts;
- analytical result streams prefer Arrow IPC when result size justifies it;
- long-running progress may use SSE; WebSockets are not a default requirement;
- every public error has a stable machine code and trace/request identifier;
- internal structs/enums are never serialized directly as an accidental public API;
- backward-compatible additive change is preferred; breaking changes require a version boundary and migration/deprecation policy.
