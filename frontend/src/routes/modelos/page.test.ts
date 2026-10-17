import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';
import Page from './+page.svelte';
import type { ModelsResponse } from '#lib/api/models.ts';

const goto = vi.hoisted(() => vi.fn());
vi.mock('$app/navigation', () => ({ goto }));
const data: ModelsResponse = {
  project_status: 'ready',
  diagnostics: [],
  models: [
    {
      status: 'valid',
      file: 'metrics/ventas.yaml',
      name: 'Ventas',
      fingerprint: 'sef-sha256-v1:abc',
      diagnostics: [],
      description: 'Ventas registradas.',
      ai_context: { synonyms: ['operaciones'] },
      datasets: [],
      metrics: [
        {
          id: 'p/metric/m',
          name: 'Importe',
          data_type: 'DECIMAL(38,2)',
          nullable: null,
          annotations: [],
        },
      ],
      dimensions: [
        {
          id: 'p/dimension/id',
          name: 'ID',
          data_type: 'INT64',
          nullable: false,
          annotations: [
            {
              path: '/datasets/0/fields/0',
              name: 'ventas.fecha',
              label: 'Fecha comercial',
              description: '<img src=x onerror=alert(1)>',
              ai_context: {
                synonyms: ['fecha de compra'],
                instructions: 'No ejecutar',
                examples: ['ayer'],
                vendor_hint: { arbitrary: true },
              },
              is_time: true,
              declared_is_time: null,
            },
          ],
        },
      ],
    },
    {
      status: 'invalid',
      file: 'metrics/error.yaml',
      name: 'Error',
      diagnostics: [
        {
          code: 'syntax',
          severity: 'error',
          message: 'El archivo no es un YAML válido.',
          path: '',
          line: 2,
          column: 3,
        },
      ],
    },
  ],
};
it('shows exact types and file diagnostics with location', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => Response.json(data)),
  );
  render(Page);
  await fireEvent.click(await screen.findByRole('button', { name: 'Ventas' }));
  expect(screen.getByText('DECIMAL(38,2)').textContent).toBe('DECIMAL(38,2)');
  expect(screen.getByText('INT64').textContent).toBe('INT64');
  expect(screen.getByText('Dimensión de tiempo')).toBeTruthy();
  expect(screen.getByText('Fecha comercial')).toBeTruthy();
  expect(screen.getByText('Ventas registradas.')).toBeTruthy();
  expect(screen.getByText('Sinónimos para IA: fecha de compra')).toBeTruthy();
  expect(screen.getByText('<img src=x onerror=alert(1)>')).toBeTruthy();
  expect(document.querySelector('img')).toBeNull();
  await fireEvent.click(screen.getByRole('button', { name: 'Error' }));
  expect(screen.getByText('El archivo no es un YAML válido.')).toBeTruthy();
  expect(screen.getByText(/metrics\/error.yaml.*Línea 2,\s*columna 3/)).toBeTruthy();
  expect(screen.queryByText('DECIMAL(38,2)')).toBeNull();
});
it.each(['not_configured', 'ready'] as const)(
  'explains an empty project: %s',
  async (project_status) => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => Response.json({ project_status, models: [], diagnostics: [] })),
    );
    render(Page);
    await waitFor(() =>
      expect(screen.getByRole('status').textContent).toContain(
        project_status === 'ready' ? 'No hay modelos' : 'No hay un proyecto configurado',
      ),
    );
  },
);
it('redirects expired sessions', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => Response.json({ detail: 'Sesión caducada' }, { status: 401 })),
  );
  render(Page);
  await waitFor(() => expect(goto).toHaveBeenCalledWith('/login'));
});
it('ignores obsolete responses and removes stale metadata while refreshing', async () => {
  const pending: { finish: (r: Response) => void; signal: AbortSignal }[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn<typeof fetch>(
      (_, init) =>
        new Promise((finish) => pending.push({ finish, signal: init!.signal as AbortSignal })),
    ),
  );
  const rendered = render(Page);
  await waitFor(() => expect(pending).toHaveLength(1));
  await fireEvent.click(screen.getByRole('button', { name: 'Actualizar modelos' }));
  await waitFor(() => expect(pending).toHaveLength(2));
  expect(pending[0]!.signal.aborted).toBe(true);
  pending[1]!.finish(Response.json(data));
  await screen.findByRole('button', { name: 'Ventas' });
  pending[0]!.finish(
    Response.json({ project_status: 'not_configured', models: [], diagnostics: [] }),
  );
  await fireEvent.click(screen.getByRole('button', { name: 'Ventas' }));
  expect(screen.getByText('DECIMAL(38,2)')).toBeTruthy();
  await fireEvent.click(screen.getByRole('button', { name: 'Actualizar modelos' }));
  expect(screen.queryByText('DECIMAL(38,2)')).toBeNull();
  rendered.unmount();
  expect(pending[2]!.signal.aborted).toBe(true);
});
const DENIED =
  'No tienes permiso para revisar las definiciones y los errores de este proyecto. Solicita acceso a quien lo administra.';
