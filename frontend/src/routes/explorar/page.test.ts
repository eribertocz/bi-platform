import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';
import Page from './+page.svelte';
import type { QueryRequest } from '#lib/api/query.ts';
const cleanupChart = vi.hoisted(() => vi.fn());
const mountChart = vi.hoisted(() => vi.fn(() => cleanupChart));
const goto = vi.hoisted(() => vi.fn());
vi.mock('$app/navigation', () => ({ goto }));
vi.mock('../../lib/features/explore/echarts', () => ({ mountChart }));
function result(
  query: QueryRequest,
  region: string | null = 'east',
  count = '9007199254740993',
  amount: string | null = '10000009275125921.28',
) {
  const grouped = query.dimensions?.length;
  return Response.json({
    columns: [
      ...(grouped ? [{ name: 'region', type: 'string', nullable: true }] : []),
      { name: 'row_count', type: 'integer', nullable: false },
      { name: 'amount_sum', type: 'decimal', precision: 38, scale: 2, nullable: true },
    ],
    rows: [[...(grouped ? [region] : []), count, amount]],
    row_count: 1,
    truncated: false,
  });
}
it.each([['pacific'], ['pacific', null, 'all', 'null']])(
  'derives options from unfiltered data %j and keeps them when filtering',
  async (...regions: (string | null)[]) => {
    const queries: QueryRequest[] = [];
    vi.stubGlobal(
      'fetch',
      vi.fn<typeof fetch>(async (_, init) => {
        const query: QueryRequest = JSON.parse(init!.body as string);
        queries.push(query);
        const response = await result(query).json();
        if (query.dimensions?.length) {
          const selected = query.filters?.length ? query.filters[0]!.values : regions;
          response.rows = selected!.map((region) => [region, '1', '12.34']);
          response.row_count = response.rows.length;
        }
        return Response.json(response);
      }),
    );
    render(Page);
    await screen.findByRole('table');
    const labels = ['Todas las regiones', ...regions.map((region) => region ?? 'Sin región')];
    expect(screen.getAllByRole('option').map((option) => option.textContent)).toEqual(labels);
    for (const region of regions) {
      await fireEvent.change(screen.getByLabelText('Región'), {
        target: { value: JSON.stringify(region) },
      });
      await waitFor(() =>
        expect(screen.getAllByRole('rowheader').map((cell) => cell.textContent)).toEqual([
          region ?? 'Sin región',
        ]),
      );
      expect(queries.slice(-2).every((query) => query.filters?.[0]?.values[0] === region)).toBe(
        true,
      );
      expect(screen.getAllByRole('option').map((option) => option.textContent)).toEqual(labels);
    }
    await fireEvent.change(screen.getByLabelText('Región'), { target: { value: 'all' } });
    await screen.findByRole('table');
    expect(queries.slice(-2).every((query) => query.filters?.length === 0)).toBe(true);
  },
);
it('discards obsolete responses even when the transport ignores abort, and disposes the chart', async () => {
  const pending: { finish: (r: Response) => void; signal: AbortSignal; query: QueryRequest }[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn<typeof fetch>(
      (_, init) =>
        new Promise((resolve) => {
          pending.push({
            finish: resolve,
            signal: init!.signal as AbortSignal,
            query: JSON.parse(init!.body as string),
          });
        }),
    ),
  );
  const rendered = render(Page);
  expect(screen.getByRole('status').textContent).toContain('Consultando');
  await waitFor(() => expect(pending).toHaveLength(2));
  await fireEvent.click(screen.getByRole('button', { name: 'Actualizar resultados' }));
  await waitFor(() => expect(pending).toHaveLength(4));
  expect(pending.slice(0, 2).every((p) => p.signal.aborted)).toBe(true);
  pending.slice(2).forEach((p) => p.finish(result(p.query, 'north', '123', '55.01')));
  await waitFor(() => expect(screen.getByTestId('kpi-count').textContent).toBe('123'));
  pending.slice(0, 2).forEach((p) => p.finish(result(p.query)));
  await waitFor(() => expect(screen.getByTestId('kpi-amount').textContent).toBe('55,01'));
  expect(screen.queryByText('east')).toBeNull(); // Obsolete data cannot populate the options either.
  expect(screen.getAllByRole('row')).toHaveLength(2);
  await waitFor(() => expect(mountChart).toHaveBeenCalledOnce());
  rendered.unmount();
  expect(pending.every((p) => p.signal.aborted)).toBe(true);
  expect(cleanupChart).toHaveBeenCalledOnce();
});
it('shows exact totals, a typed Problem and a successful manual retry', async () => {
  let fail = true;
  vi.stubGlobal(
    'fetch',
    vi.fn<typeof fetch>(async (_, init) =>
      fail
        ? Response.json(
            {
              type: 'about:blank',
              title: 'No disponible',
              detail: 'Reintenta',
              status: 503,
              code: 'analytics_unavailable',
              request_id: 'req-503',
            },
            { status: 503, headers: { 'Content-Type': 'application/problem+json' } },
          )
        : result(JSON.parse(init!.body as string)),
    ),
  );
  render(Page);
  expect((await screen.findByRole('alert')).textContent).toContain('analytics_unavailable');
  expect(screen.getByText('Solicitud: req-503')).toBeTruthy();
  fail = false;
  await fireEvent.click(screen.getByRole('button', { name: 'Reintentar' }));
  await waitFor(() =>
    expect(screen.getByTestId('kpi-count').textContent).toBe('9.007.199.254.740.993'),
  );
  expect(screen.getByTestId('kpi-amount').textContent).toBe('10.000.009.275.125.921,28');
});
it('shows empty instead of inventing a zero amount for missing aggregates', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn<typeof fetch>(async (_, init) => {
      const query: QueryRequest = JSON.parse(init!.body as string);
      const response = await result(query, 'north', '0', null).json();
      if (query.dimensions?.length) {
        response.rows = [];
        response.row_count = 0;
      }
      return Response.json(response);
    }),
  );
  render(Page);
  await screen.findByText('No hay ventas para este filtro.');
  expect(screen.getByTestId('kpi-amount').textContent).toBe('Sin dato');
  expect(screen.queryByRole('table')).toBeNull();
});
it('redirects to login on an actual 401 Problem and cancels the sibling query', async () => {
  const signals: AbortSignal[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn<typeof fetch>(async (_, init) => {
      signals.push(init!.signal as AbortSignal);
      return Response.json(
        {
          type: 'about:blank',
          title: 'Inicia sesión',
          status: 401,
          code: 'session_required',
          request_id: 'req-401',
        },
        { status: 401, headers: { 'Content-Type': 'application/problem+json' } },
      );
    }),
  );
  render(Page);
  await waitFor(() => expect(goto).toHaveBeenCalledWith('/login'));
  expect(signals.every((signal) => signal.aborted)).toBe(true);
  expect(goto).toHaveBeenCalledOnce();
  expect(screen.queryByTestId('kpi-count')).toBeNull();
});
const closedDetail =
  'Esta consulta de demostración solo está disponible en desarrollo sintético local. El flujo semántico de consulta aún no está disponible.';
