import type { CatalogModel } from '#lib/api/semantic.ts';
import { checkBlock, IncompatibleResult, type SemanticBlock } from './exact';
import { same } from './filterTree';
import {
  dimensionId,
  metricId,
  NO_FILTERS,
  type FilterProblem,
  type SemanticFilter,
  type SemanticValue,
} from './filters';
import { askable, groupable, questionProblems, withFilters, type Choice } from './question';

/**
 * Whose questions a path holds: one principal, on one model. A path is
 * never used under another (#267); the page resets it explicitly.
 */
export type PathContext = { principal: string; model: string };

/** One step taken: by which dimension, at which value of it, to which one. */
export type Step = {
  from: string;
  /** The literal chosen from the raw cell; `null` when the cell was NULL. */
  value: SemanticValue | null;
  to: string;
};

/**
 * A question left by a step, and the step taken from it. Only intention:
 * never rows, blocks, receipts nor any copy of an answer.
 */
export type Frame = { question: Choice; step: Step };

/** The questions a person drilled from, oldest first; the current one is not here. */
export type DrillPath = PathContext & { frames: readonly Frame[] };

/** A cell of an answer: the question that was answered, its block, a row and a column. */
export type DrillCell = {
  asked: Choice;
  block: SemanticBlock;
  row: number;
  selection: string;
};

/** Why a step or a return cannot be taken; nothing of the question or the path changed. */
export type DrillRefusal =
  | { reason: 'context' }
  | { reason: 'stale' }
  | { reason: 'unaskable' }
  | { reason: 'cell' }
  | { reason: 'origin' }
  | { reason: 'target' }
  | { reason: 'value' }
  | { reason: 'question'; problems: FilterProblem[] }
  | { reason: 'no_frame' };

export type Drilled =
  { ok: true; question: Choice; path: DrillPath } | ({ ok: false } & DrillRefusal);

/** A path with no step yet, for `context`: on a new model, session or catalog. */
export const startPath = (context: PathContext): DrillPath => ({ ...context, frames: [] });

const refuse = (refusal: DrillRefusal): Drilled => ({ ok: false, ...refusal });
const copy = <T>(value: T): T => structuredClone(value);
const sameContext = (path: DrillPath, context: PathContext) =>
  path.principal === context.principal && path.model === context.model;

/**
 * The literal of a raw cell for its column's type, exactly as the contract
 * carries it; `undefined` when the wire cannot carry it faithfully.
 */
export function literal(type: string, cell: unknown): SemanticValue | undefined {
  if (type === 'BOOLEAN')
    return typeof cell === 'boolean' ? { type: 'boolean', value: cell } : undefined;
  if (typeof cell !== 'string') return undefined;
  if (type === 'INT64' || type.startsWith('DECIMAL(')) return { type: 'number', value: cell };
  if (type === 'VARCHAR') return { type: 'text', value: cell };
  if (type === 'DATE') return { type: 'date', value: cell };
  // TIMESTAMPTZ and anything else: no literal until its contract exists.
  return undefined;
}

/**
 * The field of `current`'s dimension `name` when a step may start there: one
 * the question breaks down by, whose values the session sees.
 */
export function origin(current: Choice, model: CatalogModel, name: string) {
  const field = current.dimensions.includes(name)
    ? model.fields.find((f) => f.name === name)
    : undefined;
  return field?.capabilities.select === 'values' ? field : undefined;
}

/** Where a step from `from` may go: fields the session may group by, not asked yet. */
export const targets = (current: Choice, model: CatalogModel, from: string): string[] =>
  groupable(model)
    .map((field) => field.name)
    .filter((name) => name !== from && !current.dimensions.includes(name));

/**
 * The question one step below `current`, and the path that remembers it:
 * the origin dimension's value of `cell` is added by AND to the person's
 * filters, and that dimension is replaced by `to` in place. Any other
 * dimension, the metrics, `having` and order stay as they were; no other
 * value of the row is used. The whole question is checked as it would be
 * sent (#142, #149); anything that does not fit is refused, never trimmed.
 * No request is made.
 */
