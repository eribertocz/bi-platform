//! What the source of each dataset looks like: the port through which an
//! engine adapter (DuckDB's `DESCRIBE`, a `ClickHouse` system table, a test
//! fixture) tells the converter the exact type and nullability of every
//! column. Ossie's `datatype` has no precision (`Decimal`, not
//! `DECIMAL(18,2)`) and no nullability, and CSMIR needs both; this crate
//! never queries an engine itself (ADR 0008).

use std::collections::BTreeMap;

use sef_model_ir::identity::SourceHandle;
use sef_model_ir::types::ValueType;

/// The relations the datasets of a model may name, keyed by the exact
/// `source` string of the Ossie document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalog {
    pub relations: BTreeMap<String, Relation>,
}

/// One table, view or query result, as logical handles: the catalog never
/// carries URIs, credentials or SQL text.
#[derive(Debug, Clone, PartialEq)]
pub struct Relation {
    /// The connection the relation belongs to.
    pub source: SourceHandle,
    /// The relation within that connection.
    pub relation: SourceHandle,
    pub columns: Vec<Column>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    /// The physical name, matched like an SQL identifier: an unquoted
    /// reference ignores case, a quoted one must match exactly.
    pub name: String,
    pub handle: SourceHandle,
    pub value_type: ValueType,
    pub nullable: bool,
}

impl Relation {
    /// The columns a normalized Ossie identifier can refer to.
    pub(crate) fn columns_named(&self, normalized: &str) -> Vec<&Column> {
        self.columns
            .iter()
            .filter(|column| column.name == normalized || column.name.to_uppercase() == normalized)
            .collect()
    }
}
