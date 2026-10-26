import fc from 'fast-check';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { CatalogModel } from '#lib/api/semantic.ts';
import {
  back,
  drill,
  returnTo,
  startPath,
  type DrillCell,
  type DrillPath,
  type PathContext,
} from './drill';
import type { SemanticBlock, SemanticCell } from './exact';
import type { Choice } from './explore';
import { dimensionId, metricId, type SemanticFilter, type SemanticValue } from './filters';

const member = (name: string, type: string, filter = true, group = true) => ({
  name,
  type,
  lifecycle: { state: 'active' as const },
  capabilities: { select: 'values' as const, filter, group, order: true },
});
// The fixture of #143/#265, and one field of each type a step must carry exactly.
const MODEL: CatalogModel = {
  model: 'metrics/ventas.yaml',
  name: 'Ventas',
  fields: [
    member('ventas.region', 'VARCHAR'),
    member('ventas.ciudad', 'VARCHAR'),
    member('ventas.nota', 'VARCHAR'),
    member('ventas.unidades', 'INT64'),
    member('ventas.precio', 'DECIMAL(18,2)'),
    member('ventas.dia', 'DATE'),
    member('ventas.activo', 'BOOLEAN'),
    member('ventas.instante', 'TIMESTAMPTZ'),
    member('ventas.sin_filtro', 'VARCHAR', false),
    member('ventas.sin_grupo', 'VARCHAR', true, false),
  ],
  metrics: [member('importe_total', 'DECIMAL(38,2)', true, false)],
};
const A: PathContext = { principal: 'oidc-norte', model: MODEL.model };

const is = (name: string, value: SemanticValue): SemanticFilter => ({
  op: 'compare',
  target: { kind: 'dimension', name },
  comparison: '=',
  value,
});
const text = (value: string): SemanticValue => ({ type: 'text', value });
const HAVING: SemanticFilter = {
  op: 'compare',
  target: { kind: 'metric', selection: metricId('importe_total') },
  comparison: '>',
  value: { type: 'number', value: '0.00' },
};
// A previous filter with OR and NOT: kept whole, never rewritten.
const PRIOR: SemanticFilter = {
  op: 'or',
  filters: [
    is('ventas.region', text('Norte')),
    { op: 'not', filter: is('ventas.ciudad', text('Sucre')) },
  ],
};
const ask = (dimensions: string[], where: SemanticFilter | null = PRIOR): Choice => ({
  model: MODEL.model,
  metrics: ['importe_total'],
  dimensions,
  filters: { where, having: HAVING },
});

/** A checked-shape block of `question`: its dimensions, then its metrics. */
function answer(question: Choice, rows: (SemanticCell | null)[][], types: string[]): SemanticBlock {
  const columns = [
    ...question.dimensions.map((name, i) => ({
      selection_id: dimensionId(name),
      kind: 'dimension' as const,
      id: `project/dimension/${name}`,
      type: types[i]!,
    })),
    ...question.metrics.map((name) => ({
      selection_id: metricId(name),
      kind: 'metric' as const,
      id: `project/metric/${name}`,
      type: 'DECIMAL(38,2)',
    })),
  ].map((c) => ({ ...c, nullable: true, unit: null, timezone: null, masked: false }));
  return { block_id: 'b', columns, rows, row_count: rows.length, limit: null };
}
const cell = (asked: Choice, block: SemanticBlock, row: number, name: string): DrillCell => ({
  asked,
  block,
  row,
  selection: dimensionId(name),
});
const ok = (result: ReturnType<typeof drill>) => {
  if (!result.ok) throw new Error(`refused: ${JSON.stringify(result)}`);
  return result;
};

afterEach(() => vi.unstubAllGlobals());