export function drill(
  current: Choice,
  path: DrillPath,
  context: PathContext,
  model: CatalogModel,
  cell: DrillCell,
  to: string,
): Drilled {
  if (!sameContext(path, context) || current.model !== context.model) {
    return refuse({ reason: 'context' });
  }
  if (model.model !== current.model) return refuse({ reason: 'context' });
  // Only from a question the page may send: nothing invalid is built upon.
  if (!askable(current, model)) return refuse({ reason: 'unaskable' });
  // Only a cell of the answer to this very question.
  if (!same(cell.asked, current)) return refuse({ reason: 'stale' });
  let block: SemanticBlock;
  try {
    block = checkBlock(cell.block);
  } catch (error) {
    if (error instanceof IncompatibleResult) return refuse({ reason: 'cell' });
    throw error;
  }
  // The block answers this question: exactly its selections, each of its kind.
  const selected = [
    ...current.dimensions.map((name) => `dimension ${dimensionId(name)}`),
    ...current.metrics.map((name) => `metric ${metricId(name)}`),
  ].sort();
  const columns = block.columns.map((c) => `${c.kind} ${c.selection_id}`).sort();
  if (!same(columns, selected)) return refuse({ reason: 'cell' });
  const column = block.columns.findIndex((c) => c.selection_id === cell.selection);
  const row = block.rows[cell.row];
  if (column < 0 || !row) return refuse({ reason: 'cell' });
  const declared = block.columns[column]!;
  // A dimension the question breaks down by, its value not withheld by the
  // answer nor by the catalog, and of its field's very type.
  const from = current.dimensions.find((name) => dimensionId(name) === cell.selection);
  const field = from === undefined ? undefined : origin(current, model, from);
  if (from === undefined || !field || declared.kind !== 'dimension' || declared.masked) {
    return refuse({ reason: 'origin' });
  }
  if (declared.type !== field.type) return refuse({ reason: 'cell' });
  // A field of the same model the session may group by, not asked already.
  if (!targets(current, model, from).includes(to)) return refuse({ reason: 'target' });
  const raw = row[column] ?? null;
  const value = raw === null ? null : literal(declared.type, raw);
  if (value === undefined) return refuse({ reason: 'value' });
  // The selection's own path: its name, no role it does not carry.
  const target = { kind: 'dimension' as const, name: from };
  const predicate: SemanticFilter =
    value === null ? { op: 'is_null', target } : { op: 'compare', target, comparison: '=', value };
  const filters = current.filters ?? NO_FILTERS;
  const where: SemanticFilter = !filters.where
    ? predicate
    : filters.where.op === 'and'
      ? { op: 'and', filters: [...filters.where.filters, predicate] }
      : { op: 'and', filters: [filters.where, predicate] };
  const next = withFilters(
    { ...current, dimensions: current.dimensions.map((name) => (name === from ? to : name)) },
    { where, having: filters.having },
  );
  // Replaced in place, the selections keep their count: only the filters grow.
  const problems = questionProblems(next, model);
  if (problems.length > 0) return refuse({ reason: 'question', problems });
  const frame: Frame = { question: copy(current), step: { from, value: copy(value), to } };
  return {
    ok: true,
    question: copy(next),
    path: { ...context, frames: [...path.frames.map(copy), frame] },
  };
}

/**
 * The question of frame `index` exactly as it was left, and the path up to
 * it: every step below is discarded. `back` is the last frame.
 */
export function returnTo(path: DrillPath, context: PathContext, index: number): Drilled {
  if (!sameContext(path, context)) return refuse({ reason: 'context' });
  const frame = path.frames[index];
  if (!Number.isInteger(index) || index < 0 || !frame) return refuse({ reason: 'no_frame' });
  return {
    ok: true,
    question: copy(frame.question),
    path: { ...context, frames: path.frames.slice(0, index).map(copy) },
  };
}

/** One step back: the question the last step was taken from. */
export const back = (path: DrillPath, context: PathContext): Drilled =>
  returnTo(path, context, path.frames.length - 1);
