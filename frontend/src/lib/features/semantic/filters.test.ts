import fc from 'fast-check';
import { describe, expect, it } from 'vitest';
import type { CatalogModel } from '#lib/api/semantic.ts';
import { askable, reconcile, requests, type Choice } from './explore';
import {
  dimensionId,
  filterProblems,
  filterQuery,
  InvalidFilters,
  MAX_FILTER_DEPTH,
  MAX_FILTER_NODES,
  MAX_TEXT,
  metricId,
  NO_FILTERS,
  restoreFilters,
  saveFilters,
  type SemanticFilter,
  type SemanticTarget,
  type SemanticValue,
  type UserFilters,
} from './filters';

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
const MODEL: CatalogModel = {
  model: 'metrics/ventas.yaml',
  name: 'Ventas',
  fields: [
    member('ventas.region', 'VARCHAR'),
    member('ventas.dia', 'DATE'),
    member('ventas.instante', 'TIMESTAMPTZ'),
    member('ventas.activo', 'BOOLEAN'),
    member('ventas.secreto', 'VARCHAR', false),
  ],
  metrics: [
    member('filas', 'INT64'),
    member('importe_total', 'DECIMAL(38,2)'),
    member('margen', 'DECIMAL(38,2)', false, 'masked'),
  ],
};
const SELECTED = ['filas', 'importe_total'];

const dim = (name: string, roles?: string[]): SemanticTarget =>
  roles ? { kind: 'dimension', name, roles } : { kind: 'dimension', name };
const met = (name: string): SemanticTarget => ({ kind: 'metric', selection: metricId(name) });
const num = (value: string): SemanticValue => ({ type: 'number', value });
const text = (value: string): SemanticValue => ({ type: 'text', value });
const eq = (target: SemanticTarget, value: SemanticValue): SemanticFilter => ({
  op: 'compare',
  target,
  comparison: '=',
  value,
});
const where = (filter: SemanticFilter): UserFilters => ({ where: filter, having: null });
const having = (filter: SemanticFilter): UserFilters => ({ where: null, having: filter });
const problems = (filters: UserFilters, metrics = SELECTED) =>
  filterProblems(filters, MODEL, metrics).map((p) => p.path);

describe('accepted literals stay exact text at their type', () => {
  it.each([
    ['INT64 past 2^53', having(eq(met('filas'), num('9007199254740993')))],
    ['INT64 at its lower bound', having(eq(met('filas'), num('-9223372036854775808')))],
    ['INT64 at its upper bound', having(eq(met('filas'), num('9223372036854775807')))],
    [
      'an exact DECIMAL',
      having(eq(met('importe_total'), num('-123456789012345678901234567890123456.78'))),
    ],
    ['a text that reads NULL', where(eq(dim('ventas.region'), text('NULL')))],
    ['an empty text', where(eq(dim('ventas.region'), text('')))],
    ['a leap day', where(eq(dim('ventas.dia'), { type: 'date', value: '2028-02-29' }))],
    ['a boolean', where(eq(dim('ventas.activo'), { type: 'boolean', value: false }))],
    ['IS NULL on a timestamp', where({ op: 'is_null', target: dim('ventas.instante') })],
  ])('%s', (_, filters) => {
    expect(problems(filters)).toEqual([]);
    // Sent as they are: nothing rounded, coerced nor dropped.
    expect(filterQuery(filters)).toEqual({
      ...(filters.where ? { filter: filters.where } : {}),
      ...(filters.having ? { having: filters.having } : {}),
    });
  });

  it('keeps relationship roles as semantic references', () => {
    const filters = where(eq(dim('ventas.region', ['facturacion']), text('Norte')));
    expect(problems(filters)).toEqual([]);
    expect(filterQuery(filters).filter).toEqual(filters.where);
  });
});