function problem(status: number, code: string, detail?: string) {
  return Response.json(
    { type: 'about:blank', title: 'Forbidden', status, code, detail, request_id: `req-${code}` },
    { status, headers: { 'Content-Type': 'application/problem+json' } },
  );
}
it('explains the closed G1 route without previous results, chart or retry', async () => {
  let open = true;
  const request = vi.fn<typeof fetch>(async (_, init) =>
    open
      ? result(JSON.parse(init!.body as string))
      : problem(403, 'legacy_query_disabled', closedDetail),
  );
  vi.stubGlobal('fetch', request);
  render(Page);
  await screen.findByRole('table');
  await waitFor(() => expect(mountChart).toHaveBeenCalledOnce());
  open = false;
  await fireEvent.click(screen.getByRole('button', { name: 'Actualizar resultados' }));
  const alert = await screen.findByRole('alert');
  expect(within(alert).getByText(closedDetail)).toBeTruthy();
  expect(alert.textContent).toContain('HTTP 403 · Código: legacy_query_disabled');
  expect(within(alert).getByText('Solicitud: req-legacy_query_disabled')).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Reintentar' })).toBeNull();
  expect(screen.queryByTestId('kpi-count')).toBeNull();
  expect(screen.queryByTestId('kpi-amount')).toBeNull();
  expect(screen.queryByRole('table')).toBeNull();
  expect(screen.queryByRole('img')).toBeNull();
  expect(cleanupChart).toHaveBeenCalledOnce();
  // Nothing retries by itself: only the initial pair and the manual refresh.
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(request).toHaveBeenCalledTimes(4);
});
it.each([
  [403, 'origin_forbidden', 'Origen no permitido'],
  [503, 'audit_unavailable', undefined],
] as const)('keeps the manual retry for HTTP %s (%s)', async (status, code, detail) => {
  vi.stubGlobal(
    'fetch',
    vi.fn<typeof fetch>(async () => problem(status, code, detail)),
  );
  render(Page);
  expect((await screen.findByRole('alert')).textContent).toContain(`Código: ${code}`);
  expect(screen.getByRole('button', { name: 'Reintentar' })).toBeTruthy();
  expect(screen.queryByTestId('kpi-count')).toBeNull();
});
