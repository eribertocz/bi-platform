# Implementation Plan — Codex Handoff

This is a **finite six-phase product plan**. Phases are functional deliverables, not documentation stages. Implementation may overlap internally only when dependencies are respected.

## Mandatory Plan → Approval → Implementation → Report gate

Every phase and every substantial work package inside a phase is executed through the same control gate:

```text
Plan
  ↓
Explicit user approval
  ↓
Implementation of approved scope only
  ↓
Verification / tests / benchmarks
  ↓
Engineering report
  ↓
STOP
  ↓
Plan for next work package
```

### Required plan contents

Before code changes, Codex must state:

- objective and bounded scope;
- affected crates/apps/interfaces/files;
- architectural contracts and invariants touched;
- dependencies proposed, with evidence and replacement boundary;
- test/conformance/benchmark plan;
- risks and explicit non-goals;
- measurable acceptance criteria.

### Authorization rule

Implementation begins only after explicit user approval. Codex must not interpret silence, a previous phase approval, or the global project GO decision as approval for a new work package.

### Scope-change rule

If a material new dependency, public contract change, architecture deviation, security-boundary change or acceptance-criteria change appears during coding, Codex must stop that material change, propose a revised plan and obtain approval. Local implementation details that preserve the approved architecture and scope do not require a new approval.

### Required post-implementation report

At the end of the approved work package, Codex reports:

- implementation summary;
- changed modules/files;
- test/conformance matrix and benchmark results where applicable;
- quality-gate outputs;
- deviations from plan;
- unresolved risks/issues;
- acceptance criteria: PASS/FAIL for each item.

Codex then stops. It may propose the next plan, but must not implement it until separately authorized.

## Phase 1 — Semantic Kernel

**Goal:** prove semantic correctness independent of engines/product infrastructure.

Build:

- CSMIR;
- SQIR + request-envelope semantic boundary;
- canonical hashing/fingerprinting profiles;
- Semantic Correctness Engine;
- deterministic baseline Semantic Planner;
- SEIR + obligation discharge/certificate;
- 21 normative corpus cases + mandatory semantic edge suite, including post-aggregate metric predicates.

**Done when:** all normative cases produce exact expected results/diagnostics/obligations/SEIR and property/canonicalization tests pass.

No HTTP server, database registry, Svelte, Ossie adapter or optimizer is required here.

## Phase 2 — Executable Runtime

**Goal:** first real end-to-end semantic query.

Build:

- engine/source adapter contracts;
- DuckDB adapter;
- Arrow result path;
- Query Runtime lifecycle;
- admission, deadlines, cancellation and bounded streaming;
- capability negotiation (`NativeExact` / `EmulatedExact` / `Unsupported`);
- contract guards and certified provenance;
- minimal CLI/embedded facade for testability.

**Done when:** supported corpus/edge cases execute on DuckDB with certified provenance, cancellation/resource behavior and no semantic divergence from Phase 1 baseline.

## Phase 3 — Control Plane

**Goal:** convert the runtime into an operable multi-project product platform.

Build:

- Semantic Registry and immutable releases;
- Organization → Workspace → Project → Environment hierarchy;
- `MetadataStore`, `AuditStore`, `JobStore`;
- SQLite local adapter + PostgreSQL server adapter through SQLx;
- schema migrations;
- typed/versioned configuration contract;
- SourceRegistry + EngineRegistry with schema fingerprint/drift detection;
- `SecretProvider`, `IdentityProvider`, `AuthorizationProvider` contracts;
- OIDC/OAuth product-edge integration foundation;
- Cedar authorization adapter;
- durable jobs/scheduler;
- versioned OpenAPI product API;
- semantic diff + internal/registered-consumer impact analysis and promotion gate;
- Ossie import/export compatibility adapter pinned by commit/schema hash with versioned SEF compatibility profile and loss report.

**Done when:** a model can be registered/promoted, a caller authenticated/authorized, a governed source executed, jobs/audit persisted, and the server restarted without losing control-plane state.

## Phase 4 — Intelligence and Acceleration

**Goal:** create the performance/intelligence moat without changing baseline semantics.

Build:

- semantic equivalence/rewrite framework;
- optimizer with baseline-equivalence gate;
- materialization catalog/manager;
- `CacheBackend` + result/plan/model caches with semantic/policy/snapshot-safe keys;
- statistics interface;
- automatic materialization recommendation;
- cost/freshness/latency model;
- adaptive physical execution planner;
- at least one second execution engine selected by strategic benchmark value.

