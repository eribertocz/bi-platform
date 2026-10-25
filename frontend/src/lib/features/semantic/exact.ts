import type { components } from '#lib/api/generated.ts';

export type SemanticRun = components['schemas']['SemanticRun'];
export type SemanticBlock = components['schemas']['SemanticBlock'];
export type SemanticColumn = components['schemas']['SemanticColumn'];
export type SemanticCell = components['schemas']['SemanticCell'];
export type SemanticReceipt = components['schemas']['SemanticRunReceipt'];
export type SemanticNotice = components['schemas']['SemanticRunNotice'];

/** A result that does not fit its own contract: no figure of it is shown. */
export class IncompatibleResult extends Error {
  constructor(message = 'El resultado no corresponde a su contrato: no se muestra ninguna cifra.') {
    super(message);
    this.name = 'IncompatibleResult';
  }
}

type ValueType =
  | { kind: 'decimal'; precision: number; scale: number }
  | { kind: 'int64' }
  | { kind: 'varchar' }
  | { kind: 'boolean' }
  | { kind: 'date' }
  | { kind: 'timestamptz' };

/** The widest DECIMAL the contract carries (Decimal256). */
export const MAX_DECIMAL_PRECISION = 76;

/** The declared type of a column, or a refusal for one this build does not know. */
export function valueType(declared: string): ValueType {
  const decimal = /^DECIMAL\(([1-9]\d?),(0|[1-9]\d?)\)$/.exec(declared);
  if (decimal) {
    const [precision, scale] = [Number(decimal[1]), Number(decimal[2])];
    if (precision <= MAX_DECIMAL_PRECISION && scale <= precision) {
      return { kind: 'decimal', precision, scale };
    }
  }
  switch (declared) {
    case 'INT64':
      return { kind: 'int64' };
    case 'VARCHAR':
      return { kind: 'varchar' };
    case 'BOOLEAN':
      return { kind: 'boolean' };
    case 'DATE':
      return { kind: 'date' };
    case 'TIMESTAMPTZ':
      return { kind: 'timestamptz' };
  }
  throw new IncompatibleResult();
}

const INT64_MIN = -(2n ** 63n);
const INT64_MAX = 2n ** 63n - 1n;
const DECIMAL = /^-?(\d+)(?:\.(\d+))?$/;
const DAY = /^(\d{4})-(\d{2})-(\d{2})$/;
const INSTANT = /^(\d{4}-\d{2}-\d{2})T(\d{2}):(\d{2}):(\d{2})(\.\d{1,9})?Z$/;

/** Whether `text` is a decimal of at most `precision` digits, `scale` of them fractional. */
function fitsDecimal(text: string, precision: number, scale: number): boolean {
  const match = DECIMAL.exec(text);
  if (!match) return false;
  const significant = match[1]!.replace(/^0+/, '').length;
  return (match[2]?.length ?? 0) <= scale && significant <= precision - scale;
}

/** Days in `month` (1–12) of `year` in the proleptic Gregorian calendar. */
function daysIn(year: number, month: number): number {
  if (month === 2) return year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0) ? 29 : 28;
  return month === 4 || month === 6 || month === 9 || month === 11 ? 30 : 31;
}

/** Whether `text` is a real calendar day written `YYYY-MM-DD`; nothing is normalized. */
function isDay(text: string): boolean {
  const match = DAY.exec(text);
  if (!match) return false;
  const [year, month, day] = [Number(match[1]), Number(match[2]), Number(match[3])];
  return month >= 1 && month <= 12 && day >= 1 && day <= daysIn(year, month);
}

/** Whether `text` is a real UTC instant: a calendar day, 00–23 h, 00–59 min and s. */
function isInstant(text: string): boolean {
  const match = INSTANT.exec(text);
  if (!match || !isDay(match[1]!)) return false;
  const [hour, minute, second] = [Number(match[2]), Number(match[3]), Number(match[4])];
  return hour <= 23 && minute <= 59 && second <= 59;
}

