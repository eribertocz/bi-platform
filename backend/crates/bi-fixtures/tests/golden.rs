//! Same seed, same oracle bytes, on every OS and toolchain (G0.10).

use bi_fixtures::oracle::Oracle;
use bi_fixtures::sales::{self, FixtureSpec};

#[test]
fn oracle_bytes_match_committed_golden_file() {
    let spec = FixtureSpec {
        seed: 42,
        rows: 10_000,
    };
    let actual = Oracle::from_rows(spec, sales::rows(spec)).to_json();
    let expected = include_str!("../golden/sales-seed42-rows10000.oracle.json");
    assert!(
        actual == expected,
        "the sales fixture or its oracle changed. If intended, bump oracle::SCHEMA_VERSION and \
         regenerate from backend/: cargo run -p bi-fixtures -- oracle --seed 42 --rows 10000 \
         --out crates/bi-fixtures/golden/sales-seed42-rows10000.oracle.json\nActual:\n{actual}"
    );
}
