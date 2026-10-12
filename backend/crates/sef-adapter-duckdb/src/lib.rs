//! Synchronous DuckDB implementation of Ossie's catalog port (ADR 0008).
//! Receives a connection and relation names, never SQL from a document.

use std::fmt;

use duckdb::Connection;
use sef_model_ir::identity::SourceHandle;
use sef_model_ir::time::TimestampSemantics;
use sef_model_ir::types::{DecimalType, ValueType};
use sef_ossie::catalog::{Column, Relation};

pub mod execution;

#[derive(Debug)]
pub enum CatalogError {
    UnknownSource,
    AmbiguousSource,
    InvalidHandle,
    UnsupportedType { column: String, sql_type: String },
    Engine(duckdb::Error),
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSource => f.write_str("El origen no existe. Usa el nombre de una tabla o vista disponible."),
            Self::AmbiguousSource => f.write_str("El origen coincide con varias tablas o vistas que solo difieren en mayúsculas. Usa nombres distintos en el catálogo."),
            Self::InvalidHandle => f.write_str("No se pudo identificar la estructura del origen de forma segura."),
            Self::UnsupportedType { column, sql_type } => write!(f, "La columna {column:?} usa {sql_type}, un tipo que todavía no podemos representar exactamente. Cambia su tipo en el origen; no se redondeará ni se aproximará."),
            Self::Engine(_) => f.write_str("No se pudo leer la estructura del origen. Reintenta cuando el motor esté disponible."),
        }
    }
}

impl std::error::Error for CatalogError {}

impl From<duckdb::Error> for CatalogError {
    fn from(error: duckdb::Error) -> Self {
        Self::Engine(error)
    }
}

/// Exact subset supported by this adapter; all other engine types are rejected.
///
/// # Errors
/// Returns the unsupported SQL type, without a lossy fallback.
pub fn value_type(sql_type: &str) -> Result<ValueType, String> {
    let result = match sql_type {
        "BIGINT" => ValueType::Integer {
            signed: true,
            bits: Some(64),
        },
        "INTEGER" => ValueType::Integer {
            signed: true,
            bits: Some(32),
        },
        "DOUBLE" => ValueType::Float { bits: 64 },
        "VARCHAR" => ValueType::Utf8,
        "BOOLEAN" => ValueType::Boolean,
        "DATE" => ValueType::Date,
        "TIMESTAMP WITH TIME ZONE" => ValueType::Timestamp {
            timezone_semantics: TimestampSemantics::Instant,
        },
        other => {
            let decimal = other
                .strip_prefix("DECIMAL(")
                .and_then(|s| s.strip_suffix(')'))
                .and_then(|s| s.split_once(','))
                .and_then(|(p, s)| Some((p.parse::<u8>().ok()?, s.parse::<u8>().ok()?)))
                .filter(|(p, s)| (1..=38).contains(p) && s <= p)
                .and_then(|(p, s)| DecimalType::new(p, s).ok());
            return decimal
                .map(ValueType::Decimal)
                .ok_or_else(|| other.to_owned());
        }
    };
    Ok(result)
}

/// Describes a table/view on the caller's analytical connection. Qualification
/// follows DuckDB's current database/schema; names must be simple SQL identifiers.
/// Every identifier is quoted and existence is checked using bound parameters.
/// Call on a dedicated worker, never an async executor.
///
/// # Errors
/// Unknown source, unsupported column type, or native engine failure.
pub fn describe(conn: &Connection, source: &str) -> Result<Relation, CatalogError> {
    let parts: Vec<_> = source.split('.').collect();
    if !(1..=3).contains(&parts.len())
        || parts.iter().any(|s| {
            s.is_empty()
                || s.len() > 128
                || !s.bytes().enumerate().all(|(i, b)| {
                    b.is_ascii_alphabetic() || b == b'_' || (i > 0 && b.is_ascii_digit())
                })
        })
    {
        return Err(CatalogError::UnknownSource);
    }
    let (database, schema): (String, String) =
        conn.query_row("SELECT current_database(), current_schema()", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
    let (database, schema, table) = match parts.as_slice() {
        [table] => (database.as_str(), schema.as_str(), *table),
        [schema, table] => (database.as_str(), *schema, *table),
        [database, schema, table] => (*database, *schema, *table),
        _ => return Err(CatalogError::UnknownSource),
    };
    let actual = conn.prepare(
        "SELECT table_catalog, table_schema, table_name FROM information_schema.tables WHERE lower(table_catalog) = lower(?) AND lower(table_schema) = lower(?) AND lower(table_name) = lower(?) LIMIT 2",
    )?.query_map([database, schema, table], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<duckdb::Result<Vec<_>>>()?;
    let (database, schema, table) = unique_relation(actual)?;
    let quote = |name: &str| format!("\"{}\"", name.replace('"', "\"\""));
    let sql = format!(
        "DESCRIBE {}.{}.{}",
        quote(&database),
        quote(&schema),
        quote(&table)
    );
    let raw = conn
        .prepare(&sql)?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<duckdb::Result<Vec<_>>>()?;
    let columns = raw
        .into_iter()
        .map(|(name, sql_type, nullable)| {
            let value_type = value_type(&sql_type).map_err(|_| CatalogError::UnsupportedType {
                column: name.clone(),
                sql_type,
            })?;
            Ok(Column {
                handle: handle(&[&database, &schema, &table, &name])?,
                name,
                value_type,
                nullable: nullable == "YES",
            })
        })
        .collect::<Result<Vec<_>, CatalogError>>()?;
    Ok(Relation {
        source: handle(&[&database])?,
        relation: handle(&[&database, &schema, &table])?,
        columns,
    })
}

fn unique_relation(
    mut rows: Vec<(String, String, String)>,
) -> Result<(String, String, String), CatalogError> {
    if rows.len() > 1 {
        return Err(CatalogError::AmbiguousSource);
    }
    rows.pop().ok_or(CatalogError::UnknownSource)
}

// Hex is injective, case preserving and satisfies SourceHandle's logical tokens.
fn handle(parts: &[&str]) -> Result<SourceHandle, CatalogError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let encoded = parts
        .iter()
        .map(|part| {
            let mut text = String::from("n");
            for byte in part.bytes() {
                text.push(char::from(HEX[usize::from(byte >> 4)]));
                text.push(char::from(HEX[usize::from(byte & 15)]));
            }
            text
        })
        .collect::<Vec<_>>()
        .join("/");
    SourceHandle::new(encoded).map_err(|_| CatalogError::InvalidHandle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_return_validation_errors_and_preserve_encoded_tokens() {
        assert!(matches!(handle(&[]), Err(CatalogError::InvalidHandle)));
        assert!(matches!(
            handle(&["Sales", "región/total"]),
            Ok(value) if value.as_str() == "n53616c6573/n72656769c3b36e2f746f74616c"
        ));
    }

    #[test]
    fn case_folded_catalog_collisions_never_choose_a_relation() {
        let rows = vec![
            ("pg".into(), "public".into(), "Sales".into()),
            ("pg".into(), "public".into(), "sales".into()),
        ];
        assert!(matches!(
            unique_relation(rows),
            Err(CatalogError::AmbiguousSource)
        ));
    }
}
