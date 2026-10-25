import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import fc from 'fast-check';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { CatalogModel } from '#lib/api/semantic.ts';
import {
  condition,
  draftOf,
  metricsDomain,
  operators,
  rowsDomain,
  type ConditionChange,
  type Draft,
  type Operator,
} from './condition';
import ConditionEditor from './ConditionEditor.svelte';
import { metricId } from './filters';

const member = (
  name: string,
  type: string,
  filter = true,
  select: 'values' | 'masked' = 'values',
) => ({
  name,
  type,
  lifecycle: { state: 'active' as const },
  capabilities: { select, filter, group: true, order: true },
});
// The governed project's fields (#147), plus one of each type the editor adapts to.
const MODEL: CatalogModel = {
  model: 'metrics/ventas.yaml',
  name: 'Ventas',
  fields: [
    member('ventas.region', 'VARCHAR'),
    member('ventas.ciudad', 'VARCHAR'),
    member('ventas.instante', 'TIMESTAMPTZ'),
    member('ventas.dia', 'DATE'),
    member('ventas.unidades', 'INT64'),
    member('ventas.precio', 'DECIMAL(18,2)'),
    member('ventas.activo', 'BOOLEAN'),
    member('ventas.nota', 'VARCHAR', false, 'masked'),
  ],
  metrics: [member('importe_total', 'DECIMAL(38,2)')],
};

const ROWS = rowsDomain(MODEL);

afterEach(cleanup);

/** The editor, and every change it reported. */
function editor(initial: Draft) {
  const changes: ConditionChange[] = [];
  const onremove = vi.fn();
  render(ConditionEditor, {
    domain: ROWS,
    initial,
    id: 'c0',
    onchange: (change: ConditionChange) => changes.push(change),
    onremove,
  });
  const last = () => changes.at(-1)!;
  return { changes, last, onremove };
}
const field = () => screen.getByLabelText('Campo') as HTMLSelectElement;
const operator = () => screen.getByLabelText('Operador') as HTMLSelectElement;
const type = async (label: string, text: string) =>
  fireEvent.input(screen.getByLabelText(label), { target: { value: text } });
const describedError = (control: HTMLElement) => {
  expect(control.getAttribute('aria-invalid')).toBe('true');
  return document.getElementById(control.getAttribute('aria-describedby')!)!.textContent!;
};

describe('a condition on a field the session may filter', () => {
  it('offers only filterable fields: shown masked never grants a filter', () => {
    editor({ field: 'ventas.region', operator: '=', values: [''] });
    const names = within(field())
      .getAllByRole('option')
      .map((o) => o.textContent);
    expect(names).not.toContain('ventas.nota');
    expect(names).toContain('ventas.region');
  });

  it('keeps INT64 and DECIMAL exact as text, never a Number', async () => {
    const { last } = editor({ field: 'ventas.unidades', operator: '=', values: [''] });
    for (const text of ['9007199254740993', '-9223372036854775808', '9223372036854775807']) {
      await type('Valor', text);
      expect(last().condition).toEqual({
        op: 'compare',
        target: { kind: 'dimension', name: 'ventas.unidades' },
        comparison: '=',
        value: { type: 'number', value: text },
      });
      expect(last().problems).toEqual([]);
    }
    await type('Valor', '9223372036854775808');
    expect(describedError(screen.getByLabelText('Valor'))).toMatch(/entero de 64 bits/);
    expect(last().problems).not.toEqual([]);
  });

  it('refuses a decimal it would round or a comma it would have to read', async () => {
    const { last } = editor({ field: 'ventas.precio', operator: '>=', values: ['100.30'] });
    await type('Valor', '100.30');
    expect(last().problems).toEqual([]);
    for (const text of ['1.234', '1,5', '1e3', '']) {
      await type('Valor', text);
      expect(describedError(screen.getByLabelText('Valor'))).toMatch(/DECIMAL\(18,2\)/);
      // Still exactly what was written: no other, valid condition.
      expect(last().condition).toMatchObject({ value: { type: 'number', value: text } });
    }
  });

  it('a text that reads NULL stays text; no value is the null operator', async () => {
    const { last } = editor({ field: 'ventas.region', operator: '=', values: [''] });
    await type('Valor', 'NULL');
    expect(last().condition).toMatchObject({ value: { type: 'text', value: 'NULL' } });
    expect(last().problems).toEqual([]);
    await fireEvent.change(operator(), { target: { value: 'is_null' } });
    expect(last().condition).toEqual({
      op: 'is_null',
      target: { kind: 'dimension', name: 'ventas.region' },
    });
    expect(screen.queryByLabelText('Valor')).toBeNull();
  });

  it('checks dates as the server does', async () => {
    const { last } = editor({ field: 'ventas.dia', operator: '=', values: [''] });
    await type('Valor', '2028-02-29');
    expect(last().condition).toMatchObject({ value: { type: 'date', value: '2028-02-29' } });
    expect(last().problems).toEqual([]);
    await type('Valor', '2026-02-30');
    expect(describedError(screen.getByLabelText('Valor'))).toMatch(/fecha AAAA-MM-DD/);
  });

  it('offers per type only the operators the server binds', () => {
    editor({ field: 'ventas.instante', operator: 'is_null', values: [] });
    expect(
      within(operator())
        .getAllByRole('option')
        .map((o) => (o as HTMLOptionElement).value),
    ).toEqual(['is_null', 'is_not_null']);
    expect(operators(member('b', 'BOOLEAN'))).toEqual(['=', '<>', 'is_null', 'is_not_null']);
  });

  it('refuses an oversized literal', async () => {
    const { last } = editor({ field: 'ventas.region', operator: '=', values: [''] });
    await type('Valor', 'x'.repeat(257));
    expect(describedError(screen.getByLabelText('Valor'))).toMatch(/256 caracteres/);
    expect(last().problems).not.toEqual([]);
  });
});

