import { expect, it, vi } from 'vitest';
import { listModels, ModelsError } from './models';
it('uses the session, abort signal and no-store when listing models', async () => {
  const signal = new AbortController().signal;
  const body = { project_status: 'not_configured', models: [], diagnostics: [] };
  const request = vi.fn<typeof fetch>(async () => Response.json(body));
  expect(await listModels(signal, request)).toEqual(body);
  expect(request).toHaveBeenCalledWith(
    '/api/v1/models',
    expect.objectContaining({ credentials: 'same-origin', cache: 'no-store', signal }),
  );
});
it('preserves HTTP status and the Spanish problem message', async () => {
  const request = vi.fn<typeof fetch>(async () =>
    Response.json({ detail: 'El motor no está listo.' }, { status: 503 }),
  );
  await expect(listModels(new AbortController().signal, request)).rejects.toMatchObject({
    status: 503,
    message: 'El motor no está listo.',
  });
});
it('provides a useful error when an intermediary returns HTML', async () => {
  const request = vi.fn<typeof fetch>(async () => new Response('<html>', { status: 502 }));
  await expect(listModels(new AbortController().signal, request)).rejects.toBeInstanceOf(
    ModelsError,
  );
});
