//! Runtime configuration, read from environment variables.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use bi_fixtures::sales::FixtureSpec;
use bi_runtime::RuntimeConfig;

use crate::auth::AuthConfig;
use crate::metadata::MetadataMode;

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
    /// `BI_METADATA_MODE`; restricted unless development is explicitly
    /// requested in the one setup that allows it.
    pub metadata_mode: MetadataMode,
    pub log_format: LogFormat,
    /// Optional frontend build directory; unset means API-only development.
    pub static_dir: Option<PathBuf>,
    pub project_dir: Option<PathBuf>,
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
    MetadataDevelopmentNotAllowed,
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
            Self::MetadataDevelopmentNotAllowed => f.write_str(
                "BI_METADATA_MODE=development requires a dev-auth build with BI_DEV_USERS, a loopback BI_PUBLIC_ORIGIN and OIDC disabled; otherwise use the default restricted mode",
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
        let project_dir = lookup("BI_PROJECT_DIR")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
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

        let auth = AuthConfig::from_lookup(&lookup)?;
        let metadata_mode = match lookup("BI_METADATA_MODE") {
            None => MetadataMode::Restricted,
            Some(value) => MetadataMode::parse(&value).ok_or(ConfigError::Invalid {
                variable: "BI_METADATA_MODE",
                value,
            })?,
        };
        if metadata_mode == MetadataMode::Development && !auth.allows_metadata_development() {
            return Err(ConfigError::MetadataDevelopmentNotAllowed);
        }

        Ok(Self {
            project_dir,
            bind_addr,
            database_url,
            database_url_is_default,
            metadata_mode,
            log_format,
            static_dir: lookup("BI_STATIC_DIR").map(PathBuf::from),
            auth,
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
        assert_eq!(config.project_dir, None);
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
            ("BI_PROJECT_DIR", "../examples/models-project"),
        ]))
        .unwrap();
        assert_eq!(config.bind_addr, "0.0.0.0:9000".parse().unwrap());
        assert_eq!(config.database_url, "postgres://u:p@db:5432/x");
        assert!(!config.database_url_is_default);
        assert_eq!(config.log_format, LogFormat::Json);
        assert_eq!(config.static_dir, Some(PathBuf::from("../frontend/build")));
        assert_eq!(
            config.project_dir,
            Some(PathBuf::from("../examples/models-project"))
        );
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
    fn metadata_mode_is_restricted_unless_requested_and_rejects_unknown_values() {
        let mode =
            |vars: &[(&str, &str)]| Config::from_lookup(lookup(vars)).map(|c| c.metadata_mode);
        assert_eq!(mode(&[]), Ok(MetadataMode::Restricted));
        assert_eq!(
            mode(&[("BI_METADATA_MODE", "restricted")]),
            Ok(MetadataMode::Restricted)
        );
        for value in ["", "dev", "Development", "production", "local"] {
            assert_eq!(
                mode(&[("BI_METADATA_MODE", value)]),
                Err(ConfigError::Invalid {
                    variable: "BI_METADATA_MODE",
                    value: value.to_owned()
                })
            );
        }
    }

    /// Development is accepted only in its one setup, and never inferred:
    /// the default database URL, a loopback origin, dev users or the absence
    /// of OIDC never select it on their own.
    #[test]
    fn development_metadata_mode_is_refused_outside_its_one_setup() {
        const USERS: &str = r#"[{"id":"analyst","display_name":"Analista"}]"#;
        let mode =
            |vars: &[(&str, &str)]| Config::from_lookup(lookup(vars)).map(|c| c.metadata_mode);
        let development = [("BI_METADATA_MODE", "development"), ("BI_DEV_USERS", USERS)];
        assert_eq!(
            mode(&[("BI_DEV_USERS", USERS)]),
            Ok(MetadataMode::Restricted),
            "dev users alone never select development"
        );
        for refused in [
            vec![("BI_METADATA_MODE", "development")],
            [
                development.as_slice(),
                &[("BI_PUBLIC_ORIGIN", "https://bi.example.test")],
            ]
            .concat(),
            [
                development.as_slice(),
                &[
                    ("BI_OIDC_ISSUER", "http://127.0.0.1:18071/realms/bi-test"),
                    ("BI_OIDC_CLIENT_ID", "bi-platform"),
                ],
            ]
            .concat(),
        ] {
            assert_eq!(
                mode(&refused),
                Err(ConfigError::MetadataDevelopmentNotAllowed),
                "{refused:?}"
            );
        }
        let error = ConfigError::MetadataDevelopmentNotAllowed.to_string();
        assert!(error.contains("BI_METADATA_MODE") && !error.contains("analyst"));
        // Only a dev-auth build can accept it at all.
        let accepted = mode(&development);
        if cfg!(feature = "dev-auth") {
            assert_eq!(accepted, Ok(MetadataMode::Development));
            assert_eq!(
                mode(
                    &[
                        development.as_slice(),
                        &[("BI_PUBLIC_ORIGIN", "http://localhost:8080")]
                    ]
                    .concat()
                ),
                Ok(MetadataMode::Development)
            );
        } else {
            assert_eq!(accepted, Err(ConfigError::MetadataDevelopmentNotAllowed));
        }
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