/** Whether `cell`, present, is a value of `type` exactly as the contract writes it. */
function fits(type: ValueType, cell: SemanticCell): boolean {
  switch (type.kind) {
    case 'decimal':
      return typeof cell === 'string' && fitsDecimal(cell, type.precision, type.scale);
    case 'int64':
      return typeof cell === 'string' && /^-?\d+$/.test(cell) && inInt64(BigInt(cell));
    case 'varchar':
      return typeof cell === 'string';
    case 'boolean':
      return typeof cell === 'boolean';
    case 'date':
      return typeof cell === 'string' && isDay(cell);
    case 'timestamptz':
      return typeof cell === 'string' && isInstant(cell);
  }
}

const inInt64 = (value: bigint) => value >= INT64_MIN && value <= INT64_MAX;

/**
 * Checks a block against its own column contract before anything of it is
 * shown: row count, row width, and every cell of the declared type; a masked
 * column only ever carries `null`.
 */
export function checkBlock(block: SemanticBlock): SemanticBlock {
  const types = block.columns.map((column) => valueType(column.type));
  // Each column is addressed by its selection: two alike are no contract.
  if (new Set(block.columns.map((c) => c.selection_id)).size !== block.columns.length) {
    throw new IncompatibleResult();
  }
  if (block.rows.length !== block.row_count) throw new IncompatibleResult();
  if (block.limit !== null && block.row_count > block.limit) throw new IncompatibleResult();
  for (const row of block.rows) {
    if (row.length !== block.columns.length) throw new IncompatibleResult();
    row.forEach((cell, i) => {
      if (cell === null) return;
      if (block.columns[i]!.masked || !fits(types[i]!, cell)) throw new IncompatibleResult();
    });
  }
  return block;
}

/** What one cell shows: a value, a missing value or a withheld one. */
export type Shown =
  { kind: 'value'; text: string; numeric: boolean } | { kind: 'missing' } | { kind: 'masked' };

/** Exact decimal text grouped for reading; never through a float. */
export function groupExact(value: string): string {
  const [whole, fraction] = value.split('.');
  return (
    whole!.replace(/\B(?=(\d{3})+(?!\d))/g, '.') + (fraction === undefined ? '' : ',' + fraction)
  );
}

/** An instant as the contract carries it, in UTC, to the precision sent. */
export function instantText(value: string): string {
  return `${value.slice(0, 10)} ${value.slice(11, -1)} UTC`;
}

/** The cell of `column`, from a block already checked. */
export function shown(column: SemanticColumn, cell: SemanticCell | null): Shown {
  if (column.masked) return { kind: 'masked' };
  if (cell === null) return { kind: 'missing' };
  const type = valueType(column.type);
  switch (type.kind) {
    case 'decimal':
    case 'int64':
      return { kind: 'value', text: groupExact(cell as string), numeric: true };
    case 'boolean':
      return { kind: 'value', text: cell ? 'Sí' : 'No', numeric: false };
    case 'timestamptz':
      return { kind: 'value', text: instantText(cell as string), numeric: false };
    case 'varchar':
      return {
        kind: 'value',
        text: cell === '' ? '(texto vacío)' : (cell as string),
        numeric: false,
      };
    case 'date':
      return { kind: 'value', text: cell as string, numeric: false };
  }
}

/** The readable text of a cell, for tables, tooltips and KPIs. */
export function shownText(value: Shown): string {
  switch (value.kind) {
    case 'value':
      return value.text;
    case 'missing':
      return 'Sin dato';
    case 'masked':
      return 'Oculto';
  }
}

/** The name a column is shown with: the caller's label, or the last segment of its id. */
export function columnLabel(column: SemanticColumn, labels: Record<string, string> = {}): string {
  return labels[column.selection_id] ?? column.id.split(/[/:]/).at(-1) ?? column.id;
}

const isGrain = (column: SemanticColumn) => column.kind !== 'metric';

