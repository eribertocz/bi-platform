import fc from 'fast-check';
import { describe, expect, it, vi } from 'vitest';
import { ApiError } from '#lib/api/session.ts';
import { IncompatibleResult } from './exact';
import type { CatalogModel, SemanticCatalog, SemanticRun } from '#lib/api/semantic.ts';
import {
  answer,
  askable,
  consistency,
  failure,
  initialChoice,
  MAX_DIMENSIONS,
  MAX_METRICS,
  questionProblems,
  reconcile,
  requests,
  withFilters,
} from './explore';
import { metricId, NO_FILTERS, type SemanticFilter, type SemanticTarget } from './filters';
import region from './fixtures/region.json';
import total from './fixtures/total.json';

const member = (name: string, select: 'values' | 'masked' = 'values', group = false) => ({
  name,
  type: 'INT64',
  lifecycle: { state: 'active' as const },
  capabilities: { select, filter: true, group, order: true },
});
const model = (file: string, metrics: string[], fields: string[]): CatalogModel => ({
  model: `metrics/${file}`,
  name: file,
  metrics: metrics.map((m) => member(m)),
  fields: fields.map((f) => member(f, 'values', true)),
});
const ready = (...models: CatalogModel[]): SemanticCatalog => ({ status: 'ready', models });

describe('choices', () => {
  it('starts on the first metric whose values may be seen', () => {
    const masked = {
      ...model('a', [], []),
      metrics: [member('oculta', 'masked'), member('visible')],
    };
    expect(initialChoice(ready(masked))).toEqual({
      model: 'metrics/a',
      metrics: ['visible'],
      dimensions: [],
    });
    expect(initialChoice(ready())).toBeNull();
  });

  it('never keeps what a refreshed catalog no longer announces', () => {
    const names = fc.uniqueArray(fc.constantFrom<string>('a', 'b', 'c', 'd', 'e'), {
      maxLength: 5,
    });
    fc.assert(
      fc.property(names, names, names, names, (metrics, fields, kept, grain) => {
        const refreshed = ready(model('m', metrics, fields));
        const next = reconcile({ model: 'metrics/m', metrics: kept, dimensions: grain }, refreshed);
        if (next === null) {
          expect(metrics).toEqual([]);
        } else {
          expect(next.metrics.every((n) => metrics.includes(n))).toBe(true);
          expect(next.dimensions.every((n) => fields.includes(n))).toBe(true);
          expect(next.metrics.length).toBeGreaterThan(0);
        }
      }),
    );
  });

  it('starts over when the model is gone', () => {
    const choice = { model: 'metrics/old', metrics: ['x'], dimensions: [] };
    expect(reconcile(choice, ready(model('new', ['y'], [])))).toEqual({
      model: 'metrics/new',
      metrics: ['y'],
      dimensions: [],
    });
  });

  it('asks 1 to 8 metrics and 0 to 2 fields', () => {
    const many = (n: number) => Array.from({ length: n }, (_, i) => `m${i}`);
    expect(askable({ model: 'm', metrics: [], dimensions: [] })).toBe(false);
    expect(
      askable({ model: 'm', metrics: many(MAX_METRICS), dimensions: many(MAX_DIMENSIONS) }),
    ).toBe(true);
    expect(askable({ model: 'm', metrics: many(MAX_METRICS + 1), dimensions: [] })).toBe(false);
    expect(askable({ model: 'm', metrics: ['a'], dimensions: many(MAX_DIMENSIONS + 1) })).toBe(
      false,
    );
  });
});

