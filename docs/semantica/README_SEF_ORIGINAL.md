# Semantic Execution Fabric (SEF) — Engineering Blueprint

Status: **final-audited product architecture blueprint — approved for phased Codex implementation**  
Baseline date: **2026-09-27**
Audit revision: **v1.4**

## Product in one sentence

SEF is an open, engine-neutral **semantic execution runtime** that accepts standard semantic models (Apache Ossie at the boundary), validates semantic correctness, plans metrics safely, and executes them across analytical engines without coupling business meaning to any engine.

## Core pipeline

```text
External semantic model (Ossie first)
        ↓
Compatibility adapter
        ↓
CSMIR  +  SQIR
        ↓
Semantic Intelligence Core
(correctness + semantic planning)
        ↓
SEIR
        ↓
Physical planner / engine adapter
        ↓
DuckDB first; other engines later
        ↓
Arrow-compatible results + provenance
```

## The moat

The moat is **not YAML, SQL rendering, MCP, UI, or a cache**. It is the Semantic Intelligence Core:

- grain algebra;
- fanout detection/avoidance;
- relationship-path reasoning;
- multi-fact alignment;
- metric algebra and safe recomputation;
- temporal and semi-additive semantics;
- semantic equivalence and later semantic optimization;
- deterministic evidence explaining why a plan is semantically valid.

## Non-negotiable rules

1. Apache Ossie is an external interoperability boundary, never SEF's internal object model.
2. CSMIR, SQIR and SEIR are owned by SEF and are engine-neutral.
3. Correctness precedes optimization.
4. Engines and protocols live behind adapters.
5. Open source, standards, columnar/vectorized processing, interoperability and low lock-in are preferred.
6. Rust is the core implementation language; SQL is an execution language, not the semantic model.
7. Arrow is the preferred columnar interchange boundary.
8. DuckDB is the first concrete execution target, not an architectural dependency of the semantic core.
9. LLMs/agents may discover and propose semantics, but deterministic core logic defines trusted results.
10. Documentation stays small: these files are the canonical set; implementation details belong in code/tests later.

## Canonical documents

- `docs/01_PRODUCT.md` — product, scope, moat and non-goals.
- `docs/02_ARCHITECTURE.md` — layers, ownership, IRs, module boundaries and dependency laws.
- `docs/03_MODEL_AND_QUERY_IR.md` — detailed CSMIR and SQIR contracts.
- `docs/04_CORRECTNESS_AND_PLANNING.md` — correctness rules, obligations, semantic planner and SEIR.
- `docs/05_EXECUTION_AND_ADAPTERS.md` — physical lowering, engine adapters, capabilities and execution boundaries.
- `docs/06_TECHNOLOGY.md` — standards, technology policy and current baseline.
- `docs/07_TESTING_AND_CONFORMANCE.md` — semantic corpus, conformance and acceptance strategy.
- `docs/08_IMPLEMENTATION_PLAN.md` — finite handoff plan for Codex.

`corpus/` contains the normative 21-case semantic problem corpus and deterministic fixtures.


## Implementation-readiness verdict

The semantic and product architecture is closed enough to begin **Phase 1 (Semantic Kernel)** and proceed through the finite six-phase implementation plan without inventing foundational product semantics. The audited baseline freezes implementation-critical profiles for deterministic hashing, expression semantics, grain proofs, diagnostics, policy trust boundaries, engine capabilities and provenance.

This does **not** mean the public product API is frozen. External APIs remain intentionally evolvable until DuckDB conformance is proven.

## Implementation state

**Phase 1A — canonical foundations and CSMIR** is implemented in
`crates/sef-model-ir`. It provides definition-local compilation and immutable
semantic fingerprints under the frozen
[`sef-canonical-value-v1` ADR](docs/adr/0001-sef-canonical-value-v1.md).
The [coverage matrix](corpus/PHASE_1A_COVERAGE.md) covers all 21 corpus model
representations; query execution and certification belong to later work packages.
See the [Phase 1A engineering report](docs/PHASE_1A_REPORT.md) for verification,
dependencies and limitations. Phase 1B requires separate explicit authorization.

## Semantic Console

The canonical product console is **Svelte 5 + SvelteKit**. It remains outside the semantic core and uses an open-source UI stack specified in `docs/06_TECHNOLOGY.md`. Its responsibilities and implementation order are defined in `docs/01_PRODUCT.md`, `docs/02_ARCHITECTURE.md` and `docs/08_IMPLEMENTATION_PLAN.md`.

## Complete product scope

The blueprint now covers the Semantic Intelligence Core, execution runtime, control plane, metadata persistence, identity/authorization boundaries, tenancy, configuration, source-schema drift, jobs, materializations/cache, product APIs, Semantic Console, semantic diff/impact analysis, observability, deployment, recovery objectives, retention/privacy boundaries, migrations, backup/restore and software-supply-chain release requirements. Implementation is intentionally divided into **six finite phases** in `docs/08_IMPLEMENTATION_PLAN.md`.

## Codex execution protocol

Implementation is approval-gated. For every phase and substantial work package, Codex must follow **Plan → explicit authorization → implementation → verification → engineering report → stop**. It must not start the next work package automatically. The normative details are in `CODEX_START.md` and `docs/08_IMPLEMENTATION_PLAN.md`.
