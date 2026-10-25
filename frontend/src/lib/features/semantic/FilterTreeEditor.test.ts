import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import fc from 'fast-check';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { CatalogModel } from '#lib/api/semantic.ts';
import type { SemanticFilter, SemanticTarget, SemanticValue } from './filters';
import { fromFilter, keys, toFilter, type TreeChange } from './filterTree';
import FilterTreeEditor from './FilterTreeEditor.svelte';

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
// The fixture of #148: the governed project's fields and one of each type.
const MODEL: CatalogModel = {
  model: 'metrics/ventas.yaml',
  name: 'Ventas',
  fields: [
    member('ventas.region', 'VARCHAR'),
    member('ventas.ciudad', 'VARCHAR'),
    member('ventas.instante', 'TIMESTAMPTZ'),
    member('ventas.dia', 'DATE'),
    member('ventas.unidades', 'INT64'),
    member('ventas.activo', 'BOOLEAN'),
    member('ventas.nota', 'VARCHAR', false, 'masked'),
  ],
  metrics: [member('importe_total', 'DECIMAL(38,2)')],
};

const region = { kind: 'dimension' as const, name: 'ventas.region' };
const ciudad = { kind: 'dimension' as const, name: 'ventas.ciudad' };
const is = (target: SemanticTarget, value: string): SemanticFilter => ({
  op: 'compare',
  target,
  comparison: '=',
  value: { type: 'text', value },
});
// The Issue's tree, as structure only: never evaluated here.
const ISSUE: SemanticFilter = {
  op: 'or',
  filters: [is(region, 'Norte'), { op: 'not', filter: is(ciudad, 'El Alto') }],
};

afterEach(cleanup);

function tree(initial: SemanticFilter | null) {
  const changes: TreeChange[] = [];
  render(FilterTreeEditor, {
    model: MODEL,
    initial,
    id: 'f',
    onchange: (change: TreeChange) => changes.push(change),
  });
  return { changes, last: () => changes.at(-1)! };
}
const group = (legend: RegExp) => screen.getAllByRole('group', { name: legend });
const focused = () => document.activeElement?.id;

describe('a tree that keeps its intent', () => {
  it('renders the Issue tree as it is, and adds without touching the rest', async () => {
    const { last } = tree(ISSUE);
    const or = group(/Grupo O/)[0]!;
    expect(within(or).getByRole('group', { name: /^NO/ })).toBeTruthy();
    await fireEvent.click(within(or).getByRole('button', { name: 'Añadir condición al grupo' }));
    expect(last().filter).toEqual({
      op: 'or',
      filters: [is(region, 'Norte'), { op: 'not', filter: is(ciudad, 'El Alto') }, is(region, '')],
    });
    // The new condition has the focus, and the change is said.
    expect(focused()).toMatch(/^f-n\d+-field$/);
    expect(screen.getByText('Condición añadida al grupo.')).toBeTruthy();
  });

  it('never turns OR into AND but on request', async () => {
    const { last } = tree(ISSUE);
    await fireEvent.change(screen.getByLabelText('Tipo de grupo'), { target: { value: 'and' } });
    expect(last().filter).toMatchObject({
      op: 'and',
      filters: ISSUE.op === 'or' ? ISSUE.filters : [],
    });
    expect(group(/Grupo Y/)).toHaveLength(1);
  });

  it('nests two levels and removes one, the focus following', async () => {
    const { last } = tree(ISSUE);
    const or = group(/Grupo O/)[0]!;
    await fireEvent.click(within(or).getByRole('button', { name: 'Añadir grupo dentro' }));
    const inner = group(/Grupo Y/)[0]!;
    await fireEvent.click(within(inner).getByRole('button', { name: 'Añadir grupo dentro' }));
    expect(last().filter).toEqual({
      op: 'or',
      filters: [
        ...(ISSUE.op === 'or' ? ISSUE.filters : []),
        { op: 'and', filters: [is(region, ''), { op: 'and', filters: [is(region, '')] }] },
      ],
    });
    // Its own button comes after the groups it holds.
    await fireEvent.click(within(inner).getAllByRole('button', { name: 'Quitar grupo' }).at(-1)!);
    expect(last().filter).toEqual(ISSUE);
    // Back to the group that held it.
    expect(focused()).toMatch(/^f-n\d+-add$/);
    expect(screen.getByText('Elemento quitado del filtro.')).toBeTruthy();
  });

  it('negates and un-negates exactly the node asked', async () => {
    const { last } = tree(ISSUE);
    await fireEvent.click(screen.getByRole('button', { name: 'Quitar negación' }));
    expect(last().filter).toEqual({
      op: 'or',
      filters: [is(region, 'Norte'), is(ciudad, 'El Alto')],
    });
    await fireEvent.click(screen.getAllByRole('button', { name: 'Negar esta condición' })[0]!);
    expect(last().filter).toEqual({
      op: 'or',
      filters: [{ op: 'not', filter: is(region, 'Norte') }, is(ciudad, 'El Alto')],
    });
  });
});

