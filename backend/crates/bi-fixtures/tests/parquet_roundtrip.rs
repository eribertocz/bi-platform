//! The Parquet file holds exactly the fixture, with its logical types intact.

use arrow_schema::{DataType, TimeUnit};
use bi_fixtures::oracle::Oracle;
use bi_fixtures::parquet_io;
use bi_fixtures::sales::{self, FixtureSpec};

#[test]
fn parquet_round_trip_reproduces_the_oracle_and_types() {
    let spec = FixtureSpec {
        seed: 42,
        rows: 150_000,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sales.parquet");

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
