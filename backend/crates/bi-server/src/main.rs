use std::process::ExitCode;

use bi_server::config::Config;
use bi_server::db::{self, Database};
use bi_server::{AppState, openapi, router_with_static, serve, shutdown_signal, telemetry};
use sqlx::postgres::PgConnectOptions;
use tokio::net::TcpListener;

const USAGE: &str =
    "usage: bi-server            run the API server (configured by BI_* environment variables)
       bi-server openapi [PATH]  write the OpenAPI document to PATH or stdout";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => run(),
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
        "metadata database"
    );
    let db = Database::start(db::lazy_pool_with(options));

    let listener = match TcpListener::bind(config.bind_addr).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(%error, addr = %config.bind_addr, "cannot bind");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(addr = %config.bind_addr, "listening");

    let app = router_with_static(AppState { db: db.clone() }, config.static_dir);
    let result = serve(listener, app, shutdown_signal()).await;
    db.pool().close().await;

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