describe('the intention journey of #143', () => {
  it('Norte → ciudad, La Paz → nota, NULL → ciudad; Atrás and the first breadcrumb restore exactly', () => {
    const fetch = vi.fn();
    vi.stubGlobal('fetch', fetch);
    const q0 = ask(['ventas.region']);
    const byRegion = answer(
      q0,
      [
        ['Norte', '100.30'],
        ['Sur', '200.00'],
      ],
      ['VARCHAR'],
    );
    const s1 = ok(
      drill(q0, startPath(A), A, MODEL, cell(q0, byRegion, 0, 'ventas.region'), 'ventas.ciudad'),
    );
    const norte = is('ventas.region', text('Norte'));
    expect(s1.question).toEqual(ask(['ventas.ciudad'], { op: 'and', filters: [PRIOR, norte] }));

    const byCity = answer(
      s1.question,
      [
        ['La Paz', '100.10'],
        ['El Alto', '0.20'],
      ],
      ['VARCHAR'],
    );
    const s2 = ok(
      drill(
        s1.question,
        s1.path,
        A,
        MODEL,
        cell(s1.question, byCity, 0, 'ventas.ciudad'),
        'ventas.nota',
      ),
    );
    const laPaz = is('ventas.ciudad', text('La Paz'));
    // Added by AND to the same group, the previous ones exactly as they were.
    expect(s2.question).toEqual(
      ask(['ventas.nota'], { op: 'and', filters: [PRIOR, norte, laPaz] }),
    );

    // A NULL cell is NULL: `is_null`, never the text the table shows.
    const byNote = answer(s2.question, [[null, '100.10']], ['VARCHAR']);
    const s3 = ok(
      drill(
        s2.question,
        s2.path,
        A,
        MODEL,
        cell(s2.question, byNote, 0, 'ventas.nota'),
        'ventas.ciudad',
      ),
    );
    expect(s3.question.filters!.where).toEqual({
      op: 'and',
      filters: [
        PRIOR,
        norte,
        laPaz,
        { op: 'is_null', target: { kind: 'dimension', name: 'ventas.nota' } },
      ],
    });
    expect(s3.path.frames.map((f) => f.step)).toEqual([
      { from: 'ventas.region', value: text('Norte'), to: 'ventas.ciudad' },
      { from: 'ventas.ciudad', value: text('La Paz'), to: 'ventas.nota' },
      { from: 'ventas.nota', value: null, to: 'ventas.ciudad' },
    ]);

    // Atrás: the question by note; again: by city with región = Norte.
    const b1 = ok(back(s3.path, A));
    expect(b1.question).toEqual(s2.question);
    const b2 = ok(back(b1.path, A));
    expect(b2.question).toEqual(s1.question);
    expect(b2.path.frames).toHaveLength(1);
    // The first breadcrumb: the original question by region, every step below gone.
    const first = ok(returnTo(s3.path, A, 0));
    expect(first.question).toEqual(q0);
    expect(first.path).toEqual(startPath(A));
    // Each step and return keeps the person's HAVING exactly.
    for (const q of [s1, s2, s3, b1, b2, first].map((r) => r.question)) {
      expect(q.filters!.having).toEqual(HAVING);
    }
    expect(fetch).not.toHaveBeenCalled();
  });

  it('with two dimensions, only the origin is replaced and filtered; the other value of the row is not', () => {
    const q = ask(['ventas.region', 'ventas.ciudad'], null);
    const block = answer(q, [['Norte', 'La Paz', '100.10']], ['VARCHAR', 'VARCHAR']);
    const s = ok(
      drill(q, startPath(A), A, MODEL, cell(q, block, 0, 'ventas.region'), 'ventas.nota'),
    );
    expect(s.question.dimensions).toEqual(['ventas.nota', 'ventas.ciudad']);
    expect(s.question.filters!.where).toEqual(is('ventas.region', text('Norte')));
  });
});

