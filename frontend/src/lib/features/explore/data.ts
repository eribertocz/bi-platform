import type { QueryResult, QueryRequest } from '#lib/api/query.ts';
export type RegionRow = { region: string | null; count: string; amount: string | null };
export type ExploreData = { count: string; amount: string | null; rows: RegionRow[] };
export function salesQuery(region: string, grouped: boolean): QueryRequest {
  return {
    dataset: 'sales',
    dimensions: grouped ? ['region'] : [],
    measures: ['row_count', 'amount_sum'],
    filters:
      region === 'all'
        ? []
        : [{ field: 'region', op: 'in', values: [region === 'null' ? null : region] }],
    limit: 100,
  };
}
function checkColumns(result: QueryResult, grouped: boolean) {
  const names = grouped ? ['region', 'row_count', 'amount_sum'] : ['row_count', 'amount_sum'];
  const types = grouped ? ['string', 'integer', 'decimal'] : ['integer', 'decimal'];
  if (
    result.truncated ||
    result.columns.length !== names.length ||
    !result.columns.every(
      (c, i) =>
        c.name === names[i] && c.type === types[i] && (c.type !== 'decimal' || c.scale === 2),
    )
  )
    throw new Error('El resultado de ventas está incompleto o tiene un esquema incompatible.');
}
export function exploreData(totals: QueryResult, grouped: QueryResult): ExploreData {
  checkColumns(totals, false);
  checkColumns(grouped, true);
  if (
    totals.rows.length !== 1 ||
    totals.rows[0]?.[0] == null ||
    grouped.rows.some((r) => r[1] === null)
  )
    throw new Error('El resultado de ventas no contiene los totales esperados.');
  return {
    count: totals.rows[0]![0]!,
    amount: totals.rows[0]![1]!,
    rows: grouped.rows.map((r) => ({ region: r[0]!, count: r[1]!, amount: r[2]! })),
  };
}
// Presentation only: never pass exact integer/decimal values through a float.
export function formatExact(value: string | null): string {
  if (value === null) return 'Sin dato';
  const [whole, fraction] = value.split('.');
  return (
    whole!.replace(/\B(?=(\d{3})+(?!\d))/g, '.') + (fraction === undefined ? '' : ',' + fraction)
  );
}
export function regionLabel(region: string | null): string {
  return region === null ? 'Sin región' : region;
}
// Geometry is relative to the maximum absolute amount, quantized to 1e-9.
// Only the bounded quotient (<= 1e9) becomes Number; originals remain exact.
export function relativeAmounts(rows: RegionRow[]): (number | null)[] {
  const values = rows.map((r) => {
    if (r.amount === null) return null;
    const [whole, fraction = ''] = r.amount.split('.');
    if (fraction.length > 2) throw new Error('Escala decimal incompatible.');
    return BigInt(whole + fraction.padEnd(2, '0'));
  });
  const max = values.reduce<bigint>(
    (m, v) => (v === null ? m : (v < 0n ? -v : v) > m ? (v < 0n ? -v : v) : m),
    0n,
  );
  return values.map((v) =>
    v === null ? null : max === 0n ? 0 : Number((v * 1_000_000_000n) / max) / 10_000_000,
  );
}
