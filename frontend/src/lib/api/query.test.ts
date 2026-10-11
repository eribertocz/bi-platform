import { expect, it, vi } from 'vitest';
import { runQuery, QueryError } from './query';
const query = { dataset: 'sales', measures: ['row_count'] };
const result = {
  columns: [{ name: 'row_count', type: 'integer', nullable: false }],
  rows: [['9007199254740993']],
  row_count: 1,
  truncated: false,
};
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
it.each([422, 429, 500, 503, 504, 401])(
  'keeps Problem including code and request_id for HTTP %s',
  async (status) => {
    const problem = {
      type: 'about:blank',
      title: 'Error',
      status,
      code: 'query_failed',
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
