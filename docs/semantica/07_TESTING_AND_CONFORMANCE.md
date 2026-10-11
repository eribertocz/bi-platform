# Testing and Conformance

## 1. Principle

Semantic correctness is demonstrated by deterministic fixtures, structured errors, algebraic properties and cross-engine differentials. A backend is not supported merely because it connects or executes SQL.

## 2. Test layers

1. **IR validation** — IDs, types, canonicalization, hashes, model invariants.
2. **Binding/SQIR** — deterministic name/role/literal binding.
3. **Correctness** — valid/invalid requests, exact facts/obligations/diagnostics.
4. **Planner golden tests** — canonical SEIR + obligation-discharge map.
5. **Execution corpus** — exact results/errors against fixtures.
6. **Contract-violation tests** — uniqueness, temporal single-match, allocation normalization.
7. **Cross-engine differential tests** — same canonical semantics, equivalent result.
8. **Property tests** — algebraic invariants over generated small models/data.
9. **Fuzz tests** — parsers/binders/canonicalizers must not panic or accept malformed contracts.
10. **Benchmarks** — planning latency, execution overhead, memory and optimizer gains after correctness is stable.

## 3. Normative corpus v0.1

`corpus/` contains 21 deterministic, implementation-neutral cases covering:

- additive metrics;
- ordinary and multi-hop dimensions;
- cardinality;
- fanout;
- ratio recomputation;
- semi-additive period-end;
- exact distinct;
- multi-fact same/different grain;
- many-to-many allocation;
- time grain conversion;
- temporal as-of;
- null semantics;
- mandatory row policy;
- ambiguous path rejection;
- incompatible unit rejection;
- materialization equivalence;
- decimal/timezone cross-engine behavior;
- illegal temporal reaggregation;
- cumulative/window semantics.

Existing expected truth cannot be changed to make an implementation easier.

## 4. Structured-error rule

Rejection fixtures and tests assert stable structured fields (`code`, refs/evidence), not English wording. Human messages may improve without breaking machine contracts.

The three original normative error codes remain:

```text
AMBIGUOUS_RELATIONSHIP_PATH
INCOMPATIBLE_UNITS
ILLEGAL_TEMPORAL_REAGGREGATION
```

Additional implementation-critical codes are defined in `04_CORRECTNESS_AND_PLANNING.md`.

## 5. Mandatory edge suite before first public release

These do not require new architecture; they close adversarial gaps around the existing contracts and MUST be added as fixtures/tests during implementation:

1. zero denominator with both `Null` and `Error` ratio policies;
2. many-to-one target uniqueness violated at runtime;
3. bridge weights missing/not summing to required total;
4. temporal as-of returns multiple matches;
5. mandatory policy context missing;
6. decimal overflow and explicit rounding/narrowing;
7. composite-key relationship;
8. exact string equality/case behavior under backend collation differences;
9. DST-sensitive timezone conversion from absolute instants;
10. deterministic period-end tie with and without explicit tie-break;
11. typed post-aggregate metric filter (`HAVING`) at the correct semantic grain, including a derived ratio metric.

No new conceptual layer is implied by these tests.

## 6. Property tests

At minimum:

- additive partition invariance;
- ratio recomposition invariance;
- fanout invariance under safe dimension attachment;
- policy non-broadening;
- conformed-dimension alignment invariance;
- time-bucket determinism for a fixed TZDB version;
- canonical fingerprint invariance under non-semantic ordering changes;
- planner determinism for identical canonical inputs;
- materialization substitution equivalence.

## 7. New engine conformance gate

An engine adapter is marked supported only when:

- all applicable corpus and edge cases pass;
- every claimed `NativeExact`/`EmulatedExact` capability is covered;
- decimal/time/null/string semantics conform;
- mandatory policies and contract guards are preserved;
- exact semantic errors remain engine-independent;
- cross-engine result differentials are zero for exact contracts;
- known limitations are capability-rejected, not silently changed.

## 8. Optimizer gate

Every semantic rewrite/materialization substitution is compared against the deterministic baseline SEIR. If equivalence cannot be demonstrated, use the baseline plan.

## 9. Security tests

Required before service mode/public release:

- caller cannot inject policy context slots through SQIR;
- caller values cannot produce SQL injection;
- source identifiers cannot escape trusted source catalog quoting;
- protected rows cannot contribute before `PolicyGuard`;
- cache/materialization identity is partitioned by semantic policy binding;
- sensitive context values never appear in logs/provenance;
- unrestricted SQL is not exposed as the governed agent path.

## 10. Release gate

Before a release:

