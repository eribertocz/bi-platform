//! Parquet files for fixtures (Snappy, Arrow schema preserved).

use std::fs::File;
use std::path::Path;

use arrow_array::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

use crate::Error;

/// Writes `batches` (which must share `schema`) and returns the file size in bytes.
///
/// # Errors
/// I/O or Parquet encoding errors.
pub fn write(
    path: &Path,
    schema: arrow_schema::SchemaRef,
    batches: impl IntoIterator<Item = RecordBatch>,
) -> Result<u64, Error> {
    let file = File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
    for batch in batches {
        writer.write(&batch)?;
    }
    writer.close()?;
    Ok(std::fs::metadata(path)?.len())
}

/// Reads every record batch of a Parquet file.
///
/// # Errors
/// I/O or Parquet decoding errors.
pub fn read(path: &Path) -> Result<Vec<RecordBatch>, Error> {
    let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(path)?)?.build()?;
    Ok(reader.collect::<Result<Vec<_>, _>>()?)
}
