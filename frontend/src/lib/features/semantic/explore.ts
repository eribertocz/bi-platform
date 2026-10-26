import { ApiError } from '#lib/api/session.ts';
import { checkBlock, IncompatibleResult, totals } from './exact';
import { dimensionId, filterQuery, metricId } from './filters';
import {
  InvalidAnswer,
  runSemantic,
  type SemanticRequest,
  type SemanticRun,
} from '#lib/api/semantic.ts';
import type { Choice } from './question';

export * from './question';

/**
 * The requests of a question: the total without a grain (the KPIs) and, with
 * a breakdown, the grouped table; and the label of every selection id. Ids
 * are stable by name, so a filter's reference never moves to another
 * metric. The person's filters are the question's: both reads carry them,
 * each at its own grain.
 */
export function requests(choice: Choice) {
  const metrics = choice.metrics.map((name) => ({
    id: metricId(name),
    kind: 'metric' as const,
    name,
  }));
  const dimensions = choice.dimensions.map((name) => ({
    id: dimensionId(name),
    kind: 'dimension' as const,
    name,
  }));
  const labels = Object.fromEntries([...metrics, ...dimensions].map((s) => [s.id, s.name]));
  const filters = choice.filters ? filterQuery(choice.filters) : {};
  const kpi: SemanticRequest = { model: choice.model, query: { selections: metrics, ...filters } };
  const table: SemanticRequest | null =
    dimensions.length === 0
      ? null
      : { model: choice.model, query: { selections: [...dimensions, ...metrics], ...filters } };
  return { kpi, table, labels };
}

/** Whether `choice` may legitimately get no total: it filters after aggregating. */
export const emptyTotalAllowed = (choice: Choice) => Boolean(choice.filters?.having);

/** One answered question: each request with its own receipt. */
export type Answer = { kpi: SemanticRun; table: SemanticRun | null };

/**
 * Answers `choice` one request after the other, never at once: the person's
 * quota is shared by every request. Each answer is checked whole, with the
 * very validators of its presentation, before anything is published: the
 * first failure or incompatible answer ends the attempt, and nothing of it
 * is shown.
 */
export async function answer(
  choice: Choice,
  signal: AbortSignal,
  run: typeof runSemantic = runSemantic,
): Promise<Answer> {
  const { kpi, table } = requests(choice);
  const total = await run(kpi, signal);
  signal.throwIfAborted();
  // One question, one block each: anything else is shown nowhere. An
  // invalid total ends the attempt before the breakdown is asked.
  if (total.result.blocks.length !== 1) throw new IncompatibleResult();
  // A total excluded by a filter after aggregating is an answer, and the
  // breakdown is still asked: it is checked on its own, never inferred.
  totals(total, emptyTotalAllowed(choice));
  const grouped = table && (await run(table, signal));
  signal.throwIfAborted();
  if (grouped) {
    if (grouped.result.blocks.length !== 1) throw new IncompatibleResult();
    checkBlock(grouped.result.blocks[0]!);
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
              : error.code === 'result_out_of_range'
                ? 'Un total es demasiado grande para su tipo numérico. Filtra o desglosa para obtener cifras menores.'
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
