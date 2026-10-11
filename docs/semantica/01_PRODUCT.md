# Product, Scope and Strategic Moat

## Product definition

SEF is a universal semantic execution runtime. It consumes semantic definitions through compatibility adapters, normalizes them into SEF-owned internal representations, proves or rejects semantic correctness, constructs an engine-neutral semantic plan, then lowers that plan to concrete execution engines.

## Problem

Semantic layers today are fragmented by vendor model formats, runtime-specific planning, engine assumptions and agent integrations. A metric may be syntactically valid yet numerically wrong because of fanout, incompatible grains, temporal mistakes, unsafe many-to-many attribution or non-additive reaggregation. SEF treats those as compiler/planner problems, not dashboard conventions.

## Product thesis

If Apache Ossie succeeds as a common semantic interchange specification, value shifts from owning the model syntax to owning the best execution intelligence. SEF therefore does not create a competing semantic specification. It builds the runtime underneath BI, applications and agents.

## What SEF owns

- canonical internal semantic model (CSMIR);
- canonical semantic query representation (SQIR);
- semantic correctness engine;
- semantic planner and SEIR;
- semantic equivalence/rewrite framework;
- physical planning policy and adapter contracts;
- provenance and plan evidence;
- conformance suite.

## What SEF deliberately does not own

- a competing public semantic modeling standard;
- a general-purpose SQL database;
- storage formats already solved by Arrow/Parquet ecosystems;
- a custom networking protocol where standards suffice;
- a BI visualization product as part of the core;
- an LLM as a correctness authority;
- an engine-specific logical plan as the semantic IR.

## Target modes

**Embedded/local:** laptop or single process, DuckDB/Parquet first.  
**Service/enterprise:** same semantics, remote engines, policy context, observability and concurrency.

## North-star behavior

A user asks for metrics and dimensions. SEF returns the correct result or a stable semantic error, with enough provenance to explain the metric definition, grain, filters, relationship choices, policies and semantic plan.

## Strategic moat

The highest engineering investment belongs in the Semantic Intelligence Core, especially:

1. grain algebra;
2. metric additivity/recomposability;
3. fanout and cardinality reasoning;
4. relationship-path disambiguation;
5. multi-fact planning;
6. temporal/as-of and period-end semantics;
7. many-to-many allocation semantics;
8. semantic equivalence and safe rewrites;
9. proof/evidence that planned operations discharge correctness obligations.

Everything else should be replaceable infrastructure.

## Success criteria

- Same semantic request yields semantically equivalent results across supported engines.
- Invalid queries fail before execution with stable semantic diagnostics.
- DuckDB can be replaced without changing CSMIR/SQIR/SEIR.
- Ossie schema evolution affects the Ossie adapter, not the semantic core.
- Optimizations cannot change metric meaning.
- Agents consume governed semantic capabilities rather than unrestricted SQL as the primary path.
- Every returned metric can carry reproducible provenance.

## Scope discipline

A feature enters the core only when it changes semantic meaning, semantic correctness or semantic planning. Engine-specific behavior belongs in an adapter; user-interface behavior belongs at the product edge.

## Product console

SEF ships an official **Semantic Console** as a supported GA product surface. Deploying the console is optional (API/CLI-only deployments are valid), but the console itself is part of the supported product. It is not part of the Semantic Intelligence Core and cannot become a correctness authority.

The console must provide, incrementally as backend capabilities become available:

- semantic model explorer: entities, datasets, dimensions, metrics, grains, relationships and conformed dimensions;
- semantic query workbench: construct/bind SQIR without exposing raw unrestricted SQL as the primary path;
- correctness diagnostics: stable error codes, source spans, obligation explanations and remediation hints;
- semantic plan explorer: SEIR DAG, obligation-discharge mapping and plan certificate;
- lineage/relationship graph: semantic dependencies, relationship roles and metric lineage;
- result explorer: tabular results, charts and exact provenance/freshness/policy metadata;
- engine/capability view: selected execution engine, exact/emulated/unsupported capabilities and lowering diagnostics;
- later optimizer/materialization view: baseline vs optimized plan, equivalence evidence, materialization use and cost/freshness decisions;
- semantic diff/impact explorer: release changes, downstream impact and promotion blockers;
- administration edge: source registrations, model versions, policy bindings, conformance status and operational health.

The console is **not a general BI authoring suite**. Charts exist to inspect and validate semantic results and runtime behavior. Full dashboarding can be a separate consumer of SEF APIs.

### Product-edge law

