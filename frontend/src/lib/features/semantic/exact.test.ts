import fc from 'fast-check';
import { describe, expect, it } from 'vitest';
import {
  bars,
  checkBlock,
  groupExact,
  IncompatibleResult,
  instantText,
  totals,
  MAX_BARS,
  MAX_DECIMAL_PRECISION,
  shown,
  shownText,
  valueType,
  type SemanticBlock,
  type SemanticColumn,
  type SemanticRun,
} from './exact';
import regionAnswer from './fixtures/region.json';

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
    expect(() => totals(run(grouped))).toThrow(IncompatibleResult);
    const oneRow = block([region, column({})], [['Norte', '2']]);
    expect(() => totals(run(oneRow))).toThrow(IncompatibleResult);
    expect(() => totals({ result: { blocks: [] } } as unknown as SemanticRun)).toThrow();
  });
});

describe('a chart only when it can be honest', () => {
  it('omits other shapes, withheld or non-numeric metrics and too many bars', () => {
    const metric = column({});
    expect(bars(block([metric], [['4']]))).toBeNull();
    expect(
      bars(block([region, { ...region, selection_id: 'c' }, metric], [['a', 'b', '1']])),
    ).toBeNull();
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

/**
 * Codex's reproduction on #255: the real region answer reduced to its first
 * row, with column 1 retyped as `type` and holding `value`.
 */
function retyped(type: string, value: string): SemanticBlock {
  const b = structuredClone(regionAnswer.result.blocks[0]) as SemanticBlock;
  b.columns[1]!.type = type;
  b.rows = [b.rows[0]!];
  b.row_count = 1;
  b.rows[0]![1] = value;
  return b;
}

const accepts = (type: string, value: string) => {
  try {
    checkBlock(retyped(type, value));
    return true;
  } catch (error) {
    if (error instanceof IncompatibleResult) return false;
    throw error;
  }
};

/** `length` decimal digits, the first one not zero when `lead`. */
const digits = (length: number, lead: boolean) =>
  fc
    .array(fc.integer({ min: 0, max: 9 }), { minLength: length, maxLength: length })
    .map((ds) => (lead && ds[0] === 0 ? [1, ...ds.slice(1)] : ds).join(''));

/** A DECIMAL(p,s) with `p` up to the contract's limit. */
const decimalType = fc
  .integer({ min: 1, max: MAX_DECIMAL_PRECISION })
  .chain((p) => fc.tuple(fc.constant(p), fc.integer({ min: 0, max: p })));

/** `whole.fraction` with an optional sign, as the server writes it. */
const decimalText = (negative: boolean, whole: string, fraction: string) =>
  `${negative ? '-' : ''}${whole === '' ? '0' : whole}${fraction === '' ? '' : `.${fraction}`}`;

const ungrouped = (text: string) => text.replace(/\./g, '').replace(',', '.');

describe('DECIMAL up to the precision of the contract (Decimal256)', () => {
  it('accepts every precision from 1 to 76 and refuses anything else', () => {
    for (const declared of ['DECIMAL(1,0)', 'DECIMAL(38,10)', 'DECIMAL(56,6)', 'DECIMAL(76,2)']) {
      expect(valueType(declared).kind).toBe('decimal');
    }
    expect(valueType('DECIMAL(76,76)')).toEqual({ kind: 'decimal', precision: 76, scale: 76 });
    for (const declared of ['DECIMAL(77,2)', 'DECIMAL(0,0)', 'DECIMAL(5,6)', 'DECIMAL(07,2)']) {
      expect(() => valueType(declared)).toThrow(IncompatibleResult);
    }
  });

  it('keeps a DECIMAL(76,2) value at its edge digit for digit, in table and chart', () => {
    const edge = `-${'9'.repeat(74)}.99`;
    const b = retyped('DECIMAL(76,2)', edge);
    expect(checkBlock(b)).toBe(b);
    const text = shownText(shown(b.columns[1]!, edge));
    expect(ungrouped(text)).toBe(edge);
    expect(bars(b)!.exact).toEqual([text]);
    expect(bars(b)!.heights).toEqual([-100]);
  });

  it('round-trips any DECIMAL(p,s) with p <= 76 digit for digit', () => {
    fc.assert(
      fc.property(
        decimalType.chain(([p, s]) =>
          fc.tuple(
            fc.constant(p),
            fc.constant(s),
            fc.boolean(),
            fc.integer({ min: 0, max: p - s }).chain((n) => digits(n, true)),
            fc.integer({ min: 0, max: s }).chain((n) => digits(n, false)),
          ),
        ),
        ([p, s, negative, whole, fraction]) => {
          const value = decimalText(negative, whole, fraction);
          const b = retyped(`DECIMAL(${p},${s})`, value);
          expect(checkBlock(b)).toBe(b);
          const text = shownText(shown(b.columns[1]!, value));
          expect(ungrouped(text)).toBe(value);
          expect(bars(b)!.exact).toEqual([text]);
        },
      ),
    );
  });

  it('refuses any value with more integer or fractional digits than DECIMAL(p,s) holds', () => {
    fc.assert(
      fc.property(
        decimalType.chain(([p, s]) =>
          fc.tuple(
            fc.constant(p),
            fc.constant(s),
            fc.boolean(),
            fc.boolean(),
            digits(p - s + 1, true),
            digits(s + 1, false),
          ),
        ),
        ([p, s, negative, inWhole, whole, fraction]) => {
          // One digit too many in the integer part, or in the fraction.
          const value = inWhole
            ? decimalText(negative, whole, fraction.slice(1))
            : decimalText(negative, '', fraction);
          expect(accepts(`DECIMAL(${p},${s})`, value)).toBe(false);
        },
      ),
    );
  });

  it.each([
    ['DECIMAL(3,2)', '9.99', true],
    ['DECIMAL(3,2)', '-9.99', true],
    ['DECIMAL(3,2)', '0.01', true],
    ['DECIMAL(3,2)', '10.00', false],
    ['DECIMAL(3,2)', '-10.00', false],
    ['DECIMAL(3,2)', '1.001', false],
    ['DECIMAL(2,2)', '0.99', true],
    ['DECIMAL(2,2)', '1.00', false],
    ['DECIMAL(76,0)', '9'.repeat(76), true],
    ['DECIMAL(76,0)', `1${'0'.repeat(76)}`, false],
    ['DECIMAL(76,38)', `${'9'.repeat(38)}.${'9'.repeat(38)}`, true],
    ['DECIMAL(76,38)', `1${'0'.repeat(38)}.5`, false],
  ])('%s holding %s fits: %s', (type, value, fits) => {
    expect(accepts(type, value)).toBe(fits);
  });
});

/** The oracle of a calendar day, independent of the implementation. */
function realDay(year: number, month: number, day: number): boolean {
  const at = new Date(0);
  at.setUTCFullYear(year, month - 1, day);
  return at.getUTCFullYear() === year && at.getUTCMonth() === month - 1 && at.getUTCDate() === day;
}

const pad = (n: number, width: number) => String(n).padStart(width, '0');
const dayText = (y: number, m: number, d: number) => `${pad(y, 4)}-${pad(m, 2)}-${pad(d, 2)}`;

describe('dates and instants are real ones of the calendar and the clock', () => {
  it.each([
    ['DECIMAL(3,2)', '1234.56'],
    ['DATE', '2026-02-30'],
    ['TIMESTAMPTZ', '2026-13-40T25:61:61Z'],
  ])('refuses the reproduction of the review: %s holding %s', (type, value) => {
    expect(() => checkBlock(retyped(type, value))).toThrow(IncompatibleResult);
  });

  it.each([
    ['2024-02-29', true],
    ['2000-02-29', true],
    ['2023-02-29', false],
    ['1900-02-29', false],
    ['2026-02-28', true],
    ['2026-02-30', false],
    ['2026-04-30', true],
    ['2026-04-31', false],
    ['2026-12-31', true],
    ['2026-00-10', false],
    ['2026-13-01', false],
    ['2026-01-00', false],
  ])('DATE %s fits: %s', (value, fits) => {
    expect(accepts('DATE', value)).toBe(fits);
  });

  it.each([
    ['2026-01-01T23:59:59.999999999Z', true],
    ['2024-02-29T00:00:00Z', true],
    ['2023-02-29T00:00:00Z', false],
    ['2026-01-01T24:00:00Z', false],
    ['2026-01-01T23:60:00Z', false],
    ['2026-01-01T23:59:60Z', false],
    ['2026-02-30T00:00:00Z', false],
  ])('TIMESTAMPTZ %s fits: %s', (value, fits) => {
    expect(accepts('TIMESTAMPTZ', value)).toBe(fits);
  });

  it('accepts exactly the real calendar days', () => {
    fc.assert(
      fc.property(
        fc.integer({ min: 0, max: 9999 }),
        fc.integer({ min: 0, max: 13 }),
        fc.integer({ min: 0, max: 32 }),
        (y, m, d) => {
          expect(accepts('DATE', dayText(y, m, d))).toBe(realDay(y, m, d));
        },
      ),
    );
  });

  it('accepts exactly the real UTC instants and shows their fraction as sent', () => {
    fc.assert(
      fc.property(
        fc.integer({ min: 0, max: 9999 }),
        fc.integer({ min: 1, max: 12 }),
        fc.integer({ min: 1, max: 31 }),
        fc.integer({ min: 0, max: 99 }),
        fc.integer({ min: 0, max: 99 }),
        fc.integer({ min: 0, max: 99 }),
        fc.option(fc.integer({ min: 1, max: 9 }).chain((n) => digits(n, false))),
        (y, mo, d, h, mi, s, fraction) => {
          const clock = `${pad(h, 2)}:${pad(mi, 2)}:${pad(s, 2)}${fraction ? `.${fraction}` : ''}`;
          const value = `${dayText(y, mo, d)}T${clock}Z`;
          const real = realDay(y, mo, d) && h <= 23 && mi <= 59 && s <= 59;
          expect(accepts('TIMESTAMPTZ', value)).toBe(real);
          if (real) expect(instantText(value)).toBe(`${dayText(y, mo, d)} ${clock} UTC`);
        },
      ),
    );
  });
});

describe('a total excluded after aggregating (#266)', () => {
  const metric = column({ selection_id: 't', type: 'DECIMAL(38,2)' });
  const count = column({ selection_id: 'f', id: 'sef:metric/filas', type: 'INT64' });
  const total = (rows: SemanticBlock['rows']) => run(block([metric, count], rows));

  it('is empty only for a question that may exclude it, checked whole', () => {
    expect(totals(total([]), true)).toEqual({ kind: 'empty' });
    // Without that context, no answer is ever taken for an empty total.
    expect(() => totals(total([]))).toThrow(IncompatibleResult);
  });

  it('a NULL row and a zero row are values, never «empty»', () => {
    const nulls = totals(total([[null, '0']]), true);
    expect(nulls.kind).toBe('values');
    expect(nulls.kind === 'values' && nulls.items.map((i) => i.value.kind)).toEqual([
      'missing',
      'value',
    ]);
    const zeros = totals(total([['0.00', '0']]), true);
    expect(zeros.kind === 'values' && zeros.items.map((i) => shownText(i.value))).toEqual([
      '0,00',
      '0',
    ]);
  });

  it('still refuses more rows, a grain, other blocks or an inconsistent count', () => {
    expect(() =>
      totals(
        total([
          ['1.00', '1'],
          ['2.00', '1'],
        ]),
        true,
      ),
    ).toThrow(IncompatibleResult);
    expect(() => totals(run(block([region, metric], [])), true)).toThrow(IncompatibleResult);
    const two = { result: { blocks: [block([metric], []), block([metric], [])] } };
    expect(() => totals(two as unknown as SemanticRun, true)).toThrow(IncompatibleResult);
    const lying = { ...block([metric, count], [['1.00', '1']]), row_count: 0 };
    expect(() => totals(run(lying), true)).toThrow(IncompatibleResult);
    const hollow = { ...block([metric, count], []), row_count: 1 };
    expect(() => totals(run(hollow), true)).toThrow(IncompatibleResult);
  });

  it('Codex #278: a total without any metric column is incompatible even with HAVING', () => {
    expect(() => totals(run(block([], [])), true)).toThrow(IncompatibleResult);
    expect(() => totals(run(block([], [[]])), true)).toThrow(IncompatibleResult);
    expect(() => totals(run(block([], [])))).toThrow(IncompatibleResult);
  });
});
