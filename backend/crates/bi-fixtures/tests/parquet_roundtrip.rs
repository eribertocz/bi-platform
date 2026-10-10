//! The Parquet file holds exactly the fixture, with its logical types intact.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use arrow_schema::{DataType, TimeUnit};
use bi_fixtures::oracle::Oracle;
use bi_fixtures::parquet_io;
use bi_fixtures::sales::{self, FixtureSpec};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("bi-fixtures-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

#[test]
fn parquet_round_trip_reproduces_the_oracle_and_types() {
    let spec = FixtureSpec {
        seed: 42,
        rows: 150_000,
    };
    let dir = TempDir::new();
    let path = dir.0.join("sales.parquet");

    let size = parquet_io::write(
        &path,
        sales::schema(),
        sales::record_batches(spec, 64 * 1024),
    )
    .unwrap();
    assert!(size > 0);

    let batches = parquet_io::read(&path).unwrap();
    let schema = batches[0].schema();
    assert_eq!(schema.field(0).data_type(), &DataType::Int64);
    assert_eq!(
        schema.field(1).data_type(),
        &DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
    );
    assert_eq!(schema.field(4).data_type(), &DataType::Decimal128(18, 2));
    assert!(!schema.field(0).is_nullable());
    assert!(schema.field(4).is_nullable());

    assert_eq!(
        Oracle::from_batches(spec, &batches),
        Oracle::from_rows(spec, sales::rows(spec))
    );
}
