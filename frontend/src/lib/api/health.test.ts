import { describe, expect, it, vi } from 'vitest';
import { getHealth } from './health.ts';

describe('health client', () => {
  it('requests same-origin liveness without caching and decodes 200', async () => {
    const health = { status: 'ok', checks: [] };
    const request = vi.fn<typeof fetch>().mockResolvedValue(Response.json(health));
    expect(await getHealth('/health/live', undefined, request)).toEqual({
      kind: 'response',
      status: 200,
      health,
    });
    expect(request).toHaveBeenCalledWith(
      '/health/live',
      expect.objectContaining({
        cache: 'no-store',
        headers: { Accept: 'application/json' },
        signal: expect.any(AbortSignal),
      }),
    );
  });

  it('preserves readiness 503 and dependency checks', async () => {
    const health = { status: 'unavailable', checks: [{ name: 'postgres', status: 'unavailable' }] };
    const request = vi.fn<typeof fetch>().mockResolvedValue(Response.json(health, { status: 503 }));
    expect(await getHealth('/health/ready', undefined, request)).toEqual({
      kind: 'response',
      status: 503,
      health,
    });
  });

  it('reports network failure without exposing internal errors', async () => {
    const request = vi.fn<typeof fetch>().mockRejectedValue(new TypeError('private detail'));
    expect(await getHealth('/health/live', undefined, request)).toEqual({
      kind: 'error',
      message: expect.stringContaining('Comprueba la conexión'),
    });
  });

  it.each([null, { status: 'ok' }, { status: 'ok', checks: [{ name: 'db', status: 'other' }] }])(
    'rejects malformed health: %j',
    async (body) => {
      const request = vi.fn<typeof fetch>().mockResolvedValue(Response.json(body));
      expect(await getHealth('/health/ready', undefined, request)).toEqual({
        kind: 'error',
        message: expect.stringContaining('inválida'),
      });
    },
  );

  it('rejects undocumented HTTP responses', async () => {
    const request = vi
      .fn<typeof fetch>()
      .mockResolvedValue(new Response('failure', { status: 500 }));
    expect(await getHealth('/health/ready', undefined, request)).toEqual({
      kind: 'error',
      message: 'La API respondió HTTP 500.',
    });
  });

  it('propagates cancellation to the request signal', async () => {
    const controller = new AbortController();
    const request = vi
      .fn<typeof fetch>()
      .mockResolvedValue(Response.json({ status: 'ok', checks: [] }));
    await getHealth('/health/live', controller.signal, request);
    controller.abort();
    expect(request.mock.calls[0]?.[1]?.signal?.aborted).toBe(true);
  });
});
