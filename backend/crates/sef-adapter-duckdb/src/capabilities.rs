//! Versioned, inspectable capabilities. Unsupported entries are refusals;
//! they must never be implemented by forwarding user SQL to the engine.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupportLevel {
    NativeExact,
    EmulatedExact,
    Unsupported,
}

pub const PROFILE_VERSION: &str = "duckdb-seir-0.2";

/// Stable names accepted by `capability`; unknown names are rejected.
pub const CAPABILITIES: &[&str] = &[
    "decimal<=38",
    "decimal39..76-sum",
    "decimal-ratio",
    "int64",
    "null",
    "unicode-binary-order",
    "relationships",
    "unique-target",
    "required-match",
    "snapshot",
    "distinct",
    "row-filter",
    "policy",
    "multiple-blocks",
    "post-aggregate-filter",
    "order-limit",
    "two-branch-alignment",
    "temporal-as-of",
    "bridge-allocation",
    "period-end",
    "window",
    "local-date",
    "time-bucket",
    "arrow-stream",
    "execution-limits",
];

/// Exact support of this release, with a named strategy for emulation.
#[must_use]
pub fn capability(name: &str) -> (SupportLevel, &'static str) {
    match name {
        "decimal<=38" | "int64" | "null" | "unicode-binary-order" => {
            (SupportLevel::NativeExact, "native typed SQL and Arrow")
        }
        "decimal39..76-sum" => (
            SupportLevel::EmulatedExact,
            "integer coefficients after governed reads/joins; Decimal256 output; text grain and SUM only",
        ),
        "relationships" | "unique-target" | "required-match" => (
            SupportLevel::NativeExact,
            "validated key joins and runtime checks in one transaction",
        ),
        "snapshot" => (
            SupportLevel::NativeExact,
            "one DuckDB transaction for schema, guards and all blocks",
        ),
        "distinct"
        | "row-filter"
        | "policy"
        | "multiple-blocks"
        | "post-aggregate-filter"
        | "order-limit" => (
            SupportLevel::NativeExact,
            "certified CTEs and bound parameters",
        ),
        "decimal-ratio" => (
            SupportLevel::EmulatedExact,
            "BigInt quotient/remainder; declared scale and rounding; no floating point",
        ),
        "local-date" => (
            SupportLevel::EmulatedExact,
            "Gregorian local date using pinned IANA TZDB 2025b; other pins refused",
        ),
        "time-bucket" => (SupportLevel::NativeExact, "declared calendar date buckets"),
        "two-branch-alignment" => (
            SupportLevel::NativeExact,
            "two aggregated branches with a nonempty shared grain; full outer null-safe keys",
        ),
        "temporal-as-of" => (
            SupportLevel::NativeExact,
            "per-event interval single-match checks before joining",
        ),
        "bridge-allocation" => (
            SupportLevel::NativeExact,
            "unique bridge keys, normalized nonnegative weights, required coverage and exact contributions",
        ),
        "period-end" => (
            SupportLevel::NativeExact,
            "entity-scoped first/last row with checked ties or explicit tie order",
        ),
        "window" => (
            SupportLevel::NativeExact,
            "declared ordered SUM windows with explicit NULL placement",
        ),
        "arrow-stream" => (
            SupportLevel::NativeExact,
            "provisional bounded batches; completion certifies the entire read transaction",
        ),
        "execution-limits" => (
            SupportLevel::NativeExact,
            "output rows/bytes, retained emulation input bytes, checkpoints; native interruption and backpressure via worker runtime",
        ),
        _ => (
            SupportLevel::Unsupported,
            "no exact lowering in this release",
        ),
    }
}
