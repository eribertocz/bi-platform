//! Embedded analytical runtime (M13): DuckDB in memory through `duckdb-rs`,
//! a DuckLake attached as `lake` whose catalog lives in PostgreSQL and whose
//! data are Parquet files. No `.duckdb` file is involved.
//!
//! Startup has two phases. [`Bootstrap`] runs administrative work (extensions,
//! `ATTACH`, fixture loading) and then [`Bootstrap::finish`] locks the
//! configuration and hands queries to a bounded pool of connections, each
//! owned by a dedicated thread so native calls never block Tokio.

pub mod bootstrap;
pub mod config;
pub mod fixtures;
pub mod pool;
pub mod secret;

use std::fmt;

pub use bootstrap::{Bootstrap, LoadOutcome};
pub use config::{LakeConfig, RuntimeConfig};
pub use duckdb;
pub use pool::{PoolLoad, Runtime};

/// Alias under which the DuckLake is attached.
pub const LAKE: &str = "lake";

#[derive(Debug)]
pub enum RuntimeError {
    /// Invalid operator-provided resource configuration.
    Config(config::ConfigError),
    /// DuckDB exhausted the query instance's managed-memory budget.
    MemoryLimit,
    /// The engine rejected or failed the statement.
    Engine(duckdb::Error),
    /// The lake could not be attached; the message never includes credentials.
    Lake(String),
    Io(std::io::Error),
    /// The admission queue is full.
    Busy,
    /// The query was cancelled before it finished.
    Cancelled,
    /// The job panicked; its connection was replaced.
    Panicked,
    /// The runtime is shutting down.
    Unavailable,
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(f),
            Self::MemoryLimit => f.write_str("query memory limit exceeded"),
            Self::Engine(error) => write!(f, "query engine error: {error}"),
            Self::Lake(message) => write!(f, "lake unavailable: {message}"),
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Busy => f.write_str("query queue is full"),
            Self::Cancelled => f.write_str("query cancelled"),
            Self::Panicked => f.write_str("query job panicked"),
            Self::Unavailable => f.write_str("runtime is shutting down"),
        }
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::Engine(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<duckdb::Error> for RuntimeError {
    fn from(error: duckdb::Error) -> Self {
        // duckdb-rs 1.10506.0 erases duckdb_result_error_type and sets every
        // ffi::Error::code to Unknown. Match only the native exception prefix.
        if matches!(&error, duckdb::Error::DuckDBFailure(_, Some(message))
            if message.starts_with("Out of Memory Error:"))
        {
            Self::MemoryLimit
        } else {
            Self::Engine(error)
        }
    }
}

impl From<std::io::Error> for RuntimeError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Quotes `value` as a SQL string literal.
#[must_use]
pub fn sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_sql_strings() {
        assert_eq!(sql_string("a"), "'a'");
        assert_eq!(sql_string("o'clock"), "'o''clock'");
    }
}

#[cfg(test)]
mod memory_error_tests {
    use super::*;
    #[test]
    fn only_the_native_oom_prefix_is_a_memory_error() {
        for message in [
            "Binder Error: no column memory_limit",
            "Invalid Input Error: Out of Memory Error: user text",
        ] {
            let error =
                duckdb::Error::DuckDBFailure(duckdb::ffi::Error::new(1), Some(message.to_owned()));
            assert!(matches!(RuntimeError::from(error), RuntimeError::Engine(_)));
        }
    }
}
