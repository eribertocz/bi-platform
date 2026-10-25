import type { CatalogMember, CatalogModel } from '#lib/api/semantic.ts';
import {
  filterProblems,
  metricId,
  type FilterProblem,
  type SemanticFilter,
  type SemanticTarget,
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

/**
 * What a condition may name, and how: the fields of the rows before
 * aggregating (`where`), or the question's own metrics after (`having`).
 * The editor is one; only its domain changes (#149).
 */
export type Domain = {
  /** The filter's place in the query, where the core reports its problems. */
  at: '/filter' | '/having';
  /** The members a condition may name, by the name its draft writes. */
  members: CatalogMember[];
  /** The type of the member a draft names, offered or not; `''` if unknown. */
  type: (name: string) => string;
  /** The target a draft's name writes. */
  target: (name: string) => SemanticTarget;
  /** The name a draft writes for `target`; `null` when none writes it as it is. */
  name: (target: SemanticTarget) => string | null;
  /** Why `filter`, in its place, cannot be sent, as the core of #142 decides it. */
  problems: (filter: SemanticFilter) => FilterProblem[];
  /** How the editor names a member, an empty filter and its first actions. */
  words: {
    member: string;
    pick: string;
    none: string;
    add: string;
    group: string;
    another: string;
  };
};

/** Conditions on the rows, before aggregating: the fields the session may filter. */
export function rowsDomain(model: CatalogModel): Domain {
  return {
    at: '/filter',
    members: filterable(model),
    type: (name) => model.fields.find((f) => f.name === name)?.type ?? '',
    target: (name) => ({ kind: 'dimension', name }),
    name: (target) =>
      target.kind === 'dimension' && (target.roles ?? []).length === 0 ? target.name : null,
    problems: (filter) => filterProblems({ where: filter, having: null }, model, []),
    words: {
      member: 'Campo',
      pick: 'Elige un campo',
      none: 'Sin filtros: se consultan todos los datos que puedes ver.',
      add: 'Añadir condición',
      group: 'Añadir grupo',
      another: 'Añadir otra condición',
    },
  };
}

/**
 * Conditions on the question's metrics, after aggregating: those it selects
 * that the session may filter. A metric seen only withheld is never
 * selected, so never offered. A target names its selection by id, stable by
 * name: a metric removed leaves its condition naming it, never another.
 */
export function metricsDomain(model: CatalogModel, metrics: readonly string[]): Domain {
  return {
    at: '/having',
    members: metrics.flatMap((name) =>
      model.metrics.filter((m) => m.name === name && m.capabilities.filter),
    ),
    type: (name) => model.metrics.find((m) => m.name === name)?.type ?? '',
    target: (name) => ({ kind: 'metric', selection: metricId(name) }),
    name: (target) =>
      target.kind === 'metric'
        ? (metrics.find((m) => metricId(m) === target.selection) ?? null)
        : null,
    problems: (filter) => filterProblems({ where: null, having: filter }, model, metrics),
    words: {
      member: 'Métrica',
      pick: 'Elige una métrica',
      none: 'Sin filtros de métricas: se muestran todos los totales y grupos calculados.',
      add: 'Añadir condición de métrica',
      group: 'Añadir grupo de métricas',
      another: 'Añadir otra condición de métrica',
    },
  };
}

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
 * The condition `draft` writes in `domain`: exactly its member, operator and
 * values. An unknown member is kept by name, never replaced by another.
 */
export function condition(draft: Draft, domain: Domain): SemanticFilter {
  const type = domain.type(draft.field);
  const target = domain.target(draft.field);
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
 * draft of `domain` cannot write it as it is (a group, a target of another
 * domain, a NULL compared), so it is never edited into a different one.
 */
export function draftOf(filter: SemanticFilter, domain: Domain): Draft | null {
  if (!('target' in filter)) return null;
  const field = domain.name(filter.target);
  if (field === null) return null;
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
  return values ? { field, operator, values } : null;
}

/** What the editor reports on every change: the draft as written, the
 *  condition it writes and why that condition cannot be sent, if so. */
export type ConditionChange = {
  draft: Draft;
  condition: SemanticFilter;
  problems: FilterProblem[];
};
