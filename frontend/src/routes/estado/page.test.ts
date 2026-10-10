import { render, screen, within, fireEvent, waitFor } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';
import Page from './+page.svelte';

it('shows loading then independent live OK and readiness 503 checks', async () => {
  let finishReady!: (response: Response) => void;
  vi.stubGlobal(
    'fetch',
    vi.fn<typeof fetch>((input) =>
      input === '/health/live'
        ? Promise.resolve(Response.json({ status: 'ok', checks: [] }))
        : new Promise((resolve) => {
            finishReady = resolve;
          }),
    ),
  );
  render(Page);
  expect(screen.getAllByText('Consultando…')).toHaveLength(2);
  const button = screen.getByRole('button', { name: 'Actualizar estado' });
  expect((button as HTMLButtonElement).disabled).toBe(true);
  const live = within(screen.getByRole('region', { name: 'Proceso de la API' }));
  await live.findByText('OK');
  expect(screen.getByText('Consultando…')).toBeTruthy();
  finishReady(
    Response.json(
      {
        status: 'unavailable',
        checks: [
          { name: 'postgres', status: 'unavailable' },
          { name: 'migrations', status: 'ok' },
        ],
      },
      { status: 503 },
    ),
  );
  const ready = within(screen.getByRole('region', { name: 'Listo para servir' }));
  await ready.findByText('No listo');
  expect(ready.getByText('postgres')).toBeTruthy();
  expect(ready.getByText('No disponible')).toBeTruthy();
  expect(ready.getByText('migrations')).toBeTruthy();
  await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
});

it('recovers from network errors using the refresh button', async () => {
  const request = vi.fn<typeof fetch>().mockRejectedValue(new TypeError('offline'));
  vi.stubGlobal('fetch', request);
  render(Page);
  await waitFor(() => expect(screen.getAllByText('Error de consulta')).toHaveLength(2));
  const button = screen.getByRole('button', { name: 'Actualizar estado' });
  await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
  request.mockImplementation(async () => Response.json({ status: 'ok', checks: [] }));
  await fireEvent.click(button);
  await waitFor(() => expect(screen.getAllByText('OK')).toHaveLength(2));
  expect(request).toHaveBeenCalledTimes(4);
});

it('aborts outstanding requests when leaving the page', async () => {
  const signals: AbortSignal[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn<typeof fetch>((_input, init) => {
      if (init?.signal) signals.push(init.signal);
      return new Promise(() => {});
    }),
  );
  const view = render(Page);
  await waitFor(() => expect(signals).toHaveLength(2));
  view.unmount();
  expect(signals.every((signal) => signal.aborted)).toBe(true);
});
