//! Structured logging with `tracing`. Filter with `RUST_LOG` (default `info`).

use std::io::IsTerminal;

use tracing_subscriber::EnvFilter;

use crate::config::LogFormat;

pub fn init(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        // Colors only for humans; files and log collectors get plain text.
        .with_ansi(std::io::stdout().is_terminal());
    match format {
        LogFormat::Text => builder.init(),
        LogFormat::Json => builder.json().with_current_span(true).init(),
    }
}
