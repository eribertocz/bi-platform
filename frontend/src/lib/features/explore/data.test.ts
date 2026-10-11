import { expect, it } from 'vitest';
import { formatExact, relativeAmounts, salesQuery, type RegionRow } from './data';

it('formats exact integers/decimals beyond float64, zero, negatives and null', () => {
  expect(formatExact('9007199254740993')).toBe('9.007.199.254.740.993');
  expect(formatExact('10000009275125921.28')).toBe('10.000.009.275.125.921,28');
  expect(formatExact('-9999999999999999.99')).toBe('-9.999.999.999.999.999,99');
  expect(formatExact('0.00')).toBe('0,00');
  expect(formatExact(null)).toBe('Sin dato');
});
it('converts only bounded relative geometry and keeps missing values distinct from zero', () => {
  const amounts = ['9999999999999999.99', '-4999999999999999.99', '0.00', null];
  const rows: RegionRow[] = amounts.map((amount) => ({ amount, region: null, count: '1' }));
  expect(relativeAmounts(rows)).toEqual([100, -49.9999999, 0, null]);
  expect(rows.map((r) => r.amount)).toEqual(amounts);
  expect(relativeAmounts([{ region: null, count: '0', amount: '0.00' }])).toEqual([0]);
});
it('asks the server for totals and grouping with the same region/null filter', () => {
  expect(salesQuery('null', true).filters).toEqual([{ field: 'region', op: 'in', values: [null] }]);
  expect(salesQuery('north', false).dimensions).toEqual([]);
  expect(salesQuery('north', false).filters).toEqual(salesQuery('north', true).filters);
});
