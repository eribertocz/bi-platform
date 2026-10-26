import type { CatalogModel, SemanticCatalog } from '#lib/api/semantic.ts';
import { filterProblems, selectionProblems, type FilterProblem, type UserFilters } from './filters';

// What a person asks in Explorar, as data: pure, with no request nor any
// transport, so state built on it (the drill path, #267) needs no HTTP.

/** An interaction limit of this page (#147); the server keeps its own budgets. */
export const MAX_METRICS = 20;
/** From how many metrics on the page suggests removing some, without blocking. */
export const MANY_METRICS = 10;
export const MAX_DIMENSIONS = 2;

/**
 * What the person asks: one model, 1 to 20 metrics, 0 to 2 fields to break
 * them down by, and their own filters of this session, if any (#142).
 */
export type Choice = {
  model: string;
  metrics: string[];
  dimensions: string[];
  filters?: UserFilters;
};

/** The fields a question may break down by: those the session may group. */
export const groupable = (model: CatalogModel) =>
  model.fields.filter((field) => field.capabilities.group);

/**
 * The metrics a question may select: those whose values the session may
 * see. A metric seen only withheld is announced and marked, but execution
 * refuses it whole (`masked_unsupported`), so it is never sent.
 */
export const selectable = (model: CatalogModel) =>
  model.metrics.filter((metric) => metric.capabilities.select === 'values');

/** A first question on `model`: its first selectable metric, no breakdown. */
function startOn(model: CatalogModel): Choice | null {
  const metric = selectable(model)[0];
  return metric ? { model: model.model, metrics: [metric.name], dimensions: [] } : null;
}

/** A first question on the catalog: the first model with a selectable metric. */
export function initialChoice(catalog: SemanticCatalog): Choice | null {
  for (const model of catalog.models) {
    const choice = startOn(model);
    if (choice) return choice;
  }
  return null;
}

/**
 * `choice` on a refreshed catalog: what is no longer selectable is dropped;
 * a model left without metrics starts over on itself, and only a model that
 * is gone starts over on the catalog. The person's filters stay as they
 * were: one that names what is gone becomes a problem to see, never a
 * clause removed to ask something else (#142).
 */
export function reconcile(choice: Choice | null, catalog: SemanticCatalog): Choice | null {
  if (!choice) return initialChoice(catalog);
  // A question with filters never starts over by itself: it stays, blocked,
  // until the person resolves it (#142). One without keeps doing so.
  const filtered = hasFilters(choice);
  const model = catalog.models.find((m) => m.model === choice.model);
  if (!model) return filtered ? choice : initialChoice(catalog);
  const metrics = choice.metrics.filter((name) => selectable(model).some((m) => m.name === name));
  const dimensions = choice.dimensions.filter((name) =>
    groupable(model).some((f) => f.name === name),
  );
  if (metrics.length > 0 || filtered) {
    return {
      model: model.model,
      metrics,
      dimensions,
      ...(filtered ? { filters: choice.filters } : {}),
    };
  }
  return startOn(model) ?? initialChoice(catalog);
}

/** Whether `choice` carries any filter of the person. */
const hasFilters = (choice: Choice) => Boolean(choice.filters?.where || choice.filters?.having);

/**
 * The same question with the person's `filters`, before and after
 * aggregating, as one: checked together, on one budget (`questionProblems`).
 * Without any, the question carries none.
 */
export function withFilters(choice: Choice, filters: UserFilters): Choice {
  const rest = { model: choice.model, metrics: choice.metrics, dimensions: choice.dimensions };
  return filters.where || filters.having ? { ...rest, filters } : rest;
}

/**
 * Why `choice` cannot be sent on `model`: selections that would share an id,
 * or the person's filters, checked only against the question's own model.
 */
export function questionProblems(choice: Choice, model: CatalogModel): FilterProblem[] {
  if (model.model !== choice.model) {
    return [{ path: '/model', message: 'El modelo de esta pregunta ya no está en tu catálogo.' }];
  }
  return [
    ...selectionProblems(choice.metrics, choice.dimensions),
    ...(choice.filters ? filterProblems(choice.filters, model, choice.metrics) : []),
  ];
}

/**
 * Problems of `choice` that its filters have only together, such as the
 * budget both share: each editor checks its own alone and cannot mark them.
 */
export function jointProblems(choice: Choice, model: CatalogModel): FilterProblem[] {
  const alone = choice.filters
    ? [
        ...filterProblems({ where: choice.filters.where, having: null }, model, choice.metrics),
        ...filterProblems({ where: null, having: choice.filters.having }, model, choice.metrics),
      ]
    : [];
  const key = (p: FilterProblem) => `${p.path}\n${p.message}`;
  const marked = new Set(alone.map(key));
  return questionProblems(choice, model).filter((p) => !marked.has(key(p)));
}

/**
 * Whether `choice` is a question the page may send: its filters, if any,
 * valid on `model`, which they need to be checked against.
 */
export const askable = (choice: Choice | null, model?: CatalogModel): choice is Choice =>
  choice !== null &&
  choice.metrics.length >= 1 &&
  choice.metrics.length <= MAX_METRICS &&
  choice.dimensions.length <= MAX_DIMENSIONS &&
  selectionProblems(choice.metrics, choice.dimensions).length === 0 &&
  (!hasFilters(choice) || (model !== undefined && questionProblems(choice, model).length === 0));
