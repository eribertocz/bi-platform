import type { components } from '#lib/api/generated.ts';
import type { CatalogMember, CatalogModel } from '#lib/api/semantic.ts';

export type SemanticFilter = components['schemas']['SemanticFilter'];
export type SemanticTarget = components['schemas']['SemanticTarget'];
export type SemanticValue = components['schemas']['SemanticValue'];

/**
 * The person's own filters of one question, for this session only: `where`
 * before aggregating, on dimensions, and `having` after, on the question's
 * metrics. The author's intrinsic filters and the row policies are not here:
 * the server applies them, and no function of this module can name, add or
 * replace an identity, a role of authorization or a policy value (#142).
 */
export type UserFilters = { where: SemanticFilter | null; having: SemanticFilter | null };

export const NO_FILTERS: UserFilters = { where: null, having: null };

/** The server's bounds (`semantic/request.rs`), counted as it counts them. */
export const MAX_FILTER_NODES = 256;
export const MAX_FILTER_DEPTH = 16;
export const MAX_TEXT = 256;

/** FNV-1a (64 bits) of the UTF-8 bytes of `text`, as 16 hex digits. */
function fingerprint(text: string): string {
  let hash = 0xcbf29ce484222325n;
  for (const byte of new TextEncoder().encode(text)) {
    hash = ((hash ^ BigInt(byte)) * 0x100000001b3n) & 0xffffffffffffffffn;
  }
  return hash.toString(16).padStart(16, '0');
}

/**
 * A selection id stable by name and within the contract's bound: the name
 * itself when `prefix:name` fits; otherwise its start and a fingerprint of
 * the whole name, exactly `MAX_TEXT` characters. Ids the question would
 * repeat are refused by `selectionProblems`, never truncated into one.
 */
function stableId(prefix: 'm' | 'd', name: string): string {
  const id = `${prefix}:${name}`;
  if ([...id].length <= MAX_TEXT) return id;
  return `${prefix}:${[...name].slice(0, MAX_TEXT - 19).join('')}~${fingerprint(name)}`;
}

/** The selection id of a metric in a question: stable by its name, so a
 *  removed metric leaves its `having` an orphan instead of moving it. */
export const metricId = (name: string) => stableId('m', name);
/** The selection id of a dimension in a question, stable by its name. */
export const dimensionId = (name: string) => stableId('d', name);

/** Two selections of a question that would share an id: never sent. */
export function selectionProblems(
  metrics: readonly string[],
  dimensions: readonly string[],
): FilterProblem[] {
  const ids = [...metrics.map(metricId), ...dimensions.map(dimensionId)];
  return new Set(ids).size === ids.length
    ? []
    : [{ path: '/selections', message: 'Dos selecciones tendrían el mismo identificador.' }];
}

/** Why a filter cannot be sent, at a JSON pointer into the query. */
export type FilterProblem = { path: string; message: string };

const COMPARISONS = new Set(['=', '<>', '<', '<=', '>', '>=']);

/** The filters as the query sends them; nothing is dropped nor rewritten. */
export function filterQuery(filters: UserFilters): {
  filter?: SemanticFilter;
  having?: SemanticFilter;
} {
  return {
    ...(filters.where ? { filter: filters.where } : {}),
    ...(filters.having ? { having: filters.having } : {}),
  };
}

/**
 * Every reason `filters` cannot be sent for `metrics` of `model`: shape,
 * bounds, targets the session may filter, and literals at their target's
 * exact type. Empty means the server may still refuse: it authorizes.
 */
export function filterProblems(
  filters: UserFilters,
  model: CatalogModel,
  metrics: readonly string[],
): FilterProblem[] {
  const problems: FilterProblem[] = [];
  // One budget for both, as the server counts them.
  const count = { nodes: 0 };
  const check = (filter: SemanticFilter | null, path: string, after: boolean) => {
    if (filter !== null) walk(filter, path, 0, after, count, model, metrics, problems);
  };
  check(filters.where, '/filter', false);
  check(filters.having, '/having', true);
  return problems;
}

