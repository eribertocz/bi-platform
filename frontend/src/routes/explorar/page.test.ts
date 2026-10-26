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

describe('an incompatible answer withdraws the whole attempt (Codex review of #257)', () => {
  const broken = (fixture: unknown, cell: [number, number]) => {
    const copy = structuredClone(fixture) as { result: { blocks: { rows: unknown[][] }[] } };
    copy.result.blocks[0]!.rows[cell[0]]![cell[1]] = 'not-a-decimal';
    return copy;
  };

  it('an invalid breakdown leaves no total, table, chart or receipt, and a new question recovers', async () => {
    server((call) => {
      if (call.url.endsWith('/catalog')) return catalog();
      return Response.json(grouped(call) ? broken(region, [0, 1]) : total);
    });
    render(Page);
    await screen.findByTestId('kpi-t');
    await fireEvent.click(screen.getByRole('checkbox', { name: 'ventas.region' }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('no se muestra ninguna cifra');
    expect(screen.queryByTestId('kpi-t')).toBeNull();
    expect(screen.queryByRole('table')).toBeNull();
    expect(screen.queryByTestId('chart-omitted')).toBeNull();
    expect(screen.queryAllByRole('heading', { name: /Recibo de/ })).toHaveLength(0);
    expect(document.body.textContent).not.toMatch(/300,30|100,30|200,00/);
    expect(screen.queryByRole('button', { name: 'Reintentar' })).toBeNull();
    // Changing the question asks again and recovers.
    await fireEvent.click(screen.getByRole('checkbox', { name: 'ventas.region' }));
    expect((await screen.findByTestId('kpi-t')).textContent!.trim()).toBe('300,30');
  });

  it('an invalid total ends the attempt before the breakdown is asked', async () => {
    let totals = 0;
    const api = server((call) => {
      if (call.url.endsWith('/catalog')) return catalog();
      if (grouped(call)) return Response.json(region);
      return Response.json(totals++ === 0 ? total : broken(total, [0, 0]));
    });
    render(Page);
    await screen.findByTestId('kpi-t');
    await fireEvent.click(screen.getByRole('checkbox', { name: 'ventas.region' }));
    expect((await screen.findByRole('alert')).textContent).toContain('no se muestra ninguna cifra');
    expect(screen.queryByRole('table')).toBeNull();
    expect(screen.queryByTestId('kpi-t')).toBeNull();
    expect(document.body.textContent).not.toMatch(/100,30|200,00/);
    expect(api.calls.some((c) => c.body !== null && grouped(c))).toBe(false);
  });
});

describe('row filters: drafted, applied explicitly, cleared (#265)', () => {
  const typed = (fields: string[]) => ({
    ...ventas,
    fields: fields.map((name) => ({ ...member(name, 'values', true), type: 'VARCHAR' })),
  });
  const answering = (fields: () => string[]) => (call: Call) =>
    call.url.endsWith('/catalog')
      ? catalog([typed(fields())])
      : Response.json(grouped(call) ? region : total);
  const sur = {
    op: 'compare',
    target: { kind: 'dimension', name: 'ventas.region' },
    comparison: '=',
    value: { type: 'text', value: 'Sur' },
  };
  const filtersSent = (calls: Call[]) =>
    calls.map((c) => (c.body?.query as { filter?: unknown } | undefined)?.filter ?? null);

  it('editing asks nothing; applying sends it in both reads; clearing asks again without it', async () => {
    const api = server(answering(() => ['ventas.region', 'ventas.ciudad']));
    render(Page);
    await screen.findByTestId('kpi-t');
    await fireEvent.click(screen.getByRole('checkbox', { name: 'ventas.region' }));
    await screen.findByRole('table', { name: 'Desglose' });
    const before = api.calls.length;
    await fireEvent.click(screen.getByRole('button', { name: 'Añadir condición' }));
    await fireEvent.input(screen.getByLabelText('Valor'), { target: { value: 'Sur' } });
    expect(api.calls.length).toBe(before);
    expect(screen.getByText('Hay cambios sin aplicar.')).toBeTruthy();
    await fireEvent.click(screen.getByRole('button', { name: 'Aplicar filtros' }));
    await screen.findByRole('table', { name: 'Desglose' });
    await waitFor(() => expect(api.calls.length).toBe(before + 2));
    // The total and the breakdown, each with the filter, in sequence.
    expect(filtersSent(api.calls.slice(before))).toEqual([sur, sur]);
    expect(document.activeElement?.id).toBe('filters-applied');
    expect(
      screen.getByText('Filtros aplicados: antes de agregar, ventas.region es igual a «Sur».'),
    ).toBeTruthy();
    await fireEvent.click(screen.getByRole('button', { name: 'Limpiar filtros' }));
    await waitFor(() => expect(api.calls.length).toBe(before + 4));
    expect(filtersSent(api.calls.slice(before + 2))).toEqual([null, null]);
    expect(document.activeElement?.id).toBe('filters-add');
    expect(screen.getByText('Sin filtros aplicados.')).toBeTruthy();
    expect(api.overlapped()).toBe(false);
  });

  it('a draft that cannot be sent is never sent: the error is said and focused', async () => {
    const api = server(answering(() => ['ventas.region']));
    render(Page);
    await screen.findByTestId('kpi-t');
    const before = api.calls.length;
    await fireEvent.click(screen.getByRole('button', { name: 'Añadir condición' }));
    const value = screen.getByLabelText('Valor');
    await fireEvent.input(value, { target: { value: 'x'.repeat(257) } });
    await fireEvent.click(screen.getByRole('button', { name: 'Aplicar filtros' }));
    expect(screen.getByRole('alert').textContent).toMatch(/Corrige los errores/);
    expect(document.activeElement).toBe(value);
    // An empty group is an error too, not «no filter».
    await fireEvent.click(screen.getByRole('button', { name: 'Quitar condición' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Añadir grupo' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Quitar condición' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Aplicar filtros' }));
    expect(document.activeElement?.id).toBe('filters-notice');
    expect(api.calls.length).toBe(before);
  });

  it('Codex #277: refreshing the catalog cannot apply a draft absent from the editor', async () => {
    const api = server(answering(() => ['ventas.region', 'ventas.ciudad']));
    render(Page);
    await screen.findByTestId('kpi-t');
    await fireEvent.click(screen.getByRole('button', { name: 'Añadir condición' }));
    await fireEvent.input(screen.getByLabelText('Valor'), { target: { value: 'Sur' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Actualizar catálogo' }));
    await screen.findByRole('button', { name: 'Añadir condición' });
    expect(screen.queryByLabelText('Valor')).toBeNull();
    const before = api.calls.length;
    await fireEvent.click(screen.getByRole('button', { name: 'Aplicar filtros' }));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(filtersSent(api.calls.slice(before))).not.toContainEqual(sur);
  });

  it('refreshing with an unapplied draft offers nothing to apply and asks without it', async () => {
    const api = server(answering(() => ['ventas.region', 'ventas.ciudad']));
    render(Page);
    await screen.findByTestId('kpi-t');
    await fireEvent.click(screen.getByRole('button', { name: 'Añadir condición' }));
    await fireEvent.input(screen.getByLabelText('Valor'), { target: { value: 'Sur' } });
    const before = api.calls.length;
    await fireEvent.click(screen.getByRole('button', { name: 'Actualizar catálogo' }));
    await screen.findByTestId('kpi-t');
    expect(screen.getByText('Sin filtros aplicados.')).toBeTruthy();
    expect(screen.queryByText('Hay cambios sin aplicar.')).toBeNull();
    expect(
      screen.getByRole<HTMLButtonElement>('button', { name: 'Aplicar filtros' }).disabled,
    ).toBe(true);
    const sent = filtersSent(api.calls.slice(before).filter((c) => !c.url.endsWith('/catalog')));
    expect(sent).toEqual([null]);
  });

  it('refreshing while editing applied filters keeps them as applied and drops the edit', async () => {
    const api = server(answering(() => ['ventas.region', 'ventas.ciudad']));
    render(Page);
    await screen.findByTestId('kpi-t');
    await fireEvent.click(screen.getByRole('button', { name: 'Añadir condición' }));
    await fireEvent.input(screen.getByLabelText('Valor'), { target: { value: 'Sur' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Aplicar filtros' }));
    await waitFor(() => expect(filtersSent(api.calls).at(-1)).toEqual(sur));
    await fireEvent.input(screen.getByLabelText('Valor'), { target: { value: 'Norte' } });
    expect(screen.getByText('Hay cambios sin aplicar.')).toBeTruthy();
    const before = api.calls.length;
    await fireEvent.click(screen.getByRole('button', { name: 'Actualizar catálogo' }));
    await screen.findByTestId('kpi-t');
    expect(screen.getByLabelText<HTMLInputElement>('Valor').value).toBe('Sur');
    expect(screen.queryByText('Hay cambios sin aplicar.')).toBeNull();
    expect(
      screen.getByRole<HTMLButtonElement>('button', { name: 'Aplicar filtros' }).disabled,
    ).toBe(true);
    // The question asked again is the applied one, never the edit.
    const sent = filtersSent(api.calls.slice(before).filter((c) => !c.url.endsWith('/catalog')));
    expect(sent).toEqual([sur]);
  });

  it('applied filters the refreshed catalog no longer allows are shown, never sent', async () => {
    let fields = ['ventas.region', 'ventas.ciudad'];
    const api = server(answering(() => fields));
    render(Page);
    await screen.findByTestId('kpi-t');
    await fireEvent.click(screen.getByRole('button', { name: 'Añadir condición' }));
    await fireEvent.input(screen.getByLabelText('Valor'), { target: { value: 'Sur' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Aplicar filtros' }));
    await waitFor(() => expect(filtersSent(api.calls).at(-1)).toEqual(sur));
    fields = ['ventas.ciudad'];
    const before = api.calls.length;
    await fireEvent.click(screen.getByRole('button', { name: 'Actualizar catálogo' }));
    expect((await screen.findAllByRole('alert')).map((a) => a.textContent).join(' ')).toMatch(
      /ya no se pueden consultar/,
    );
    // Only the catalog was asked: no query with the clause removed.
    expect(api.calls.slice(before).map((c) => c.url)).toEqual(['/api/v1/semantic/catalog']);
  });
});

describe('metric filters after aggregating, with the same editor (#149)', () => {
  const ids = { importe_total: 'm:importe_total', filas: 'm:filas' } as const;
  const over = (metric: keyof typeof ids, comparison: string, value: string) => ({
    op: 'compare',
    target: { kind: 'metric', selection: ids[metric] },
    comparison,
    value: { type: 'number', value },
  });
  const norte = {
    op: 'compare',
    target: { kind: 'dimension', name: 'ventas.region' },
    comparison: '=',
    value: { type: 'text', value: 'Norte' },
  };
  const typed = (metrics = ventas.metrics) => ({
    ...ventas,
    metrics,
    fields: ['ventas.region', 'ventas.ciudad'].map((name) => ({
      ...member(name, 'values', true),
      type: 'VARCHAR',
    })),
  });
  /** The recorded total, without its row: a total excluded by `having`. */
  const excluded = () => {
    const copy = structuredClone(total);
    copy.result.blocks[0]!.rows = [];
    copy.result.blocks[0]!.row_count = 0;
    return copy;
  };
  const answering =
    (models = () => [typed()], kpi = (): object => total) =>
    (call: Call) =>
      call.url.endsWith('/catalog')
        ? catalog(models())
        : Response.json(grouped(call) ? region : kpi());
  type Sent = { filter?: unknown; having?: unknown; selections: { name: string }[] };
  const queries = (calls: Call[]) =>
    calls.filter((c) => !c.url.endsWith('/catalog')).map((c) => c.body!.query as Sent);
  const sent = (calls: Call[]) => queries(calls).map((q) => [q.filter ?? null, q.having ?? null]);
  const metrics = () => within(screen.getByRole('region', { name: 'Filtros de métricas' }));
  const rows = () => within(screen.getByRole('region', { name: 'Filtros de filas' }));
  const applyButton = () =>
    screen.getByRole<HTMLButtonElement>('button', { name: 'Aplicar filtros' });
  /** Clicks «Aplicar» as a person can: jsdom would deliver a click to a disabled button. */
  async function apply() {
    expect(applyButton().disabled).toBe(false);
    await fireEvent.click(applyButton());
  }
  /** Writes one condition after aggregating, starting the tree if it is empty. */
  async function writeMetric(metric: string, operator: string, value: string) {
    const start = metrics().queryByRole('button', { name: 'Añadir condición de métrica' });
    if (start) await fireEvent.click(start);
    await fireEvent.change(metrics().getAllByLabelText('Métrica').at(-1)!, {
      target: { value: metric },
    });
    await fireEvent.change(metrics().getAllByLabelText('Operador').at(-1)!, {
      target: { value: operator },
    });
    await fireEvent.input(metrics().getAllByLabelText('Valor').at(-1)!, { target: { value } });
  }
  const check = (name: string) => fireEvent.click(screen.getByRole('checkbox', { name }));

  it('offers only the selected metrics the session may filter and sends them as having', async () => {
    const unfilterable = {
      ...member('filas'),
      capabilities: { ...member('filas').capabilities, filter: false },
    };
    const api = server(
      answering(() => [typed([member('importe_total'), unfilterable, member('margen', 'masked')])]),
    );
    render(Page);
    await screen.findByTestId('kpi-t');
    await check('filas');
    await check('ventas.region');
    await screen.findByRole('table', { name: 'Desglose' });
    await fireEvent.click(metrics().getByRole('button', { name: 'Añadir condición de métrica' }));
    // Selected and filterable only: neither `filas` (no filter) nor `margen` (withheld).
    const options = [...metrics().getByLabelText<HTMLSelectElement>('Métrica').options];
    expect(options.map((o) => o.value)).toEqual(['importe_total']);
    // The rows' editor still names fields only.
    expect(rows().queryByLabelText('Métrica')).toBeNull();
    const before = api.calls.length;
    await writeMetric('importe_total', '>', '100.00');
    expect(api.calls.length).toBe(before);
    await apply();
    await waitFor(() => expect(api.calls.length).toBe(before + 2));
    // Both reads, in sequence, each with the same `having` and no `where`.
    expect(sent(api.calls.slice(before))).toEqual([
      [null, over('importe_total', '>', '100.00')],
      [null, over('importe_total', '>', '100.00')],
    ]);
    expect(api.overlapped()).toBe(false);
    expect(document.activeElement?.id).toBe('filters-applied');
    expect(document.activeElement?.textContent?.trim()).toBe(
      'Filtros aplicados: después de agregar, importe_total es mayor que 100.00.',
    );
  });

  it('before and after aggregating apply together, at their places, and clear together', async () => {
    const api = server(answering());
    render(Page);
    await screen.findByTestId('kpi-t');
    await check('ventas.region');
    await screen.findByRole('table', { name: 'Desglose' });
    await fireEvent.click(rows().getByRole('button', { name: 'Añadir condición' }));
    await fireEvent.input(rows().getByLabelText('Valor'), { target: { value: 'Norte' } });
    await writeMetric('importe_total', '>', '100.00');
    const before = api.calls.length;
    await apply();
    await waitFor(() => expect(api.calls.length).toBe(before + 2));
    const both = [norte, over('importe_total', '>', '100.00')];
    expect(sent(api.calls.slice(before))).toEqual([both, both]);
    expect(document.getElementById('filters-applied')!.textContent!.trim()).toBe(
      'Filtros aplicados: antes de agregar, ventas.region es igual a «Norte»; después de agregar, importe_total es mayor que 100.00.',
    );
    await fireEvent.click(screen.getByRole('button', { name: 'Limpiar filtros' }));
    await waitFor(() => expect(api.calls.length).toBe(before + 4));
    expect(sent(api.calls.slice(before + 2))).toEqual([
      [null, null],
      [null, null],
    ]);
    expect(metrics().getByRole('button', { name: 'Añadir condición de métrica' })).toBeTruthy();
    expect(api.overlapped()).toBe(false);
  });

  it('a total excluded after aggregating says so, invents no figure, and shows the breakdown', async () => {
    let filtered = false;
    server(answering(undefined, () => (filtered ? excluded() : total)));
    render(Page);
    await screen.findByTestId('kpi-t');
    await check('ventas.region');
    await screen.findByRole('table', { name: 'Desglose' });
    await writeMetric('importe_total', '<', '100.20');
    filtered = true;
    await apply();
    expect((await screen.findByTestId('kpi-empty')).textContent).toBe(
      'No hay un total que cumpla este filtro.',
    );
    expect(screen.queryByTestId('kpi-t')).toBeNull();
    expect(screen.queryByRole('alert')).toBeNull();
    // The breakdown is the server's, read on its own: never summed into a total.
    expect(screen.getByRole('table', { name: 'Desglose' })).toBeTruthy();
    expect(document.body.textContent).not.toContain('300,30');
  });

  it('removing the filtered metric invalidates its clause; nothing is asked until it is back', async () => {
    const api = server(answering());
    render(Page);
    await screen.findByTestId('kpi-t');
    await check('filas');
    await writeMetric('importe_total', '>', '100.00');
    await apply();
    await waitFor(() => expect(queries(api.calls).at(-1)!.having).toBeTruthy());
    const before = api.calls.length;
    await check('importe_total');
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('La métrica no está seleccionada en esta consulta.');
    const select = metrics().getByLabelText<HTMLSelectElement>('Métrica');
    expect(select.getAttribute('aria-invalid')).toBe('true');
    expect(select.selectedOptions[0]!.textContent!.trim()).toBe('importe_total (no disponible)');
    expect(screen.queryByTestId('kpi-t')).toBeNull();
    await new Promise((resolve) => setTimeout(resolve, 20));
    // No clause dropped, none moved to `filas`: nothing is asked.
    expect(api.calls.length).toBe(before);
    // Nothing was edited, so nothing can be applied: only correcting or clearing it.
    expect(applyButton().disabled).toBe(true);
    // Back, now second in the order: the clause still names it, never `filas`.
    await check('importe_total');
    await waitFor(() => expect(api.calls.length).toBe(before + 1));
    const asked = queries(api.calls).at(-1)!;
    expect(asked.selections.map((s) => s.name)).toEqual(['filas', 'importe_total']);
    expect(asked.having).toEqual(over('importe_total', '>', '100.00'));
    expect(select.getAttribute('aria-invalid')).toBeNull();
  });

  it('a drafted metric condition whose metric is removed is never sent', async () => {
    const api = server(answering());
    render(Page);
    await screen.findByTestId('kpi-t');
    await check('filas');
    await writeMetric('importe_total', '>', '100.00');
    await check('importe_total');
    // The question changed and is asked without the unapplied draft.
    await waitFor(() =>
      expect(
        queries(api.calls)
          .at(-1)!
          .selections.map((s) => s.name),
      ).toEqual(['filas']),
    );
    expect(queries(api.calls).some((q) => q.having)).toBe(false);
    const before = api.calls.length;
    await apply();
    expect(screen.getByRole('alert').textContent).toMatch(/Corrige los errores/);
    expect(document.activeElement).toBe(metrics().getByLabelText('Métrica'));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(api.calls.length).toBe(before);
  });

  it('literals at the metric type, exactly: an INT64 past 2^53 travels as text, a lost digit never', async () => {
    const counted = { ...member('filas'), type: 'INT64' };
    const api = server(answering(() => [typed([member('importe_total'), counted])]));
    render(Page);
    await screen.findByTestId('kpi-t');
    await check('filas');
    await writeMetric('importe_total', '=', '1.234');
    const before = api.calls.length;
    await apply();
    const value = metrics().getByLabelText<HTMLInputElement>('Valor');
    expect(document.activeElement).toBe(value);
    expect(document.getElementById(value.getAttribute('aria-describedby')!)!.textContent).toBe(
      'El valor no cabe exacto en DECIMAL(18,2).',
    );
    expect(api.calls.length).toBe(before);
    await writeMetric('filas', '>=', '9007199254740993');
    await apply();
    await waitFor(() => expect(api.calls.length).toBe(before + 1));
    expect(queries(api.calls).at(-1)!.having).toEqual(over('filas', '>=', '9007199254740993'));
  });

  it('a neutral 404 retires the selection, never the filters the editors show', async () => {
    let revoked = false;
    const api = server((call) =>
      call.url.endsWith('/catalog')
        ? catalog([typed()])
        : revoked
          ? problem(404, 'model_not_found')
          : Response.json(grouped(call) ? region : total),
    );
    render(Page);
    await screen.findByTestId('kpi-t');
    await writeMetric('importe_total', '>', '100.00');
    await apply();
    await waitFor(() => expect(queries(api.calls).at(-1)!.having).toBeTruthy());
    revoked = true;
    await check('ventas.region');
    expect((await screen.findAllByRole('alert')).map((a) => a.textContent).join(' ')).toContain(
      'ya no está disponible',
    );
    expect(screen.getByText(/^Filtros aplicados:/).textContent).toContain(
      'después de agregar, importe_total es mayor que 100.00',
    );
    revoked = false;
    await check('importe_total');
    await screen.findByTestId('kpi-t');
    // Asked with the clause on screen, never silently without it.
    expect(metrics().getByLabelText<HTMLInputElement>('Valor').value).toBe('100.00');
    expect(queries(api.calls).at(-1)!.having).toEqual(over('importe_total', '>', '100.00'));
  });

  it('a late answer of a former filter never shows; a denial or an unconfirmed audit shows no figure', async () => {
    let release!: () => void;
    const held = new Promise<void>((resolve) => (release = resolve));
    let answer: (call: Call) => Response | Promise<Response> = answering();
    // The held answer ignores its abort, as a slow network may: only the page discards it.
    server((call) => answer(call));
    render(Page);
    await screen.findByTestId('kpi-t');
    await writeMetric('importe_total', '>', '100.00');
    answer = async () => {
      await held;
      const late = structuredClone(total);
      late.result.blocks[0]!.rows = [['999.99', '9']];
      return Response.json(late);
    };
    await apply();
    await waitFor(() => expect(screen.getByText('Consultando…')).toBeTruthy());
    // Another question before it arrives: the late answer is discarded.
    answer = answering();
    await fireEvent.click(screen.getByRole('button', { name: 'Limpiar filtros' }));
    await screen.findByTestId('kpi-t');
    release();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(document.body.textContent).not.toContain('999,99');
    expect(screen.getByTestId('kpi-t').textContent!.trim()).toBe('300,30');
    // Denied: the neutral failure, nothing of the former answer kept.
    answer = (call) =>
      call.url.endsWith('/catalog') ? catalog([typed()]) : problem(403, 'denied');
    await writeMetric('importe_total', '>', '100.00');
    await apply();
    expect((await screen.findByRole('alert')).textContent).toContain('HTTP 403 · Código: denied');
    expect(screen.queryByTestId('kpi-t')).toBeNull();
    // The audit of the breakdown not confirmed: the whole answer is withdrawn.
    answer = (call) =>
      call.url.endsWith('/catalog')
        ? catalog([typed()])
        : grouped(call)
          ? problem(503, 'audit_unavailable')
          : Response.json(total);
    await check('ventas.region');
    expect((await screen.findByRole('alert')).textContent).toContain('audit_unavailable');
    expect(screen.queryByTestId('kpi-t')).toBeNull();
    expect(screen.queryByRole('table')).toBeNull();
    expect(document.body.textContent).not.toMatch(/300,30|100,30|200,00/);
  });
});

describe('drill from a cell and back, with the coordinator (#143)', () => {
  const field = (name: string, type = 'VARCHAR', capabilities = {}) => ({
    ...member(name, 'values', true),
    type,
    capabilities: { ...member(name, 'values', true).capabilities, ...capabilities },
  });
  const drillModel = (
    fields = ['ventas.region', 'ventas.ciudad', 'ventas.nota'].map((n) => field(n)),
  ) => ({
    ...ventas,
    fields,
  });
  type Query = {
    selections: { id: string; kind: string; name: string }[];
    filter?: unknown;
    having?: unknown;
  };
  // Canned answers by the dimension asked: the double does not evaluate filters.
  const ROWS: Record<string, (string | null)[][]> = {
    'ventas.region': [['Norte', '100.30']],
    'ventas.ciudad': [
      ['La Paz', '100.10'],
      ['El Alto', '0.20'],
    ],
    'ventas.nota': [[null, '100.10']],
  };
  /** A recorded answer whose block carries the very selections of the request. */
  function run(query: Query, rows?: (string | null)[][]) {
    const answer = structuredClone(total) as typeof total;
    const dimension = query.selections.find((s) => s.kind === 'dimension');
    const body = rows ?? (dimension ? ROWS[dimension.name]! : [['100.30']]);
    answer.result.blocks[0] = {
      ...answer.result.blocks[0]!,
      columns: query.selections.map((s) => ({
        selection_id: s.id,
        kind: s.kind,
        id: `project/${s.kind}/${s.name}`,
        type:
          s.kind === 'metric'
            ? 'DECIMAL(38,2)'
            : s.name === 'ventas.instante'
              ? 'TIMESTAMPTZ'
              : 'VARCHAR',
        nullable: true,
        unit: null,
        timezone: null,
        masked: false,
      })),
      rows: body,
      row_count: body.length,
      limit: null,
    } as (typeof answer.result.blocks)[0];
    return Response.json(answer);
  }
  const query = (call: Call) => call.body!.query as Query;
  const asking =
    (models = () => [drillModel()]) =>
    (call: Call) =>
      call.url.endsWith('/catalog') ? catalog(models()) : run(query(call));
  const asked = (calls: Call[]) =>
    calls
      .filter((c) => !c.url.endsWith('/catalog'))
      .map((c) => ({
        names: query(c).selections.map((s) => s.name),
        filter: query(c).filter ?? null,
      }));
  const norte = {
    op: 'compare',
    target: { kind: 'dimension', name: 'ventas.region' },
    comparison: '=',
    value: { type: 'text', value: 'Norte' },
  };
  const laPaz = {
    ...norte,
    target: { kind: 'dimension', name: 'ventas.ciudad' },
    value: { type: 'text', value: 'La Paz' },
  };
  const cellButton = (text: string) =>
    within(screen.getByRole('table', { name: 'Desglose' })).getByRole('button', {
      name: new RegExp(`^${text} — explorar`),
    });
  const path = () => within(screen.getByRole('navigation', { name: 'Camino de exploración' }));
  async function byRegion() {
    await screen.findByTestId('kpi-m:importe_total');
    await fireEvent.click(screen.getByRole('checkbox', { name: 'ventas.region' }));
    await screen.findByRole('table', { name: 'Desglose' });
  }
  async function step(value: string, to: string) {
    await fireEvent.click(cellButton(value));
    const target = screen.getByLabelText<HTMLSelectElement>('Desglosar por');
    expect(document.activeElement).toBe(target);
    await fireEvent.change(target, { target: { value: to } });
    await fireEvent.click(screen.getByRole('button', { name: 'Explorar' }));
  }

  it('two steps, Atrás and the first breadcrumb: each asked once, in sequence, exactly', async () => {
    const api = server(asking());
    render(Page);
    await byRegion();
    const before = api.calls.length;
    await step('Norte', 'ventas.ciudad');
    await waitFor(() => expect(api.calls.length).toBe(before + 2));
    // The total and the breakdown of the step, each with the cell's filter.
    expect(asked(api.calls.slice(before))).toEqual([
      { names: ['importe_total'], filter: norte },
      { names: ['ventas.ciudad', 'importe_total'], filter: norte },
    ]);
    expect(api.overlapped()).toBe(false);
    expect(document.activeElement?.id).toBe('drill-current');
    expect(path().getByRole('button', { name: 'Inicio: por ventas.region' })).toBeTruthy();
    expect(screen.getByText('ventas.region = «Norte», por ventas.ciudad')).toBeTruthy();
    // The editor shows the filter the step applied.
    expect(
      screen.getByText(/^Filtros aplicados: antes de agregar, ventas.region es igual a «Norte»/),
    ).toBeTruthy();

    await screen.findByRole('table', { name: 'Desglose' });
    await step('La Paz', 'ventas.nota');
    await waitFor(() => expect(api.calls.length).toBe(before + 4));
    expect(asked(api.calls).at(-1)).toEqual({
      names: ['ventas.nota', 'importe_total'],
      filter: { op: 'and', filters: [norte, laPaz] },
    });
    // A NULL cell starts a step too: offered as what it is.
    await screen.findByRole('table', { name: 'Desglose' });
    expect(cellButton('Sin dato')).toBeTruthy();

    // Atrás: by city with región = Norte, asked again with current permissions.
    await fireEvent.click(path().getByRole('button', { name: 'Atrás' }));
    await waitFor(() => expect(api.calls.length).toBe(before + 6));
    expect(asked(api.calls).at(-1)).toEqual({
      names: ['ventas.ciudad', 'importe_total'],
      filter: norte,
    });
    expect(screen.getByText('ventas.region = «Norte», por ventas.ciudad')).toBeTruthy();
    // The first breadcrumb: the original question, the path gone.
    await fireEvent.click(path().getByRole('button', { name: 'Inicio: por ventas.region' }));
    await waitFor(() => expect(api.calls.length).toBe(before + 8));
    expect(asked(api.calls).at(-1)).toEqual({
      names: ['ventas.region', 'importe_total'],
      filter: null,
    });
    expect(screen.queryByRole('navigation', { name: 'Camino de exploración' })).toBeNull();
    expect(document.activeElement?.id).toBe('explore-answer');
    expect(api.overlapped()).toBe(false);
  });

  it('Cancelar asks nothing and gives the focus back to the cell', async () => {
    const api = server(asking());
    render(Page);
    await byRegion();
    const before = api.calls.length;
    const cell = cellButton('Norte');
    await fireEvent.click(cell);
    await fireEvent.click(screen.getByRole('button', { name: 'Cancelar' }));
    expect(document.activeElement).toBe(cell);
    expect(screen.queryByLabelText('Desglosar por')).toBeNull();
    expect(api.calls.length).toBe(before);
  });

  it('unapplied filters stop a step and a return, and stay as drafted', async () => {
    const api = server(asking());
    render(Page);
    await byRegion();
    await step('Norte', 'ventas.ciudad');
    await screen.findByRole('table', { name: 'Desglose' });
    const rowsRegion = within(screen.getByRole('region', { name: 'Filtros de filas' }));
    await fireEvent.click(rowsRegion.getByRole('button', { name: 'Añadir otra condición' }));
    const before = api.calls.length;
    await fireEvent.click(cellButton('La Paz'));
    expect(document.activeElement?.id).toBe('drill-notice');
    expect(screen.getByRole('alert').textContent).toMatch(/sin aplicar/);
    await fireEvent.click(path().getByRole('button', { name: 'Atrás' }));
    expect(document.activeElement?.id).toBe('drill-notice');
    expect(api.calls.length).toBe(before);
    expect(screen.getByText('Hay cambios sin aplicar.')).toBeTruthy();
    expect(rowsRegion.getAllByLabelText('Campo')).toHaveLength(2);
  });

  it('Codex #292: a reverted edit before a step is not pending after it; Aplicar keeps its filter', async () => {
    const api = server(asking());
    render(Page);
    await byRegion();
    const rowsRegion = () => within(screen.getByRole('region', { name: 'Filtros de filas' }));
    await fireEvent.click(rowsRegion().getByRole('button', { name: 'Añadir condición' }));
    await fireEvent.click(rowsRegion().getByRole('button', { name: 'Quitar condición' }));
    expect(screen.queryByText('Hay cambios sin aplicar.')).toBeNull();
    await step('Norte', 'ventas.ciudad');
    await screen.findByRole('table', { name: 'Desglose' });
    expect(asked(api.calls).at(-1)?.filter).toEqual(norte);
    expect(screen.queryByText('Hay cambios sin aplicar.')).toBeNull();
    expect(rowsRegion().getByLabelText<HTMLInputElement>('Valor').value).toBe('Norte');
    await fireEvent.click(screen.getByRole('button', { name: 'Aplicar filtros' }));
    await screen.findByRole('table', { name: 'Desglose' });
    expect(asked(api.calls).at(-1)?.filter).toEqual(norte);
    // The next step is not stopped by a draft that is no longer there.
    const before = api.calls.length;
    await step('La Paz', 'ventas.nota');
    await waitFor(() => expect(api.calls.length).toBe(before + 2));
    expect(asked(api.calls).at(-1)).toEqual({
      names: ['ventas.nota', 'importe_total'],
      filter: { op: 'and', filters: [norte, laPaz] },
    });
  });

  it('Codex #292: a reverted edit before Atrás is not pending after it; Aplicar asks no filter', async () => {
    const api = server(asking());
    render(Page);
    await byRegion();
    await step('Norte', 'ventas.ciudad');
    await screen.findByRole('table', { name: 'Desglose' });
    const rowsRegion = () => within(screen.getByRole('region', { name: 'Filtros de filas' }));
    const value = rowsRegion().getByLabelText<HTMLInputElement>('Valor');
    await fireEvent.input(value, { target: { value: 'Sur' } });
    expect(screen.getByText('Hay cambios sin aplicar.')).toBeTruthy();
    await fireEvent.input(value, { target: { value: 'Norte' } });
    expect(screen.queryByText('Hay cambios sin aplicar.')).toBeNull();
    const before = api.calls.length;
    await fireEvent.click(path().getByRole('button', { name: 'Atrás' }));
    await waitFor(() => expect(api.calls.length).toBe(before + 2));
    await screen.findByRole('table', { name: 'Desglose' });
    expect(asked(api.calls).at(-1)).toEqual({
      names: ['ventas.region', 'importe_total'],
      filter: null,
    });
    expect(screen.queryByText('Hay cambios sin aplicar.')).toBeNull();
    expect(rowsRegion().queryByLabelText('Valor')).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: 'Aplicar filtros' }));
    await screen.findByRole('table', { name: 'Desglose' });
    expect(asked(api.calls).at(-1)?.filter).toBeNull();
  });

  it('a denied step keeps no old detail; Atrás asks again with the current permissions', async () => {
    let denied = false;
    const api = server((call) => {
      if (call.url.endsWith('/catalog')) return catalog([drillModel()]);
      return denied ? problem(403, 'semantic_access_denied') : run(query(call));
    });
    render(Page);
    await byRegion();
    denied = true;
    await step('Norte', 'ventas.ciudad');
    expect((await screen.findByRole('alert')).textContent).toContain('HTTP 403');
    expect(screen.queryByRole('table')).toBeNull();
    expect(screen.queryByTestId('kpi-m:importe_total')).toBeNull();
    denied = false;
    const before = api.calls.length;
    await fireEvent.click(path().getByRole('button', { name: 'Atrás' }));
    await screen.findByRole('table', { name: 'Desglose' });
    expect(asked(api.calls.slice(before))).toEqual([
      { names: ['importe_total'], filter: null },
      { names: ['ventas.region', 'importe_total'], filter: null },
    ]);
  });

  it('a neutral 404 retires the path with the question', async () => {
    let gone = false;
    server((call) => {
      if (call.url.endsWith('/catalog')) return catalog([drillModel()]);
      return gone ? problem(404, 'model_not_found') : run(query(call));
    });
    render(Page);
    await byRegion();
    await step('Norte', 'ventas.ciudad');
    await screen.findByRole('navigation', { name: 'Camino de exploración' });
    await screen.findByRole('table', { name: 'Desglose' });
    gone = true;
    await step('La Paz', 'ventas.nota');
    expect((await screen.findAllByRole('alert')).map((a) => a.textContent).join(' ')).toContain(
      'ya no está disponible',
    );
    expect(screen.queryByRole('navigation', { name: 'Camino de exploración' })).toBeNull();
  });

  it('a late answer of a step left by Atrás is discarded', async () => {
    let release!: () => void;
    const held = new Promise<void>((resolve) => (release = resolve));
    server(async (call) => {
      if (call.url.endsWith('/catalog')) return catalog([drillModel()]);
      // The step's reads arrive only after the person went back.
      if (query(call).filter !== undefined) {
        await held;
        return run(query(call), [['Tarija', '999.99']]);
      }
      return run(query(call));
    });
    render(Page);
    await byRegion();
    await step('Norte', 'ventas.ciudad');
    await waitFor(() => expect(screen.getByText('Consultando…')).toBeTruthy());
    await fireEvent.click(path().getByRole('button', { name: 'Atrás' }));
    await screen.findByRole('table', { name: 'Desglose' });
    release();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(document.body.textContent).not.toMatch(/999,99|Tarija/);
    expect(cellButton('Norte')).toBeTruthy();
  });

  it('offers no step from an unfiltrable or instant cell, nor from a metric', async () => {
    const fields = [
      field('ventas.region', 'VARCHAR', { filter: false }),
      field('ventas.ciudad'),
      field('ventas.instante', 'TIMESTAMPTZ'),
    ];
    server((call) => {
      if (call.url.endsWith('/catalog')) return catalog([drillModel(fields)]);
      const q = query(call);
      const dimensions = q.selections.filter((s) => s.kind === 'dimension');
      return run(q, [
        [
          ...dimensions.map((d) =>
            d.name === 'ventas.instante' ? '2026-01-01T00:00:00Z' : 'Norte',
          ),
          '1.00',
        ],
      ]);
    });
    render(Page);
    await screen.findByTestId('kpi-m:importe_total');
    await fireEvent.click(screen.getByRole('checkbox', { name: 'ventas.region' }));
    await fireEvent.click(screen.getByRole('checkbox', { name: 'ventas.instante' }));
    const table = await screen.findByRole('table', { name: 'Desglose' });
    await waitFor(() => expect(within(table).getAllByRole('columnheader')).toHaveLength(3));
    // `region` may not be filtered, an instant has no literal, a metric is no origin.
    expect(within(table).queryAllByRole('button')).toHaveLength(0);
  });

  it('another model starts without a path', async () => {
    const other = { ...drillModel(), model: 'metrics/otro.yaml', name: 'otro' };
    server(asking(() => [drillModel(), other]));
    render(Page);
    await byRegion();
    await step('Norte', 'ventas.ciudad');
    await screen.findByRole('navigation', { name: 'Camino de exploración' });
    await fireEvent.change(screen.getByLabelText('Modelo'), {
      target: { value: 'metrics/otro.yaml' },
    });
    expect(screen.queryByRole('navigation', { name: 'Camino de exploración' })).toBeNull();
  });
});
