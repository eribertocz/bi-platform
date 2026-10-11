//! Deterministic synthetic fixtures, their exact oracle and a minimal
//! benchmark harness (G0.10, G0.13). G1 compares DuckDB and Polars results
//! against these oracles.

pub mod bench;
pub mod oracle;
pub mod parquet_io;
pub mod rng;
pub mod sales;
pub mod typed_json;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
