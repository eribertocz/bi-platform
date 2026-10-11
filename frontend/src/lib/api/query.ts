import type { components } from './generated';
export type QueryRequest = components['schemas']['QueryRequest'];
export type QueryResult = components['schemas']['QueryResult'];
export type Problem = components['schemas']['Problem'];
export class QueryError extends Error {
  constructor(public problem: Problem) {
    super(problem.detail ?? problem.title);
  }
}
function isProblem(body: unknown): body is Problem {
  if (!body || typeof body !== 'object') return false;
  const p = body as Partial<Problem>;
  return (
    typeof p.code === 'string' &&
    typeof p.request_id === 'string' &&
    typeof p.title === 'string' &&
    typeof p.type === 'string' &&
    typeof p.status === 'number' &&
    (p.detail == null || typeof p.detail === 'string')
  );
}
function isResult(body: unknown): body is QueryResult {
  if (!body || typeof body !== 'object') return false;
  const r = body as Partial<QueryResult>;
  if (
    !Array.isArray(r.columns) ||
    !Array.isArray(r.rows) ||
    r.row_count !== r.rows.length ||
    typeof r.truncated !== 'boolean'
  )
    return false;
  const columns = r.columns;
  if (
    !columns.every(
      (c) =>
        c &&
        typeof c.name === 'string' &&
        typeof c.nullable === 'boolean' &&
        ['string', 'integer', 'decimal', 'timestamptz'].includes(c.type) &&
        (c.type !== 'decimal' || (Number.isInteger(c.precision) && Number.isInteger(c.scale))),
    )
  )
    return false;
  return r.rows.every(
    (row) =>
      Array.isArray(row) &&
      row.length === columns.length &&
      row.every((v, i) => {
        const column = columns[i];
        if (!column) return false;
        return v === null
          ? column.nullable
          : typeof v === 'string' &&
              (column.type === 'integer'
                ? /^-?\d+$/.test(v)
                : column.type === 'decimal'
                  ? /^-?\d+(\.\d+)?$/.test(v)
                  : true);
      }),
  );
}
export async function runQuery(
  query: QueryRequest,
  signal: AbortSignal,
  request: typeof fetch = fetch,
): Promise<QueryResult> {
  const response = await request('/api/v1/queries', {
    method: 'POST',
    credentials: 'same-origin',
    cache: 'no-store',
    signal,
    headers: {
      Accept: 'application/json, application/problem+json',
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(query),
  });
  const body: unknown = await response.json().catch((error: unknown) => {
    if (signal.aborted) throw error;
    throw new Error(`La API respondió HTTP ${response.status} sin JSON válido.`);
  });
  if (!response.ok) {
    if (
      response.headers.get('content-type')?.includes('application/problem+json') &&
      isProblem(body)
    )
      throw new QueryError(body);
    throw new Error(`La API respondió HTTP ${response.status} sin un Problem válido.`);
  }
  if (!isResult(body)) throw new Error('La API devolvió un resultado tipado inválido.');
  return body;
}
