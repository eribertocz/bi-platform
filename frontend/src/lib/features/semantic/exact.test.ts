import fc from 'fast-check';
import { describe, expect, it } from 'vitest';
import {
  bars,
  checkBlock,
  groupExact,
  IncompatibleResult,
  kpis,
  MAX_BARS,
  shown,
  shownText,
  valueType,
  type SemanticBlock,
  type SemanticColumn,
  type SemanticRun,
} from './exact';

// Contract-shaped blocks for the refusals no real answer produces on purpose.
const column = (over: Partial<SemanticColumn>): SemanticColumn => ({
  id: 'sef:metric/m',
  kind: 'metric',
  masked: false,
  nullable: true,
  selection_id: 'm',
  timezone: null,
  type: 'INT64',
  unit: null,
  ...over,
});
const block = (columns: SemanticColumn[], rows: SemanticBlock['rows']): SemanticBlock => ({
  block_id: 'b',
  columns,
  limit: null,
  row_count: rows.length,
  rows,
});
const run = (b: SemanticBlock) => ({ result: { blocks: [b] } }) as unknown as SemanticRun;
const region = column({
  id: 'sef:dimension/region',
  kind: 'dimension',
  selection_id: 'r',
  type: 'VARCHAR',
});

describe('exact values', () => {
  it('keeps integers beyond 2^53 and high-precision decimals as exact text', () => {
    expect(groupExact('9007199254740993')).toBe('9.007.199.254.740.993');
    expect(groupExact('-9223372036854775808')).toBe('-9.223.372.036.854.775.808');
    expect(groupExact('-1234.5678901234')).toBe('-1.234,5678901234');
    expect(groupExact('0.20')).toBe('0,20');
  });

  it('groups any exact decimal without changing a digit', () => {
    fc.assert(
      fc.property(fc.bigInt({ min: -(10n ** 30n), max: 10n ** 30n }), fc.nat(9), (n, s) => {
        const text = s === 0 ? n.toString() : `${n.toString()}.${'7'.repeat(s)}`;
        expect(groupExact(text).replace(/\./g, '').replace(',', '.')).toBe(text);
      }),
    );
  });

  it('tells a missing value from a withheld one', () => {
    expect(shownText(shown(column({}), null))).toBe('Sin dato');
    expect(shownText(shown(column({ masked: true }), null))).toBe('Oculto');
  });

  it('shows instants in UTC, as sent, and never in the browser zone', () => {
    const at = column({ type: 'TIMESTAMPTZ' });
    expect(shownText(shown(at, '2026-01-02T03:04:05.123456Z'))).toBe(
      '2026-01-02 03:04:05.123456 UTC',
    );
  });
});

describe('a block that does not fit its contract shows no figure', () => {
  it.each([
    ['a float for an INT64', column({}), 1.5],
    ['an INT64 beyond 64 bits', column({}), '9223372036854775808'],
    ['more decimals than the scale', column({ type: 'DECIMAL(18,2)' }), '1.234'],
    ['a number for a DECIMAL', column({ type: 'DECIMAL(18,2)' }), 100],
    ['an offset instant', column({ type: 'TIMESTAMPTZ' }), '2026-01-01T00:00:00+02:00'],
    ['a value in a masked column', column({ masked: true }), '1'],
  ])('%s', (_, c, cell) => {
    expect(() => checkBlock(block([c], [[cell as never]]))).toThrow(IncompatibleResult);
  });

  it('refuses an unknown type, a short row, a wrong count or rows over the limit', () => {
    expect(() => valueType('HUGEINT')).toThrow(IncompatibleResult);
    expect(() => checkBlock(block([column({}), region], [['1']]))).toThrow(IncompatibleResult);
    expect(() => checkBlock({ ...block([column({})], [['1']]), row_count: 2 })).toThrow();
    expect(() => checkBlock({ ...block([column({})], [['1'], ['2']]), limit: 1 })).toThrow();
  });
});

describe('KPIs come only from a total without a grain', () => {
  it('refuses a grouped or limited table instead of summing it', () => {
    const grouped = block(
      [region, column({})],
      [
        ['Norte', '2'],
        ['Sur', '2'],
      ],
    );
    expect(() => kpis(run(grouped))).toThrow(IncompatibleResult);
    const oneRow = block([region, column({})], [['Norte', '2']]);
    expect(() => kpis(run(oneRow))).toThrow(IncompatibleResult);
    expect(() => kpis({ result: { blocks: [] } } as unknown as SemanticRun)).toThrow();
  });
});

describe('a chart only when it can be honest', () => {
  it('omits other shapes, withheld or non-numeric metrics and too many bars', () => {
    const metric = column({});
    expect(bars(block([metric], [['4']]))).toBeNull();
    expect(bars(block([region, region, metric], [['a', 'b', '1']]))).toBeNull();
    expect(bars(block([region, metric], [['a', '1']]), 'absent')).toBeNull();
    expect(bars(block([region, column({ masked: true })], [['Norte', null]]))).toBeNull();
    expect(bars(block([region, column({ type: 'VARCHAR' })], [['Norte', 'x']]))).toBeNull();
    expect(bars(block([region, metric], []))).toBeNull();
    const many = Array.from({ length: MAX_BARS + 1 }, (_, i) => [`r${i}`, '1']);
    expect(bars(block([region, metric], many))).toBeNull();
  });

  it('scales by the largest magnitude exactly, keeping gaps and the exact tooltip', () => {
    const amount = column({ type: 'DECIMAL(38,10)' });
    const projected = bars(
      block(
        [region, amount],
        [
          ['a', '9007199254740993.0000000001'],
          ['b', '-4503599627370496.5'],
          ['c', null],
        ],
      ),
    )!;
    // Truncated toward zero at 1e-7 %: geometry only, never the figure.
    expect(projected.heights[0]).toBe(100);
    expect(projected.heights[1]).toBeCloseTo(-50, 6);
    expect(projected.heights[2]).toBeNull();
    expect(projected.exact).toEqual([
      '9.007.199.254.740.993,0000000001',
      '-4.503.599.627.370.496,5',
      'Sin dato',
    ]);
  });
});