describe('the literal of a raw cell, exactly', () => {
  const step = (name: string, type: string, raw: SemanticCell | null) => {
    const q = ask([name], null);
    return drill(
      q,
      startPath(A),
      A,
      MODEL,
      cell(q, answer(q, [[raw, '1.00']], [type]), 0, name),
      'ventas.region',
    );
  };
  const where = (result: ReturnType<typeof drill>) => ok(result).question.filters!.where;

  it.each([
    ['the text «NULL» is a text', 'ventas.nota', 'VARCHAR', 'NULL', text('NULL')],
    ['an empty text is a text', 'ventas.nota', 'VARCHAR', '', text('')],
    [
      'an INT64 past 2^53, as text',
      'ventas.unidades',
      'INT64',
      '9007199254740993',
      { type: 'number', value: '9007199254740993' },
    ],
    [
      'a negative DECIMAL, every digit',
      'ventas.precio',
      'DECIMAL(18,2)',
      '-0.20',
      { type: 'number', value: '-0.20' },
    ],
    ['a day', 'ventas.dia', 'DATE', '2026-01-02', { type: 'date', value: '2026-01-02' }],
    ['false', 'ventas.activo', 'BOOLEAN', false, { type: 'boolean', value: false }],
  ] as const)('%s', (_, name, type, raw, value) => {
    expect(where(step(name, type, raw))).toEqual(is(name, value as SemanticValue));
  });

  it('NULL of any type is `is_null`', () => {
    expect(where(step('ventas.unidades', 'INT64', null))).toEqual({
      op: 'is_null',
      target: { kind: 'dimension', name: 'ventas.unidades' },
    });
  });

  it('an instant has no literal on the wire: refused, never converted', () => {
    expect(step('ventas.instante', 'TIMESTAMPTZ', '2026-01-01T00:00:00Z')).toEqual({
      ok: false,
      reason: 'value',
    });
  });

  it('a text over the contract bound is refused by the core, not cut', () => {
    const result = step('ventas.nota', 'VARCHAR', 'x'.repeat(257));
    expect(result).toMatchObject({ ok: false, reason: 'question' });
  });
});

