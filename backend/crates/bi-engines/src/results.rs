//! Engine-neutral results: every value as exact text, so DuckDB and Polars
//! outputs compare value by value with the oracle.

use bi_fixtures::oracle::Oracle;

pub type Row = Vec<Option<String>>;

/// The benchmark queries, identical in meaning on both engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Query {
    /// `row_count`, `order_id_sum`, `amount_sum`, `amount_min`, `amount_max`,
    /// `quantity_sum`, `ordered_at_min`, `ordered_at_max`, `region_null_count`.
    Totals,
    /// `region`, `row_count`, `quantity_sum`, `amount_sum`; region ascending, NULL last.
    ByRegion,
    /// `region`, `amount_sum` for regions north and south; region ascending.
    NorthSouth,
}

impl Query {
    pub const ALL: [Self; 3] = [Self::Totals, Self::ByRegion, Self::NorthSouth];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Totals => "totals",
            Self::ByRegion => "by_region",
            Self::NorthSouth => "north_south",
        }
    }

    /// What the oracle says the query returns.
    #[must_use]
    pub fn expected(self, oracle: &Oracle) -> Vec<Row> {
        let totals = &oracle.totals;
        match self {
            Self::Totals => vec![vec![
                Some(totals.row_count.clone()),
                totals.order_id.sum.clone(),
                totals.amount.sum.clone(),
                totals.amount.min.clone(),
                totals.amount.max.clone(),
                totals.quantity.sum.clone(),
                totals.ordered_at.min.clone(),
                totals.ordered_at.max.clone(),
                Some(totals.region_null_count.clone()),
            ]],
            Self::ByRegion => oracle
                .by_region
                .iter()
                .map(|group| {
                    vec![
                        group.region.clone(),
                        Some(group.row_count.clone()),
                        group.quantity_sum.clone(),
                        group.amount_sum.clone(),
                    ]
                })
                .collect(),
            Self::NorthSouth => oracle
                .by_region
                .iter()
                .filter(|group| matches!(group.region.as_deref(), Some("north" | "south")))
                .map(|group| vec![group.region.clone(), group.amount_sum.clone()])
                .collect(),
        }
    }
}