describe('refused before sending, never trimmed to fit', () => {
  const deep = (levels: number): SemanticFilter =>
    levels === 0
      ? eq(dim('ventas.region'), text('Norte'))
      : { op: 'not', filter: deep(levels - 1) };
  const many = (n: number): SemanticFilter[] =>
    Array.from({ length: n }, () => eq(dim('ventas.region'), text('Norte')));

  it.each([
    [
      'INT64 past its upper bound',
      having(eq(met('filas'), num('9223372036854775808'))),
      '/having/value',
    ],
    ['INT64 with a fraction', having(eq(met('filas'), num('1.5'))), '/having/value'],
    ['DECIMAL losing a digit', having(eq(met('importe_total'), num('1.234'))), '/having/value'],
    [
      'DECIMAL past its precision',
      having(eq(met('importe_total'), num('1' + '0'.repeat(36)))),
      '/having/value',
    ],
    ['a float written as exponent', having(eq(met('importe_total'), num('1e3'))), '/having/value'],
    ['a number as text', having(eq(met('filas'), text('4'))), '/having/value'],
    ['NULL compared', where(eq(dim('ventas.region'), { type: 'null' })), '/filter/value'],
    [
      'an impossible date',
      where(eq(dim('ventas.dia'), { type: 'date', value: '2026-02-30' })),
      '/filter/value',
    ],
    [
      'a timestamp literal',
      where(eq(dim('ventas.instante'), text('2026-01-01T00:00:00Z'))),
      '/filter/value',
    ],
    [
      'an oversized text',
      where(eq(dim('ventas.region'), text('x'.repeat(MAX_TEXT + 1)))),
      '/filter/value',
    ],
    ['an empty group', where({ op: 'and', filters: [] }), '/filter'],
    [
      'an empty IN',
      where({ op: 'in', target: dim('ventas.region'), values: [] }),
      '/filter/values',
    ],
    [
      'a field the session may not filter',
      where(eq(dim('ventas.secreto'), text('x'))),
      '/filter/target',
    ],
    ['a field that does not exist', where(eq(dim('ventas.nada'), text('x'))), '/filter/target'],
    [
      'a masked-only metric',
      having(eq({ kind: 'metric', selection: metricId('margen') }, num('1'))),
      '/having/target',
    ],
    [
      'a metric not selected',
      having(eq(met('importe_total'), num('1'))),
      '/having/target',
      ['filas'],
    ],
    ['a metric before aggregating', where(eq(met('filas'), num('1'))), '/filter/target'],
    [
      'a dimension after aggregating',
      having(eq(dim('ventas.region'), text('x'))),
      '/having/target',
    ],
    ['an empty role', where(eq(dim('ventas.region', ['']), text('x'))), '/filter/target'],
  ] as [string, UserFilters, string, string[]?][])('%s', (_, filters, path, metrics = SELECTED) => {
    expect(problems(filters, metrics)).toContain(path);
  });

  it(`allows ${MAX_FILTER_DEPTH} levels of nesting, not one more`, () => {
    expect(problems(where(deep(MAX_FILTER_DEPTH)))).toEqual([]);
    expect(problems(where(deep(MAX_FILTER_DEPTH + 1))).length).toBeGreaterThan(0);
  });

  it(`counts ${MAX_FILTER_NODES} conditions over both filters together`, () => {
    // One group node plus its children on each side.
    const half = (MAX_FILTER_NODES - 2) / 2;
    const fits = { where: { op: 'and', filters: many(half) }, having: null } as UserFilters;
    expect(problems(fits)).toEqual([]);
    const both: UserFilters = {
      where: { op: 'and', filters: many(half) },
      having: {
        op: 'and',
        filters: Array.from({ length: half + 1 }, () => eq(met('filas'), num('1'))),
      },
    };
    expect(problems(both).length).toBeGreaterThan(0);
    // Each alone fits: only the shared count refuses them.
    expect(problems({ ...both, where: null })).toEqual([]);
  });
});

describe('the question carries the filters, and loses none of them', () => {
  const choice: Choice = {
    model: MODEL.model,
    metrics: ['filas', 'importe_total'],
    dimensions: ['ventas.region'],
    filters: {
      where: eq(dim('ventas.region'), text('Norte')),
      having: eq(met('importe_total'), num('100.30')),
    },
  };

  it('sends them in both reads, by stable ids', () => {
    const { kpi, table } = requests(choice);
    for (const request of [kpi, table!]) {
      expect(request.query.filter).toEqual(choice.filters!.where);
      expect(request.query.having).toEqual(choice.filters!.having);
    }
    expect(kpi.query.selections.map((s) => s.id)).toEqual(['m:filas', 'm:importe_total']);
    expect(askable(choice, MODEL)).toBe(true);
    // Unchecked filters are never sent.
    expect(askable(choice)).toBe(false);
  });

  it('makes a removed metric an explicit orphan, never another metric', () => {
    const fewer = { ...choice, metrics: ['filas'] };
    expect(askable(fewer, MODEL)).toBe(false);
    expect(filterProblems(fewer.filters!, MODEL, fewer.metrics)).toEqual([
      { path: '/having/target', message: 'La métrica no está seleccionada en esta consulta.' },
    ]);
    // The refreshed catalog keeps the clause for the person to see.
    const refreshed = reconcile(choice, {
      status: 'ready',
      models: [{ ...MODEL, metrics: MODEL.metrics.filter((m) => m.name !== 'importe_total') }],
    });
    expect(refreshed?.filters).toEqual(choice.filters);
    expect(askable(refreshed, MODEL)).toBe(false);
  });

  it('a question without filters is asked as before', () => {
    const plain = { model: MODEL.model, metrics: ['filas'], dimensions: [] };
    expect(askable(plain)).toBe(true);
    expect(requests(plain).kpi.query).toEqual({
      selections: [{ id: 'm:filas', kind: 'metric', name: 'filas' }],
    });
    expect(askable({ ...plain, filters: NO_FILTERS })).toBe(true);
  });
});

