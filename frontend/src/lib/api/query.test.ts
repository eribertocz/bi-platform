import { expect, it, vi } from 'vitest';
import { runQuery, QueryError } from './query';
const query = { dataset: 'sales', measures: ['row_count'] };
const result = {
  columns: [{ name: 'row_count', type: 'integer', nullable: false }],
  rows: [['9007199254740993']],
  row_count: 1,
  truncated: false,
};
it.each([502, 200])('reports HTTP %s without exposing a non-JSON parser error', async (status) => {
  const request = vi.fn<typeof fetch>().mockResolvedValue(
    new Response('<html>Bad gateway</html>', {
      status,
      headers: { 'Content-Type': 'text/html' },
    }),
  );
  await expect(runQuery(query, new AbortController().signal, request)).rejects.toThrow(
    `La API respondió HTTP ${status} sin JSON válido.`,
  );
});
it('preserves exact strings and forwards cancellation and cookies', async () => {
  const request = vi.fn<typeof fetch>().mockResolvedValue(Response.json(result));
  const signal = new AbortController().signal;
  expect(await runQuery(query, signal, request)).toEqual(result);
  expect(request.mock.calls[0]?.[1]).toMatchObject({
    signal,
    credentials: 'same-origin',
    method: 'POST',
  });
});
it.each([
  [401, 'session_required'],
  [403, 'origin_forbidden'],
  [422, 'invalid_query'],
  [422, 'result_too_large'],
  [422, 'query_memory_limit'],
  [429, 'too_many_queries'],
  [500, 'query_failed'],
  [503, 'analytics_unavailable'],
  [504, 'query_timeout'],
] as const)(
  'keeps Problem including code and request_id for HTTP %s (%s)',
  async (status, code) => {
    const problem = {
      type: 'about:blank',
      title: 'Error',
      status,
      code,
      request_id: 'request-test',
      detail: 'Intenta de nuevo',
    };
    const request = vi
      .fn<typeof fetch>()
      .mockResolvedValue(
        Response.json(problem, { status, headers: { 'Content-Type': 'application/problem+json' } }),
      );
    const failure = runQuery(query, new AbortController().signal, request);
    await expect(failure).rejects.toMatchObject({
      problem,
    });
    await expect(failure).rejects.toBeInstanceOf(QueryError);
  },
);
it.each([[[9007199254740992]], [[null]], [[]]])(
  'rejects malformed or lossy rows %j',
  async (rows) => {
    const request = vi.fn<typeof fetch>().mockResolvedValue(Response.json({ ...result, rows }));
    await expect(runQuery(query, new AbortController().signal, request)).rejects.toThrow(
      'inválido',
    );
  },
);
