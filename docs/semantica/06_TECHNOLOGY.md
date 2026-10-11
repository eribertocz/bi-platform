# Technology and Standards

Baseline verified: **2026-09-27**.

## 1. Selection law

Every significant dependency is evaluated on:

1. open-source availability and license compatibility;
2. standard/interoperable interfaces;
3. active maintenance and ecosystem health;
4. performance evidence on relevant hot paths;
5. columnar/vectorized design where analytical data is involved;
6. low lock-in and replaceability;
7. cross-platform/deployment fit;
8. security and supply-chain quality;
9. operational simplicity;
10. ability to remain behind a clean boundary.

Popularity alone is never a reason to adopt a dependency.

## 2. Baseline stack

| Area | Decision | Role |
|---|---|---|
| Core language | **Rust, Edition 2024** | semantic core/runtime |
| Semantic interchange | **Apache Ossie** | external model spec only |
| Columnar memory/interchange | **Apache Arrow** | data boundary |
| Embedded analytical DB | **DuckDB** | first execution adapter |
| Rust execution framework | **Apache DataFusion** | optional execution framework |
| DataFrame/ETL tooling | **Polars** | tooling/SDK/workflows, never correctness core |
| Files | **Parquet** | analytical persistence/interchange |
| Engine language | **SQL** | backend execution language |
| Connectivity | **ADBC** where mature | interoperable connectivity |
| Plan interchange | **Substrait**, isolated/evaluated | optional interoperability only |
| Observability | **OpenTelemetry / OTLP** | telemetry |
| Lineage export | **OpenLineage** | lineage interoperability |
| Version control | **Git** | source/semantic changes |
| Web | **HTML/CSS/JS/TS standards** | product surfaces |
| Python | optional SDK/tooling/tests | not core correctness |

## 3. Verified upstream state

- Rust stable toolchain verified: `1.98.1` (2026-09-03); repository pins one tested stable toolchain.
- Apache Ossie core spec: `0.2.0.dev0`, explicitly draft/mutable; latest released core remains `0.1.1`. Adapter pinning by commit/schema hash is mandatory.
- DuckDB Rust client: `1.5.5`, primary support tier.
- Apache DataFusion: `55.0.0` baseline.
- Apache Arrow: `25.0.1` baseline; Decimal128 supports up to 38 digits and Decimal256 up to 76 digits.
- ADBC libraries: release 24; ADBC API specification `1.1.0`; Rust library remains pre-1.0 and therefore stays behind a boundary.
- OTLP specification: `1.11.0`, stable for traces, metrics and logs.
- Substrait remains an interoperability dependency, not the canonical SEF IR.
- Axum verified line: `0.8.9`; SQLx verified line: `0.9.0`.
- OpenAPI verified current specification: `3.2.1` (2026-09-10).
- PostgreSQL server baseline: stable `18.x` line (official release notes list 18.6 at audit date); PostgreSQL 19 remains Beta 4 and is not the production baseline.
- Apache ECharts verified current major/minor line: `6.1.x`; the console pins a tested 6.x release.

Version numbers are evidence, not architecture. Implementations pin tested versions and upgrade deliberately.

## 4. Adopt / isolate / defer

### Adopt

Rust, Arrow, DuckDB for first engine, Parquet, SQL, Git, OpenTelemetry.

### Adopt behind a boundary when its feature is implemented

Apache Ossie first. OpenLineage is adopted only when lineage export is implemented. ADBC is added only for a concrete remote adapter whose driver/conformance justify it.

### Evaluate / isolate — do not add as dependencies preemptively

DataFusion, Substrait, Flight SQL, specific SQL parser libraries, table-format integrations.

No optional technology in this section is added to the workspace merely because it appears in the architecture. A dependency enters only with a consuming feature, tests and benchmark/security/license evidence.

### Defer until product evidence

Distributed scheduler, custom storage engine, custom network protocol, GPU execution, approximate/sketch metrics, differential privacy, semantic write operations.

## 5. Canonical standards decisions

- semantic fingerprint encoding: deterministic CBOR per **RFC 8949**;
- semantic fingerprint digest: **SHA-256**;
- object semantic versions: **SemVer 2.0.0**;
- timezone IDs: **IANA TZDB**;
- currencies: **ISO 4217**;
- physical/scientific units: **UCUM**;
- telemetry: **OpenTelemetry/OTLP**;
- lineage export: **OpenLineage** where it fits without weakening SEF semantics.