describe('problems shown where they are, nothing trimmed', () => {
  it('no filter and an empty group are not the same', async () => {
    tree(null);
    expect(screen.getByText(/Sin filtros/)).toBeTruthy();
    cleanup();
    const { last } = tree({ op: 'and', filters: [] });
    const empty = group(/Grupo Y/)[0]!;
    expect(within(empty).getByRole('status').textContent).toMatch(/al menos una condición/);
    await fireEvent.click(within(empty).getByRole('button', { name: 'Añadir condición al grupo' }));
    expect(last().problems).toEqual([]);
  });

  it(`a tree past 16 levels says so at its node and keeps every level`, async () => {
    let deep: SemanticFilter = is(region, 'Norte');
    for (let i = 0; i < 17; i++) deep = { op: 'not', filter: deep };
    const { last } = tree(deep);
    expect(screen.getAllByText(/16 niveles de anidamiento/).length).toBeGreaterThan(0);
    await fireEvent.input(screen.getByLabelText('Valor'), { target: { value: 'Sur' } });
    let depth = 0;
    let node = last().filter!;
    while (node.op === 'not') {
      depth++;
      node = node.filter;
    }
    expect(depth).toBe(17);
    expect(last().problems.length).toBeGreaterThan(0);
  });

  it('a condition no draft writes as it is stays as it is', async () => {
    const nullCompared: SemanticFilter = {
      op: 'compare',
      target: region,
      comparison: '=',
      value: { type: 'null' },
    };
    const numberOnText: SemanticFilter = {
      op: 'compare',
      target: region,
      comparison: '=',
      value: { type: 'number', value: '5' },
    };
    const { last } = tree({
      op: 'and',
      filters: [nullCompared, numberOnText, is(ciudad, 'Sucre')],
    });
    expect(screen.getAllByRole('group', { name: 'Condición guardada' })).toHaveLength(2);
    await fireEvent.input(screen.getByLabelText('Valor'), { target: { value: 'Tarija' } });
    expect(last().filter).toEqual({
      op: 'and',
      filters: [nullCompared, numberOnText, is(ciudad, 'Tarija')],
    });
    expect(last().problems.map((p) => p.path)).toEqual(
      expect.arrayContaining(['/filter/filters/0/value', '/filter/filters/1/value']),
    );
  });

  it('a field the session may not filter stays named and invalid', async () => {
    const { last } = tree({
      op: 'and',
      filters: [is({ kind: 'dimension', name: 'ventas.nota' }, 'x'), is(region, 'Norte')],
    });
    await fireEvent.input(screen.getAllByLabelText('Valor')[1]!, { target: { value: 'Sur' } });
    expect(last().filter).toMatchObject({ filters: [{ target: { name: 'ventas.nota' } }, {}] });
    expect(last().problems.map((p) => p.path)).toContain('/filter/filters/0/target');
  });

  it('editing never calls the API', async () => {
    const fetch = vi.spyOn(globalThis, 'fetch');
    tree(null);
    await fireEvent.click(screen.getByRole('button', { name: 'Añadir grupo' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Añadir condición al grupo' }));
    expect(fetch).not.toHaveBeenCalled();
    fetch.mockRestore();
  });
});

// Any filter of the contract's shape over the fixture's names (as in #142).
const values: fc.Arbitrary<SemanticValue> = fc.oneof(
  fc.constant({ type: 'null' as const }),
  fc.boolean().map((value) => ({ type: 'boolean' as const, value })),
  fc.string().map((value) => ({ type: 'number' as const, value })),
  fc.string().map((value) => ({ type: 'text' as const, value })),
  fc.string().map((value) => ({ type: 'date' as const, value })),
);
const targets: fc.Arbitrary<SemanticTarget> = fc.oneof(
  fc.record({
    kind: fc.constant('dimension' as const),
    name: fc.constantFrom(...MODEL.fields.map((f) => f.name), 'x'),
  }),
  fc.record({ kind: fc.constant('metric' as const), selection: fc.constant('m:importe_total') }),
);
const filters = fc.letrec<{ filter: SemanticFilter }>((tie) => ({
  filter: fc.oneof(
    { depthSize: 'small', withCrossShrink: true },
    fc.record({
      op: fc.constant('compare' as const),
      target: targets,
      comparison: fc.constantFrom('=' as const, '<>' as const, '<' as const),
      value: values,
    }),
    fc.record({ op: fc.constant('between' as const), target: targets, low: values, high: values }),
    fc.record({
      op: fc.constant('in' as const),
      target: targets,
      values: fc.array(values, { maxLength: 3 }),
    }),
    fc.record({ op: fc.constantFrom('is_null' as const, 'is_not_null' as const), target: targets }),
    fc.record({
      op: fc.constantFrom('and' as const, 'or' as const),
      filters: fc.array(tie('filter'), { maxLength: 3 }),
    }),
    fc.record({ op: fc.constant('not' as const), filter: tie('filter') }),
  ),
})).filter;

describe('properties', () => {
  it('the tree of any filter writes it back exactly', () => {
    fc.assert(
      fc.property(filters, (filter) => {
        expect(toFilter(fromFilter(filter, MODEL, keys()), MODEL)).toEqual(filter);
      }),
    );
  });
});
