import { cleanup, render, screen, waitFor, within } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { SemanticRun } from './exact';
import KpiStrip from './KpiStrip.svelte';
import ReceiptPanel from './ReceiptPanel.svelte';
import ResultChart from './ResultChart.svelte';
import ResultTable from './ResultTable.svelte';
// Answers of the real server (#138), regenerated and compared by the backend
// test `semantic_fixtures`: never written by hand.
import deprecated from './fixtures/deprecated.json';
import extremes from './fixtures/extremes.json';
import instant from './fixtures/instant.json';
import limited from './fixtures/limited.json';
import region from './fixtures/region.json';
import total from './fixtures/total.json';
import wide from './fixtures/wide.json';

const mountBars = vi.hoisted(() => vi.fn());
vi.mock('./bars', () => ({ mountBars }));

const real = (fixture: unknown) => structuredClone(fixture) as SemanticRun;
const first = (fixture: unknown) => real(fixture).result.blocks[0]!;
const cells = (table: HTMLElement) =>
  within(table)
    .getAllByRole('row')
    .slice(1)
    .map((row) =>
      within(row)
        .getAllByRole('cell')
        .map((cell) => cell.textContent!.trim()),
    );

describe('exact table', () => {
  it('shows the oracle by region as exact text, with the limit the plan applied', () => {
    render(ResultTable, { block: first(region), caption: 'Ventas por región' });
    const table = screen.getByRole('table', { name: 'Ventas por región' });
    expect(
      within(table)
        .getAllByRole('columnheader')
        .map((h) => h.textContent!.trim()),
    ).toEqual(['ventas.region', 'importe_total', 'filas']);
    expect(cells(table)).toEqual([
      ['Norte', '100,30', '2'],
      ['Sur', '200,00', '2'],
    ]);
    expect(screen.getByTestId('applied-limit').textContent).toMatch(
      /Límite aplicado: 1000\s+filas\./,
    );
  });

  it('says more rows may exist when the limit cut the answer', () => {
    render(ResultTable, { block: first(limited), caption: 'Primera región' });
    expect(cells(screen.getByRole('table'))).toEqual([['Norte', '100,30']]);
    expect(screen.getByTestId('applied-limit').textContent).toMatch(
      /1\s+fila; puede haber\s+más filas que no se muestran/,
    );
  });

  it('keeps extremes of INT64 and DECIMAL(38,10) digit for digit, and missing values as such', () => {
    render(ResultTable, { block: first(extremes), caption: 'Extremos' });
    expect(cells(screen.getByRole('table'))).toEqual([
      [
        'A',
        '9.007.199.254.740.993',
        '-9.223.372.036.854.775.808',
        '9.999.999.999.999.999.999.999.999.999,9999999998',
      ],
      ['B', '9.223.372.036.854.775.807', '9.223.372.036.854.775.807', '-12.345,6789012345'],
      ['C', 'Sin dato', 'Sin dato', 'Sin dato'],
      ['Sin dato', '42', '42', '0,0000000001'],
    ]);
  });

  it('shows instants in UTC and the governed zone of a time bucket', () => {
    render(ResultTable, { block: first(instant), caption: 'Por mes' });
    const table = screen.getByRole('table');
    expect(within(table).getAllByRole('columnheader')[0]!.textContent).toContain(
      'Zona horaria: America/La_Paz',
    );
    expect(cells(table)).toEqual([
      ['2025-12-01', '2026-01-01 00:00:00 UTC', '2'],
      ['2026-01-01', '2026-01-02 00:00:00 UTC', '2'],
    ]);
  });

  // The server refuses to execute masked values today (masked_unsupported),
  // so this contract case is the real answer with its column withheld.
  it('tells a withheld column apart from missing values', () => {
    const withheld = first(region);
    withheld.columns[1]!.masked = true;
    withheld.rows.forEach((row) => (row[1] = null));
    render(ResultTable, { block: withheld, caption: 'Con máscara' });
    expect(screen.getAllByLabelText('Valor oculto')).toHaveLength(2);
    expect(
      screen.getByRole('columnheader', { name: /Valores ocultos por tus permisos/ }),
    ).toBeTruthy();
  });

  it('shows no figure at all when one cell breaks the contract', () => {
    const broken = first(region);
    broken.rows[1]![1] = 200 as never;
    render(ResultTable, { block: broken, caption: 'Roto' });
    expect(screen.getByRole('alert').textContent).toContain('no se muestra ninguna cifra');
    expect(screen.queryByRole('table')).toBeNull();
    expect(document.body.textContent).not.toContain('100,30');
  });
});

