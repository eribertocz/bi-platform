import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
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
  region = 'east',
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
  await fireEvent.change(screen.getByLabelText('Región'), { target: { value: 'north' } });
  await waitFor(() => expect(pending).toHaveLength(4));
  expect(pending.slice(0, 2).every((p) => p.signal.aborted)).toBe(true);
  pending.slice(2).forEach((p) => p.finish(result(p.query, 'north', '123', '55.01')));
  await waitFor(() => expect(screen.getByTestId('kpi-count').textContent).toBe('123'));
  pending.slice(0, 2).forEach((p) => p.finish(result(p.query)));
  await waitFor(() => expect(screen.getByTestId('kpi-amount').textContent).toBe('55,01'));
  expect(screen.queryByText('east')).toBeTruthy(); // Filter option remains, obsolete row does not.
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
