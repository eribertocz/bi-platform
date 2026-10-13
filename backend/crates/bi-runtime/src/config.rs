//! Runtime configuration. Values come from the operator (environment), never
//! from API requests; they are still validated before reaching SQL.

use std::fmt;
use std::path::PathBuf;

/// DuckLake catalog used when `BI_LAKE_CATALOG` is unset: the development
/// PostgreSQL from `compose.yaml`. Never a production credential.
pub const DEV_LAKE_CATALOG: &str =
    "dbname=bi_lake_catalog host=127.0.0.1 port=54320 user=bi password=bi";
pub const DEFAULT_LAKE_DATA_PATH: &str = "data/lake";
pub const DEFAULT_QUEUE_CAPACITY: usize = 64;
pub const DEFAULT_QUERY_MEMORY_LIMIT: &str = "256MiB";
const MAX_WORKERS: usize = 4;

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Query connections, each owned by one dedicated thread.
    pub workers: usize,
    /// Jobs allowed to wait for a free worker; beyond that, requests are refused.
    pub queue_capacity: usize,
    /// DuckDB thread budget, divided across independent query instances.
    pub duckdb_threads: Option<u32>,
    /// Total DuckDB managed-memory budget, e.g. `2GB`; divided across
    /// workers when query isolation is enabled.
    pub memory_limit: Option<String>,
    /// Independent managed-memory budget per active query. `None` keeps a
    /// shared instance for embedders that need shared in-memory tables.
    /// The server always configures a budget and uses independent instances.
    pub query_memory_limit: Option<String>,
    /// Where DuckDB installs extensions (the default lives in the user's home).
    pub extension_directory: Option<PathBuf>,
    pub lake: Option<LakeConfig>,
}

#[derive(Clone)]
pub struct LakeConfig {
    /// libpq connection string of the DuckLake catalog database. Secret.
    pub catalog: String,
    /// PostgreSQL schema for the DuckLake metadata tables (default: DuckLake's own).
    pub metadata_schema: Option<String>,
    /// Directory holding the lake's Parquet files.
    pub data_path: PathBuf,
}