## 6. Project license

SEF core is specified for **Apache-2.0** licensing. Dependencies must be compatible with that distribution model. A future commercial offering may add services or proprietary edge components, but the semantic core license must not be changed accidentally by dependency choice.

## 7. Rust/toolchain policy

- repository pins one tested stable Rust toolchain in `rust-toolchain.toml`;
- Edition 2024;
- upgrades are deliberate PRs with full conformance and benchmark runs;
- `Cargo.lock` is committed for reproducible application/workspace builds;
- unsafe Rust is denied by default in semantic crates and requires explicit reviewed exception if ever introduced.

## 8. Supply-chain baseline

CI/release must include:

- license/source policy (`cargo-deny` or equivalent);
- known-vulnerability audit (`cargo-audit`/RustSec or equivalent);
- locked dependency graph;
- SBOM generation in an open format (CycloneDX or SPDX) for releases;
- provenance/checksums for release artifacts;
- no Git dependency on an unpinned moving branch in release builds.

These are engineering controls, not semantic-core dependencies.

## 9. Performance rules

- benchmark hot paths before/after significant algorithm or dependency changes;
- use representative semantic workloads, not microbenchmarks alone;
- exact correctness cannot be traded for speed silently;
- prefer columnar movement and avoid row materialization unless justified;
- minimize copies across Arrow-compatible boundaries;
- do not introduce distributed complexity before profiling justifies it.

## 10. Upgrade rule

A dependency upgrade is accepted only after unit/integration/corpus/conformance tests pass, benchmark regressions are understood, security/license checks pass and architecture boundaries remain intact. Major upstream redesigns are absorbed by adapters, not propagated through the core.

## 11. Semantic Console technology baseline

The console follows the same selection policy as the backend: open source, modern, performant, interoperable and replaceable. UI libraries are product-edge dependencies and cannot leak into semantic contracts.

### Semantic Console adopted stack

| Concern | Choice | Reason |
|---|---|---|
| Web framework | **Svelte 5 + SvelteKit** | compact compiled UI model, modern reactivity, SSR/client rendering, Vite ecosystem |
| Language | **TypeScript** | typed product contracts and tooling |
| Styling | **Tailwind CSS 4 + native CSS variables/tokens** | modern CSS-first workflow; tokens remain standard CSS and portable |
| Accessible primitives | **Bits UI** | Svelte 5 headless primitives with styling freedom |
| Component recipes | **shadcn-svelte** | source-owned components built for Svelte 5/Tailwind 4; avoids runtime design-system lock-in |
| Tables | **TanStack Table + TanStack Virtual (Svelte)** | headless large-data table logic + virtualization while keeping markup/style control |
| Server state | **TanStack Query for Svelte, only where caching/revalidation complexity justifies it** | first-class Svelte integration; do not use for simple local state |
| Charts | **Apache ECharts 6.1.x** | Apache-2.0, broad analytical chart coverage, modular imports, high-performance canvas/SVG |
| Graph/plan UI | **Svelte Flow (`@xyflow/svelte`)** | MIT, Svelte-native node/edge interaction for SEIR and lineage explorers |
| Code/model editor | **CodeMirror 6** | open/MIT, modular browser editor; suitable for JSON/YAML/SQL-like text surfaces |
| Runtime validation | **Valibot via Standard Schema-compatible boundaries where useful** | small modular validator; UI/API validation remains separate from semantic validation |
| Icons | **Lucide Svelte** | open, lightweight, consistent SVG icon set |

### Native-first rules

Prefer Svelte 5 runes, SvelteKit routing/load/actions, native `fetch`, HTML, CSS, Web APIs and browser accessibility primitives before adding another state/router/form abstraction.

Do **not** introduce a global client state library by default. Domain server state belongs in the API/query layer; local interaction state stays local. Add a library only after a concrete cross-feature state requirement appears.

### UI components we intentionally avoid as architecture

- proprietary enterprise grid/chart libraries as mandatory foundations;
- React-wrapper dependencies inside a Svelte application;
- UI kits that require adopting their global runtime/theme model across the product;
- chart libraries for semantic correctness logic;
- browser-side SQL/semantic planners duplicating the Rust core.

