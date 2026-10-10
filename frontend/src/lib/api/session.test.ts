import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  ApiError,
  apiRequest,
  getDevUsers,
  getSession,
  login,
  logout,
  setUnauthorizedHandler,
} from './session.ts';

const session = {
  user: { id: 'analyst', display_name: 'Analista' },
  data_scope: 'synthetic',
  expires_at: '2026-10-11T00:00:00.000000Z',
};
afterEach(() => setUnauthorizedHandler());

describe('server session client', () => {
  it('uses same-origin HttpOnly cookies and server-derived identity', async () => {
    const request = vi.fn<typeof fetch>().mockImplementation(async () => Response.json(session));
    expect(await login('analyst', request)).toEqual(session);
    expect(request).toHaveBeenCalledWith(
      '/api/v1/dev/login',
      expect.objectContaining({
        credentials: 'same-origin',
        cache: 'no-store',
        method: 'POST',
        body: '{"user_id":"analyst"}',
      }),
    );
    expect(await getSession(undefined, request)).toEqual(session);
  });

  it('clears session on any API 401 and preserves request id', async () => {
    const unauthorized = vi.fn();
    setUnauthorizedHandler(unauthorized);
    const request = vi
      .fn<typeof fetch>()
      .mockResolvedValue(
        new Response(null, { status: 401, headers: { 'x-request-id': 'request-1' } }),
      );
    await expect(apiRequest('/api/v1/session', {}, request)).rejects.toMatchObject({
      status: 401,
      requestId: 'request-1',
    });
    expect(unauthorized).toHaveBeenCalledOnce();
  });

  it('keeps logout failures distinct from an expired session', async () => {
    const unauthorized = vi.fn();
    setUnauthorizedHandler(unauthorized);
    const request = vi.fn<typeof fetch>().mockResolvedValue(new Response(null, { status: 503 }));
    await expect(logout(request)).rejects.toBeInstanceOf(ApiError);
    expect(unauthorized).not.toHaveBeenCalled();
    request.mockResolvedValue(new Response(null, { status: 204 }));
    await logout(request);
    expect(request).toHaveBeenLastCalledWith(
      '/api/v1/logout',
      expect.objectContaining({ method: 'POST' }),
    );
  });

  it.each([
    null,
    { ...session, data_scope: 'real' },
    { ...session, expires_at: 'invalid' },
    { ...session, user: { id: 1 } },
  ])('rejects malformed session %j', async (body) => {
    const request = vi.fn<typeof fetch>().mockResolvedValue(Response.json(body));
    await expect(getSession(undefined, request)).rejects.toThrow('sesión inválida');
  });

  it('validates configured users and reports disabled development access', async () => {
    const request = vi.fn<typeof fetch>().mockResolvedValue(Response.json([session.user]));
    expect(await getDevUsers(undefined, request)).toEqual([session.user]);
    request.mockResolvedValue(Response.json([{ id: 'wrong' }]));
    await expect(getDevUsers(undefined, request)).rejects.toThrow('usuarios inválidos');
    request.mockResolvedValue(new Response(null, { status: 404 }));
    await expect(getDevUsers(undefined, request)).rejects.toMatchObject({ status: 404 });
  });

  it('propagates cancellation', async () => {
    const controller = new AbortController();
    const request = vi.fn<typeof fetch>().mockResolvedValue(Response.json(session));
    await getSession(controller.signal, request);
    controller.abort();
    expect(request.mock.calls[0]?.[1]?.signal?.aborted).toBe(true);
  });
});