impl fmt::Debug for LakeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LakeConfig")
            // Never printed, not even partially: it holds the password.
            .field("catalog", &"<redacted>")
            .field("metadata_schema", &self.metadata_schema)
            .field("data_path", &self.data_path)
            .finish()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    Invalid {
        variable: &'static str,
        value: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { variable, value } => write!(f, "invalid {variable}: {value:?}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl RuntimeConfig {
    /// In-memory DuckDB without a lake, e.g. for tests.
    #[must_use]
    pub fn in_memory(workers: usize) -> Self {
        Self {
            workers,
            queue_capacity: DEFAULT_QUEUE_CAPACITY,
            duckdb_threads: None,
            memory_limit: None,
            query_memory_limit: None,
            extension_directory: None,
            lake: None,
        }
    }

    /// Reads `BI_LAKE_*` and `BI_DUCKDB_*` variables through `lookup`.
    ///
    /// # Errors
    /// When a variable holds an invalid value.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let number = |variable: &'static str| -> Result<Option<usize>, ConfigError> {
            lookup(variable)
                .map(|value| {
                    value
                        .parse::<usize>()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or(ConfigError::Invalid { variable, value })
                })
                .transpose()
        };
        let default_workers = std::thread::available_parallelism()
            .map_or(1, std::num::NonZero::get)
            .min(MAX_WORKERS);

        let memory_limit = lookup("BI_DUCKDB_MEMORY_LIMIT");
        if let Some(value) = &memory_limit
            && !is_memory_size(value)
        {
            return Err(ConfigError::Invalid {
                variable: "BI_DUCKDB_MEMORY_LIMIT",
                value: value.clone(),
            });
        }
        let query_memory_limit = lookup("BI_QUERY_MEMORY_LIMIT")
            .unwrap_or_else(|| DEFAULT_QUERY_MEMORY_LIMIT.to_owned());
        if !is_memory_size(&query_memory_limit) {
            return Err(ConfigError::Invalid {
                variable: "BI_QUERY_MEMORY_LIMIT",
                value: query_memory_limit,
            });
        }
        let metadata_schema = lookup("BI_LAKE_METADATA_SCHEMA");
        if let Some(value) = &metadata_schema
            && !is_identifier(value)
        {
            return Err(ConfigError::Invalid {
                variable: "BI_LAKE_METADATA_SCHEMA",
                value: value.clone(),
            });
        }
        let duckdb_threads = number("BI_DUCKDB_THREADS")?
            .map(|n| {
                u32::try_from(n).map_err(|_| ConfigError::Invalid {
                    variable: "BI_DUCKDB_THREADS",
                    value: n.to_string(),
                })
            })
            .transpose()?;

        let config = Self {
            workers: number("BI_DUCKDB_WORKERS")?.unwrap_or(default_workers),
            queue_capacity: number("BI_DUCKDB_QUEUE")?.unwrap_or(DEFAULT_QUEUE_CAPACITY),
            duckdb_threads,
            memory_limit,
            query_memory_limit: Some(query_memory_limit),
            extension_directory: lookup("BI_DUCKDB_EXTENSION_DIR").map(PathBuf::from),
            lake: Some(LakeConfig {
                catalog: lookup("BI_LAKE_CATALOG").unwrap_or_else(|| DEV_LAKE_CATALOG.to_owned()),
                metadata_schema,
                data_path: lookup("BI_LAKE_DATA_PATH")
                    .map_or_else(|| PathBuf::from(DEFAULT_LAKE_DATA_PATH), PathBuf::from),
            }),
        };
        config.query_worker()?;
        Ok(config)
    }

    /// Configuration for one independent query instance. The old global
    /// budget and CPU count are divided across workers instead of multiplied.
    pub(crate) fn query_worker(&self) -> Result<Option<Self>, ConfigError> {
        let Some(limit) = &self.query_memory_limit else {
            return Ok(None);
        };
        let parse = |variable, value: &str| {
            memory_bytes(value).ok_or_else(|| ConfigError::Invalid {
                variable,
                value: value.to_owned(),
            })
        };
        let count = self.workers.max(1);
        let mut bytes = parse("BI_QUERY_MEMORY_LIMIT", limit)?;
        if let Some(total) = &self.memory_limit {
            bytes = bytes.min(parse("BI_DUCKDB_MEMORY_LIMIT", total)? / count as u64);
        }
        if bytes == 0 {
            return Err(ConfigError::Invalid {
                variable: "BI_QUERY_MEMORY_LIMIT",
                value: limit.clone(),
            });
        }
        let mut worker = self.clone();
        worker.workers = 1;
        worker.query_memory_limit = None;
        worker.memory_limit = Some(format!("{bytes}B"));
        let threads = self.duckdb_threads.map_or_else(
            || std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
            |threads| threads as usize,
        );
        worker.duckdb_threads = Some(u32::try_from((threads / count).max(1)).unwrap_or(u32::MAX));
        Ok(Some(worker))
    }
}

/// `[a-z_][a-z0-9_]*`, at most 63 bytes (PostgreSQL identifier limit).
#[must_use]
pub fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    value.len() <= 63
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// A number followed by a DuckDB size unit, e.g. `512MB`, `1.5 GiB`.
fn is_memory_size(value: &str) -> bool {
    memory_bytes(value).is_some()
}

