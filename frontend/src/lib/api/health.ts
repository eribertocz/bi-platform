import type { components, paths } from './generated.ts';

export type Health = components['schemas']['Health'];
export type HealthStatus = components['schemas']['HealthStatus'];
export type HealthEndpoint = Extract<keyof paths, `/health/${string}`>;
export type HealthResult =
  { kind: 'response'; status: 200 | 503; health: Health } | { kind: 'error'; message: string };

function isStatus(value: unknown): value is HealthStatus {
  return value === 'ok' || value === 'degraded' || value === 'unavailable';
}

function isHealth(value: unknown): value is Health {
  if (typeof value !== 'object' || value === null) return false;
  const health = value as Record<string, unknown>;
  return (
    isStatus(health.status) &&
    Array.isArray(health.checks) &&
    health.checks.every(
      (check: unknown) =>
        typeof check === 'object' &&
        check !== null &&
        'name' in check &&
        typeof check.name === 'string' &&
        'status' in check &&
        isStatus(check.status),
    )
  );
}

export async function getHealth(
  endpoint: HealthEndpoint,
  signal?: AbortSignal,
  request: typeof fetch = fetch,
): Promise<HealthResult> {
  try {
    const response = await request(endpoint, {
      headers: { Accept: 'application/json' },
      cache: 'no-store',
      signal: signal
        ? AbortSignal.any([signal, AbortSignal.timeout(5000)])
        : AbortSignal.timeout(5000),
    });
    // A readiness 503 is a documented health payload, not a network failure.
    if (response.status !== 200 && !(endpoint === '/health/ready' && response.status === 503)) {
      return { kind: 'error', message: `La API respondió HTTP ${response.status}.` };
    }
    const body: unknown = await response.json();
    if (!isHealth(body)) {
      return { kind: 'error', message: 'La API devolvió una respuesta de estado inválida.' };
    }
    return { kind: 'response', status: response.status as 200 | 503, health: body };
  } catch {
    return {
      kind: 'error',
      message: 'No se pudo consultar la API. Comprueba la conexión e intenta de nuevo.',
    };
  }
}