A commercial/proprietary component may be evaluated later only if it provides material value unavailable in the open stack and remains replaceable behind a product-edge component interface.

### Large-result transport/rendering

- API control payloads: typed JSON over HTTPS;
- analytical bulk results: Arrow-compatible transport when measurements justify it;
- render only bounded/virtualized rows in the browser;
- filtering/sorting/aggregation over large result sets remains server-side unless the result is explicitly bounded and local processing is safe;
- ECharts should use modular imports (`echarts/core`) to control bundle size.

### Browser baseline

Tailwind CSS 4 targets modern browsers. SEF Console therefore targets current evergreen Chromium/Firefox/Safari-class browsers unless an enterprise deployment explicitly requires an older compatibility profile.

## 12. Server/control-plane stack

Preferred implementation baseline for the product service:

| Concern | Preferred technology | Boundary |
|---|---|---|
| async runtime | Tokio | internal runtime detail |
| HTTP/server | Axum + Tower | `product-api` adapter |
| TLS | rustls ecosystem | transport boundary |
| control-plane SQL access | SQLx 0.9.x | repository adapters |
| local metadata | SQLite stable | `MetadataStore` adapter |
| server metadata | PostgreSQL 18 stable line | `MetadataStore` adapter |
| serialization | Serde | internal/public DTO-specific |
| analytical interchange | Apache Arrow | result boundary |
| API description | OpenAPI 3.2.1 baseline | public HTTP contract |

Do not adopt PostgreSQL 19 while it remains beta. Dependency versions are pinned in implementation lockfiles; this document records technology families and maturity policy, not an evergreen lockfile.

## 13. Identity, authorization and secrets

### Identity

- default enterprise integration: OpenID Connect 1.0 over OAuth 2.0;
- follow OAuth 2.0 Security Best Current Practice (RFC 9700);
- authorization-code + PKCE for browser flows;
- workload identity / OAuth client-credentials for service-to-service clients where available;
- optional API-key fallback is scoped/rotatable/revocable and stored only as a verifier/hash;
- SEF does not implement an identity provider;
- browser tokens should remain server-side where topology permits; session cookies are `Secure`, `HttpOnly` and restrictive `SameSite` by default.

### Product authorization

`AuthorizationProvider` is the contract. Preferred first policy adapter: **Cedar** (Apache-2.0), because it is purpose-built for fast/analyzable RBAC/ABAC. Cedar policy objects never become CSMIR/SQIR/SEIR.

### Secrets

`SecretProvider` is mandatory. Baseline providers:

- local/development: environment/file-backed provider with strict file permissions;
- production OSS option: OpenBao adapter;
- cloud/vendor KMS adapters are optional product integrations.

Secrets are never written to logs, provenance, model IRs or browser payloads. Secret values are zeroized/redacted where practical and rotated independently of semantic releases.

## 14. API/product contracts

- HTTP API: HTTPS + OpenAPI 3.2.1-baseline JSON contracts; future compatible 3.2.x patch upgrades follow the upgrade rule;
- analytical large-result path: Arrow IPC;
- progress/events: SSE by default when push is needed;
- CLI talks through the same service contracts or an embedded facade, not privileged hidden semantics;
- Python SDK is generated/thin over public contracts plus Arrow/PyArrow-friendly result handling;
- MCP is an adapter over governed product/query APIs, never a direct database channel;
- Semantic SQL is a later public surface over the same correctness/planning core.

### Configuration/security edge baseline

- typed configuration schema with explicit version; unknown critical keys fail validation;
- precedence: safe defaults < file < environment < explicit CLI/deployment overrides;
- secret configuration contains only `SecretProvider` references;
- cookie-authenticated browser mutations require anti-CSRF protection;
- CORS is deny-by-default; CSP/frame restrictions and HSTS are enabled in production profiles where SEF terminates TLS;
- metadata/backup encryption at rest is provided by the deployment/storage platform; network links carrying credentials/tokens use TLS;
- source connectors enforce administrator-controlled egress/path policy to reduce SSRF/path traversal exposure.

## 15. Observability stack

