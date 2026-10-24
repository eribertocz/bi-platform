import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import Page from './+page.svelte';
// Real answers of #138 (see src/lib/features/semantic/fixtures).
import region from '#lib/features/semantic/fixtures/region.json';
import total from '#lib/features/semantic/fixtures/total.json';

const mountBars = vi.hoisted(() => vi.fn(() => () => {}));
vi.mock('#lib/features/semantic/bars.ts', () => ({ mountBars }));
vi.mock('../../lib/features/semantic/bars', () => ({ mountBars }));

const member = (
  name: string,
  select = 'values',
  group = false,
  lifecycle: object = { state: 'active' },
) => ({
  name,
  type: 'DECIMAL(18,2)',
  unit: null,
  lifecycle,
  capabilities: { select, filter: true, group, order: true },
});
const ventas = {
  model: 'metrics/ventas.yaml',
  name: 'ventas',
  metrics: [
    member('importe_total'),
    member('filas'),
    member('margen', 'masked'),
    member('vieja', 'values', false, {
      state: 'deprecated',
      sunset: '2027-01-01',
      replacement: 'importe_total',
    }),
  ],
  fields: [
    member('ventas.region', 'values', true),
    member('ventas.ciudad', 'values', true),
    member('ventas.mes', 'values', true),
    // Not groupable for this session: never offered to break down by.
    member('ventas.nota'),
  ],
};
const catalog = (models: object[] = [ventas], status = 'ready') =>
  Response.json({ status, models });
const problem = (status: number, code: string) =>
  new Response(
    JSON.stringify({
      type: 'about:blank',
      title: code,
      status,
      code,
      request_id: 'req-1',
      detail: code,
    }),
    { status, headers: { 'content-type': 'application/problem+json', 'x-request-id': 'req-1' } },
  );

type Call = {
  url: string;
  body: { model: string; query: { selections: { kind: string; name: string }[] } } | null;
};

/**
 * A server double for the page's coordination only: real recorded answers,
 * every request logged, and at most one in flight checked.
 */
function server(answer: (call: Call, n: number) => Response | Promise<Response>) {
  const calls: Call[] = [];
  let inFlight = 0;
  let overlapped = false;
  vi.stubGlobal(
    'fetch',
    vi.fn<typeof fetch>(async (input, init) => {
      const call: Call = {
        url: String(input),
        body: init?.body ? JSON.parse(init.body as string) : null,
      };
      calls.push(call);
      if (++inFlight > 1) overlapped = true;
      try {
        return await answer(call, calls.length - 1);
      } finally {
        inFlight--;
      }
    }),
  );
  return { calls, overlapped: () => overlapped };
}
const grouped = (call: Call) => call.body!.query.selections.some((s) => s.kind === 'dimension');
const realAnswer = (call: Call) =>
  call.url.endsWith('/catalog') ? catalog() : Response.json(grouped(call) ? region : total);

describe('one question at a time, in sequence', () => {
  it('loads the catalog, then the total, then the breakdown; never two at once', async () => {
    const api = server(realAnswer);
    render(Page);
    await screen.findByTestId('kpi-t');
    expect(screen.getByTestId('kpi-t').textContent!.trim()).toBe('300,30');
    await fireEvent.click(screen.getByRole('checkbox', { name: 'ventas.region' }));
    await screen.findByRole('table', { name: 'Desglose' });
    expect(api.calls.map((c) => [c.url, c.body?.query.selections.map((s) => s.name)])).toEqual([
      ['/api/v1/semantic/catalog', undefined],
      ['/api/v1/semantic/query', ['importe_total']],
      ['/api/v1/semantic/query', ['importe_total']],
      ['/api/v1/semantic/query', ['ventas.region', 'importe_total']],
    ]);
    expect(api.overlapped()).toBe(false);
    expect(api.calls.some((c) => /\/api\/v1\/(queries|models)/.test(c.url))).toBe(false);
    expect(screen.getAllByRole('heading', { name: /Recibo de/ }).map((h) => h.textContent)).toEqual(
      ['Recibo del desglose', 'Recibo de los totales'],
    );
    expect(screen.getByText(/dos lecturas, cada una con su recibo/)).toBeTruthy();
  });

  it('marks values seen only withheld and deprecated metrics, and bounds the choice', async () => {
    server(realAnswer);
    render(Page);
    await screen.findByTestId('kpi-t');
    expect(screen.getByRole('checkbox', { name: /margen.*solo valores ocultos/ })).toBeTruthy();
    expect(screen.getByRole('checkbox', { name: /vieja.*obsoleta/ })).toBeTruthy();
    await fireEvent.click(screen.getByRole('checkbox', { name: 'ventas.region' }));
    await fireEvent.click(screen.getByRole('checkbox', { name: 'ventas.ciudad' }));
    // Two fields at most: the third is disabled; a field it may not group is absent.
    expect(
      (screen.getByRole('checkbox', { name: 'ventas.mes' }) as HTMLInputElement).disabled,
    ).toBe(true);
    expect(screen.queryByRole('checkbox', { name: 'ventas.nota' })).toBeNull();
    await fireEvent.click(screen.getByRole('checkbox', { name: 'importe_total' }));
    expect(await screen.findByText('Elige al menos una métrica.')).toBeTruthy();
  });
});