/**
 * The totals of a result aggregated without a grain: its values, or none
 * when a filter after aggregating excluded the whole total (#266).
 */
export type Totals =
  { kind: 'values'; items: { column: SemanticColumn; value: Shown }[] } | { kind: 'empty' };

/**
 * The totals of `run`: one block, no dimension, exactly one row. Zero rows
 * is a total only when the question may exclude it after aggregating
 * (`emptyAllowed`, a HAVING): never inferred from any empty answer. The
 * block is checked whole either way; anything else is refused, never summed.
 */
export function totals(run: SemanticRun, emptyAllowed = false): Totals {
  const [block, ...rest] = run.result.blocks;
  const rows = emptyAllowed ? [0, 1] : [1];
  // A total keeps its metric columns even with no row: without any, the
  // answer is incomplete, never a total the filter excluded.
  if (
    !block ||
    rest.length > 0 ||
    block.columns.length === 0 ||
    block.columns.some(isGrain) ||
    !rows.includes(block.row_count)
  ) {
    throw new IncompatibleResult('Este resultado no es un total sin desglose.');
  }
  checkBlock(block);
  const row = block.rows[0];
  if (!row) return { kind: 'empty' };
  return {
    kind: 'values',
    items: block.columns.map((column, i) => ({ column, value: shown(column, row[i] ?? null) })),
  };
}

/** The most bars a chart draws before it gives way to the table. */
export const MAX_BARS = 50;

export type Bars = {
  dimension: SemanticColumn;
  metric: SemanticColumn;
  /** One per row, as the table shows them. */
  categories: string[];
  /** Exact values as shown, for the tooltip. */
  exact: string[];
  /** Percent of the largest absolute value; `null` for a missing value. */
  heights: (number | null)[];
};

/** A decimal or integer as a scaled integer: `12.3` at scale 2 is `1230n`. */
function scaled(value: string, scale: number): bigint {
  const [whole, fraction = ''] = value.split('.');
  const negative = whole!.startsWith('-');
  const digits = BigInt(whole!.replace('-', '') + fraction.padEnd(scale, '0'));
  return negative ? -digits : digits;
}

/**
 * The geometry of a bar chart of one numeric metric by the block's single
 * grain, or `null` when a chart could not show it honestly (another shape,
 * a withheld or non-numeric metric, no rows, too many bars). `selection`
 * picks the metric; by default, the first one. Only the bounded quotient
 * becomes a Number; the exact values stay text.
 */
export function bars(block: SemanticBlock, selection?: string): Bars | null {
  checkBlock(block);
  const grains = block.columns.filter(isGrain);
  const metric = block.columns.find(
    (column) => !isGrain(column) && (selection === undefined || column.selection_id === selection),
  );
  if (grains.length !== 1 || !metric) return null;
  const dimension = grains[0]!;
  const type = valueType(metric.type);
  if (metric.masked || (type.kind !== 'decimal' && type.kind !== 'int64')) return null;
  if (block.rows.length === 0 || block.rows.length > MAX_BARS) return null;
  const scale = type.kind === 'decimal' ? type.scale : 0;
  const [d, m] = [block.columns.indexOf(dimension), block.columns.indexOf(metric)];
  const values = block.rows.map((row) => {
    const cell = row[m] ?? null;
    return cell === null ? null : scaled(cell as string, scale);
  });
  const max = values.reduce<bigint>((top, v) => {
    const magnitude = v === null ? 0n : v < 0n ? -v : v;
    return magnitude > top ? magnitude : top;
  }, 0n);
  return {
    dimension,
    metric,
    categories: block.rows.map((row) => shownText(shown(dimension, row[d] ?? null))),
    exact: block.rows.map((row) => shownText(shown(metric, row[m] ?? null))),
    heights: values.map((v) =>
      v === null ? null : max === 0n ? 0 : Number((v * 1_000_000_000n) / max) / 10_000_000,
    ),
  };
}