function walk(
  filter: SemanticFilter,
  path: string,
  depth: number,
  after: boolean,
  count: { nodes: number },
  model: CatalogModel,
  metrics: readonly string[],
  problems: FilterProblem[],
): void {
  count.nodes++;
  if (depth > MAX_FILTER_DEPTH || count.nodes > MAX_FILTER_NODES) {
    problems.push({
      path,
      message: `Un filtro tiene como máximo ${MAX_FILTER_NODES} condiciones y ${MAX_FILTER_DEPTH} niveles de anidamiento.`,
    });
    return;
  }
  const add = (at: string, message: string) => problems.push({ path: `${path}${at}`, message });
  switch (filter.op) {
    case 'and':
    case 'or':
      if (filter.filters.length === 0) add('', 'AND y OR necesitan al menos una condición.');
      filter.filters.forEach((child, i) =>
        walk(child, `${path}/filters/${i}`, depth + 1, after, count, model, metrics, problems),
      );
      return;
    case 'not':
      walk(filter.filter, `${path}/filter`, depth + 1, after, count, model, metrics, problems);
      return;
    case 'is_null':
    case 'is_not_null':
      target(filter.target, path, after, model, metrics, problems);
      return;
    case 'compare': {
      const member = target(filter.target, path, after, model, metrics, problems);
      if (!COMPARISONS.has(filter.comparison)) add('/comparison', 'Comparador desconocido.');
      if (member) literal(filter.value, member.type, `${path}/value`, problems);
      return;
    }
    case 'between': {
      const member = target(filter.target, path, after, model, metrics, problems);
      if (member) {
        literal(filter.low, member.type, `${path}/low`, problems);
        literal(filter.high, member.type, `${path}/high`, problems);
      }
      return;
    }
    case 'in': {
      const member = target(filter.target, path, after, model, metrics, problems);
      if (filter.values.length === 0 || filter.values.length > MAX_FILTER_NODES) {
        add('/values', `IN admite entre 1 y ${MAX_FILTER_NODES} valores.`);
      }
      if (member) {
        filter.values.forEach((value, i) =>
          literal(value, member.type, `${path}/values/${i}`, problems),
        );
      }
      return;
    }
  }
}

/** The member a target names, when the session may filter by it there. */
function target(
  target: SemanticTarget,
  path: string,
  after: boolean,
  model: CatalogModel,
  metrics: readonly string[],
  problems: FilterProblem[],
): CatalogMember | null {
  const fail = (message: string) => {
    problems.push({ path: `${path}/target`, message });
    return null;
  };
  if (target.kind === 'dimension') {
    if (after) return fail('Las dimensiones se filtran antes de agregar.');
    if (!sized(target.name) || !(target.roles ?? []).every(sized)) {
      return fail(`Los nombres tienen entre 1 y ${MAX_TEXT} caracteres.`);
    }
    // The same answer for a field that does not exist and one the session
    // may not filter: nothing tells them apart.
    const field = model.fields.find((f) => f.name === target.name);
    return field?.capabilities.filter ? field : fail('No se puede filtrar por este campo.');
  }
  if (!after) return fail('Las métricas se filtran después de agregar.');
  const name = metrics.find((m) => metricId(m) === target.selection);
  if (name === undefined) return fail('La métrica no está seleccionada en esta consulta.');
  const metric = model.metrics.find((m) => m.name === name);
  return metric?.capabilities.filter ? metric : fail('No se puede filtrar por esta métrica.');
}

const sized = (text: string) => text.length > 0 && [...text].length <= MAX_TEXT;

/** `value` at the exact type of its target, or why it is not. */
function literal(value: SemanticValue, type: string, path: string, problems: FilterProblem[]) {
  const fail = (message: string) => problems.push({ path, message });
  if (value.type === 'null') return fail('NULL no se compara: usa «es nulo» o «no es nulo».');
  const decimal = /^DECIMAL\((\d+),(\d+)\)$/.exec(type);
  if (type === 'INT64') {
    if (value.type !== 'number' || !integer(value.value)) {
      fail('El valor no es un entero de 64 bits.');
    }
  } else if (decimal) {
    if (value.type !== 'number' || !fits(value.value, Number(decimal[1]), Number(decimal[2]))) {
      fail(`El valor no cabe exacto en ${type}.`);
    }
  } else if (type === 'VARCHAR') {
    if (value.type !== 'text' || [...value.value].length > MAX_TEXT) {
      fail(`El valor es un texto de ${MAX_TEXT} caracteres como máximo.`);
    }
  } else if (type === 'BOOLEAN') {
    if (value.type !== 'boolean') fail('El valor es verdadero o falso.');
  } else if (type === 'DATE') {
    if ((value.type !== 'date' && value.type !== 'text') || !isoDate(value.value)) {
      fail('El valor no es una fecha AAAA-MM-DD válida.');
    }
  } else {
    // TIMESTAMPTZ and anything else: no literal until its contract exists.
    fail('Este campo todavía no se filtra por valor; usa «es nulo» o «no es nulo».');
  }
}

/** `(negative, integer digits, fraction digits)`, as the server reads numbers. */
function number(text: string): [boolean, string, string] | null {
  if ([...text].length === 0 || [...text].length > MAX_TEXT) return null;
  const negative = text.startsWith('-');
  const digits = negative ? text.slice(1) : text.startsWith('+') ? text.slice(1) : text;
  const [whole = '', fraction = '', ...rest] = digits.split('.');
  const valid =
    rest.length === 0 && (whole !== '' || fraction !== '') && /^\d*$/.test(whole + fraction);
  return valid ? [negative, whole, fraction] : null;
}