describe('KPIs', () => {
  it('show the total the server aggregated', () => {
    render(KpiStrip, { run: real(total) });
    expect(screen.getByTestId('kpi-t').textContent!.trim()).toBe('300,30');
    expect(screen.getByTestId('kpi-f').textContent!.trim()).toBe('4');
  });

  it('refuse a grouped or limited answer instead of adding its rows', () => {
    for (const fixture of [region, limited]) {
      const { unmount } = render(KpiStrip, { run: real(fixture) });
      expect(screen.getByRole('alert').textContent).toContain('no es un total sin desglose');
      expect(document.body.textContent).not.toMatch(/300,30|100,30/);
      unmount();
    }
  });
});

describe('chart', () => {
  it('draws relative heights with exact tooltips, and replaces the former question', async () => {
    const dispose = vi.fn();
    mountBars.mockReturnValue(dispose);
    const rendered = render(ResultChart, { block: first(region) });
    await waitFor(() => expect(mountBars).toHaveBeenCalledTimes(1));
    const [, projection, axes] = mountBars.mock.calls[0]!;
    expect(projection.categories).toEqual(['Norte', 'Sur']);
    expect(projection.exact).toEqual(['100,30', '200,00']);
    expect(projection.heights).toEqual([50.15, 100]);
    expect(axes).toEqual(['ventas.region', 'importe_total']);
    await rendered.rerender({ block: first(limited) });
    await waitFor(() => expect(mountBars).toHaveBeenCalledTimes(2));
    expect(dispose).toHaveBeenCalledTimes(1);
    expect(mountBars.mock.calls[1]![1].categories).toEqual(['Norte']);
  });

  it('charts the metric asked for, and nothing when a chart would deform', async () => {
    mountBars.mockReturnValue(() => {});
    render(ResultChart, { block: first(region), metric: 'f' });
    await waitFor(() => expect(mountBars).toHaveBeenCalled());
    expect(mountBars.mock.calls[0]![1].exact).toEqual(['2', '2']);
    render(ResultChart, { block: first(total) });
    expect(screen.getByTestId('chart-omitted')).toBeTruthy();
  });

  it('never draws after an immediate unmount, and reports a renderer failure', async () => {
    const rendered = render(ResultChart, { block: first(region) });
    rendered.unmount();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(mountBars).not.toHaveBeenCalled();
    mountBars.mockImplementationOnce(() => {
      throw new Error('renderer unavailable');
    });
    render(ResultChart, { block: first(region) });
    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toContain('Consulta los valores en la tabla'),
    );
  });
});

describe('beyond DECIMAL(38)', () => {
  // The cube of `importe`: DECIMAL(56,6), summed by the adapter's exact emulation.
  it('shows the real DECIMAL(56,6) total digit for digit in the table and as a KPI', () => {
    render(ResultTable, { block: first(wide), caption: 'Cubo' });
    expect(cells(screen.getByRole('table'))).toEqual([['9.027.027,009000']]);
    render(KpiStrip, { run: real(wide) });
    expect(screen.getByTestId('kpi-c').textContent!.trim()).toBe('9.027.027,009000');
  });

  it('omits the chart of the total, and charts its column by region with exact tooltips', async () => {
    render(ResultChart, { block: first(wide) });
    expect(screen.getByTestId('chart-omitted')).toBeTruthy();
    // The server answers no grouped wide block yet: the wide column's own
    // contract and value, on the grouped rows of the region answer.
    const byRegion = first(region);
    const [cube] = first(wide).columns;
    byRegion.columns[1] = cube!;
    byRegion.rows[0]![1] = first(wide).rows[0]![0]!;
    mountBars.mockReturnValue(() => {});
    render(ResultChart, { block: byRegion });
    await waitFor(() => expect(mountBars).toHaveBeenCalledTimes(1));
    const [, projection, axes] = mountBars.mock.calls[0]!;
    expect(projection.exact).toEqual(['9.027.027,009000', '200,00']);
    expect(projection.heights[0]).toBe(100);
    expect(projection.heights[1]).toBeCloseTo((200 / 9027027.009) * 100, 6);
    expect(axes).toEqual(['ventas.region', 'cubo']);
  });
});

