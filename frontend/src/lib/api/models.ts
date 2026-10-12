import type { components } from './generated';
export type ModelsResponse = components['schemas']['ModelsResponse'];
export type ModelFile = components['schemas']['ModelFile'];
export type ModelMember = components['schemas']['ModelMember'];
export class ModelsError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}
export async function listModels(
  signal: AbortSignal,
  request: typeof fetch = fetch,
): Promise<ModelsResponse> {
  const response = await request('/api/v1/models', {
    credentials: 'same-origin',
    cache: 'no-store',
    signal,
    headers: { Accept: 'application/json, application/problem+json' },
  });
  if (!response.ok) {
    const problem: unknown = await response.json().catch(() => null);
    const message =
      problem &&
      typeof problem === 'object' &&
      'detail' in problem &&
      typeof problem.detail === 'string'
        ? problem.detail
        : 'No se pudo cargar los modelos. Reintenta en unos segundos.';
    throw new ModelsError(response.status, message);
  }
  return response.json();
}