**Done when:** optimized/materialized/alternate-engine plans prove semantic equivalence to the baseline and show reproducible benchmark improvement.

## Phase 5 — Product Surfaces / Premium UX

**Goal:** expose the platform as a high-quality developer/data product without duplicating semantic truth.

Build:

- Svelte 5 + SvelteKit Semantic Console;
- source-owned design system (Tailwind 4 tokens + Bits UI/shadcn-svelte);
- Model Explorer;
- Query Workbench;
- Correctness/Plan Explorer;
- Lineage graph;
- Semantic Diff / Impact view;
- Result/Provenance Explorer;
- Runtime/Engine/Materialization views;
- virtualized tables and ECharts 6.1.x;
- command palette, keyboard-first workflows, deep links, resizable workspaces, density/theme modes;
- full CLI;
- Python SDK;
- MCP adapter;
- Semantic SQL surface only after its semantic contract is proven.

**Done when:** primary product workflow passes Playwright + accessibility/performance gates and every displayed semantic decision comes from backend contracts.

## Phase 6 — Production / GA

**Goal:** make SEF safe to operate and upgrade in serious environments.

Build/close:

- OTLP observability taxonomy and dashboards/alerts examples;
- deployment-profile SLIs/SLOs, retention policy and tested RPO/RTO;
- tenant quotas/rate limits;
- production OIDC/session hardening plus CSRF/CORS/CSP/security-header baseline;
- OpenBao/cloud secret-provider adapters as required;
- backup/restore tooling and recovery drill automation;
- graceful shutdown/recovery;
- migration compatibility window and upgrade/rollback procedure;
- signed OCI/binary releases;
- CycloneDX SBOM and Sigstore/Cosign signatures/attestations;
- load/resilience/security testing;
- second-engine differential conformance complete;
- release pipeline and support/deprecation policy;
- explicit tested OS/architecture support matrix;
- generated API reference + operator quickstart/upgrade documentation.

**Done when:** GA release gate in `07_TESTING_AND_CONFORMANCE.md` passes.

## Canonical repository shape

```text
crates/
  sef-model-ir/          # CSMIR
  sef-query-ir/          # SQIR
  sef-correctness/
  sef-exec-ir/           # SEIR
  sef-planner/
  sef-runtime/
  sef-adapter-duckdb/
  sef-adapter-ossie/
  sef-control-plane/
  sef-metadata/
  sef-authz/
  sef-api/
  sef-cli/

apps/
  console/               # SvelteKit

sdk/
  python/

corpus/
  cases/

tests/
  conformance/
  integration/
  security/
  performance/
```

Create packages when their phase begins; do not generate empty scaffolding merely to match this tree.

## Dependency laws Codex must enforce

- `model-ir` and `query-ir` have no engine/UI/control-plane dependency;
- correctness/planner depend on IRs, never DuckDB/Ossie/Svelte/Axum/PostgreSQL;
- execution adapters depend inward on SEIR/runtime contracts;
- control plane references semantic snapshots by stable IDs/hashes, not internal mutable Rust objects;
- API DTOs are separate from core structs;
- UI consumes generated/public contracts only;
- secrets/identity are product-edge/runtime services, never semantic IR fields;
- optimizer may rewrite SEIR only through a proven equivalence gate;
- approximate semantics require an explicit future profile; they never silently enter certified exact execution.

## Engineering quality gates for every phase

Rust:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test/cargo-nextest
architecture/dependency checks
security/advisory/license checks
```

Frontend when present:

```text
svelte-check
lint + format
Vitest
Playwright
production build/bundle checks
accessibility/performance gate
```

All phases:

- structured errors, no panic/unwrap on untrusted/product paths without proof;
- bounded concurrency/memory/queues;
- tracing at subsystem boundaries;
- tests for every bug fixed in semantic/runtime/security behavior;
- benchmark evidence before performance-driven complexity;
- no new foundational document unless implementation reveals a durable architectural/semantic decision.

## Decision budget

Codex MAY choose local algorithms/data structures/helpers that preserve documented observable semantics and boundaries.

Codex MUST NOT invent or silently change:

- canonical semantic/hash profiles;
- grain/additivity/time/decimal/null semantics;
- policy trust boundaries;
- diagnostic/certification meanings;
- adapter exactness rules;
- tenancy isolation rules;
- public compatibility/versioning behavior once stabilized.

When a real ambiguity appears, make the smallest durable ADR/change required and continue implementation.
