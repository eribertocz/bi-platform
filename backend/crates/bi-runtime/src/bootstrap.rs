//! Administrative phase: extensions, lake attachment and data loading, on a
//! connection that still has file-system access. Ends with a lockdown.

use std::path::Path;

use duckdb::Connection;

use crate::config::{is_identifier, redact};
use crate::{LAKE, Runtime, RuntimeConfig, RuntimeError, sql_string};

/// Statements applied before any query connection exists. After them, no
/// connection can read or write files outside the lake's `DATA_PATH`
/// (`allowed_directories`, set just before), install or load extensions,
/// attach databases with storage, or change settings.
const LOCKDOWN: &str = "
    SET enable_external_access = false;
    SET autoinstall_known_extensions = false;
    SET autoload_known_extensions = false;
    SET lock_configuration = true;";

/// Records which fixture each lake table holds, so loading is idempotent.
const FIXTURES_TABLE: &str = "lake.main.bi_fixtures";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadOutcome {
    Loaded,
    AlreadyLoaded,
}

/// Absolute, `/`-separated, with a trailing slash. DuckDB only lets a
/// locked-down connection reach the lake's files when their paths start with
/// the `DATA_PATH` exactly as attached; a relative or `\`-separated value
/// makes DuckLake lose access to its own data after the lockdown.
fn data_path_for_duckdb(path: &Path) -> std::io::Result<String> {
    let absolute = std::path::absolute(path)?;
    let mut normalized = absolute.to_string_lossy().replace('\\', "/");
    if !normalized.ends_with('/') {
        normalized.push('/');
    }
    Ok(normalized)
}

pub struct Bootstrap {
    conn: Connection,
    config: RuntimeConfig,
    /// The lake's normalized `DATA_PATH`, the only directory left readable.
    data_path: Option<String>,
}

impl Bootstrap {
    /// Opens DuckDB in memory and, when configured, attaches the lake.
    ///
    /// # Errors
    /// Engine errors, or [`RuntimeError::Lake`] when the catalog or data path
    /// cannot be reached.
    pub fn open(config: RuntimeConfig) -> Result<Self, RuntimeError> {
        let conn = Connection::open_in_memory()?;
        if let Some(directory) = &config.extension_directory {
            std::fs::create_dir_all(directory)?;
            conn.execute_batch(&format!(
                "SET extension_directory = {}",
                sql_string(&directory.to_string_lossy())
            ))?;
        }
        if let Some(threads) = config.duckdb_threads {
            conn.execute_batch(&format!("SET threads = {threads}"))?;
        }
        if let Some(limit) = &config.memory_limit {
            conn.execute_batch(&format!("SET memory_limit = {}", sql_string(limit)))?;
        }
        let mut attached_data_path = None;
        if let Some(lake) = &config.lake {
            std::fs::create_dir_all(&lake.data_path)?;
            conn.execute_batch(
                "INSTALL ducklake; INSTALL postgres; LOAD ducklake; LOAD postgres;",
            )?;
            let normalized = data_path_for_duckdb(&lake.data_path)?;
            let data_path = sql_string(&normalized);
            let options = match &lake.metadata_schema {
                Some(schema) => {
                    debug_assert!(is_identifier(schema), "validated by RuntimeConfig");
                    format!(
                        "DATA_PATH {data_path}, METADATA_SCHEMA {}",
                        sql_string(schema)
                    )
                }
                None => format!("DATA_PATH {data_path}"),
            };
            let target = sql_string(&format!("ducklake:postgres:{}", lake.catalog));
            conn.execute_batch(&format!("ATTACH {target} AS {LAKE} ({options})"))
                .map_err(|error| {
                    // Engine messages may echo the connection string.
                    let message = error
                        .to_string()
                        .replace(&lake.catalog, &redact(&lake.catalog));
                    RuntimeError::Lake(message)
                })?;
            tracing::info!(
                catalog = %redact(&lake.catalog),
                data_path = %lake.data_path.display(),
                "lake attached"
            );
            attached_data_path = Some(normalized);
        }
        Ok(Self {
            conn,
            config,
            data_path: attached_data_path,
        })
    }

    /// Loads `parquet` into `lake.main.<table>` unless the lake already holds
    /// the fixture identified by `fingerprint`. Replacement is atomic.
    ///
    /// # Errors
    /// When no lake is configured, the table name is invalid, or the engine fails.
    pub fn load_parquet_fixture(
        &self,
        table: &str,
        parquet: &Path,
        fingerprint: &str,
    ) -> Result<LoadOutcome, RuntimeError> {
        if self.config.lake.is_none() {
            return Err(RuntimeError::Lake("no lake configured".to_owned()));
        }
        if !is_identifier(table) {
            return Err(RuntimeError::Lake(format!("invalid table name {table:?}")));
        }
        self.conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS {FIXTURES_TABLE} (name VARCHAR, fingerprint VARCHAR)"
        ))?;
        let current: Option<String> = self
            .conn
            .prepare(&format!(
                "SELECT fingerprint FROM {FIXTURES_TABLE} WHERE name = ?"
            ))?
            .query_row([table], |row| row.get(0))
            .map(Some)
            .or_else(|error| match error {
                duckdb::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        if current.as_deref() == Some(fingerprint) {
            return Ok(LoadOutcome::AlreadyLoaded);
        }

        self.conn.execute_batch("BEGIN TRANSACTION")?;
        let result = (|| -> Result<(), duckdb::Error> {
            self.conn.execute_batch(&format!(
                "CREATE OR REPLACE TABLE {LAKE}.main.{table} AS SELECT * FROM read_parquet({})",
                sql_string(&parquet.to_string_lossy())
            ))?;
            self.conn.execute(
                &format!("DELETE FROM {FIXTURES_TABLE} WHERE name = ?"),
                [table],
            )?;
            self.conn.execute(
                &format!("INSERT INTO {FIXTURES_TABLE} VALUES (?, ?)"),
                [table, fingerprint],
            )?;
            self.conn.execute_batch("COMMIT")
        })();
        if let Err(error) = result {
            self.conn.execute_batch("ROLLBACK").ok();
            return Err(error.into());
        }
        tracing::info!(table, fingerprint, "fixture loaded into the lake");
        Ok(LoadOutcome::Loaded)
    }

    /// Locks the configuration and starts the query pool.
    ///
    /// # Errors
    /// When the lockdown or a worker connection fails.
    pub fn finish(self) -> Result<Runtime, RuntimeError> {
        // DuckLake reads its Parquet files through the same file-system checks
        // as user SQL, so its directory must stay allowed after the lockdown.
        let allowed = self
            .data_path
            .as_deref()
            .map_or_else(String::new, sql_string);
        self.conn
            .execute_batch(&format!("SET allowed_directories = [{allowed}]"))?;
        self.conn.execute_batch(LOCKDOWN)?;
        Runtime::start(self.conn, &self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_path_is_absolute_slash_separated_and_terminated() {
        let normalized = data_path_for_duckdb(Path::new("data/lake")).unwrap();
        assert!(Path::new(&normalized).is_absolute(), "{normalized}");
        assert!(!normalized.contains('\\'), "{normalized}");
        assert!(normalized.ends_with("data/lake/"), "{normalized}");
    }
}