// Any filter of the contract's shape, over any names and values.
const values: fc.Arbitrary<SemanticValue> = fc.oneof(
  fc.constant({ type: 'null' as const }),
  fc.boolean().map((value) => ({ type: 'boolean' as const, value })),
  fc.string().map((value) => ({ type: 'number' as const, value })),
  fc.string().map((value) => ({ type: 'text' as const, value })),
  fc.string().map((value) => ({ type: 'date' as const, value })),
);
const names = fc.constantFrom('ventas.region', 'ventas.dia', 'ventas.secreto', 'ventas.nada', 'x');
const targets: fc.Arbitrary<SemanticTarget> = fc.oneof(
  fc.record({ kind: fc.constant('dimension' as const), name: names }),
  fc.record({
    kind: fc.constant('dimension' as const),
    name: names,
    roles: fc.array(fc.string(), { maxLength: 2 }),
  }),
  fc.record({
    kind: fc.constant('metric' as const),
    selection: fc.constantFrom('filas', 'importe_total', 'margen', 'otra').map(metricId),
  }),
);
const filters = fc.letrec<{ filter: SemanticFilter }>((tie) => ({
  filter: fc.oneof(
    { depthSize: 'small', withCrossShrink: true },
    fc.record({
      op: fc.constant('compare' as const),
      target: targets,
      comparison: fc.constantFrom(
        '=' as const,
        '<>' as const,
        '<' as const,
        '<=' as const,
        '>' as const,
        '>=' as const,
      ),
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
const states = fc.record({
  where: fc.option(filters, { nil: null }),
  having: fc.option(filters, { nil: null }),
});

const targetsOf = (filter: SemanticFilter | null): SemanticTarget[] => {
  if (filter === null) return [];
  if (filter.op === 'and' || filter.op === 'or') return filter.filters.flatMap(targetsOf);
  if (filter.op === 'not') return targetsOf(filter.filter);
  return [filter.target];
};

describe('properties', () => {
  it('saving and restoring keeps every clause, NULL and text exactly', () => {
    fc.assert(
      fc.property(states, (state) => {
        expect(restoreFilters(saveFilters(state))).toEqual(state);
      }),
    );
  });

  it('a filter it accepts never filters by what the session may not filter', () => {
    fc.assert(
      fc.property(states, (state) => {
        if (filterProblems(state, MODEL, SELECTED).length > 0) return;
        for (const target of targetsOf(state.where)) {
          expect(target.kind).toBe('dimension');
          const field = MODEL.fields.find(
            (f) => target.kind === 'dimension' && f.name === target.name,
          );
          expect(field?.capabilities.filter).toBe(true);
        }
        for (const target of targetsOf(state.having)) {
          expect(
            target.kind === 'metric' && SELECTED.map(metricId).includes(target.selection),
          ).toBe(true);
        }
        // And it is sent exactly as it is.
        expect(filterQuery(state)).toEqual({
          ...(state.where ? { filter: state.where } : {}),
          ...(state.having ? { having: state.having } : {}),
        });
      }),
    );
  });

  it('restores nothing outside the contract: the whole state is refused', () => {
    fc.assert(
      fc.property(fc.jsonValue(), (junk) => {
        try {
          const restored = restoreFilters(JSON.stringify(junk));
          // Only a state of the contract's shape comes back, unchanged.
          expect(JSON.parse(saveFilters(restored))).toEqual(junk);
        } catch (error) {
          expect(error).toBeInstanceOf(InvalidFilters);
        }
      }),
    );
  });

  it('refuses a clause with an unknown key instead of dropping it', () => {
    const tampered = JSON.stringify({
      version: 1,
      where: {
        op: 'and',
        filters: [{ ...eq(dim('ventas.region'), text('Norte')), roles_grant: ['admin'] }],
      },
      having: null,
    });
    expect(() => restoreFilters(tampered)).toThrow(InvalidFilters);
  });
});

// Reproductions of Codex's review of #274, kept as regressions.
describe('a filtered question is never asked as another one (Codex review of #274)', () => {
  const plain = (name: string) => ({
    name,
    type: 'INT64',
    lifecycle: { state: 'active' as const },
    capabilities: { select: 'values' as const, filter: true, group: true, order: true },
  });
  const ventas: CatalogModel = {
    model: 'metrics/ventas.yaml',
    name: 'Ventas',
    fields: [],
    metrics: [plain('old'), plain('new')],
  };
  const choice: Choice = {
    model: ventas.model,
    metrics: ['old'],
    dimensions: [],
    filters: {
      where: null,
      having: {
        op: 'compare',
        target: { kind: 'metric', selection: metricId('old') },
        comparison: '>',
        value: { type: 'number', value: '100' },
      },
    },
  };

  it('keeps the predicate, blocked, when the last selected metric disappears', () => {
    const updated = { ...ventas, metrics: [plain('new')] };
    const next = reconcile(choice, { status: 'ready', models: [updated] });
    expect(next?.filters).toEqual(choice.filters);
    expect(next?.metrics).toEqual([]);
    expect(askable(next, updated)).toBe(false);
  });

  it('never asks another model, nor the same one with no catalog left', () => {
    const other = { ...ventas, model: 'metrics/other.yaml' };
    const moved = reconcile(choice, { status: 'ready', models: [other] });
    expect(moved).toEqual(choice);
    expect(askable(moved, other)).toBe(false);
    expect(reconcile(choice, { status: 'ready', models: [] })).toEqual(choice);
  });

  it('a question without filters starts over as before', () => {
    const plainChoice = { model: ventas.model, metrics: ['gone'], dimensions: [] };
    expect(reconcile(plainChoice, { status: 'ready', models: [ventas] })).toEqual({
      model: ventas.model,
      metrics: ['old'],
      dimensions: [],
    });
  });
});

describe('stable ids within the bound (Codex review of #274)', () => {
  const long = (n: number, tail = '') => 'a'.repeat(n - tail.length) + tail;
  const catalogWith = (metric: string, field: string): CatalogModel => ({
    model: 'metrics/m.yaml',
    name: 'm',
    fields: [member(field, 'VARCHAR')],
    metrics: [member(metric, 'INT64')],
  });

  it.each([254, 255, MAX_TEXT])('a name of %i characters keeps an id of at most 256', (n) => {
    for (const id of [metricId(long(n)), dimensionId(long(n))]) {
      expect([...id].length).toBeLessThanOrEqual(MAX_TEXT);
    }
    // Names that only differ past the kept start still get different ids.
    expect(metricId(long(n, 'x'))).not.toBe(metricId(long(n, 'y')));
  });

  it('asks a question over the longest names, its HAVING by the same id', () => {
    const [metric, field] = [long(MAX_TEXT, 'm'), `d.${long(MAX_TEXT - 2)}`];
    const current = catalogWith(metric, field);
    const choice: Choice = {
      model: current.model,
      metrics: [metric],
      dimensions: [field],
      filters: {
        where: null,
        having: eq({ kind: 'metric', selection: metricId(metric) }, num('1')),
      },
    };
    expect(askable(choice, current)).toBe(true);
    const { table, labels } = requests(choice);
    for (const s of table!.query.selections) expect([...s.id].length).toBeLessThanOrEqual(MAX_TEXT);
    expect(labels[metricId(metric)]).toBe(metric);
    expect(table!.query.having).toEqual(choice.filters!.having);
  });

  it('refuses a question whose selections would share an id', () => {
    const name = long(MAX_TEXT);
    const twin = `${[...name].slice(0, MAX_TEXT - 19).join('')}~${metricId(name).slice(-16)}`;
    const current: CatalogModel = {
      model: 'metrics/m.yaml',
      name: 'm',
      fields: [],
      metrics: [member(name, 'INT64'), member(twin, 'INT64')],
    };
    const choice = { model: current.model, metrics: [name, twin], dimensions: [] };
    expect(metricId(twin)).toBe(metricId(name));
    expect(askable(choice, current)).toBe(false);
  });
});
