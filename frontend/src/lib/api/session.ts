import type { components, paths } from './generated.ts';

export type Session = components['schemas']['Session'];
export type User = components['schemas']['User'];
type Endpoint = Extract<keyof paths, `/api/v1/${string}`>;

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    public readonly requestId?: string,
    public readonly code?: string,
    public readonly publicOrigin?: string,
  ) {
    super(`La API respondió HTTP ${status}.`);
  }
}

// All data clients use this boundary so a server 401 clears the displayed session.
let onUnauthorized: (() => void) | undefined;
export function setUnauthorizedHandler(handler?: () => void) {
  onUnauthorized = handler;
}

export async function apiRequest(
  endpoint: Endpoint,
  init: RequestInit = {},
  request: typeof fetch = fetch,
): Promise<Response> {
  const headers = new Headers(init.headers);
  if (!headers.has('Accept')) headers.set('Accept', 'application/json');
  const response = await request(endpoint, {
    ...init,
    credentials: 'same-origin',
    cache: 'no-store',
    headers,
    signal: init.signal
      ? AbortSignal.any([init.signal, AbortSignal.timeout(5000)])
      : AbortSignal.timeout(5000),
  });
  if (!response.ok) {
    if (response.status === 401 && !init.signal?.aborted) onUnauthorized?.();
    let code: string | undefined;
    let publicOrigin: string | undefined;
    if (
      response.headers.get('content-type')?.split(';')[0]?.trim() === 'application/problem+json'
    ) {
      try {
        const problem: unknown = await response.json();
        if (typeof problem === 'object' && problem !== null) {
          if ('code' in problem && typeof problem.code === 'string') code = problem.code;
          if ('public_origin' in problem && typeof problem.public_origin === 'string') {
            const url = new URL(problem.public_origin);
            if (
              ['http:', 'https:'].includes(url.protocol) &&
              !url.username &&
              !url.password &&
              !url.search &&
              !url.hash &&
              url.pathname === '/'
            )
              publicOrigin = problem.public_origin;
          }
        }
      } catch {
        // A malformed problem keeps its HTTP status; never invent a destination.
      }
    }
    throw new ApiError(
      response.status,
      response.headers.get('x-request-id') ?? undefined,
      code,
      publicOrigin,
    );
  }
  return response;
}

function isUser(value: unknown): value is User {
  return (
    typeof value === 'object' &&
    value !== null &&
    'id' in value &&
    typeof value.id === 'string' &&
    'display_name' in value &&
    typeof value.display_name === 'string'
  );
}

async function decodeSession(response: Response): Promise<Session> {
  const value: unknown = await response.json();
  if (
    typeof value !== 'object' ||
    value === null ||
    !('user' in value) ||
    !isUser(value.user) ||
    !('data_scope' in value) ||
    value.data_scope !== 'synthetic' ||
    !('expires_at' in value) ||
    typeof value.expires_at !== 'string' ||
    !Number.isFinite(Date.parse(value.expires_at))
  ) {
    throw new Error('La API devolvió una sesión inválida.');
  }
  return value as Session;
}

export async function getSession(
  signal?: AbortSignal,
  request: typeof fetch = fetch,
): Promise<Session> {
  return decodeSession(await apiRequest('/api/v1/session', { signal }, request));
}

export async function getDevUsers(
  signal?: AbortSignal,
  request: typeof fetch = fetch,
): Promise<User[]> {
  const value: unknown = await (await apiRequest('/api/v1/dev/users', { signal }, request)).json();
  if (!Array.isArray(value) || !value.every(isUser))
    throw new Error('La API devolvió usuarios inválidos.');
  return value;
}

export async function login(userId: string, request: typeof fetch = fetch): Promise<Session> {
  const body: components['schemas']['DevLogin'] = { user_id: userId };
  return decodeSession(
    await apiRequest(
      '/api/v1/dev/login',
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      },
      request,
    ),
  );
}

export async function logout(request: typeof fetch = fetch): Promise<void> {
  await apiRequest('/api/v1/logout', { method: 'POST' }, request);
}

export async function getOidcConfig(signal?: AbortSignal): Promise<boolean> {
  const value: unknown = await (await apiRequest('/api/v1/oidc/config', { signal })).json();
  if (
    typeof value !== 'object' ||
    value === null ||
    !('enabled' in value) ||
    typeof value.enabled !== 'boolean'
  )
    throw new Error('La API devolvió una configuración inválida.');
  return value.enabled;
}

export async function startCorporateLogin(): Promise<void> {
  const value: components['schemas']['LoginStart'] = await (
    await apiRequest('/api/v1/oidc/start', { method: 'POST' })
  ).json();
  window.location.assign(value.authorization_url);
}
