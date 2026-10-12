//! Runtime configuration, read from environment variables.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use bi_fixtures::sales::FixtureSpec;
use bi_runtime::RuntimeConfig;

use crate::auth::AuthConfig;

/// Database used when `BI_DATABASE_URL` is unset: the local development
/// PostgreSQL from `compose.yaml`. Never a production credential.
pub const DEV_DATABASE_URL: &str = "postgres://bi:bi@127.0.0.1:54320/bi";
pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8080";
pub const DEFAULT_QUERY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Text,
    Json,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub database_url: String,
    /// True when `BI_DATABASE_URL` was not set and the development default is used.
    pub database_url_is_default: bool,
    pub log_format: LogFormat,
    /// Optional frontend build directory; unset means API-only development.
    pub static_dir: Option<PathBuf>,
    pub auth: AuthConfig,
    /// Analytical runtime; `None` when `BI_ANALYTICS=off`.
    pub analytics: Option<RuntimeConfig>,
    /// Synthetic `sales` fixture loaded at startup (`BI_DEV_FIXTURE_ROWS`),
    /// development only.
    pub dev_fixture: Option<FixtureSpec>,
    /// Deadline after which a query is cancelled (`BI_QUERY_TIMEOUT_MS`).
    pub query_timeout: Duration,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    InvalidBindAddr(String),
    InvalidLogFormat(String),
    InvalidAuthConfig,
    Invalid {
        variable: &'static str,
        value: String,
    },
    Runtime(bi_runtime::config::ConfigError),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAuthConfig => write!(
                f,
                "invalid authentication configuration: check BI_PUBLIC_ORIGIN, BI_SESSION_TTL_SECONDS, BI_DEV_USERS and BI_OIDC_*"
            ),
            Self::InvalidBindAddr(value) => {
                write!(
                    f,
                    "BI_BIND_ADDR must be a socket address like 127.0.0.1:8080, got {value:?}"
                )
            }
            Self::InvalidLogFormat(value) => {
                write!(
                    f,
                    "BI_LOG_FORMAT must be \"text\" or \"json\", got {value:?}"
                )
            }
            Self::Invalid { variable, value } => write!(f, "invalid {variable}: {value:?}"),
            Self::Runtime(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Reads the configuration from the process environment.
    ///
    /// # Errors
    /// Returns an error when a variable is set to an invalid value.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Reads the configuration through `lookup`, so it can be tested without
    /// touching the process environment.
    ///
    /// # Errors
    /// Returns an error when a variable is set to an invalid value.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let bind_addr = lookup("BI_BIND_ADDR").unwrap_or_else(|| DEFAULT_BIND_ADDR.to_owned());
        let bind_addr = bind_addr
            .parse()
            .map_err(|_| ConfigError::InvalidBindAddr(bind_addr))?;

        let (database_url, database_url_is_default) = match lookup("BI_DATABASE_URL") {
            Some(url) => (url, false),
            None => (DEV_DATABASE_URL.to_owned(), true),
        };

        let log_format = match lookup("BI_LOG_FORMAT").as_deref() {
            None | Some("text") => LogFormat::Text,
            Some("json") => LogFormat::Json,
            Some(other) => return Err(ConfigError::InvalidLogFormat(other.to_owned())),
        };

        let positive = |variable: &'static str| -> Result<Option<u64>, ConfigError> {
            lookup(variable)
                .map(|value| {
                    value
                        .parse::<u64>()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or(ConfigError::Invalid { variable, value })
                })
                .transpose()
        };
        let analytics = match lookup("BI_ANALYTICS").as_deref() {
            None | Some("on") => {
                Some(RuntimeConfig::from_lookup(&lookup).map_err(ConfigError::Runtime)?)
            }
            Some("off") => None,
            Some(other) => {
                return Err(ConfigError::Invalid {
                    variable: "BI_ANALYTICS",
                    value: other.to_owned(),
                });
            }
        };

        Ok(Self {
            bind_addr,
            database_url,
            database_url_is_default,
            log_format,
            static_dir: lookup("BI_STATIC_DIR").map(PathBuf::from),
            auth: AuthConfig::from_lookup(&lookup)?,
            analytics,
            dev_fixture: positive("BI_DEV_FIXTURE_ROWS")?
                .map(|rows| FixtureSpec { seed: 42, rows }),
            query_timeout: positive("BI_QUERY_TIMEOUT_MS")?
                .map_or(DEFAULT_QUERY_TIMEOUT, Duration::from_millis),
        })
    }
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
    fn defaults_to_local_development_setup() {
        let config = Config::from_lookup(lookup(&[])).unwrap();
        assert_eq!(config.bind_addr, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(config.database_url, DEV_DATABASE_URL);
        assert!(config.database_url_is_default);
        assert_eq!(config.log_format, LogFormat::Text);
        assert_eq!(config.static_dir, None);
        assert!(config.analytics.is_some());
        assert_eq!(config.dev_fixture, None);
        assert_eq!(config.query_timeout, DEFAULT_QUERY_TIMEOUT);
    }

    #[test]
    fn reads_explicit_values() {
        let config = Config::from_lookup(lookup(&[
            ("BI_BIND_ADDR", "0.0.0.0:9000"),
            ("BI_DATABASE_URL", "postgres://u:p@db:5432/x"),
            ("BI_LOG_FORMAT", "json"),
            ("BI_STATIC_DIR", "../frontend/build"),
        ]))
        .unwrap();
        assert_eq!(config.bind_addr, "0.0.0.0:9000".parse().unwrap());
        assert_eq!(config.database_url, "postgres://u:p@db:5432/x");
        assert!(!config.database_url_is_default);
        assert_eq!(config.log_format, LogFormat::Json);
        assert_eq!(config.static_dir, Some(PathBuf::from("../frontend/build")));
    }

    #[test]
    fn rejects_invalid_values() {
        assert_eq!(
            Config::from_lookup(lookup(&[("BI_BIND_ADDR", "localhost")])).unwrap_err(),
            ConfigError::InvalidBindAddr("localhost".to_owned())
        );
        assert_eq!(
            Config::from_lookup(lookup(&[("BI_LOG_FORMAT", "xml")])).unwrap_err(),
            ConfigError::InvalidLogFormat("xml".to_owned())
        );
        for (variable, value) in [
            ("BI_ANALYTICS", "maybe"),
            ("BI_DEV_FIXTURE_ROWS", "-5"),
            ("BI_QUERY_TIMEOUT_MS", "0"),
        ] {
            assert_eq!(
                Config::from_lookup(lookup(&[(variable, value)])).unwrap_err(),
                ConfigError::Invalid {
                    variable,
                    value: value.to_owned()
                }
            );
        }
        assert!(matches!(
            Config::from_lookup(lookup(&[("BI_DUCKDB_WORKERS", "0")])),
            Err(ConfigError::Runtime(_))
        ));
    }

    #[test]
    fn reads_analytics_settings() {
        let config = Config::from_lookup(lookup(&[
            ("BI_DEV_FIXTURE_ROWS", "1000"),
            ("BI_QUERY_TIMEOUT_MS", "2500"),
        ]))
        .unwrap();
        assert_eq!(
            config.dev_fixture,
            Some(FixtureSpec {
                seed: 42,
                rows: 1000
            })
        );
        assert_eq!(config.query_timeout, Duration::from_millis(2500));
        let disabled = Config::from_lookup(lookup(&[("BI_ANALYTICS", "off")])).unwrap();
        assert!(disabled.analytics.is_none());
    }
}