/// Parse operator-configured sizes without rounding or accepting infinity.
fn memory_bytes(value: &str) -> Option<u64> {
    let value = value.trim();
    let split = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let multiplier: u128 = match unit.trim().to_ascii_uppercase().as_str() {
        "KB" => 1000,
        "MB" => 1000_u128.pow(2),
        "GB" => 1000_u128.pow(3),
        "TB" => 1000_u128.pow(4),
        "KIB" => 1024,
        "MIB" => 1024_u128.pow(2),
        "GIB" => 1024_u128.pow(3),
        "TIB" => 1024_u128.pow(4),
        _ => return None,
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if (whole.is_empty() && fraction.is_empty())
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let denominator = 10_u128.checked_pow(u32::try_from(fraction.len()).ok()?)?;
    let numerator = (if whole.is_empty() {
        0
    } else {
        whole.parse::<u128>().ok()?
    })
    .checked_mul(denominator)?
    .checked_add(if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u128>().ok()?
    })?;
    let bytes = numerator
        .checked_mul(multiplier)?
        .checked_div(denominator)?;
    u64::try_from(bytes).ok().filter(|bytes| *bytes > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        |key| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn defaults_point_to_the_development_lake() {
        let config = RuntimeConfig::from_lookup(lookup(&[])).unwrap();
        let lake = config.lake.unwrap();
        assert_eq!(lake.catalog, DEV_LAKE_CATALOG);
        assert_eq!(lake.data_path, PathBuf::from("data/lake"));
        assert!((1..=MAX_WORKERS).contains(&config.workers));
        assert_eq!(config.queue_capacity, DEFAULT_QUEUE_CAPACITY);
    }

    #[test]
    fn rejects_values_that_would_reach_sql_unchecked() {
        for (variable, value) in [
            ("BI_LAKE_METADATA_SCHEMA", "x'; DROP"),
            ("BI_LAKE_METADATA_SCHEMA", "Upper"),
            ("BI_DUCKDB_MEMORY_LIMIT", "2GB'; SET"),
            ("BI_DUCKDB_MEMORY_LIMIT", "lots"),
            ("BI_DUCKDB_WORKERS", "0"),
            ("BI_DUCKDB_QUEUE", "-1"),
        ] {
            assert_eq!(
                RuntimeConfig::from_lookup(lookup(&[(variable, value)])).unwrap_err(),
                ConfigError::Invalid {
                    variable,
                    value: value.to_owned()
                },
                "{variable}={value}"
            );
        }
        let config = RuntimeConfig::from_lookup(lookup(&[
            ("BI_DUCKDB_MEMORY_LIMIT", "1.5 GiB"),
            ("BI_LAKE_METADATA_SCHEMA", "bi_test_1"),
        ]))
        .unwrap();
        assert_eq!(config.memory_limit.as_deref(), Some("1.5 GiB"));
    }

    #[test]
    fn debug_output_never_shows_the_catalog() {
        let lake = LakeConfig {
            catalog: "dbname=x host=h password='s3cret with spaces' user=u".to_owned(),
            metadata_schema: None,
            data_path: PathBuf::from("lake"),
        };
        let debug = format!("{lake:?}");
        assert!(!debug.contains("s3cret"), "{debug}");
        assert!(!debug.contains("dbname"), "{debug}");
    }
}

#[cfg(test)]
mod memory_properties {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn sizes_are_exact_and_global_budget_is_never_multiplied(kib in 1_u64..1_000_000, workers in 1_usize..8) {
            let text = format!("{kib}KiB");
            prop_assert_eq!(memory_bytes(&text), Some(kib * 1024));
            let config = RuntimeConfig { memory_limit: Some(text), query_memory_limit: Some("1TiB".to_owned()), ..RuntimeConfig::in_memory(workers) };
            let worker = config.query_worker().unwrap().unwrap();
            prop_assert_eq!(worker.memory_limit, Some(format!("{}B", kib*1024/workers as u64)));
        }

        #[test]
        fn arbitrary_sizes_never_panic(text in ".{0,100}") {
            let _ = memory_bytes(&text);
        }
    }

    #[test]
    fn server_budget_is_default_and_cannot_be_disabled_by_environment() {
        assert_eq!(
            RuntimeConfig::from_lookup(|_| None)
                .unwrap()
                .query_memory_limit
                .as_deref(),
            Some(DEFAULT_QUERY_MEMORY_LIMIT)
        );
        for value in [
            "",
            "0MB",
            "-1GB",
            "unlimited",
            "1e999GB",
            "999999999999999999999999999TB",
            "1GB'; SET",
            "0.00000000000000001KB",
        ] {
            assert!(
                RuntimeConfig::from_lookup(
                    |key| (key == "BI_QUERY_MEMORY_LIMIT").then(|| value.to_owned())
                )
                .is_err(),
                "{value}"
            );
        }
        assert_eq!(memory_bytes("1.5 GiB"), Some(1_610_612_736));
    }
}
