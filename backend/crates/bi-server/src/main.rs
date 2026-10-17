use std::process::ExitCode;

use bi_server::analytics::Analytics;
use bi_server::config::Config;
use bi_server::db::{self, Database};
use bi_server::{
    AppState, metrics, openapi, router_with_static, serve, shutdown_signal, telemetry,
};
use sqlx::postgres::PgConnectOptions;
use tokio::net::TcpListener;

const USAGE: &str =
    "usage: bi-server            run the API server (configured by BI_* environment variables)
       bi-server migrate         apply metadata migrations with the administrative BI_DATABASE_URL, then exit
       bi-server openapi [PATH]  write the OpenAPI document to PATH or stdout
       bi-server audit-verify [SEQUENCE HASH]  verify audit history (BI_DATABASE_URL; read-only)
       bi-server audit-verify --report [SEQUENCE HASH]  bounded JSON report instead of strict pass/fail";

fn main() -> ExitCode {
    install_tls_provider();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => run(),
        ["migrate"] => migrate_metadata(),
        ["audit-verify"] => verify_audit(None),
        ["audit-verify", "--report"] => report_audit(None),
        ["audit-verify", sequence, hash] => {
            if let Some(checkpoint) = parse_checkpoint(sequence, hash) {
                verify_audit(Some(checkpoint))
            } else {
                eprintln!("audit_invalid_checkpoint");
                ExitCode::from(2)
            }
        }
        ["audit-verify", "--report", sequence, hash] => {
            if let Some(checkpoint) = parse_checkpoint(sequence, hash) {
                report_audit(Some(checkpoint))
            } else {
                eprintln!("audit_invalid_checkpoint");
                ExitCode::from(2)
            }
        }
        ["openapi"] => {
            print!("{}", openapi::json());
            ExitCode::SUCCESS
        }
        ["openapi", path] => match std::fs::write(path, openapi::json()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("cannot write {path}: {error}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn parse_checkpoint(sequence: &str, hash: &str) -> Option<bi_audit::Checkpoint> {
    let sequence = sequence.parse::<u64>().ok()?;
    if sequence > i64::MAX.cast_unsigned()
        || hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hash[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(bi_audit::Checkpoint {
        sequence,
        hash: bytes,
    })
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(text, "{byte:02x}").expect("String writing cannot fail");
    }
    text
}

/// Opens the read-only audit pool from `BI_DATABASE_URL`, or exits with the
/// configuration error every audit CLI mode shares.
fn audit_pool() -> Result<sqlx::PgPool, ExitCode> {
    let url = std::env::var("BI_DATABASE_URL").map_err(|_| {
        eprintln!("audit_database_url_required");
        ExitCode::from(2)
    })?;
    db::lazy_pool(&url).map_err(|_| {
        eprintln!("audit_invalid_database_url");
        ExitCode::from(2)
    })
}

/// `migrate`: the administrative step the restricted server never performs.
/// Finite (at most [`db::MIGRATION_TIMEOUT`], lock waits included) and
/// side-effect free beyond the migrations: no HTTP, analytical runtime, OIDC
/// or exporters, and the URL is never printed or kept.
#[tokio::main]
async fn migrate_metadata() -> ExitCode {
    let Ok(url) = std::env::var("BI_DATABASE_URL") else {
        eprintln!("metadata_database_url_required");
        return ExitCode::from(2);
    };
    let Ok(options) = url.parse::<PgConnectOptions>() else {
        eprintln!("metadata_invalid_database_url");
        return ExitCode::from(2);
    };
    if let Err(error) = db::migrate(&options, db::MIGRATION_TIMEOUT).await {
        eprintln!("{}", migration_failure_code(&error));
        return ExitCode::FAILURE;
    }
    println!(
        "{}",
        serde_json::json!({
            "schema_version": 1,
            "migrations": db::MIGRATOR.iter().count().to_string(),
        })
    );
    ExitCode::SUCCESS
}

/// The stable code `migrate` prints for each failure; never the database error.
fn migration_failure_code(error: &db::MigrationError) -> &'static str {
    match error {
        db::MigrationError::TimedOut => "metadata_migration_timed_out",
        db::MigrationError::Failed(_) => "metadata_migration_failed",
    }
}

#[tokio::main]
async fn verify_audit(trusted: Option<bi_audit::Checkpoint>) -> ExitCode {
    let pool = match audit_pool() {
        Ok(pool) => pool,
        Err(code) => return code,
    };
    // No server, migration, engine or telemetry side effects in this command.
    let verified = bi_server::audit::verify(&pool, trusted).await;
    pool.close().await;
    match verified {
        Ok(checkpoint) => {
            println!(
                "{}",
                serde_json::json!({
                    "schema_version": 1,
                    "sequence": checkpoint.sequence.to_string(),
                    "hash": hex(&checkpoint.hash),
                })
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// `--report`: unlike `verify_audit`, a cryptographic or content defect is
/// printed, not a bare failure. Only a storage/connection error prints
/// nothing but a safe code, so no partial report is ever shown as complete.
#[tokio::main]
async fn report_audit(trusted: Option<bi_audit::Checkpoint>) -> ExitCode {
    let pool = match audit_pool() {
        Ok(pool) => pool,
        Err(code) => return code,
    };
    let outcome = bi_server::audit::report(&pool, trusted).await;
    pool.close().await;
    match outcome {
        Ok(report) => {
            let fully_valid = report.chain_valid && report.content_valid == Some(true);
            println!(
                "{}",
                serde_json::json!({
                    "schema_version": 1,
                    "chain_valid": report.chain_valid,
                    "content_valid": report.content_valid,
                    "invalid_content_count": report.invalid_content_count.to_string(),
                    "first_invalid_content_sequence":
                        report.first_invalid_content_sequence.map(|s| s.to_string()),
                    "integrity_error": report.integrity_error.map(|e| e.to_string()),
                    "first_integrity_error_sequence":
                        report.first_integrity_error_sequence.map(|s| s.to_string()),
                    "checkpoint": report.checkpoint.map(|c| serde_json::json!({
                        "sequence": c.sequence.to_string(),
                        "hash": hex(&c.hash),
                    })),
                })
            );
            if fully_valid {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn install_tls_provider() {
    // reqwest's rustls-no-provider requires this before constructing any client.
    // The binary owns process-wide TLS policy; library modules do not install it.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("TLS provider must be installed once at application startup");
}

#[tokio::main]
async fn run() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("invalid configuration: {error}");
            return ExitCode::FAILURE;
        }
    };
    telemetry::init(config.log_format);

    let options: PgConnectOptions = match config.database_url.parse() {
        Ok(options) => options,
        Err(error) => {
            tracing::error!(%error, "BI_DATABASE_URL is not a valid PostgreSQL URL");
            return ExitCode::FAILURE;
        }
    };
    // Log where we connect, never the credentials.
    tracing::info!(
        host = options.get_host(),
        port = options.get_port(),
        database = options.get_database().unwrap_or_default(),
        development_default = config.database_url_is_default,
        mode = ?config.metadata_mode,
        "metadata database"
    );

    let listener = match TcpListener::bind(config.bind_addr).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(%error, addr = %config.bind_addr, "cannot bind");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(addr = %config.bind_addr, "listening");

    let analytics = match config.analytics.clone() {
        Some(runtime_config) => Analytics::start(runtime_config, config.dev_fixture),
        None => Analytics::disabled(),
    };
    let (metrics, metrics_provider) = metrics::from_env(analytics.clone()).await;
    let metadata_metrics =
        metrics_provider
            .as_ref()
            .map_or_else(db::MetadataMetrics::default, |provider| {
                use opentelemetry::metrics::MeterProvider as _;
                db::MetadataMetrics::new(&provider.meter("bi-server"))
            });
    let db = Database::start_observed(&options, config.metadata_mode, metadata_metrics);
    let mut models = bi_server::models::Models::new(config.project_dir);
    if let Some(provider) = &metrics_provider {
        use opentelemetry::metrics::MeterProvider as _;
        models = models.with_meter(&provider.meter("bi-server"));
    }
    let state = AppState {
        db: db.clone(),
        auth: config.auth,
        analytics,
        query_timeout: config.query_timeout,
        metrics,
        models,
    };
    let app = router_with_static(state, config.static_dir);
    let result = serve(listener, app, shutdown_signal()).await;
    db.close().await;
    if let Some(provider) = metrics_provider {
        tokio::task::spawn_blocking(move || {
            if provider.shutdown().is_err() {
                tracing::warn!(
                    event_code = "metrics_shutdown_failed",
                    "optional metrics could not flush at shutdown"
                );
            }
        })
        .await
        .ok();
    }

    match result {
        Ok(()) => {
            tracing::info!("stopped");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(%error, "server error");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn checkpoint_arguments_are_bounded_and_redacted() {
        assert!(super::parse_checkpoint("1", &"a".repeat(64)).is_some());
        for (sequence, hash) in [
            ("-1", "a".repeat(64)),
            ("18446744073709551615", "a".repeat(64)),
            ("1", "A".repeat(64)),
            ("1", "private@example.test".into()),
        ] {
            assert!(super::parse_checkpoint(sequence, &hash).is_none());
        }
    }

    #[test]
    fn migration_failures_print_only_their_stable_codes() {
        use bi_server::db::MigrationError;
        assert_eq!(
            super::migration_failure_code(&MigrationError::TimedOut),
            "metadata_migration_timed_out"
        );
        let failed = MigrationError::Failed(sqlx::migrate::MigrateError::VersionMissing(7));
        assert_eq!(
            super::migration_failure_code(&failed),
            "metadata_migration_failed"
        );
    }

    #[test]
    fn startup_installs_ring_before_constructing_http_clients() {
        super::install_tls_provider();
        let installed = rustls::crypto::CryptoProvider::get_default().unwrap();
        let ring = rustls::crypto::ring::default_provider();
        assert!(std::ptr::eq(installed.secure_random, ring.secure_random));
        reqwest::blocking::Client::builder().build().unwrap();
    }
}