describe('editing never loses nor replaces intent', () => {
  it('a range and back keeps both ends', async () => {
    const { last } = editor({
      field: 'ventas.precio',
      operator: 'between',
      values: ['1.00', '9.99'],
    });
    expect(screen.getByLabelText('Desde')).toHaveProperty('value', '1.00');
    expect(screen.getByLabelText('Hasta')).toHaveProperty('value', '9.99');
    await fireEvent.change(operator(), { target: { value: '=' } });
    expect(last().condition).toMatchObject({ comparison: '=', value: { value: '1.00' } });
    await fireEvent.change(operator(), { target: { value: 'between' } });
    expect(last().condition).toMatchObject({
      op: 'between',
      low: { value: '1.00' },
      high: { value: '9.99' },
    });
  });

  it('a list adds and removes exactly the values asked', async () => {
    const { last } = editor({ field: 'ventas.region', operator: 'in', values: ['Norte'] });
    await fireEvent.click(screen.getByRole('button', { name: 'Añadir valor' }));
    await type('Valor 2', 'Sur');
    expect(last().condition).toMatchObject({
      op: 'in',
      values: [
        { type: 'text', value: 'Norte' },
        { type: 'text', value: 'Sur' },
      ],
    });
    await fireEvent.click(screen.getByRole('button', { name: 'Quitar valor 1' }));
    expect(last().condition).toMatchObject({ values: [{ type: 'text', value: 'Sur' }] });
  });

  it('a field the session may not filter stays named, invalid, never swapped', () => {
    const { changes } = editor({ field: 'ventas.nota', operator: '=', values: ['x'] });
    expect(field().value).toBe('ventas.nota');
    expect(describedError(field())).toMatch(/No se puede filtrar/);
    // Nothing reported until the person edits: nothing to send.
    expect(changes).toEqual([]);
  });

  it('an operator the new field does not admit is shown, not replaced', async () => {
    editor({ field: 'ventas.region', operator: '<', values: ['M'] });
    await fireEvent.change(field(), { target: { value: 'ventas.activo' } });
    expect(operator().value).toBe('<');
    expect(describedError(operator())).toMatch(/no se admite/);
  });

  it('removes the condition only when asked, and never calls the API', async () => {
    const fetch = vi.spyOn(globalThis, 'fetch');
    const { onremove } = editor({ field: 'ventas.region', operator: '=', values: [''] });
    await type('Valor', 'Norte');
    await fireEvent.change(operator(), { target: { value: '<>' } });
    expect(onremove).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByRole('button', { name: 'Quitar condición' }));
    expect(onremove).toHaveBeenCalledOnce();
    expect(fetch).not.toHaveBeenCalled();
    fetch.mockRestore();
  });
});

describe('a draft owned elsewhere', () => {
  it("copies an initial draft that is another component's reactive state", async () => {
    // Svelte state is a Proxy: a structured clone refuses it.
    const initial = new Proxy<Draft>(
      { field: 'ventas.region', operator: '=', values: ['Norte'] },
      {},
    );
    const { last } = editor(initial);
    await type('Valor', 'Sur');
    expect(last().condition).toMatchObject({ value: { type: 'text', value: 'Sur' } });
    expect(initial.values).toEqual(['Norte']);
  });
});

describe('drafts and conditions', () => {
  const OPERATORS: Operator[] = [
    '=',
    '<>',
    '<',
    '<=',
    '>',
    '>=',
    'between',
    'in',
    'is_null',
    'is_not_null',
  ];
  it('a condition written by a draft reads back as that draft', () => {
    fc.assert(
      fc.property(
        fc.constantFrom(...MODEL.fields.map((f) => f.name)),
        fc.constantFrom(...OPERATORS),
        fc.array(fc.string(), { minLength: 1, maxLength: 4 }),
        (name, op, values) => {
          const used =
            op === 'in'
              ? values
              : values.slice(0, op === 'between' ? 2 : op.startsWith('is_') ? 0 : 1);
          const draft: Draft = {
            field: name,
            operator: op,
            values: op === 'between' ? [...used, ''].slice(0, 2) : used,
          };
          const back = draftOf(condition(draft, ROWS), ROWS);
          // A boolean written otherwise reads back as written, still text.
          expect(back).toEqual(draft);
        },
      ),
    );
  });

  it('never drafts what a draft cannot write as it is', () => {
    const region = { kind: 'dimension' as const, name: 'ventas.region' };
    expect(
      draftOf({ op: 'compare', target: region, comparison: '=', value: { type: 'null' } }, ROWS),
    ).toBeNull();
    expect(
      draftOf({ op: 'is_null', target: { ...region, roles: ['facturacion'] } }, ROWS),
    ).toBeNull();
    expect(draftOf({ op: 'and', filters: [] }, ROWS)).toBeNull();
    expect(
      draftOf({ op: 'is_null', target: { kind: 'metric', selection: 'm:importe_total' } }, ROWS),
    ).toBeNull();
  });
});