const MIN_INT64 = -(2n ** 63n);
const MAX_INT64 = 2n ** 63n - 1n;

function integer(text: string): boolean {
  const parts = number(text);
  if (!parts || /[1-9]/.test(parts[2])) return false;
  const value = BigInt(`${parts[0] ? '-' : ''}${parts[1] || '0'}`);
  return value >= MIN_INT64 && value <= MAX_INT64;
}

function fits(text: string, precision: number, scale: number): boolean {
  const parts = number(text);
  if (!parts) return false;
  const fraction = parts[2].replace(/0+$/, '');
  if (fraction.length > scale) return false;
  const coefficient = `${parts[1]}${fraction.padEnd(scale, '0')}`.replace(/^0+/, '');
  return coefficient.length <= precision;
}

function isoDate(text: string): boolean {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(text);
  if (!match) return false;
  const [year, month, day] = [Number(match[1]), Number(match[2]), Number(match[3])];
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][month - 1];
  return days !== undefined && day >= 1 && day <= days;
}

/** The filters of a session as text, to restore them later in it. */
export function saveFilters(filters: UserFilters): string {
  return JSON.stringify({ version: 1, ...filters });
}

/** Saved filters that do not have the contract's shape: none is kept. */
export class InvalidFilters extends Error {
  constructor() {
    super('Los filtros guardados no tienen la forma del contrato: no se restaura ninguno.');
  }
}

/**
 * Filters saved by `saveFilters`, exactly: every clause, NULL and text as
 * they were. Anything outside the contract's shape refuses the whole state;
 * no clause is dropped to keep the rest.
 */
export function restoreFilters(text: string): UserFilters {
  let saved: unknown;
  try {
    saved = JSON.parse(text);
  } catch {
    throw new InvalidFilters();
  }
  if (
    !record(saved, ['version', 'where', 'having']) ||
    saved.version !== 1 ||
    !(saved.where === null || isFilter(saved.where)) ||
    !(saved.having === null || isFilter(saved.having))
  ) {
    throw new InvalidFilters();
  }
  return { where: saved.where, having: saved.having };
}

/** `value` is an object with exactly `keys` (optional ones may be absent). */
function record(
  value: unknown,
  keys: string[],
  optional: string[] = [],
): value is Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return false;
  const present = Object.keys(value);
  return (
    keys.every((k) => present.includes(k)) &&
    present.every((k) => keys.includes(k) || optional.includes(k))
  );
}

function isFilter(value: unknown): value is SemanticFilter {
  if (
    !record(
      value,
      ['op'],
      ['target', 'comparison', 'value', 'low', 'high', 'values', 'filters', 'filter'],
    )
  ) {
    return false;
  }
  switch (value.op) {
    case 'compare':
      return (
        record(value, ['op', 'target', 'comparison', 'value']) &&
        isTarget(value.target) &&
        typeof value.comparison === 'string' &&
        COMPARISONS.has(value.comparison) &&
        isValue(value.value)
      );
    case 'between':
      return (
        record(value, ['op', 'target', 'low', 'high']) &&
        isTarget(value.target) &&
        isValue(value.low) &&
        isValue(value.high)
      );
    case 'in':
      return (
        record(value, ['op', 'target', 'values']) &&
        isTarget(value.target) &&
        Array.isArray(value.values) &&
        value.values.every(isValue)
      );
    case 'is_null':
    case 'is_not_null':
      return record(value, ['op', 'target']) && isTarget(value.target);
    case 'and':
    case 'or':
      return (
        record(value, ['op', 'filters']) &&
        Array.isArray(value.filters) &&
        value.filters.every(isFilter)
      );
    case 'not':
      return record(value, ['op', 'filter']) && isFilter(value.filter);
    default:
      return false;
  }
}

function isTarget(value: unknown): value is SemanticTarget {
  if (record(value, ['kind', 'name'], ['roles']) && value.kind === 'dimension') {
    return (
      typeof value.name === 'string' &&
      (value.roles === undefined ||
        (Array.isArray(value.roles) && value.roles.every((r) => typeof r === 'string')))
    );
  }
  return (
    record(value, ['kind', 'selection']) &&
    value.kind === 'metric' &&
    typeof value.selection === 'string'
  );
}

function isValue(value: unknown): value is SemanticValue {
  if (record(value, ['type']) && value.type === 'null') return true;
  if (!record(value, ['type', 'value'])) return false;
  switch (value.type) {
    case 'boolean':
      return typeof value.value === 'boolean';
    case 'number':
    case 'text':
    case 'date':
      return typeof value.value === 'string';
    default:
      return false;
  }
}
