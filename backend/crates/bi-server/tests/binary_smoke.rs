//! Smoke test of the real binary, run by `cargo test` on every OS (G0.14):
//! it starts without PostgreSQL, is live, and reports itself not ready.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BINARY: &str = env!("CARGO_BIN_EXE_bi-server");

/// Kills the server even when an assertion fails.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        self.0.kill().ok();
        self.0.wait().ok();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Minimal HTTP/1.1 GET returning `(status, body)`, or `None` if not reachable.
fn get(port: u16, path: &str) -> Option<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    let status = response.split_whitespace().nth(1)?.parse().ok()?;
    let body = response.split_once("\r\n\r\n")?.1.to_owned();
    Some((status, body))
}

#[test]
fn starts_without_database_live_and_not_ready() {
    let port = free_port();
    let child = Command::new(BINARY)
        .env("BI_BIND_ADDR", format!("127.0.0.1:{port}"))
        .env("BI_DATABASE_URL", "postgres://bi:bi@127.0.0.1:1/bi")
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _server = Server(child);

    let deadline = Instant::now() + Duration::from_secs(30);
    let live = loop {
        if let Some(response) = get(port, "/health/live") {
            break response;
        }
        assert!(
            Instant::now() < deadline,
            "bi-server did not start listening"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(live, (200, r#"{"status":"ok","checks":[]}"#.to_owned()));

    let (status, body) = get(port, "/health/ready").expect("ready answers");
    assert_eq!(status, 503);
    assert!(body.contains(r#""status":"unavailable""#), "{body}");
}

#[test]
fn rejects_invalid_configuration() {
    let output = Command::new(BINARY)
        .env("BI_BIND_ADDR", "not-an-address")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("BI_BIND_ADDR"));
}

#[test]
fn invalid_optional_metrics_configuration_does_not_prevent_startup() {
    starts_with_optional_metrics("http://[invalid", false);
}

#[test]
fn https_optional_metrics_does_not_panic_at_startup() {
    starts_with_optional_metrics("https://127.0.0.1:1", true);
}

fn starts_with_optional_metrics(endpoint: &str, valid: bool) {
    let port = free_port();
    let log = tempfile::NamedTempFile::new().unwrap();
    let child = Command::new(BINARY)
        .env("BI_BIND_ADDR", format!("127.0.0.1:{port}"))
        .env("BI_DATABASE_URL", "postgres://bi:bi@127.0.0.1:1/bi")
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::from(log.as_file().try_clone().unwrap()))
        .stderr(Stdio::from(log.as_file().try_clone().unwrap()))
        .spawn()
        .unwrap();
    let _server = Server(child);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some((status, _)) = get(port, "/health/live") {
            assert_eq!(status, 200);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "optional exporter must not block startup"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let log = std::fs::read_to_string(log.path()).unwrap();
    assert!(!log.contains("panicked"), "{log}");
    assert_eq!(
        log.contains("metrics_exporter_setup_failed"),
        !valid,
        "{log}"
    );
}

/// `migrate` is finite: 2 for missing or invalid configuration, 1 when the
/// database or a migration fails, and only stable codes, never the URL or
/// its password, on either stream.
#[test]
fn migrate_subcommand_exit_codes_are_stable_and_redacted() {
    let decoy = "decoy-password-7f3a";
    for (url, code, message) in [
        (None, 2, "metadata_database_url_required"),
        (
            Some("not a url".to_owned()),
            2,
            "metadata_invalid_database_url",
        ),
        (
            Some(format!("postgres://bi:{decoy}@127.0.0.1:1/bi")),
            1,
            "metadata_migration_failed",
        ),
    ] {
        let mut command = Command::new(BINARY);
        command.arg("migrate").env_remove("BI_DATABASE_URL");
        if let Some(url) = &url {
            command.env("BI_DATABASE_URL", url);
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(code), "{url:?}");
        assert_eq!(output.stdout, b"", "no partial receipt: {url:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(stderr.trim(), message, "{url:?}");
        assert!(!stderr.contains(decoy) && !stderr.contains("postgres://"));
    }
    let output = Command::new(BINARY)
        .args(["migrate", "--force"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "unknown arguments are refused"
    );
}

#[test]
fn openapi_subcommand_prints_the_contract() {
    let output = Command::new(BINARY).arg("openapi").output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        bi_server::openapi::json()
    );
}