The UI may author requests and display evidence, but it MUST NOT independently derive semantic truth. Grain resolution, fanout safety, metric recomposition, relationship validity, policy application and certified provenance always come from backend semantic/runtime APIs.

## Product completeness model

A finished SEF product is five coordinated subsystems with strict ownership boundaries:

1. **Semantic Intelligence Core** — CSMIR, SQIR, correctness, baseline semantic planning, SEIR and later semantic optimization.
2. **Execution Runtime** — admission, cancellation, deadlines, engine selection/lowering, streaming results, materializations and cache.
3. **Control Plane** — semantic registry, versioning, environments, engine/source registry, metadata persistence, jobs, identity bindings, authorization configuration and audit.
4. **Product Surfaces** — Semantic Console, HTTP API, CLI, Python SDK, MCP and later Semantic SQL endpoint.
5. **Operations Plane** — observability, migrations, backup/restore, deployment packaging, health/readiness, release integrity and upgrade/rollback.

No subsystem may redefine semantics owned by the Semantic Intelligence Core.

## Product hierarchy

The control plane uses the following stable ownership hierarchy:

```text
Organization
  └── Workspace
       └── Project
            └── Environment
                 ├── semantic model releases
                 ├── source bindings
                 ├── engine bindings
                 ├── policy bindings
                 └── materialization/runtime state
```

Canonical environment names are not hard-coded; `dev`, `stage` and `prod` are conventional labels. A semantic release is immutable once promoted. Promotion creates or advances an environment binding; it does not mutate the released semantic content.

## Product-level objects

The product layer may persist and expose:

- organizations, workspaces, projects and environments;
- model releases and their canonical hashes;
- source registrations and opaque credential references;
- engine registrations and versioned capability profiles;
- policy bundles/bindings and identity-to-context mappings;
- query/job history and structured diagnostics;
- materialization definitions, instances, refresh state and freshness;
- audit events and provenance references;
- API/service-account registrations where deployment mode requires them;
- optional registered consumer contracts used by semantic impact/promotion gates.

These are product/control-plane objects. They are not silently injected into CSMIR/SQIR/SEIR fingerprints unless the semantic specification explicitly requires them.

## Finished-product definition

SEF is considered product-complete only when a supported deployment can:

1. register/pin semantic models and sources;
2. authenticate a caller through a standards-based identity boundary;
3. authorize product actions and bind trusted semantic policy context;
4. validate and execute a semantic query end-to-end;
5. cancel/time out work safely and apply resource limits;
6. return Arrow-compatible results plus reproducible provenance;
7. persist registry/audit/job state durably;
8. migrate/upgrade metadata safely;
9. expose health, traces, metrics and structured logs;
10. back up/restore control-plane state;
11. operate through CLI/API and the Semantic Console;
12. ship signed release artifacts with an SBOM and verifiable build provenance; bit-reproducible builds are tracked as a hardening target and are never implied merely by deterministic semantic hashes.

## Change safety and source drift

A finished product must make semantic change safe, not merely executable:

- model releases are immutable and comparable by semantic diff;
- promotion can run impact analysis over internal dependencies, materializations and registered consumer contracts;
- source registrations expose a schema fingerprint/snapshot; incompatible source-schema drift invalidates affected certification evidence and blocks certified execution until revalidated;
- Ossie interoperability supports import and export for the representable subset, with an explicit loss/extension report rather than silent degradation.

## Operational profile contract

Operational guarantees are deployment-profile properties, not semantic meaning. Every production profile MUST declare and expose:

- availability/latency SLIs and target SLOs;
- backup cadence and recovery objectives (RPO/RTO);
- query/audit/job history retention and redaction policy;
- encryption-at-rest responsibility for metadata/backups and encryption-in-transit policy;
- concurrency/queue/rate quotas;
- telemetry sampling/retention policy.

Local/embedded mode makes no HA guarantee. Service/enterprise profiles must publish only guarantees that are measured and exercised by tests/drills; SEF must not claim an availability or recovery objective that the deployment topology cannot satisfy.

## Product non-goals

SEF does **not** become:

- an identity provider — integrate with OIDC/OAuth providers;
- a secret-vault implementation — use `SecretProvider` adapters;
- a general workflow orchestrator — jobs are limited to SEF runtime/control-plane responsibilities;
- a general message broker — distributed event infrastructure is optional behind an adapter;
- a dashboard/BI authoring suite — the Semantic Console is an engineering/semantic operations surface;
- a replacement for Git — authored semantic source remains Git-friendly and reviewable.