- fmt/lint/unit/integration/corpus/property tests pass;
- no dependency-boundary violation;
- no undocumented semantic-code change;
- engine capability matrices are current;
- benchmarks have no unexplained material regression;
- dependency license/security checks pass;
- SBOM/release provenance generated;
- documentation and implementation profile versions agree.

The fixture audit in `corpus/AUDIT_V0.1.md` proves only the expected fixture truth; it does not certify an implementation.

## 11. Runtime/control-plane tests

Before product beta, add:

- metadata repository contract tests against SQLite and PostgreSQL;
- migration forward/rollback-or-restore tests;
- query lifecycle/cancellation/deadline tests;
- admission/backpressure tests under overload;
- scheduler lease/idempotency/retry recovery tests, including two service replicas contending for work;
- materialization atomic publish/failure recovery tests;
- cache isolation tests across policy/tenant/model/source snapshots;
- source-schema drift invalidation/revalidation tests;
- typed configuration precedence/validation/snapshot-stability tests;
- query/audit/job retention and redaction tests;
- graceful shutdown/resource leak tests.

## 12. Identity/authorization/security tests

Mandatory:

- OIDC session/token validation integration tests using a disposable test issuer/mock conforming to the chosen flow;
- service/workload identity and API-key fallback scope/revocation tests when those modes are enabled;
- authorization matrix tests across organization/workspace/project/environment/resource/action;
- semantic policy cannot be bypassed by product authorization;
- cross-tenant cache/result/materialization isolation;
- CSRF, CORS, CSP/security-header and cookie configuration tests for browser deployments;
- token/secret/log redaction tests;
- SQL/identifier injection tests at every adapter boundary;
- SSRF/path traversal tests for source/config import surfaces;
- rate/admission abuse tests;
- dependency/advisory/license scanning in CI.

## 13. API compatibility tests

Public contracts require:

- OpenAPI schema validation;
- golden request/response/error fixtures;
- backward-compatibility diff in CI for stable API versions;
- generated client smoke tests;
- Arrow result schema/metadata compatibility tests;
- old-client/new-server compatibility for the documented support window;
- semantic diff/impact classification golden tests;
- Ossie supported-subset import/export round-trip tests plus explicit loss-report tests.

## 14. UI E2E and accessibility tests

Semantic Console release gate includes:

- Playwright E2E for model browse → query → diagnostics/plan → result/provenance;
- keyboard-only primary workflows;
- focus restoration for dialogs/drawers/navigation;
- automated accessibility scan plus manual WCAG 2.2 AA spot checks;
- large-table virtualization and graph interaction performance scenarios;
- cancellation/timeout/partial-progress UX, including proof that incomplete streams are never presented as certified complete results;
- dark/light and density-token visual regression for core surfaces;
- no client-side semantic decision differs from backend truth.

## 15. Performance and resilience benchmarks

Track separately:

```text
semantic compile latency
correctness latency
planning latency
engine lowering latency
first-batch latency
total execution latency
stream throughput
memory peak
concurrency saturation
cache/materialization hit ratio
control-plane API latency
UI route/interaction budgets
```

Benchmark modes include cold/warm, single/concurrent, local/remote engine and materialized/non-materialized paths. Performance improvements fail the gate if semantic equivalence is lost.

Resilience scenarios include engine disconnect, metadata DB restart, cancellation storm, telemetry exporter outage, failed materialization refresh and process restart with durable jobs.

## 16. Backup/restore and upgrade tests

Before GA:

- create metadata backup from a populated realistic environment and measure recovery against the declared deployment-profile RPO/RTO;
- restore into a clean compatible deployment and compare registry/audit/job invariants;
- migration from every supported previous metadata schema version;
- failure during migration leaves a recoverable state;
- release rollback procedure is documented/tested when metadata compatibility permits it;
- secret references remain references and do not leak into backup artifacts unintentionally.

## 17. GA release gate

A GA release requires all earlier gates plus:

- semantic corpus and mandatory edge suite green;
- DuckDB plus at least one second engine differential conformance for the declared shared capability set;
- security matrix and tenant-isolation suite green;
- load/admission benchmarks within published budgets;
- metadata migration/backup/restore drill green against the declared recovery profile;
- published SLI/SLO and retention/redaction profile has executable monitoring/tests;
- Semantic Console primary E2E/WCAG gate green;
- SBOM generated and release artifacts signed/attested;
- every advertised OS/architecture release passes packaging + smoke + applicable semantic conformance gates;
- no unresolved critical/high vulnerability in the shipped path unless explicitly risk-accepted with documented mitigation.
