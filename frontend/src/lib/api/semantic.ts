import type { components } from './generated.ts';
import { apiRequest } from './session.ts';

export type SemanticCatalog = components['schemas']['SemanticCatalog'];
export type CatalogModel = components['schemas']['CatalogModel'];
export type CatalogMember = components['schemas']['CatalogMember'];
export type SemanticRequest = components['schemas']['ExplainRequest'];
export type SemanticRun = components['schemas']['SemanticRun'];

/** The server's own budget plus a margin: the client never gives up first. */
const QUERY_TIMEOUT_MS = 15_000;

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

function isRun(value: unknown): value is SemanticRun {
  return (
    isObject(value) &&
    isObject(value.result) &&
    Array.isArray(value.result.blocks) &&
    isObject(value.receipt) &&
    Array.isArray(value.notices)
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
  if (!isCatalog(body)) throw new Error('La API devolvió un catálogo inválido.');
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
  if (!isRun(body)) throw new Error('La API devolvió una respuesta semántica inválida.');
  return body;
}
