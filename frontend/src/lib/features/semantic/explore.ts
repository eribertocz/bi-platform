import { ApiError } from '#lib/api/session.ts';
import { IncompatibleResult } from './exact';
import {
  InvalidAnswer,
  runSemantic,
  type CatalogModel,
  type SemanticCatalog,
  type SemanticRequest,
  type SemanticRun,
} from '#lib/api/semantic.ts';

/** An interaction limit of this page (#147); the server keeps its own budgets. */
export const MAX_METRICS = 20;
/** From how many metrics on the page suggests removing some, without blocking. */
export const MANY_METRICS = 10;
export const MAX_DIMENSIONS = 2;

/** What the person asks: one model, 1 to 20 metrics, 0 to 2 fields to break them down by. */
export type Choice = { model: string; metrics: string[]; dimensions: string[] };

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
 * is gone starts over on the catalog.
 */
export function reconcile(choice: Choice | null, catalog: SemanticCatalog): Choice | null {
  const model = catalog.models.find((m) => m.model === choice?.model);
  if (!choice || !model) return initialChoice(catalog);
  const metrics = choice.metrics.filter((name) => selectable(model).some((m) => m.name === name));
  const dimensions = choice.dimensions.filter((name) =>
    groupable(model).some((f) => f.name === name),
  );
  return metrics.length > 0
    ? { model: model.model, metrics, dimensions }
    : (startOn(model) ?? initialChoice(catalog));
}

/** Whether `choice` is a question the page may send. */
export const askable = (choice: Choice | null): choice is Choice =>
  choice !== null &&
  choice.metrics.length >= 1 &&
  choice.metrics.length <= MAX_METRICS &&
  choice.dimensions.length <= MAX_DIMENSIONS;

/**
 * The requests of a question: the total without a grain (the KPIs) and, with
 * a breakdown, the grouped table; and the label of every selection id.
 */
export function requests(choice: Choice) {
  const metrics = choice.metrics.map((name, i) => ({
    id: `m${i}`,
    kind: 'metric' as const,
    name,
  }));
  const dimensions = choice.dimensions.map((name, i) => ({
    id: `d${i}`,
    kind: 'dimension' as const,
    name,
  }));
  const labels = Object.fromEntries([...metrics, ...dimensions].map((s) => [s.id, s.name]));
  const kpi: SemanticRequest = { model: choice.model, query: { selections: metrics } };
  const table: SemanticRequest | null =
    dimensions.length === 0
      ? null
      : { model: choice.model, query: { selections: [...dimensions, ...metrics] } };
  return { kpi, table, labels };
}

/** One answered question: each request with its own receipt. */
export type Answer = { kpi: SemanticRun; table: SemanticRun | null };

/**
 * Answers `choice` one request after the other, never at once: the person's
 * quota is shared by every request. The first failure ends the attempt.
 */
export async function answer(
  choice: Choice,
  signal: AbortSignal,
  run: typeof runSemantic = runSemantic,
): Promise<Answer> {
  const { kpi, table } = requests(choice);
  const total = await run(kpi, signal);
  signal.throwIfAborted();
  const grouped = table && (await run(table, signal));
  signal.throwIfAborted();
  // One question, one block each: anything else is shown nowhere.
  if (total.result.blocks.length !== 1 || (grouped && grouped.result.blocks.length !== 1)) {
    throw new IncompatibleResult();
  }
  return { kpi: total, table: grouped };
}

/**
 * How the two reads of an answer relate: one read, two separate reads, or
 * two reads that saw different versions of a source. Never one snapshot.
 */
export function consistency(answer: Answer): 'single' | 'separate' | 'diverged' {
  if (!answer.table) return 'single';
  const [kpi, table] = [answer.kpi.receipt, answer.table.receipt];
  const seen = new Map(kpi.sources.map((s) => [s.dataset, s.data_snapshot]));
  // The model's and the policies' fingerprints are comparable within one
  // instance and one principal: exactly the two reads of one page.
  const differs =
    kpi.versions.engine !== table.versions.engine ||
    kpi.fingerprints.model !== table.fingerprints.model ||
    kpi.fingerprints.policies !== table.fingerprints.policies ||
    table.sources.some((s) => seen.has(s.dataset) && seen.get(s.dataset) !== s.data_snapshot);
  return differs ? 'diverged' : 'separate';
}

/** What the page tells about a failure, and what it offers next. */
export type Failure = {
  message: string;
  status?: number;
  code?: string;
  requestId?: string;
  /** The same question may succeed again. */
  retry: boolean;
  /** The question is no longer usable: the catalog should be refreshed. */
  gone: boolean;
};

export function failure(error: unknown): Failure {
  if (error instanceof IncompatibleResult || error instanceof InvalidAnswer) {
    return { message: error.message, retry: false, gone: false };
  }
  if (error instanceof DOMException && error.name === 'TimeoutError') {
    return {
      message: 'La consulta no respondió a tiempo. Reintenta en unos segundos.',
      retry: true,
      gone: false,
    };
  }
  if (!(error instanceof ApiError)) {
    return {
      message: 'No se pudo consultar la API. Revisa la conexión.',
      retry: true,
      gone: false,
    };
  }
  const known = { status: error.status, code: error.code, requestId: error.requestId };
  switch (error.status) {
    case 401:
      return { ...known, message: 'Tu sesión terminó.', retry: false, gone: false };
    case 403:
      return {
        ...known,
        message: 'Tus permisos no permiten esa consulta. Cambia la selección.',
        retry: false,
        gone: false,
      };
    case 404:
      return {
        ...known,
        message: 'Esta consulta ya no está disponible para tu sesión. Actualiza el catálogo.',
        retry: false,
        gone: true,
      };
    case 422:
      return {
        ...known,
        message:
          error.code === 'masked_unsupported'
            ? 'La selección incluye valores que solo puedes ver ocultos, y todavía no se pueden consultar.'
            : error.code === 'result_too_large'
              ? 'El resultado supera el tamaño máximo. Usa menos campos para desglosar.'
              : 'Esta combinación no se puede consultar en el modelo. Cambia la selección.',
        retry: false,
        gone: false,
      };
    case 429:
      return {
        ...known,
        message: 'El servidor está ocupado. Reintenta en unos segundos.',
        retry: true,
        gone: false,
      };
    case 504:
      return {
        ...known,
        message: 'La consulta superó el tiempo máximo.',
        retry: true,
        gone: false,
      };
    default:
      return {
        ...known,
        message: 'El servicio no está disponible en este momento.',
        retry: true,
        gone: false,
      };
  }
}