describe('a value that does not fit its declared type shows no figure anywhere', () => {
  it.each([
    ['DECIMAL(3,2)', '1234.56'],
    ['DATE', '2026-02-30'],
    ['TIMESTAMPTZ', '2026-13-40T25:61:61Z'],
  ])('%s holding %s: no table, no KPI, no chart', (type, value) => {
    // The review's reproduction: the region answer reduced to its first row.
    const reduced = first(region);
    reduced.columns[1]!.type = type;
    reduced.rows = [reduced.rows[0]!];
    reduced.row_count = 1;
    reduced.rows[0]![1] = value;
    render(ResultTable, { block: reduced, caption: 'Roto' });
    expect(screen.getByRole('alert').textContent).toContain('no se muestra ninguna cifra');
    expect(screen.queryByRole('table')).toBeNull();
    cleanup();

    const kpi = real(total);
    kpi.result.blocks[0]!.columns[0]!.type = type;
    kpi.result.blocks[0]!.rows[0]![0] = value;
    render(KpiStrip, { run: kpi });
    expect(screen.getByRole('alert').textContent).toContain('no se muestra ninguna cifra');
    expect(screen.queryByTestId('kpi-f')).toBeNull();
    cleanup();

    // Charting the other metric: only the broken cell keeps the chart away.
    render(ResultChart, { block: reduced, metric: 'f' });
    expect(screen.getByTestId('chart-omitted')).toBeTruthy();
    expect(mountBars).not.toHaveBeenCalled();
    expect(document.body.textContent).not.toMatch(/1234|2026|Norte/);
  });
});

describe('receipt', () => {
  it('names a deprecated metric and only a replacement the session may see', () => {
    const run = real(deprecated);
    render(ReceiptPanel, { receipt: run.receipt, notices: run.notices });
    const notes = screen
      .getAllByRole('note')
      .map((note) => note.textContent!.replace(/\s+/g, ' ').trim());
    expect(notes).toEqual([
      'La métrica antigua está obsoleta y se retira el 2027-06-30.',
      'La métrica vieja está obsoleta y se retira el 2027-01-01. Usa importe_total.',
    ]);
    expect(document.body.textContent).not.toMatch(/reserva|centinela/);
  });

  it('states an unversioned source and unknown freshness, never inventing them', () => {
    const run = real(region);
    render(ReceiptPanel, { receipt: run.receipt, notices: run.notices });
    const source = screen.getByTestId('receipt-source').textContent!.replace(/\s+/g, ' ');
    expect(source).toContain('sin versión persistente');
    expect(source).toContain('frescura desconocida');
    expect(screen.getByText(run.receipt.id)).toBeTruthy();
    expect(document.body.textContent).toContain(run.receipt.fingerprints.query);
    // The receipt carries no SQL, roles or physical relations: neither does the panel.
    expect(document.body.textContent).not.toMatch(/SELECT|viewer|analyst|lake\.main/);
  });
});

describe('columns are addressed by their selection', () => {
  it('shows no table when two columns share one', () => {
    const twin = first(region);
    twin.columns[2]!.selection_id = twin.columns[1]!.selection_id;
    render(ResultTable, { block: twin, caption: 'Repetidas' });
    expect(screen.getByRole('alert').textContent).toContain('no se muestra ninguna cifra');
    expect(screen.queryByRole('table')).toBeNull();
  });
});

describe('an excluded total (#266)', () => {
  const empty = () => {
    const run = real(total);
    run.result.blocks[0]!.rows = [];
    run.result.blocks[0]!.row_count = 0;
    return run;
  };

  it('says there is no total, with no invented card, when the question may exclude it', () => {
    render(KpiStrip, { run: empty(), emptyAllowed: true });
    expect(screen.getByTestId('kpi-empty').textContent).toBe(
      'No hay un total que cumpla este filtro.',
    );
    expect(screen.queryByRole('term')).toBeNull();
    expect(screen.queryByRole('alert')).toBeNull();
    cleanup();
    // The same answer to a question without that filter is incompatible.
    render(KpiStrip, { run: empty() });
    expect(screen.getByRole('alert')).toBeTruthy();
    expect(screen.queryByTestId('kpi-empty')).toBeNull();
  });
});
