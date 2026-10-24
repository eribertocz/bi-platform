import type { components } from './generated.ts';
import { apiRequest } from './session.ts';

export type SemanticCatalog = components['schemas']['SemanticCatalog'];
export type CatalogModel = components['schemas']['CatalogModel'];
export type CatalogMember = components['schemas']['CatalogMember'];
export type SemanticRequest = components['schemas']['ExplainRequest'];
export type SemanticRun = components['schemas']['SemanticRun'];

/** An answer that arrived but does not fit its contract. */
export class InvalidAnswer extends Error {
  constructor() {
    super('La API devolvió una respuesta que no corresponde a su contrato.');
    this.name = 'InvalidAnswer';
  }
}

/**
 * A ceiling for a request the server never answered; the server ends every
 * query within its own configurable budget long before.
 */
const QUERY_TIMEOUT_MS = 60_000;

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null;

function isMember(value: unknown): value is CatalogMember {
  return (
    isObject(value) &&
    typeof value.name === 'string' &&
    typeof value.type === 'string' &&
    isObject(value.capabilities) &&
    (value.capabilities.select === 'values' || value.capabilities.select === 'masked') &&
    isObject(value.lifecycle)
  );
}

function isCatalog(value: unknown): value is SemanticCatalog {
  return (
    isObject(value) &&
    (value.status === 'ready' || value.status === 'not_configured') &&
    Array.isArray(value.models) &&
    value.models.every(
      (model: unknown) =>
        isObject(model) &&
        typeof model.model === 'string' &&
        typeof model.name === 'string' &&
        Array.isArray(model.metrics) &&
        model.metrics.every(isMember) &&
        Array.isArray(model.fields) &&
        model.fields.every(isMember),
    )
  );
}

const strings = (value: unknown, keys: string[]) =>
  isObject(value) && keys.every((key) => typeof value[key] === 'string');

/** The receipt as the page shows it, field by field: nothing is read unchecked. */
function isReceipt(value: unknown): boolean {
  return (
    strings(value, ['id', 'version', 'evidence', 'read_started_at', 'read_finished_at']) &&
    isObject(value) &&
    strings(value.fingerprints, ['query', 'model', 'plan', 'decision', 'policies', 'context']) &&
    strings(value.query, ['encoding', 'sqir']) &&
    strings(value.versions, ['engine', 'adapter', 'capability_profile', 'tzdb']) &&
    Array.isArray(value.definitions) &&
    value.definitions.every((d: unknown) => strings(d, ['id', 'content'])) &&
    Array.isArray(value.sources) &&
    value.sources.every(
      (s: unknown) =>
        strings(s, ['dataset']) &&
        isObject(s) &&
        (s.data_snapshot === null || typeof s.data_snapshot === 'string') &&
        (s.source_refreshed_at === null || typeof s.source_refreshed_at === 'string'),
    )
  );
}

function isRun(value: unknown): value is SemanticRun {
  return (
    isObject(value) &&
    isObject(value.result) &&
    Array.isArray(value.result.blocks) &&
    isReceipt(value.receipt) &&
    Array.isArray(value.notices) &&
    value.notices.every(
      (n: unknown) =>
        strings(n, ['metric']) &&
        isObject(n) &&
        (n.sunset === null || typeof n.sunset === 'string') &&
        (n.replacement === null || typeof n.replacement === 'string'),
    )
  );
}

/** What the session may use of the project (#140). */
export async function listCatalog(
  signal: AbortSignal,
  request: typeof fetch = fetch,
): Promise<SemanticCatalog> {
  const response = await apiRequest(
    '/api/v1/semantic/catalog',
    { signal },
    request,
    QUERY_TIMEOUT_MS,
  );
  const body: unknown = await response.json();
  if (!isCatalog(body)) throw new InvalidAnswer();
  return body;
}

/** Executes one semantic query (#138); its cells are checked by the presentation. */
export async function runSemantic(
  query: SemanticRequest,
  signal: AbortSignal,
  request: typeof fetch = fetch,
): Promise<SemanticRun> {
  const response = await apiRequest(
    '/api/v1/semantic/query',
    {
      method: 'POST',
      signal,
      headers: {
        Accept: 'application/json, application/problem+json',
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(query),
    },
    request,
    QUERY_TIMEOUT_MS,
  );
  const body: unknown = await response.json();
  if (!isRun(body)) throw new InvalidAnswer();
  return body;
}