const problem = (status: number, code: string, detail: string) =>
  Response.json(
    { type: 'about:blank', title: 'Problem', status, code, detail, request_id: 'req-1' },
    { status, headers: { 'content-type': 'application/problem+json' } },
  );
/** Text of the catalog that a refused session must not keep. */
const catalog = [
  'Ventas',
  'DECIMAL(38,2)',
  'metrics/ventas.yaml',
  'metrics/error.yaml',
  'Fecha comercial',
];
function assertNoCatalog() {
  for (const text of catalog) expect(document.body.textContent).not.toContain(text);
}
it('a denial clears the catalog and explains it; restored access loads it again', async () => {
  const answers = [
    Response.json(data),
    problem(403, 'models_forbidden', DENIED),
    Response.json(data),
  ];
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => answers.shift()!),
  );
  render(Page);
  await fireEvent.click(await screen.findByRole('button', { name: 'Ventas' }));
  expect(screen.getByText('DECIMAL(38,2)')).toBeTruthy();
  await fireEvent.click(screen.getByRole('button', { name: 'Actualizar modelos' }));
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain('No tienes acceso a los modelos de este proyecto');
  expect(alert.textContent).toContain(DENIED);
  assertNoCatalog();
  expect(screen.queryByRole('list', { name: 'Modelos del proyecto' })).toBeNull();
  await fireEvent.click(screen.getByRole('button', { name: 'Comprobar el acceso de nuevo' }));
  await screen.findByRole('button', { name: 'Ventas' });
  // The file open before the denial is not reopened by itself.
  expect(screen.queryByRole('region')).toBeNull();
  expect(screen.queryByRole('alert')).toBeNull();
});
it('a temporary failure is not presented as a lack of permission', async () => {
  const unavailable = 'El motor analítico no está listo. Reintenta en unos segundos.';
  const answers = [problem(503, 'analytics_unavailable', unavailable), Response.json(data)];
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => answers.shift()!),
  );
  render(Page);
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain('Los modelos no están disponibles en este momento');
  expect(alert.textContent).toContain(unavailable);
  expect(alert.textContent).not.toContain('No tienes acceso');
  await fireEvent.click(screen.getByRole('button', { name: 'Reintentar' }));
  await screen.findByRole('button', { name: 'Ventas' });
});
it('an answer older than a denial never brings the catalog back', async () => {
  const pending: { finish: (r: Response) => void }[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn<typeof fetch>(() => new Promise((finish) => pending.push({ finish }))),
  );
  render(Page);
  await waitFor(() => expect(pending).toHaveLength(1));
  await fireEvent.click(screen.getByRole('button', { name: 'Actualizar modelos' }));
  await waitFor(() => expect(pending).toHaveLength(2));
  pending[1]!.finish(problem(403, 'models_forbidden', DENIED));
  await screen.findByRole('alert');
  pending[0]!.finish(Response.json(data));
  await new Promise((settle) => setTimeout(settle, 0));
  expect(screen.getByRole('alert').textContent).toContain(DENIED);
  assertNoCatalog();
});