describe('answers of a former question never show', () => {
  it('discards a late answer after the question changed', async () => {
    let release!: () => void;
    const held = new Promise<void>((resolve) => (release = resolve));
    server(async (call, n) => {
      if (call.url.endsWith('/catalog')) return catalog();
      if (n === 1) {
        await held; // the first total arrives after the second question
        return Response.json({
          ...total,
          result: { blocks: [{ ...total.result.blocks[0], rows: [['999.99', '9']] }] },
        });
      }
      return realAnswer(call);
    });
    render(Page);
    await waitFor(() => expect(screen.getByText('Consultando…')).toBeTruthy());
    await fireEvent.click(screen.getByRole('checkbox', { name: 'filas' }));
    await screen.findByTestId('kpi-t');
    release();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(document.body.textContent).not.toContain('999,99');
    expect(screen.getByTestId('kpi-t').textContent!.trim()).toBe('300,30');
  });

  it('withdraws the whole answer when its second request fails, and retries only on request', async () => {
    const api = server((call) => {
      if (call.url.endsWith('/catalog')) return catalog();
      return grouped(call) ? problem(503, 'analytics_failed') : Response.json(total);
    });
    render(Page);
    await screen.findByTestId('kpi-t');
    await fireEvent.click(screen.getByRole('checkbox', { name: 'ventas.region' }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('HTTP 503 · Código: analytics_failed');
    expect(alert.textContent).toContain('Solicitud: req-1');
    expect(screen.queryByTestId('kpi-t')).toBeNull();
    const before = api.calls.length;
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(api.calls.length).toBe(before);
    await fireEvent.click(screen.getByRole('button', { name: 'Reintentar' }));
    await waitFor(() => expect(api.calls.length).toBe(before + 2));
  });

  it('offers an explicit retry on 429, with no automatic loop', async () => {
    const api = server((call) =>
      call.url.endsWith('/catalog') ? catalog() : problem(429, 'semantic_busy'),
    );
    render(Page);
    expect((await screen.findByRole('alert')).textContent).toContain('ocupado');
    const before = api.calls.length;
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(api.calls.length).toBe(before);
    expect(screen.getByRole('button', { name: 'Reintentar' })).toBeTruthy();
  });

  it('retires a selection that is no longer usable and refreshes the catalog on request', async () => {
    let revoked = false;
    const api = server((call) => {
      if (call.url.endsWith('/catalog'))
        return catalog(revoked ? [{ ...ventas, metrics: [member('filas')] }] : [ventas]);
      revoked = true;
      return problem(404, 'model_not_found');
    });
    render(Page);
    expect((await screen.findByRole('alert')).textContent).toContain('ya no está disponible');
    expect(screen.getAllByRole('checkbox').every((c) => !(c as HTMLInputElement).checked)).toBe(
      true,
    );
    expect(screen.queryByRole('table')).toBeNull();
    await fireEvent.click(screen.getAllByRole('button', { name: 'Actualizar catálogo' }).at(-1)!);
    await waitFor(() =>
      expect(screen.queryByRole('checkbox', { name: 'importe_total' })).toBeNull(),
    );
    expect(api.calls.filter((c) => c.url.endsWith('/catalog'))).toHaveLength(2);
    // The refreshed catalog gives a new first question, asked once.
    await waitFor(() =>
      expect(api.calls.at(-1)!.body?.query.selections.map((s) => s.name)).toEqual(['filas']),
    );
  });

  it('aborts what is pending when the page goes away', async () => {
    const signals: AbortSignal[] = [];
    vi.stubGlobal(
      'fetch',
      vi.fn<typeof fetch>((input, init) => {
        if (String(input).endsWith('/catalog')) return Promise.resolve(catalog());
        signals.push(init!.signal!);
        return new Promise(() => {});
      }),
    );
    const page = render(Page);
    await waitFor(() => expect(signals).toHaveLength(1));
    page.unmount();
    expect(signals[0]!.aborted).toBe(true);
  });
});

describe('catalog states', () => {
  it.each([
    [[], 'ready', 'no tiene métricas disponibles'],
    [[], 'not_configured', 'no tiene un proyecto configurado'],
  ])('%j %s', async (models, status, text) => {
    const api = server(() => catalog(models, status));
    render(Page);
    expect(await screen.findByText(new RegExp(text))).toBeTruthy();
    expect(api.calls).toHaveLength(1);
  });

  it('explains a catalog failure with its request id and a retry', async () => {
    server(() => problem(503, 'catalog_unavailable'));
    render(Page);
    const alert = await screen.findByRole('alert');
    expect(within(alert).getByText(/catalog_unavailable/)).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Reintentar' })).toBeTruthy();
  });
});

describe('found by the adversarial review before delivery', () => {
  it('marks and disables a metric seen only withheld: it is never sent', async () => {
    const api = server(realAnswer);
    render(Page);
    await screen.findByTestId('kpi-t');
    const margen = screen.getByRole('checkbox', { name: /margen/ }) as HTMLInputElement;
    expect(margen.disabled).toBe(true);
    expect(api.calls.some((c) => c.body?.query.selections.some((s) => s.name === 'margen'))).toBe(
      false,
    );
  });

  it('shows a malformed receipt as an invalid answer instead of breaking the page', async () => {
    server((call) => {
      if (call.url.endsWith('/catalog')) return catalog();
      const broken = structuredClone(total) as Record<string, unknown>;
      delete (broken.receipt as Record<string, unknown>).read_started_at;
      return Response.json(broken);
    });
    render(Page);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('no corresponde a su contrato');
    expect(alert.textContent).not.toContain('conexión');
    expect(screen.queryByRole('button', { name: 'Reintentar' })).toBeNull();
    expect(screen.queryByTestId('kpi-t')).toBeNull();
  });
});

describe('up to 20 metrics per question (#147, decided by Codex)', () => {
  const HELP =
    'Muchas métricas pueden dificultar la lectura. Puedes quitar algunas para comparar mejor.';
  const MAX = 'Máximo 20 métricas: quita una para agregar otra.';
  const wide = {
    ...ventas,
    metrics: Array.from({ length: 21 }, (_, i) => member(`m${String(i + 1).padStart(2, '0')}`)),
  };
  const metricBoxes = () =>
    screen
      .getAllByRole('checkbox')
      .filter((c) =>
        /^m\d\d$/.test(c.getAttribute('aria-label') ?? c.parentElement!.textContent!.trim()),
      ) as HTMLInputElement[];
  async function tick(count: number) {
    for (const box of metricBoxes()) {
      const want = metricBoxes().indexOf(box) < count;
      if (box.checked !== want && !box.disabled) await fireEvent.click(box);
    }
  }

  it('asks 0, 1, 9, 10 and 20 in the order chosen, helps from 10 and stops the 21st', async () => {
    const api = server((call) =>
      call.url.endsWith('/catalog') ? catalog([wide]) : Response.json(total),
    );
    render(Page);
    await screen.findByTestId('kpi-t');
    const asked = () => api.calls.at(-1)!.body?.query.selections.map((s) => s.name);
    // 0: nothing is sent.
    await fireEvent.click(metricBoxes()[0]!);
    expect(await screen.findByText('Elige al menos una métrica.')).toBeTruthy();
    const before = api.calls.length;
    await new Promise((resolve) => setTimeout(resolve, 10));
    expect(api.calls.length).toBe(before);
    // 1, then 9: no help yet.
    await tick(1);
    await waitFor(() => expect(asked()).toEqual(['m01']));
    await tick(9);
    await waitFor(() => expect(asked()).toHaveLength(9));
    expect(screen.queryByText(HELP)).toBeNull();
    // 10: help, not blocking.
    await tick(10);
    await waitFor(() => expect(asked()).toHaveLength(10));
    expect(screen.getByText(HELP)).toBeTruthy();
    // 20: the 21st cannot be added; any one can be removed.
    await tick(20);
    await waitFor(() =>
      expect(asked()).toEqual(
        Array.from({ length: 20 }, (_, i) => `m${String(i + 1).padStart(2, '0')}`),
      ),
    );
    expect(screen.getByText(MAX)).toBeTruthy();
    expect(metricBoxes()[20]!.disabled).toBe(true);
    expect(
      metricBoxes()
        .slice(0, 20)
        .every((b) => !b.disabled),
    ).toBe(true);
    await fireEvent.click(metricBoxes()[20]!);
    expect(asked()).toHaveLength(20);
    expect(metricBoxes()[20]!.checked).toBe(false);
    // Removing one frees the max and keeps the order of the rest.
    await fireEvent.click(metricBoxes()[4]!);
    await waitFor(() => expect(asked()).toHaveLength(19));
    expect(asked()).not.toContain('m05');
    expect(screen.queryByText(MAX)).toBeNull();
    expect(metricBoxes()[20]!.disabled).toBe(false);
    // Back to 9: the help goes away.
    for (const box of metricBoxes()
      .filter((b) => b.checked)
      .slice(9))
      await fireEvent.click(box);
    await waitFor(() => expect(asked()).toHaveLength(9));
    expect(screen.queryByText(HELP)).toBeNull();
    expect(api.overlapped()).toBe(false);
  });
});
