# References

Only upstream/official sources are used for architectural baseline decisions.

## Apache Ossie

- Core spec (draft 0.2.0.dev0): https://github.com/apache/ossie/blob/main/core-spec/spec.md
- Repository overview: https://github.com/apache/ossie/blob/main/README.md
- Roadmap: https://github.com/apache/ossie/blob/main/ROADMAP.md
- Expression language: https://github.com/apache/ossie/blob/main/core-spec/expression_language.md
- Version maturity discussion (1.0 → 0.1.1): https://github.com/apache/ossie/discussions/80

## Apache Arrow / ADBC / Flight SQL

- ADBC API Standard: https://arrow.apache.org/adbc/current/format/specification.html
- ADBC 24 release (API standard 1.1.0): https://arrow.apache.org/blog/2026/07/28/adbc-24-release/
- Flight SQL specification: https://arrow.apache.org/docs/format/FlightSql.html

## Apache DataFusion

- DataFusion 55.0.0 release: https://datafusion.apache.org/blog/output/2026/08/25/datafusion-55.0.0/
- User guide: https://datafusion.apache.org/user-guide/

## Substrait

- Specification/status: https://substrait.io/spec/specification/

## OpenTelemetry

- Specification index: https://opentelemetry.io/docs/specs/
- OpenTelemetry spec: https://opentelemetry.io/docs/specs/otel/
- OTLP: https://opentelemetry.io/docs/specs/otlp/
- Semantic conventions: https://opentelemetry.io/docs/specs/semconv/

## OpenLineage

- Specification/schema documentation: https://openlineage.io/docs/spec/schemas/

## Competitive/reference implementations

- Cube Core 1.7 / Tesseract GA: https://cube.dev/blog/cube-core-v1-7-tesseract-ga-data-modeling-performance
- MetricFlow repository: https://github.com/dbt-labs/metricflow

## Baseline review rule

Before starting each major milestone:

1. Re-check Apache Ossie roadmap/spec status.
2. Re-check current Arrow/ADBC/DataFusion releases.
3. Re-check Substrait 1.0 status.
4. Review Cube/MetricFlow only for capabilities and failure modes; do not copy their internal contracts blindly.
5. Update ADRs when a baseline decision changes.

## Semantic corpus review sources

- Apache Ossie current core specification: https://github.com/apache/ossie/blob/main/core-spec/spec.md
- Apache Ossie expression language proposal: https://github.com/apache/ossie/blob/main/core-spec/expression_language.md
- Cube data modeling: https://cube.dev/product/data-modeling
- Cube Core 1.7 / Tesseract GA: https://cube.dev/blog/cube-core-v1-7-tesseract-ga-data-modeling-performance
- Cube Semantic SQL / E-Graphs: https://cube.dev/blog/how-semantic-sql-works


## Semantic type/unit/time standards

- UCUM specification 2.2: https://ucum.org/ucum
- ISO 4217 Maintenance Agency (SIX): https://www.six-group.com/en/products-services/financial-information/market-reference-data/data-standards.html
- IANA Time Zone Database: https://www.iana.org/time-zones
- IANA TZ database releases: https://www.iana.org/time-zones/releases

## Deterministic identity / engineering standards

- RFC 8949 — CBOR and deterministic encoding requirements: https://www.rfc-editor.org/rfc/rfc8949.html
- SemVer 2.0.0: https://semver.org/spec/v2.0.0.html
- RustSec Advisory Database: https://rustsec.org/
- CycloneDX: https://cyclonedx.org/
- SPDX: https://spdx.dev/

## Product/runtime/security baseline verified 2026-09-27

- Rust 1.98.1 stable release: https://blog.rust-lang.org/releases/latest/
- Axum 0.8.9: https://docs.rs/crate/axum/latest/builds
- SQLx 0.9.0: https://docs.rs/crate/sqlx/latest/source/CHANGELOG.md
- PostgreSQL release notes — stable 18.x line; 19 is Beta 4 at baseline date: https://www.postgresql.org/docs/release/
- PostgreSQL 19 Beta 4 notice: https://www.postgresql.org/about/news/postgresql-19-beta-4-released-3386/
- SQLite 3.53.4 release: https://www.sqlite.org/releaselog/3_53_4.html
- Cedar language documentation (language v4.5 at baseline; SDK is pinned separately): https://docs.cedarpolicy.com/
- Cedar language/SDK history: https://docs.cedarpolicy.com/other/doc-history.html
- OpenAPI Specification 3.2.1: https://spec.openapis.org/oas/v3.2.1.html
- RFC 9700 / BCP 240 — OAuth 2.0 Security Best Current Practice: https://www.rfc-editor.org/rfc/rfc9700.html
- Svelte/SvelteKit official docs: https://svelte.dev/docs
- Tailwind CSS v4 compatibility: https://tailwindcss.com/docs/compatibility
- Bits UI (Svelte 5 primitives): https://bits-ui.com/docs/getting-started
- shadcn-svelte Svelte 5/Tailwind 4 migration baseline: https://shadcn-svelte.com/docs/migration/tailwind-v4
- Apache ECharts changelog — 6.1.0 current 6.x release at baseline: https://echarts.apache.org/en/changelog.html
- WCAG 2.2: https://www.w3.org/TR/WCAG22/
- CycloneDX 1.7 stable BOM reference: https://cyclonedx.org/docs/1.7/json/
- Sigstore/Cosign: https://docs.sigstore.dev/cosign/signing/overview/