describe('requests', () => {
  it('asks the total without a grain and the breakdown with it, by semantic names only', () => {
    const { kpi, table, labels } = requests({
      model: 'metrics/v',
      metrics: ['importe'],
      dimensions: ['region'],
    });
    expect(kpi).toEqual({
      model: 'metrics/v',
      query: { selections: [{ id: 'm:importe', kind: 'metric', name: 'importe' }] },
    });
    expect(table!.query.selections.map((s) => [s.kind, s.name])).toEqual([
      ['dimension', 'region'],
      ['metric', 'importe'],
    ]);
    expect(labels).toEqual({ 'm:importe': 'importe', 'd:region': 'region' });
    expect(requests({ model: 'metrics/v', metrics: ['importe'], dimensions: [] }).table).toBeNull();
  });

  it('runs them one after the other, and stops at the first failure', async () => {
    const order: string[] = [];
    let open = 0;
    const run = vi.fn(async (query: { query: { selections: { kind: string }[] } }) => {
      open++;
      expect(open).toBe(1);
      const grouped = query.query.selections.some((s) => s.kind === 'dimension');
      order.push(grouped ? 'table' : 'kpi');
      await Promise.resolve();
      open--;
      return (grouped ? region : total) as unknown as SemanticRun;
    });
    const choice = { model: 'metrics/v', metrics: ['importe'], dimensions: ['region'] };
    await answer(choice, new AbortController().signal, run as never);
    expect(order).toEqual(['kpi', 'table']);
    const failing = vi.fn().mockRejectedValueOnce(new ApiError(503));
    await expect(answer(choice, new AbortController().signal, failing)).rejects.toThrow(ApiError);
    expect(failing).toHaveBeenCalledTimes(1);
  });

  it('does not send the breakdown once aborted', async () => {
    const controller = new AbortController();
    const run = vi.fn(async () => {
      controller.abort();
      return total as unknown as SemanticRun;
    });
    const choice = { model: 'm', metrics: ['a'], dimensions: ['b'] };
    await expect(answer(choice, controller.signal, run)).rejects.toThrow();
    expect(run).toHaveBeenCalledTimes(1);
  });
});

describe('consistency of two reads', () => {
  const run = (snapshot: string | null) => {
    const copy = structuredClone(total) as unknown as SemanticRun;
    copy.receipt.sources = copy.receipt.sources.map((s) => ({ ...s, data_snapshot: snapshot }));
    return copy;
  };
  it('never claims one snapshot', () => {
    expect(consistency({ kpi: run(null), table: null })).toBe('single');
    expect(consistency({ kpi: run('7'), table: run('7') })).toBe('separate');
    expect(consistency({ kpi: run('7'), table: run('8') })).toBe('diverged');
  });
});

describe('failures', () => {
  it.each([
    [404, 'model_not_found', { gone: true, retry: false }],
    [429, 'semantic_busy', { gone: false, retry: true }],
    [403, 'semantic_access_denied', { gone: false, retry: false }],
    [422, 'masked_unsupported', { gone: false, retry: false }],
    [422, 'result_out_of_range', { gone: false, retry: false }],
    [503, 'analytics_failed', { gone: false, retry: true }],
    [504, 'semantic_timeout', { gone: false, retry: true }],
  ])('%i %s', (status, code, expected) => {
    const failed = failure(new ApiError(status, 'req-9', code));
    expect(failed).toMatchObject({ ...expected, status, code, requestId: 'req-9' });
  });
});

describe('found by the adversarial review before delivery', () => {
  const masked = (name: string) => member(name, 'masked');
  it('never starts on, nor keeps, a metric seen only withheld', () => {
    const onlyMasked = { ...model('a', [], []), metrics: [masked('oculta')] };
    const next = model('b', ['visible'], []);
    expect(initialChoice(ready(onlyMasked, next))).toEqual({
      model: 'metrics/b',
      metrics: ['visible'],
      dimensions: [],
    });
    expect(initialChoice(ready(onlyMasked))).toBeNull();
    const mixed = {
      ...model('m', ['visible'], []),
      metrics: [masked('oculta'), member('visible')],
    };
    const kept = reconcile(
      { model: 'metrics/m', metrics: ['oculta', 'visible'], dimensions: [] },
      ready(mixed),
    );
    expect(kept?.metrics).toEqual(['visible']);
  });

  it('keeps the model after a selection was retired, when the model is still there', () => {
    const catalog = ready(model('first', ['x'], []), model('mine', ['y', 'z'], []));
    expect(reconcile({ model: 'metrics/mine', metrics: [], dimensions: [] }, catalog)).toEqual({
      model: 'metrics/mine',
      metrics: ['y'],
      dimensions: [],
    });
  });

  it('calls two reads under another model or other policies diverged', () => {
    const base = structuredClone(total) as unknown as SemanticRun;
    for (const key of ['model', 'policies'] as const) {
      const other = structuredClone(total) as unknown as SemanticRun;
      other.receipt.fingerprints[key] = `${key}-hmac-sha256-v1:${'f'.repeat(64)}`;
      expect(consistency({ kpi: base, table: other })).toBe('diverged');
    }
  });

  it('refuses an answer with other than one block, and says why without a retry loop', async () => {
    const twoBlocks = structuredClone(total) as unknown as SemanticRun;
    twoBlocks.result.blocks.push(twoBlocks.result.blocks[0]!);
    const run = vi.fn(async () => twoBlocks);
    const error = await answer(
      { model: 'm', metrics: ['a'], dimensions: [] },
      new AbortController().signal,
      run,
    ).catch((e: unknown) => e);
    expect(failure(error)).toMatchObject({ retry: false, gone: false });
    expect(failure(error).message).toContain('no se muestra ninguna cifra');
  });

  it("tells a client timeout apart from a network failure, and a busy server from one's own query", () => {
    expect(failure(new DOMException('late', 'TimeoutError')).message).toContain(
      'no respondió a tiempo',
    );
    expect(failure(new ApiError(429, 'r', 'semantic_busy')).message).not.toContain('tuya');
  });
});