describe('a refused step changes nothing', () => {
  const q = ask(['ventas.region', 'ventas.ciudad']);
  const block = answer(q, [['Norte', 'La Paz', '100.10']], ['VARCHAR', 'VARCHAR']);
  const good = cell(q, block, 0, 'ventas.region');

  it.each<[string, () => ReturnType<typeof drill>, string]>([
    [
      'another principal',
      () => drill(q, startPath(A), { ...A, principal: 'oidc-sur' }, MODEL, good, 'ventas.nota'),
      'context',
    ],
    [
      'a path of another principal',
      () => drill(q, startPath({ ...A, principal: 'oidc-sur' }), A, MODEL, good, 'ventas.nota'),
      'context',
    ],
    [
      'another model',
      () =>
        drill(q, startPath(A), A, { ...MODEL, model: 'metrics/otro.yaml' }, good, 'ventas.nota'),
      'context',
    ],
    [
      'a cell of another attempt',
      () =>
        drill(
          q,
          startPath(A),
          A,
          MODEL,
          { ...good, asked: ask(['ventas.region', 'ventas.ciudad'], null) },
          'ventas.nota',
        ),
      'stale',
    ],
    [
      'a row that is not there',
      () => drill(q, startPath(A), A, MODEL, { ...good, row: 1 }, 'ventas.nota'),
      'cell',
    ],
    [
      'a block outside its contract',
      () =>
        drill(
          q,
          startPath(A),
          A,
          MODEL,
          { ...good, block: { ...block, rows: [['Norte']] } },
          'ventas.nota',
        ),
      'cell',
    ],
    [
      'a metric',
      () =>
        drill(
          q,
          startPath(A),
          A,
          MODEL,
          { ...good, selection: metricId('importe_total') },
          'ventas.nota',
        ),
      'origin',
    ],
    [
      'a withheld column',
      () =>
        drill(
          q,
          startPath(A),
          A,
          MODEL,
          {
            ...good,
            block: {
              ...block,
              rows: [[null, 'La Paz', '1.00']],
              columns: block.columns.map((c, i) => (i === 0 ? { ...c, masked: true } : c)),
            },
          },
          'ventas.nota',
        ),
      'origin',
    ],
    ['the same dimension', () => drill(q, startPath(A), A, MODEL, good, 'ventas.region'), 'target'],
    [
      'a dimension already asked',
      () => drill(q, startPath(A), A, MODEL, good, 'ventas.ciudad'),
      'target',
    ],
    [
      'a field it may not group',
      () => drill(q, startPath(A), A, MODEL, good, 'ventas.sin_grupo'),
      'target',
    ],
    [
      'a field not in the catalog',
      () => drill(q, startPath(A), A, MODEL, good, 'ventas.secreto'),
      'target',
    ],
  ])('%s', (_, run, reason) => {
    const before = JSON.stringify([q, block]);
    expect(run()).toMatchObject({ ok: false, reason });
    expect(JSON.stringify([q, block])).toBe(before);
  });

  it('an origin the session may not filter is refused by the core, its filters kept', () => {
    const own = ask(['ventas.sin_filtro']);
    const b = answer(own, [['x', '1.00']], ['VARCHAR']);
    const result = drill(
      own,
      startPath(A),
      A,
      MODEL,
      cell(own, b, 0, 'ventas.sin_filtro'),
      'ventas.region',
    );
    expect(result).toMatchObject({ ok: false, reason: 'question' });
    expect(own.filters).toEqual({ where: PRIOR, having: HAVING });
  });

  it('a step past the budget both filters share is refused, nothing trimmed', () => {
    // 1 group + 254 conditions + `having` = 256 nodes; the step would make 257.
    const many: SemanticFilter = {
      op: 'and',
      filters: Array.from({ length: 254 }, () => is('ventas.ciudad', text('x'))),
    };
    const full = ask(['ventas.region'], many);
    const b = answer(full, [['Norte', '1.00']], ['VARCHAR']);
    const path = startPath(A);
    const result = drill(full, path, A, MODEL, cell(full, b, 0, 'ventas.region'), 'ventas.nota');
    expect(result).toMatchObject({
      ok: false,
      reason: 'question',
      problems: [
        { message: 'Un filtro tiene como máximo 256 condiciones y 16 niveles de anidamiento.' },
      ],
    });
    expect(full.filters!.where).toBe(many);
    expect(many.op === 'and' && many.filters).toHaveLength(254);
    expect(path.frames).toEqual([]);
  });

  it('names the origin by the selection itself: no role it does not carry', () => {
    const s = ok(drill(q, startPath(A), A, MODEL, good, 'ventas.nota'));
    const where = s.question.filters!.where;
    const added = where?.op === 'and' ? where.filters.at(-1)! : where!;
    expect(added).toEqual(is('ventas.region', text('Norte')));
    expect('target' in added && 'roles' in added.target).toBe(false);
  });
});

describe('returns', () => {
  it('without a frame, or to one that is not there, are refused', () => {
    expect(back(startPath(A), A)).toEqual({ ok: false, reason: 'no_frame' });
    const q = ask(['ventas.region']);
    const s = ok(
      drill(
        q,
        startPath(A),
        A,
        MODEL,
        cell(q, answer(q, [['Norte', '1.00']], ['VARCHAR']), 0, 'ventas.region'),
        'ventas.ciudad',
      ),
    );
    for (const index of [-1, 1, 0.5, Number.NaN]) {
      expect(returnTo(s.path, A, index)).toEqual({ ok: false, reason: 'no_frame' });
    }
    // Never under another principal: the page starts a new path.
    expect(back(s.path, { ...A, principal: 'oidc-sur' })).toEqual({ ok: false, reason: 'context' });
    expect(startPath({ ...A, principal: 'oidc-sur' }).frames).toEqual([]);
  });

  it('frames are copies: nothing aliases the question asked, the path, or what a return gives', () => {
    const q = structuredClone(ask(['ventas.region']));
    const block = answer(q, [['Norte', '1.00']], ['VARCHAR']);
    const path = startPath(A);
    const s = ok(drill(q, path, A, MODEL, cell(q, block, 0, 'ventas.region'), 'ventas.ciudad'));
    const kept = structuredClone(s.path);
    q.dimensions.push('ventas.nota');
    (q.filters!.where as { op: string }).op = 'and';
    s.question.metrics.push('otra');
    expect(s.path).toEqual(kept);
    const r = ok(back(s.path, A));
    r.question.dimensions.length = 0;
    expect(s.path).toEqual(kept);
    expect(path.frames).toEqual([]);
    // A frame holds intention only: no rows, blocks or receipts.
    expect(JSON.stringify(s.path)).not.toMatch(/rows|block_id|receipt|100\.30/);
  });
});

