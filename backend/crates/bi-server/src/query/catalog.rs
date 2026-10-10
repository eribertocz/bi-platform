//! Allowlist of what a client may query (G1: the synthetic `sales` dataset).
//! Requests only name entries of this catalog; SQL fragments come from here,
//! never from the request.

use serde::Serialize;
use utoipa::ToSchema;

/// Logical type of a result column. Every non-null value travels as a JSON
/// string, so `integer` (64- or 128-bit) and `decimal` stay exact in clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ColumnType {
    String,
    Integer,
    Decimal,
    Timestamptz,
}

#[derive(Debug)]
pub struct Field {
    pub name: &'static str,
    /// SQL expression over the dataset table.
    pub sql: &'static str,
    pub column_type: ColumnType,
    /// `(precision, scale)` for decimals.
    pub decimal: Option<(u8, u8)>,
    pub nullable: bool,
}

#[derive(Debug)]
pub struct Dataset {
    pub name: &'static str,
    pub table: &'static str,
    pub dimensions: &'static [Field],
    pub measures: &'static [Field],
}

impl Dataset {
    #[must_use]
    pub fn dimension(&self, name: &str) -> Option<&'static Field> {
        self.dimensions.iter().find(|field| field.name == name)
    }

    #[must_use]
    pub fn measure(&self, name: &str) -> Option<&'static Field> {
        self.measures.iter().find(|field| field.name == name)
    }
}

pub static SALES: Dataset = Dataset {
    name: "sales",
    table: "lake.main.sales",
    dimensions: &[Field {
        name: "region",
        sql: "region",
        column_type: ColumnType::String,
        decimal: None,
        nullable: true,
    }],
    measures: &[
        Field {
            name: "row_count",
            sql: "count(*)",
            column_type: ColumnType::Integer,
            decimal: None,
            nullable: false,
        },
        Field {
            name: "amount_sum",
            sql: "sum(amount)",
            column_type: ColumnType::Decimal,
            // DuckDB widens SUM(DECIMAL(18,2)) to DECIMAL(38,2).
            decimal: Some((38, 2)),
            nullable: true,
        },
        Field {
            name: "quantity_sum",
            sql: "sum(quantity)",
            // SUM(BIGINT) is HUGEINT in DuckDB: 128-bit, exact.
            column_type: ColumnType::Integer,
            decimal: None,
            nullable: true,
        },
    ],
};

pub static DATASETS: &[&Dataset] = &[&SALES];

#[must_use]
pub fn dataset(name: &str) -> Option<&'static Dataset> {
    DATASETS
        .iter()
        .copied()
        .find(|dataset| dataset.name == name)
}