describe('the same editor after aggregating (#149)', () => {
  // The question's metrics: one of each kind the domain must tell apart.
  const AFTER: CatalogModel = {
    ...MODEL,
    metrics: [
      member('importe_total', 'DECIMAL(38,2)'),
      member('filas', 'INT64'),
      member('margen', 'DECIMAL(38,2)', true, 'masked'),
      member('sin_filtro', 'INT64', false),
      member('otra', 'INT64'),
    ],
  };
  const SELECTED = ['importe_total', 'filas', 'sin_filtro'];
  const total = { kind: 'metric' as const, selection: metricId('importe_total') };

  function metricEditor(initial: Draft, metrics = SELECTED) {
    const changes: ConditionChange[] = [];
    const view = render(ConditionEditor, {
      domain: metricsDomain(AFTER, metrics),
      initial,
      id: 'h0',
      onchange: (change: ConditionChange) => changes.push(change),
      onremove: vi.fn(),
    });
    return { view, last: () => changes.at(-1)! };
  }
  const metric = () => screen.getByLabelText('Métrica') as HTMLSelectElement;

  it('names only the selected metrics the session may filter, by their selection id', async () => {
    const { last } = metricEditor({ field: 'importe_total', operator: '>', values: ['100.00'] });
    // Not `sin_filtro` (no filter), not `otra` (not selected), not `margen` (withheld).
    expect([...metric().options].map((o) => o.value)).toEqual(['importe_total', 'filas']);
    expect(screen.queryByLabelText('Campo')).toBeNull();
    await type('Valor', '100.10');
    expect(last().condition).toEqual({
      op: 'compare',
      target: total,
      comparison: '>',
      value: { type: 'number', value: '100.10' },
    });
    expect(last().problems).toEqual([]);
  });

  it('checks literals at the metric type, reported after aggregating beside their control', async () => {
    const { last } = metricEditor({ field: 'filas', operator: '=', values: ['1.5'] });
    expect(describedError(screen.getByLabelText('Valor'))).toBe(
      'El valor no es un entero de 64 bits.',
    );
    await type('Valor', '9223372036854775808');
    expect(last().problems).toEqual([
      { path: '/having/value', message: 'El valor no es un entero de 64 bits.' },
    ]);
    // Past 2^53, exact: text, never a Number.
    await type('Valor', '9223372036854775807');
    expect(last().problems).toEqual([]);
    expect(last().condition).toMatchObject({
      value: { type: 'number', value: '9223372036854775807' },
    });
  });

  it('a metric removed from the question keeps its condition, marked, never another', async () => {
    const { view } = metricEditor({ field: 'filas', operator: '>', values: ['1'] });
    await view.rerender({ domain: metricsDomain(AFTER, ['importe_total']) });
    expect(describedError(metric())).toBe('La métrica no está seleccionada en esta consulta.');
    expect(metric().selectedOptions[0]!.textContent!.trim()).toBe('filas (no disponible)');
    // Back in the question: the same condition, valid again.
    await view.rerender({ domain: metricsDomain(AFTER, ['importe_total', 'filas']) });
    expect(metric().getAttribute('aria-invalid')).toBeNull();
    expect(metric().value).toBe('filas');
  });

  it('drafts back exactly what it writes, and nothing of another domain', () => {
    const domain = metricsDomain(AFTER, SELECTED);
    fc.assert(
      fc.property(
        fc.constantFrom(...SELECTED),
        fc.constantFrom(...operators(member('n', 'INT64'))),
        fc.array(fc.string(), { minLength: 1, maxLength: 4 }),
        (name, op, values) => {
          const used =
            op === 'in'
              ? values
              : values.slice(0, op === 'between' ? 2 : op.startsWith('is_') ? 0 : 1);
          const draft: Draft = {
            field: name,
            operator: op,
            values: op === 'between' ? [...used, ''].slice(0, 2) : used,
          };
          expect(draftOf(condition(draft, domain), domain)).toEqual(draft);
        },
      ),
    );
    const dimension = { kind: 'dimension' as const, name: 'ventas.region' };
    expect(draftOf({ op: 'is_null', target: dimension }, domain)).toBeNull();
    expect(
      draftOf({ op: 'is_null', target: { kind: 'metric', selection: metricId('otra') } }, domain),
    ).toBeNull();
    expect(draftOf({ op: 'is_null', target: total }, ROWS)).toBeNull();
  });
});
