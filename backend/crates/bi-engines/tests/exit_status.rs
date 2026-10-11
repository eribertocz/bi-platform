//! The exit status is the benchmark's CI gate: non-zero unless every engine
//! result and the publication (unless `--no-publish`) match the oracle. The
//! report is written either way, for diagnosis.

use std::path::Path;
use std::process::{Command, Output};

fn bench(out: &Path, extra: &[&str], catalog: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bi-engines"));
    command
        .args(["--rows", "2000", "--iterations", "1", "--out"])
        .arg(out)
        .args(extra);
    if let Some(catalog) = catalog {
        command.env("BI_LAKE_CATALOG", catalog);
    }
    command.output().expect("the benchmark starts")
}

fn report(out: &Path) -> serde_json::Value {
    let bytes = std::fs::read(out).expect("the report is written");
    serde_json::from_slice(&bytes).expect("the report is JSON")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn an_explicit_no_publish_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("report.json");
    let output = bench(&out, &["--no-publish"], None);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(report(&out)["publication"]["status"], "skipped");
}

#[cfg(feature = "postgres-tests")]
mod lake {
    use super::{bench, report, stderr};

    #[test]
    fn an_unreachable_catalog_fails_and_keeps_the_report_without_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("report.json");
        let output = bench(
            &out,
            &[],
            Some("dbname=bi_lake_catalog host=127.0.0.1 port=1 user=bi password=s3cret"),
        );
        assert!(
            !output.status.success(),
            "an unreachable catalog must fail the run"
        );
        assert!(
            stderr(&output).contains("the publication into DuckLake failed"),
            "{}",
            stderr(&output)
        );
        let report = report(&out);
        assert_eq!(report["publication"]["status"], "failed");
        assert!(
            report["results"]
                .as_array()
                .unwrap()
                .iter()
                .all(|result| result["exact"] == true),
            "only the publication failed"
        );
        let written = std::fs::read_to_string(&out).unwrap();
        assert!(!written.contains("s3cret") && !stderr(&output).contains("s3cret"));
    }

    /// Uses `BI_LAKE_CATALOG` or the development catalog.
    #[test]
    fn an_exact_publication_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("report.json");
        let output = bench(&out, &[], None);
        assert!(output.status.success(), "{}", stderr(&output));
        let publication = &report(&out)["publication"];
        assert_eq!(publication["status"], "published");
        assert_eq!(publication["exact"], true);
    }
}