describe('properties', () => {
  const regions = fc.constantFrom('Norte', 'Sur', 'NULL', '', 'x'.repeat(10));
  const prior = fc.option(
    fc.constantFrom<SemanticFilter>(
      PRIOR,
      { op: 'not', filter: is('ventas.ciudad', text('Sucre')) },
      { op: 'and', filters: [is('ventas.nota', text('n'))] },
      is('ventas.ciudad', text('Tarija')),
    ),
    { nil: null },
  );

  it('a step adds exactly one condition by AND and keeps the rest; Atrás gives back the question exactly', () => {
    fc.assert(
      fc.property(prior, fc.option(regions, { nil: null }), (where, region) => {
        const q = ask(['ventas.region'], where);
        const b = answer(q, [[region, '1.00']], ['VARCHAR']);
        const s = ok(
          drill(q, startPath(A), A, MODEL, cell(q, b, 0, 'ventas.region'), 'ventas.ciudad'),
        );
        const added: SemanticFilter =
          region === null
            ? { op: 'is_null', target: { kind: 'dimension', name: 'ventas.region' } }
            : is('ventas.region', text(region));
        const expected: SemanticFilter = !where
          ? added
          : where.op === 'and'
            ? { op: 'and', filters: [...where.filters, added] }
            : { op: 'and', filters: [where, added] };
        expect(s.question.filters).toEqual({ where: expected, having: HAVING });
        expect(ok(back(s.path, A)).question).toEqual(q);
      }),
    );
  });

  it('any walk of steps and returns matches a stack of the questions left', () => {
    const fields = ['ventas.ciudad', 'ventas.nota', 'ventas.region'];
    fc.assert(
      fc.property(
        fc.array(fc.oneof(fc.constant('back' as const), fc.integer({ min: 0, max: 3 }), regions), {
          maxLength: 12,
        }),
        (moves) => {
          let question = ask(['ventas.region'], null);
          let path: DrillPath = startPath(A);
          const stack: Choice[] = [];
          for (const move of moves) {
            if (move === 'back') {
              const r = back(path, A);
              if (stack.length === 0) {
                expect(r).toEqual({ ok: false, reason: 'no_frame' });
                continue;
              }
              question = ok(r).question;
              path = ok(r).path;
              expect(question).toEqual(stack.pop());
            } else if (typeof move === 'number') {
              const r = returnTo(path, A, move);
              if (move >= stack.length) {
                expect(r).toEqual({ ok: false, reason: 'no_frame' });
                continue;
              }
              question = ok(r).question;
              path = ok(r).path;
              expect(question).toEqual(stack[move]);
              stack.length = move;
            } else {
              const from = question.dimensions[0]!;
              const to = fields.find((f) => f !== from)!;
              const b = answer(question, [[move, '1.00']], ['VARCHAR']);
              const r = ok(drill(question, path, A, MODEL, cell(question, b, 0, from), to));
              stack.push(question);
              question = r.question;
              path = r.path;
            }
            expect(path.frames.map((f) => f.question)).toEqual(stack);
          }
        },
      ),
    );
  });
});