- Rust instrumentation: `tracing` facade plus OpenTelemetry integration;
- export: OTLP;
- telemetry types: traces, metrics and structured logs;
- correlation: request/query/job/trace IDs propagated across product/runtime boundaries;
- semantic telemetry uses stable low-cardinality codes/IDs, never high-cardinality raw user data by default;
- sensitive filter values, tokens, credentials and raw policy context are redacted;
- no vendor/product analytics or crash-reporting egress is enabled by default; OTLP/export destinations are explicit operator configuration.

OpenTelemetry Rust library maturity is evaluated/pinned at implementation time; the product depends on the OTLP boundary rather than one exporter implementation.

## 16. UI/UX quality bar

The Semantic Console targets an engineering-tool UX comparable to premium developer/data products while remaining open-source and source-owned.

Baseline remains:

- Svelte 5 + SvelteKit + TypeScript;
- Tailwind CSS 4 with design tokens in standard CSS custom properties;
- Bits UI + shadcn-svelte source-owned components;
- TanStack Table + Virtual;
- Apache ECharts 6.1.x;
- Svelte Flow;
- CodeMirror 6;
- Lucide Svelte.

Required UX principles:

- keyboard-first navigation and command palette for primary workflows;
- resizable/savable workspaces for query/plan/model inspection;
- deep links to model/query/plan/job IDs;
- progressive streaming with clear execution states and cancellation;
- skeletons/optimistic UI only for product state, never fabricated semantic results;
- dense and comfortable display modes using the same token system;
- first-class dark/light themes using OKLCH-compatible tokens;
- virtualized large tables/graphs and incremental rendering;
- visible provenance/freshness/certification adjacent to results;
- structured error remediation instead of raw stack traces;
- no layout shift when streaming results/diagnostics;
- accessibility target: WCAG 2.2 AA for primary workflows.

Indicative web-performance budgets for production builds:

- no unnecessary framework-wide hydration for static surfaces;
- route-level code splitting;
- Core Web Vitals good-range targets for ordinary console pages;
- 60fps target for direct manipulation of typical graph/table interactions;
- large-data workload is server-filtered/streamed rather than loaded wholesale into browser memory.

## 17. Frontend engineering toolchain

- package manager: pnpm;
- build/dev: Vite through SvelteKit;
- unit/component tests: Vitest;
- browser E2E: Playwright;
- Svelte type checks: `svelte-check`;
- lint/format: official Svelte-compatible ESLint + Prettier toolchain unless a faster replacement demonstrates complete Svelte support;
- accessibility automation supplements, never replaces, manual keyboard/screen-reader verification.

## 18. Software supply-chain security

Every release pipeline should produce verifiable artifacts:

- Cargo lockfile and JS lockfile committed for applications;
- `rustfmt`, Clippy `-D warnings`, `cargo test`/`cargo-nextest`;
- Rust advisory/license/source policy via RustSec/cargo-audit and cargo-deny (or equivalent OSS successors);
- frontend dependency/advisory scan;
- fuzzing for parsers/IR canonicalization/unsafe input boundaries;
- CycloneDX SBOM (current stable standard pinned by release pipeline);
- OCI/binary signatures using Sigstore/Cosign where distribution channel supports it;
- provenance/attestations for release artifacts;
- reproducible-build work tracked explicitly; deterministic semantic hashes do not imply bit-reproducible binaries.

No dependency is accepted solely for convenience if it introduces an avoidable proprietary runtime requirement into the core product path.

### Supported-platform policy

The GA support matrix is explicit and tested, not inferred from Rust portability:

- server/OCI baseline: Linux x86_64 and arm64 where all pinned native dependencies pass conformance;
- local CLI/embedded targets: Windows, macOS and Linux architectures actually covered by CI/release artifacts;
- a platform is not advertised as supported until the semantic corpus, packaging and smoke tests pass there.

The Console remains browser-based and follows the browser baseline above.

## 19. Deployment baseline

Order of operational complexity:

1. **embedded/local** — single process, SQLite metadata, DuckDB/local sources;
2. **single-node service** — `sef-server`, PostgreSQL optional/required by scale, external OIDC, external secrets;
3. **OCI deployment** — signed container image, persistent metadata DB, stateless API/runtime where possible;
4. **distributed workers** — only after measured need, through execution/job interfaces;
5. Kubernetes manifests/charts are packaging, not architecture.

The default install should stay simple enough that `sef start` (or equivalent) can run a useful local product without a cluster.
