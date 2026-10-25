import type { CatalogMember, CatalogModel } from '#lib/api/semantic.ts';
import {
  filterProblems,
  type FilterProblem,
  type SemanticFilter,
  type SemanticValue,
} from './filters';

/** What one condition can ask of a field. */
export type Operator =
  '=' | '<>' | '<' | '<=' | '>' | '>=' | 'between' | 'in' | 'is_null' | 'is_not_null';

/**
 * The condition as the person writes it: a field, an operator and the text
 * of every value typed so far. Values the operator does not use are kept,
 * so changing it back loses nothing (#148).
 */
export type Draft = { field: string; operator: Operator; values: string[] };

const COMPARISONS = ['=', '<>', '<', '<=', '>', '>='] as const;
const NULLS = ['is_null', 'is_not_null'] as const;

/** The operators a field of this type admits, as the server binds them. */
export function operators(member: CatalogMember): Operator[] {
  if (member.type === 'TIMESTAMPTZ') return [...NULLS];
  if (member.type === 'BOOLEAN') return ['=', '<>', ...NULLS];
  return [...COMPARISONS, 'between', 'in', ...NULLS];
}

/** How each operator reads, for the editor and the summary of a filter. */
export const OPERATOR_LABELS: Record<Operator, string> = {
  '=': 'es igual a',
  '<>': 'es distinto de',
  '<': 'es menor que',
  '<=': 'es menor o igual que',
  '>': 'es mayor que',
  '>=': 'es mayor o igual que',
  between: 'está entre',
  in: 'es uno de',
  is_null: 'no tiene valor (nulo)',
  is_not_null: 'tiene valor (no nulo)',
};

/** The fields a condition may name: those the session may filter. */
export const filterable = (model: CatalogModel) =>
  model.fields.filter((f) => f.capabilities.filter);

/** How many values `operator` uses; `in` uses every one written. */
export function arity(operator: Operator, values: readonly string[]): number {
  if (operator === 'is_null' || operator === 'is_not_null') return 0;
  if (operator === 'between') return 2;
  if (operator === 'in') return Math.max(values.length, 1);
  return 1;
}

/** `text` as a literal for `type`, exactly: never a Number, never coerced. */
function literal(text: string, type: string): SemanticValue {
  if (type === 'INT64' || type.startsWith('DECIMAL(')) return { type: 'number', value: text };
  if (type === 'DATE') return { type: 'date', value: text };
  if (type === 'BOOLEAN' && (text === 'true' || text === 'false')) {
    return { type: 'boolean', value: text === 'true' };
  }
  // A boolean written otherwise stays text: the core refuses it as such.
  return { type: 'text', value: text };
}

/** The literal's text as the person wrote it; NULL has none. */
function written(value: SemanticValue): string | null {
  if (value.type === 'null') return null;
  return value.type === 'boolean' ? String(value.value) : value.value;
}

/**
 * The condition `draft` writes on `model`: exactly its field, operator and
 * values. An unknown field is kept by name, never replaced by another.
 */
export function condition(draft: Draft, model: CatalogModel): SemanticFilter {
  const type = model.fields.find((f) => f.name === draft.field)?.type ?? '';
  const target = { kind: 'dimension' as const, name: draft.field };
  const value = (i: number) => literal(draft.values[i] ?? '', type);
  switch (draft.operator) {
    case 'is_null':
    case 'is_not_null':
      return { op: draft.operator, target };
    case 'between':
      return { op: 'between', target, low: value(0), high: value(1) };
    case 'in':
      return {
        op: 'in',
        target,
        values: Array.from({ length: arity('in', draft.values) }, (_, i) => value(i)),
      };
    default:
      return { op: 'compare', target, comparison: draft.operator, value: value(0) };
  }
}

/**
 * A condition back as the draft that writes it, for editing; `null` when a
 * draft cannot write it as it is (a group, a metric, a NULL compared), so
 * it is never edited into a different condition.
 */
export function draftOf(filter: SemanticFilter): Draft | null {
  if (!('target' in filter) || filter.target.kind !== 'dimension') return null;
  const field = filter.target.name;
  const texts = (values: SemanticValue[]) => {
    const all = values.map(written);
    return all.every((v): v is string => v !== null) ? all : null;
  };
  let values: string[] | null;
  let operator: Operator;
  switch (filter.op) {
    case 'compare':
      [operator, values] = [filter.comparison, texts([filter.value])];
      break;
    case 'between':
      [operator, values] = ['between', texts([filter.low, filter.high])];
      break;
    case 'in':
      [operator, values] = ['in', texts(filter.values)];
      break;
    default:
      [operator, values] = [filter.op, []];
  }
  return values && (filter.target.roles ?? []).length === 0 ? { field, operator, values } : null;
}

/** What the editor reports on every change: the draft as written, the
 *  condition it writes and why that condition cannot be sent, if so. */
export type ConditionChange = {
  draft: Draft;
  condition: SemanticFilter;
  problems: FilterProblem[];
};

/** Why the condition cannot be sent, as the core of #142 decides it. */
export const conditionProblems = (filter: SemanticFilter, model: CatalogModel): FilterProblem[] =>
  filterProblems({ where: filter, having: null }, model, []);