it('explains a total out of range as such, not as an impossible selection', () => {
  const message = failure(new ApiError(422, 'r', 'result_out_of_range')).message;
  expect(message).toContain('demasiado grande');
  expect(message).not.toContain('no se puede consultar en el modelo');
});

describe('a total excluded after aggregating, then the breakdown (#266)', () => {
  const emptyTotal = () => {
    const run = structuredClone(total) as unknown as SemanticRun;
    run.result.blocks[0]!.rows = [];
    run.result.blocks[0]!.row_count = 0;
    return run;
  };
  const having = {
    op: 'compare' as const,
    target: { kind: 'metric' as const, selection: 'm:importe' },
    comparison: '<' as const,
    value: { type: 'number' as const, value: '100.20' },
  };

  it('accepts it for a HAVING question and still asks and checks the breakdown', async () => {
    const run = vi.fn(async (request: { query: { selections: { kind: string }[] } }) =>
      request.query.selections.some((s) => s.kind === 'dimension')
        ? (region as unknown as SemanticRun)
        : emptyTotal(),
    );
    const choice = {
      model: 'm',
      metrics: ['importe'],
      dimensions: ['region'],
      filters: { where: null, having },
    };
    const answered = await answer(choice, new AbortController().signal, run as never);
    expect(run).toHaveBeenCalledTimes(2);
    expect(answered.kpi.result.blocks[0]!.row_count).toBe(0);
    // The breakdown is its own read: never assumed empty nor summed.
    expect(answered.table?.result.blocks[0]!.rows.length).toBeGreaterThan(0);
  });

  it('refuses a total without columns even with HAVING, before the breakdown is asked', async () => {
    const run = vi.fn(async () => {
      const hollow = emptyTotal();
      hollow.result.blocks[0]!.columns = [];
      return hollow;
    });
    const choice = {
      model: 'm',
      metrics: ['importe'],
      dimensions: ['region'],
      filters: { where: null, having },
    };
    await expect(answer(choice, new AbortController().signal, run)).rejects.toThrow(
      IncompatibleResult,
    );
    expect(run).toHaveBeenCalledTimes(1);
  });

  it('refuses it without HAVING, before the breakdown is asked', async () => {
    const run = vi.fn(async () => emptyTotal());
    const choice = { model: 'm', metrics: ['importe'], dimensions: ['region'] };
    await expect(answer(choice, new AbortController().signal, run)).rejects.toThrow(
      IncompatibleResult,
    );
    expect(run).toHaveBeenCalledTimes(1);
  });
});

describe('filters before and after aggregating, applied as one (#149)', () => {
  const ventas = model('ventas.yaml', ['filas'], ['ventas.region']);
  const choice = { model: ventas.model, metrics: ['filas'], dimensions: ['ventas.region'] };
  const one = (target: SemanticTarget): SemanticFilter => ({
    op: 'compare',
    target,
    comparison: '=',
    value: { type: 'number', value: '1' },
  });
  const rows = one({ kind: 'dimension', name: 'ventas.region' });
  const counted = one({ kind: 'metric', selection: metricId('filas') });
  const group = (n: number, filter: SemanticFilter): SemanticFilter => ({
    op: 'and',
    filters: Array.from({ length: n }, () => filter),
  });

  it('carries both, or none, never an empty filters object', () => {
    expect(withFilters(choice, { where: null, having: counted })).toEqual({
      ...choice,
      filters: { where: null, having: counted },
    });
    expect(withFilters({ ...choice, filters: { where: rows, having: null } }, NO_FILTERS)).toEqual(
      choice,
    );
  });

  it('is checked on one budget for both, as the server counts them', () => {
    // A group of n children is n + 1 nodes: 128 + 128 = 256 fits, 257 does not.
    const fits = withFilters(choice, { where: group(127, rows), having: group(127, counted) });
    expect(questionProblems(fits, ventas)).toEqual([]);
    const over = withFilters(choice, { where: group(127, rows), having: group(128, counted) });
    expect(questionProblems(over, ventas).length).toBeGreaterThan(0);
    expect(askable(over, ventas)).toBe(false);
  });
});
