import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page, type Request, type Response } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import {
  filterProblems,
  filterQuery,
  metricId,
  type UserFilters,
} from '../src/lib/features/semantic/filters';

// The semantic Explorar (#147) against a real production server: corporate
// login with the real provider, PostgreSQL with the restricted runtime
// role (#115), DuckDB/DuckLake holding `semantic/ventas.sql` and the
// governed project `semantic/project` (row policy on the verified
// `department`). Nothing is intercepted or mocked: the server decides.
//
// Server as in README «Login corporativo», with the analytical runtime on
// (no BI_ANALYTICS=off) and BI_OIDC_ATTRIBUTE_CLAIMS=department,
// BI_PROJECT_DIR=<abs>/frontend/e2e/semantic/project and BI_LAKE_*; the
// rows are written once with DuckDB 1.5.6 attaching the same lake and
// running semantic/ventas.sql. With oidc.spec.ts a run signs in nine times: the server's
// OIDC admission (20 per IP, one more every 3 s) asks for a pause between
// consecutive runs.
//
// Oracle, computed independently of the server: Norte = 100.30 over 2 rows,
// Sur = 200.00 over 2 rows.

test.beforeEach(async ({ request }) => {
  const config = await request.get('/api/v1/oidc/config');
  expect(config.status()).toBe(200);
  const enabled = (await config.json()).enabled;
  if (process.env.BI_E2E_REQUIRE_OIDC === '1') expect(enabled).toBe(true);
  test.skip(!enabled, 'Needs the real provider, the governed project and the lake fixture');
});

// `amount` is the oracle's decimal; `shown`, the same value as the page writes it (es).
const NORTE = {
  user: 'oidc-norte',
  name: 'Norte Analyst',
  region: 'Norte',
  amount: '100.30',
  shown: '100,30',
};
const SUR = {
  user: 'oidc-sur',
  name: 'Sur Analyst',
  region: 'Sur',
  amount: '200.00',
  shown: '200,00',
};

/** Text the viewer's catalog never announces: restricted members and the policy's claim. */
const SENTINELS = /centinela|secreto_|reserva_|department|\bnota\b/i;

/**
 * Every API request the page sends, and the semantic requests that overlapped
 * one still open. A replaced attempt is aborted while the next one starts;
 * two answered requests in flight at once would be two reads at the same time.
 */
function watch(page: Page) {
  const sent: string[] = [];
  const open = new Set<Request>();
  const overlaps: { open: Request; next: Request }[] = [];
  const dropped = new Set<Request>();
  const semantic = (request: Request) =>
    new URL(request.url()).pathname.startsWith('/api/v1/semantic/');
  page.on('request', (request) => {
    const { pathname } = new URL(request.url());
    if (pathname.startsWith('/api/')) sent.push(`${request.method()} ${pathname}`);
    if (!semantic(request)) return;
    for (const other of open) overlaps.push({ open: other, next: request });
    open.add(request);
  });
  page.on('requestfinished', (request) => open.delete(request));
  page.on('requestfailed', (request) => {
    open.delete(request);
    dropped.add(request);
  });
  return {
    sent,
    /** Overlaps where both requests were answered: two reads at once. */
    concurrent: () =>
      overlaps
        .filter(({ open, next }) => !dropped.has(open) && !dropped.has(next))
        .map(
          ({ open, next }) =>
            `${open.postData() ?? open.url()} || ${next.postData() ?? next.url()}`,
        ),
  };
}

async function signIn(page: Page, user: string, name: string) {
  await page.goto('/login');
  await page.getByRole('button', { name: 'Entrar con mi empresa' }).click();
  await page.getByLabel('Username or email').fill(user);
  await page.getByLabel('Password', { exact: true }).fill('oidc-test-only');
  await page.getByRole('button', { name: 'Sign In', exact: true }).click();
  await expect(page).toHaveURL('/');
  await expect(page.getByText(`Sesión: ${name} · Datos sintéticos`)).toBeVisible();
}

/** Signs out of BI and of the provider's SSO, so the next login asks again. */
async function signOut(page: Page) {
  await page.getByRole('button', { name: 'Cerrar sesión' }).click();
  await expect(page).toHaveURL('/login');
  await page.context().clearCookies({ path: /^\/realms\// });
}

async function openExplore(page: Page) {
  await page.getByRole('link', { name: 'Exploración', exact: true }).click();
  await expect(page).toHaveURL('/explorar');
  // The catalog lists models in the order of their files: the count first.
  await expect(page.getByLabel('Modelo')).toHaveValue('metrics/recuento.yaml');
  await expect(kpi(page, 'filas')).toHaveText('2');
}

async function chooseVentas(page: Page) {
  await page.getByLabel('Modelo').selectOption('metrics/ventas.yaml');
  await expect(page.getByRole('checkbox', { name: 'filas', exact: true })).toBeChecked();
  await settled(page, 1);
}

/**
 * The question on screen is answered with `metrics` totals and nothing
 * failed. Each step waits for it, at a person's pace: a request replaced
 * mid-flight still holds one of the server's two compilation slots.
 */
async function settled(page: Page, metrics: number) {
  await expect(page.locator('[aria-live="polite"]')).toHaveAttribute('aria-busy', 'false');
  await expect(page.locator('dl.kpis > div')).toHaveCount(metrics);
  await expect(page.getByRole('alert')).toHaveCount(0);
}

const kpi = (page: Page, metric: string) =>
  page
    .locator('dl.kpis > div')
    .filter({ has: page.getByRole('term').getByText(metric, { exact: true }) })
    .getByRole('definition');
const breakdown = (page: Page) => page.getByRole('table', { name: 'Desglose' });

/** Body rows of the breakdown, cell by cell. */
async function bodyRows(page: Page) {
  return breakdown(page)
    .locator('tbody tr')
    .evaluateAll((rows) =>
      rows.map((row) => [...row.children].map((cell) => cell.textContent!.trim())),
    );
}

/** Selects importe_total next to filas and breaks both down by region. */
async function askByRegion(page: Page, who: typeof NORTE) {
  await page.getByRole('checkbox', { name: 'importe_total', exact: true }).check();
  await expect(kpi(page, 'importe_total')).toHaveText(who.shown);
  await settled(page, 2);
  await page.getByRole('checkbox', { name: 'ventas.region', exact: true }).check();
  await expect(kpi(page, 'filas')).toHaveText('2');
  await expect.poll(() => bodyRows(page)).toEqual([[who.region, '2', who.shown]]);
  await expect(page.getByRole('heading', { name: 'Recibo de los totales' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Recibo del desglose' })).toBeVisible();
}

/** The two receipt IDs shown: each read keeps its own. */
async function receiptIds(page: Page) {
  const ids = await page.locator('dt:text-is("Identificador") + dd code').allTextContents();
  expect(ids).toHaveLength(2);
  expect(ids[0]).not.toBe(ids[1]);
  return ids;
}

async function axe(page: Page) {
  const { violations } = await new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21aa', 'wcag22aa'])
    .analyze();
  expect(violations).toEqual([]);
}

/** Neither the page nor its attributes hold anything outside the session's catalog and rows. */
async function noLeaks(page: Page, other: typeof NORTE) {
  const html = await page.content();
  expect(html).not.toMatch(SENTINELS);
  expect(html).not.toContain(other.amount);
  expect(html).not.toContain(other.shown);
  expect(html).not.toMatch(new RegExp(`\\b${other.region}\\b`));
}

function onlySemantic(sent: string[]) {
  expect(sent.filter((line) => /\/api\/v1\/(queries|models)(\/|$)/.test(line))).toEqual([]);
  expect(sent).toContain('GET /api/v1/semantic/catalog');
  expect(sent).toContain('POST /api/v1/semantic/query');
}

test('Norte and Sur see only their rows, and logout leaves nothing of A to B', async ({ page }) => {
  test.setTimeout(120_000);
  const network = watch(page);
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));

  await signIn(page, NORTE.user, NORTE.name);
  await openExplore(page);
  await chooseVentas(page);
  await askByRegion(page, NORTE);
  const ofNorte = await receiptIds(page);
  await expect(
    page.getByRole('img', {
      name: 'filas por ventas.region. Los valores exactos están en la tabla de resultados.',
    }),
  ).toBeVisible();
  await noLeaks(page, SUR);
  await axe(page);

  // A metric the session may only see masked: the page offers it marked and
  // disabled, never asks it, and the server refuses it anyway.
  const margen = page.getByRole('checkbox', { name: /^margen/ });
  await expect(margen).toBeDisabled();
  await expect(margen).toHaveAccessibleName(/solo valores ocultos/);
  const refused = await page.evaluate(async () => {
    const response = await fetch('/api/v1/semantic/query', {
      method: 'POST',
      headers: { Accept: 'application/json', 'Content-Type': 'application/json' },
      body: JSON.stringify({
        model: 'metrics/ventas.yaml',
        query: { selections: [{ id: 'm0', kind: 'metric', name: 'margen' }] },
      }),
    });
    return {
      status: response.status,
      header: response.headers.get('x-request-id'),
      body: await response.json(),
    };
  });
  expect(refused.status).toBe(422);
  expect(refused.body.code).toBe('masked_unsupported');
  expect(refused.body.request_id).toBe(refused.header);
  expect(JSON.stringify(refused.body)).not.toMatch(/\d+\.\d{2}/);
  // The page's answer is untouched by a request it never made.
  await expect.poll(() => bodyRows(page)).toEqual([[NORTE.region, '2', NORTE.shown]]);
  await expect(page.getByRole('alert')).toHaveCount(0);

  // A signs out; B signs in on the same browser context.
  await signOut(page);
  const leftover = await page.content();
  for (const id of ofNorte) expect(leftover).not.toContain(id);
  expect(leftover).not.toContain(NORTE.amount);
  expect(leftover).not.toContain(NORTE.shown);
  const storage = await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage }));
  expect(storage).not.toContain(NORTE.amount);
  expect(storage).not.toContain(NORTE.shown);
  for (const id of ofNorte) expect(storage).not.toContain(id);

  await signIn(page, SUR.user, SUR.name);
  // B starts from her own catalog: none of A's model, selection, result or receipt.
  await openExplore(page);
  await expect(page.getByRole('checkbox', { name: 'importe_total', exact: true })).toHaveCount(0);
  await expect(breakdown(page)).toHaveCount(0);
  const ofSur = await page.locator('dt:text-is("Identificador") + dd code').allTextContents();
  expect(ofSur).toHaveLength(1);
  for (const id of ofNorte) expect(ofSur).not.toContain(id);
  await noLeaks(page, NORTE);
  await chooseVentas(page);
  await expect(
    page.getByRole('checkbox', { name: 'importe_total', exact: true }),
  ).not.toBeChecked();
  await expect(
    page.getByRole('checkbox', { name: 'ventas.region', exact: true }),
  ).not.toBeChecked();
  await expect(breakdown(page)).toHaveCount(0);

  await askByRegion(page, SUR);
  await noLeaks(page, NORTE);
  await axe(page);

  onlySemantic(network.sent);
  // Catalog first, then the total, then the breakdown: never two answered at once.
  expect(network.concurrent()).toEqual([]);
  expect(errors).toEqual([]);
});

test('an identity without membership sees the empty catalog and asks nothing', async ({ page }) => {
  const network = watch(page);
  await signIn(page, 'oidc-outsider', 'Outside Analyst');
  await page.getByRole('link', { name: 'Exploración', exact: true }).click();
  await expect(page.getByRole('status')).toHaveText(
    'Tu sesión no tiene métricas disponibles para consultar.',
  );
  await expect(page.getByLabel('Modelo')).toHaveCount(0);
  await expect(page.locator('dl.kpis')).toHaveCount(0);
  const html = await page.content();
  expect(html).not.toMatch(SENTINELS);
  expect(html).not.toMatch(/importe_total|filas|ventas\.|Norte|Sur|100[.,]30|200[.,]00/);
  await axe(page);
  expect(network.sent.filter((line) => /\/api\/v1\/(queries|models)(\/|$)/.test(line))).toEqual([]);
  expect(network.sent).toContain('GET /api/v1/semantic/catalog');
  expect(network.sent).not.toContain('POST /api/v1/semantic/query');
});

/** Presses Tab until the focused element is `target`. */
async function tabTo(page: Page, target: ReturnType<Page['locator']>) {
  for (let i = 0; i < 40; i++) {
    if (await target.evaluate((element) => element === document.activeElement)) return;
    await page.keyboard.press('Tab');
  }
  throw new Error('not reachable with Tab');
}

test('keyboard only: choose the model, toggle metrics and a breakdown, read the result', async ({
  page,
}) => {
  test.setTimeout(90_000);
  const network = watch(page);
  await signIn(page, NORTE.user, NORTE.name);
  const explore = page.getByRole('link', { name: 'Exploración', exact: true });
  await explore.focus();
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL('/explorar');

  const model = page.getByLabel('Modelo');
  await tabTo(page, model);
  await expect(model).toHaveValue('metrics/recuento.yaml');
  await expect(kpi(page, 'filas')).toHaveText('2');
  // Another model, by keyboard: its own metrics, its own answer.
  await page.keyboard.press('ArrowDown');
  await expect(model).toHaveValue('metrics/ventas.yaml');
  await expect(page.getByRole('checkbox', { name: 'importe_total', exact: true })).toBeVisible();
  await expect(kpi(page, 'filas')).toHaveText('2');

  const amount = page.getByRole('checkbox', { name: 'importe_total', exact: true });
  await tabTo(page, amount);
  await page.keyboard.press('Space');
  await expect(amount).toBeChecked();
  await expect(kpi(page, 'importe_total')).toHaveText(NORTE.shown);
  const region = page.getByRole('checkbox', { name: 'ventas.region', exact: true });
  await tabTo(page, region);
  await page.keyboard.press('Space');
  await expect(region).toBeChecked();
  await expect(region).toBeFocused();
  await expect.poll(() => bodyRows(page)).toEqual([[NORTE.region, '2', NORTE.shown]]);
  // What a screen reader announces: the polite region, settled.
  const live = page.locator('[aria-live="polite"]');
  await expect(live).toHaveAttribute('aria-busy', 'false');
  await expect(live.getByRole('heading', { name: 'Totales', exact: true })).toBeVisible();
  // The receipt's technical detail opens from the keyboard too.
  const detail = page.locator('summary', { hasText: 'Detalle técnico' }).first();
  await tabTo(page, detail);
  await page.keyboard.press('Enter');
  await expect(detail.locator('..')).toHaveAttribute('open', '');
  await noLeaks(page, SUR);
  await axe(page);
  onlySemantic(network.sent);
});

/**
 * #261: a SUM wider than its DECIMAL(18, 2) source. Oracle, computed apart
 * from the server with exact decimals: Norte's two rows of `grandes` in
 * `semantic/ventas.sql`, 9999999999999999.99 + 48735710.81.
 */
const LARGE = {
  option: 'Importes grandes · metrics/ventas_grandes.yaml',
  model: 'metrics/ventas_grandes.yaml',
  total: {
    type: 'DECIMAL(38,2)',
    value: '10000000048735710.80',
    shown: '10.000.000.048.735.710,80',
  },
  doble: {
    type: 'DECIMAL(39,2)',
    value: '20000000097471421.60',
    shown: '20.000.000.097.471.421,60',
  },
};

test('a SUM beyond 18 digits: the real answer, every digit and its scale on screen', async ({
  page,
}) => {
  test.setTimeout(90_000);
  const network = watch(page);
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const metric = (name: string) => page.getByRole('checkbox', { name, exact: true });

  await signIn(page, NORTE.user, NORTE.name);
  await openExplore(page);
  const model = page.getByLabel('Modelo');
  await expect(model.getByRole('option', { name: LARGE.option, exact: true })).toHaveCount(1);
  // The model, by keyboard: past `ventas`, the last one of the catalog.
  await tabTo(page, model);
  await page.keyboard.press('ArrowDown');
  await expect(model).toHaveValue('metrics/ventas.yaml');
  await settled(page, 1);
  await page.keyboard.press('ArrowDown');
  await expect(model).toHaveValue(LARGE.model);
  // Its first selectable metric starts the question.
  await expect(metric('doble')).toBeChecked();
  await expect(kpi(page, 'doble')).toHaveText(LARGE.doble.shown);
  await settled(page, 1);

  // `total` next to it: the answer the page reads, observed on the network.
  const both = page.waitForResponse((response) => {
    const request = response.request();
    if (request.method() !== 'POST') return false;
    if (new URL(response.url()).pathname !== '/api/v1/semantic/query') return false;
    const names = JSON.parse(request.postData() ?? '{}').query?.selections?.map(
      (selection: { name: string }) => selection.name,
    );
    return names?.length === 2 && names.includes('total') && names.includes('doble');
  });
  await tabTo(page, metric('total'));
  await page.keyboard.press('Space');
  await expect(metric('total')).toBeChecked();
  const response = await both;
  expect(response.status()).toBe(200);
  const sent: { model: string; query: { selections: { id: string; name: string }[] } } = JSON.parse(
    response.request().postData()!,
  );
  expect(sent.model).toBe(LARGE.model);
  const raw = await response.text();
  // The exact decimals travel as JSON strings: never a number to round.
  expect(raw).toContain(`"${LARGE.total.value}"`);
  expect(raw).toContain(`"${LARGE.doble.value}"`);
  const [block, ...others] = JSON.parse(raw).result.blocks;
  expect(others).toEqual([]);
  expect(block.row_count).toBe(1);
  expect(block.rows).toHaveLength(1);
  for (const name of ['total', 'doble'] as const) {
    const id = sent.query.selections.find((selection) => selection.name === name)!.id;
    const at = block.columns.findIndex(
      (column: { selection_id: string }) => column.selection_id === id,
    );
    expect(block.columns[at]).toMatchObject({
      kind: 'metric',
      type: LARGE[name].type,
      masked: false,
    });
    expect(block.rows[0][at]).toBe(LARGE[name].value);
  }

  // The KPI strip: every digit and the scale, grouped as es writes them.
  await settled(page, 2);
  await expect(kpi(page, 'total')).toHaveText(LARGE.total.shown);
  await expect(kpi(page, 'doble')).toHaveText(LARGE.doble.shown);
  await expect(page.getByRole('heading', { name: 'Recibo de los totales' })).toBeVisible();
  await expect(page.locator('dt:text-is("Identificador") + dd code')).toHaveCount(1);
  await noLeaks(page, SUR);
  await axe(page);

  onlySemantic(network.sent);
  expect(network.concurrent()).toEqual([]);
  expect(errors).toEqual([]);
});

/**
 * The 21 metrics of `semantic/project/metrics/ventas.yaml` the viewer may
 * select, in the catalog's order; `margen` (masked) is not among them.
 */
const SELECTABLE = [
  'filas',
  'id_doble',
  'id_extremos',
  'id_maximo',
  'id_medio',
  'id_medio_redondeado',
  'id_minimo',
  'id_por_fila',
  'id_rango',
  'id_total',
  'importe_doble',
  'importe_extremos',
  'importe_maximo',
  'importe_medio',
  'importe_medio_fino',
  'importe_minimo',
  'importe_neto',
  'importe_por_fila',
  'importe_rango',
  'importe_total',
  'importes_contados',
];
const TWENTY = SELECTABLE.slice(0, 20);
const MANY =
  'Muchas métricas pueden dificultar la lectura. Puedes quitar algunas para comparar mejor.';
const MAX = 'Máximo 20 métricas: quita una para agregar otra.';

test('20 metrics and 2 breakdowns: exact Norte values, the help from 10, the limit at 20', async ({
  page,
}) => {
  test.setTimeout(180_000);
  const network = watch(page);
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const metric = (name: string) => page.getByRole('checkbox', { name, exact: true });
  /** Adds `name` with the keyboard: Tab to its checkbox, Space. */
  const add = async (name: string) => {
    await tabTo(page, metric(name));
    await page.keyboard.press('Space');
    await expect(metric(name)).toBeChecked();
    await expect(kpi(page, name)).toBeVisible();
    await settled(page, TWENTY.indexOf(name) + 1);
  };

  await signIn(page, NORTE.user, NORTE.name);
  await openExplore(page);
  await chooseVentas(page);
  // Every selectable metric is offered, and `filas` starts the question.
  for (const name of SELECTABLE) await expect(metric(name)).toBeEnabled();

  for (const name of TWENTY.slice(1, 9)) await add(name);
  await expect(page.getByText(MANY)).toHaveCount(0);
  // 9 → 10: the help appears, without blocking anything.
  await add(TWENTY[9]!);
  await expect(page.getByText(MANY)).toBeVisible();
  await expect(metric(TWENTY[10]!)).toBeEnabled();
  // 10 → 9 (Space on the same checkbox): the help leaves; back to 10, it returns.
  await page.keyboard.press('Space');
  await expect(metric(TWENTY[9]!)).not.toBeChecked();
  await expect(page.getByText(MANY)).toHaveCount(0);
  await settled(page, 9);
  await page.keyboard.press('Space');
  await expect(metric(TWENTY[9]!)).toBeChecked();
  await expect(page.getByText(MANY)).toBeVisible();
  await settled(page, 10);

  await expect(page.getByText(MAX)).toHaveCount(0);
  for (const name of TWENTY.slice(10)) await add(name);
  // At 20: the 21st cannot be added, every chosen one can still be removed.
  const extra = metric(SELECTABLE[20]!);
  await expect(extra).toBeDisabled();
  await expect(extra).not.toBeChecked();
  await expect(page.getByText(MAX)).toBeVisible();
  for (const name of TWENTY) await expect(metric(name)).toBeEnabled();
  await expect(page.getByText(MANY)).toBeVisible();

  for (const name of ['ventas.region', 'ventas.ciudad']) {
    const field = page.getByRole('checkbox', { name, exact: true });
    await tabTo(page, field);
    await page.keyboard.press('Space');
    await expect(field).toBeChecked();
    await settled(page, 20);
  }

  // The KPIs: 20 totals over Norte's two rows (La Paz 100.10, El Alto 0.20; ids 1 and 2).
  await expect(page.locator('dl.kpis > div')).toHaveCount(20);
  const totals: Record<string, string> = {
    filas: '2',
    id_total: '3',
    id_medio: '1,50',
    // AVG(1, 2) = 1.5 cast to DECIMAL(18, 0), half-even.
    id_medio_redondeado: '2',
    id_rango: '1',
    importe_total: NORTE.shown,
    importe_minimo: '0,20',
    importe_maximo: '100,10',
    importe_medio: '50,15',
    importe_medio_fino: '50,1500',
    importe_por_fila: '50,150',
    importe_rango: '99,90',
    importe_neto: '100,10',
    importe_doble: '200,60',
  };
  for (const [name, value] of Object.entries(totals))
    await expect(kpi(page, name)).toHaveText(value);

  // The breakdown: 2 fields and the 20 metrics, in the order they were chosen.
  await expect(breakdown(page).getByRole('columnheader')).toHaveText([
    'ventas.region',
    'ventas.ciudad',
    ...TWENTY,
  ]);
  await expect
    .poll(() => bodyRows(page))
    .toEqual([
      [
        ...['Norte', 'El Alto', '1', '4', '4', '2', '2,00', '2', '2', '2,000', '0', '2'],
        ...['0,40', '0,40', '0,20', '0,20', '0,2000', '0,20', '0,00', '0,200', '0,00', '0,20'],
      ],
      [
        ...['Norte', 'La Paz', '1', '2', '2', '1', '1,00', '1', '1', '1,000', '0', '1'],
        ...['200,20', '200,20', '100,10', '100,10', '100,1000', '100,10', '0,00', '100,100'],
        ...['0,00', '100,10'],
      ],
    ]);
  await receiptIds(page);
  await noLeaks(page, SUR);
  await axe(page);

  onlySemantic(network.sent);
  // Each question: the total, then the breakdown; never two answered at once.
  expect(network.concurrent()).toEqual([]);
  expect(errors).toEqual([]);
});

/** A direct query of the same session (its cookie), as the page would send it. */
async function askDirectly(page: Page, origin: string, metric: string) {
  const response = await page.request.post('/api/v1/semantic/query', {
    headers: { Origin: origin, Accept: 'application/json' },
    data: {
      model: 'metrics/ventas.yaml',
      query: { selections: [{ id: 'm0', kind: 'metric', name: metric }] },
    },
  });
  return { response, body: await response.json() };
}

/**
 * The neutral 404: its code, its fixed detail and its correlation, nothing
 * that names what was asked. Returns the body without the correlation.
 */
/** What a page response and a direct API response have in common. */
type Answered = { status(): number; headers(): Record<string, string> };

function neutral({ response, body }: { response: Answered; body: Record<string, unknown> }) {
  expect(response.status()).toBe(404);
  expect(Object.keys(body).sort()).toEqual([
    'code',
    'detail',
    'request_id',
    'status',
    'title',
    'type',
  ]);
  expect(body.code).toBe('model_not_found');
  expect(body.request_id).toBe(response.headers()['x-request-id']);
  expect(JSON.stringify(body)).not.toMatch(
    /centinela|reserva|inexistente|importe|filas|ventas|recuento|metrics\//i,
  );
  const rest = { ...body };
  delete rest.request_id;
  return rest;
}

test('membership revoked with the page open: neutral 404, nothing kept, refresh, recovery', async ({
  page,
  baseURL,
}) => {
  test.setTimeout(150_000);
  // The project file the server reads on every request; restored whatever happens.
  const members = fileURLToPath(
    new URL('./semantic/project/policies/members.yaml', import.meta.url),
  );
  const original = readFileSync(members);
  // Norte's provider group loses its mapping, as in the #138/#140 revocation tests.
  const revoked = original
    .toString('utf8')
    .replace(/^ {2}analysts: \[viewer\]$/m, '  catalog_authors: [viewer]');
  expect(revoked).not.toBe(original.toString('utf8'));
  const network = watch(page);
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const isQuery = (response: Response) =>
    new URL(response.url()).pathname === '/api/v1/semantic/query' &&
    response.request().method() === 'POST';
  const isCatalog = (response: Response) =>
    new URL(response.url()).pathname === '/api/v1/semantic/catalog';
  const metric = (name: string) => page.getByRole('checkbox', { name, exact: true });

  try {
    // 1. The selection is on screen with its exact answer and its receipt.
    await signIn(page, NORTE.user, NORTE.name);
    await openExplore(page);
    await chooseVentas(page);
    await metric('importe_total').check();
    await expect(kpi(page, 'importe_total')).toHaveText(NORTE.shown);
    await settled(page, 2);
    await expect(page.getByRole('heading', { name: 'Recibo de los totales' })).toBeVisible();
    const receipts = await page.locator('dt:text-is("Identificador") + dd code').allTextContents();
    expect(receipts).toHaveLength(1);
    const receipt = receipts[0]!;

    // 2. Revoked while the page stays open; the session cookie itself stays valid.
    writeFileSync(members, revoked);
    expect((await page.request.get('/api/v1/session')).status()).toBe(200);

    // 3. The same model asked again: the real authorizer refuses it with the neutral 404.
    const refusal = page.waitForResponse(isQuery);
    // A click, not check(): the page unticks it as soon as the refusal arrives.
    await metric('importe_minimo').click();
    const refused = await refusal;
    const fromPage = neutral({ response: refused, body: await refused.json() });
    const requestId = refused.headers()['x-request-id']!;
    const alert = page.getByRole('alert');
    await expect(alert).toContainText(
      'Esta consulta ya no está disponible para tu sesión. Actualiza el catálogo.',
    );
    await expect(alert).toContainText('HTTP 404 · Código: model_not_found');
    await expect(alert).toContainText(`Solicitud: ${requestId}`);
    // The selection goes with its answer: nothing ticked, no figure, table or receipt.
    await expect(page.getByRole('checkbox', { checked: true })).toHaveCount(0);
    await expect(page.locator('dl.kpis')).toHaveCount(0);
    await expect(page.getByRole('table')).toHaveCount(0);
    await expect(page.getByRole('img')).toHaveCount(0);
    await expect(page.getByRole('heading', { name: /^Recibo/ })).toHaveCount(0);
    await expect(page.locator('dt:text-is("Identificador")')).toHaveCount(0);
    const withdrawn = await page.content();
    expect(withdrawn).not.toContain(NORTE.shown);
    expect(withdrawn).not.toContain(NORTE.amount);
    expect(withdrawn).not.toContain(receipt);
    await noLeaks(page, SUR);
    await axe(page);

    // 4. Only an explicit refresh asks again, and the catalog no longer offers it.
    const sinceRefusal = network.sent.length;
    const emptied = page.waitForResponse(isCatalog);
    await page
      .locator('[aria-live="polite"]')
      .getByRole('button', { name: 'Actualizar catálogo' })
      .click();
    const empty = await emptied;
    expect(empty.status()).toBe(200);
    expect(await empty.json()).toEqual({ status: 'ready', models: [] });
    await expect(page.getByRole('status')).toHaveText(
      'Tu sesión no tiene métricas disponibles para consultar.',
    );
    await expect(page.getByLabel('Modelo')).toHaveCount(0);
    await expect(metric('importe_total')).toHaveCount(0);
    await expect(page.getByRole('alert')).toHaveCount(0);
    await expect(page.locator('dl.kpis')).toHaveCount(0);
    // No retry of its own: between the refusal and the refresh, only the catalog.
    expect(network.sent.slice(sinceRefusal)).toEqual(['GET /api/v1/semantic/catalog']);
    await axe(page);

    // 5a. While revoked, the metric it saw, one that does not exist and one it
    // may never see answer alike: the page's body, apart from the correlation.
    for (const name of ['importe_total', 'metrica_inexistente', 'reserva_centinela'])
      expect(neutral(await askDirectly(page, baseURL!, name))).toEqual(fromPage);

    // 6. Restored: offered again, and only after the explicit refresh.
    writeFileSync(members, original);
    await expect(page.getByLabel('Modelo')).toHaveCount(0);
    const recovered = page.waitForResponse(isCatalog);
    await page.getByRole('button', { name: 'Actualizar catálogo' }).click();
    expect((await recovered).status()).toBe(200);
    await expect(page.getByLabel('Modelo')).toHaveValue('metrics/recuento.yaml');
    await expect(kpi(page, 'filas')).toHaveText('2');
    await chooseVentas(page);
    await expect(metric('importe_total')).toBeEnabled();
    await expect(metric('importe_total')).not.toBeChecked();
    await metric('importe_total').check();
    await expect(kpi(page, 'importe_total')).toHaveText(NORTE.shown);
    await settled(page, 2);

    // 5b. Absent vs hidden, mapped again: a metric the model does not declare
    // and the restricted sentinel get the same neutral 404, naming neither,
    // while the same session still reads what it may.
    const absent = neutral(await askDirectly(page, baseURL!, 'metrica_inexistente'));
    const hidden = neutral(await askDirectly(page, baseURL!, 'reserva_centinela'));
    expect(hidden).toEqual(absent);
    expect(absent).toEqual(fromPage);
    const allowed = await askDirectly(page, baseURL!, 'importe_total');
    expect(allowed.response.status()).toBe(200);
    expect(JSON.stringify(allowed.body)).toContain(NORTE.amount);
    await expect(page.getByRole('alert')).toHaveCount(0);
    await noLeaks(page, SUR);
  } finally {
    writeFileSync(members, original);
  }
  onlySemantic(network.sent);
  expect(network.concurrent()).toEqual([]);
  expect(errors).toEqual([]);
});

test('filters as the page would send them: the server answers what the client accepts, refuses what it refuses', async ({
  page,
}) => {
  // #142: the client's check against the real server, row policy on.
  await signIn(page, NORTE.user, NORTE.name);
  const origin = new URL(page.url()).origin;
  const catalog = await (await page.request.get('/api/v1/semantic/catalog')).json();
  const model = catalog.models.find((m: { model: string }) => m.model === 'metrics/ventas.yaml');
  const metrics = ['importe_total', 'filas'];
  const region = { kind: 'dimension' as const, name: 'ventas.region' };
  const total = { kind: 'metric' as const, selection: metricId('importe_total') };
  const norte = {
    op: 'compare' as const,
    target: region,
    comparison: '=' as const,
    value: { type: 'text' as const, value: 'Norte' },
  };
  const many = (n: number) => Array.from({ length: n }, () => norte);
  const cases: [string, UserFilters, unknown][] = [
    ['own region', { where: norte, having: null }, [[NORTE.amount, '2']]],
    // Another region's filter never passes the row policy: nothing of Sur.
    [
      'other region',
      { where: { ...norte, value: { type: 'text', value: 'Sur' } }, having: null },
      [[null, '0']],
    ],
    [
      'after aggregating',
      {
        where: null,
        having: {
          op: 'compare',
          target: total,
          comparison: '>',
          value: { type: 'number', value: '100.00' },
        },
      },
      [[NORTE.amount, '2']],
    ],
    [
      'not and in',
      {
        where: {
          op: 'not',
          filter: { op: 'in', target: region, values: [{ type: 'text', value: 'Sur' }] },
        },
        having: null,
      },
      [[NORTE.amount, '2']],
    ],
    [
      'a digit lost',
      {
        where: null,
        having: {
          op: 'compare',
          target: total,
          comparison: '=',
          value: { type: 'number', value: '1.234' },
        },
      },
      null,
    ],
    ['NULL compared', { where: { ...norte, value: { type: 'null' } }, having: null }, null],
    [
      'a metric before aggregating',
      { where: { ...norte, target: total, value: { type: 'number', value: '1' } }, having: null },
      null,
    ],
    ['an empty group', { where: { op: 'or', filters: [] }, having: null }, null],
    [
      'a field never announced',
      {
        where: { ...norte, target: { kind: 'dimension', name: 'ventas.secreto_centinela' } },
        having: null,
      },
      null,
    ],
    ['257 conditions', { where: { op: 'and', filters: many(256) }, having: null }, null],
  ];
  for (const [name, filters, rows] of cases) {
    const accepted = filterProblems(filters, model, metrics).length === 0;
    expect(accepted, name).toBe(rows !== null);
    const response = await page.request.post('/api/v1/semantic/query', {
      headers: { Origin: origin, Accept: 'application/json' },
      data: {
        model: 'metrics/ventas.yaml',
        query: {
          selections: metrics.map((m) => ({ id: metricId(m), kind: 'metric', name: m })),
          ...filterQuery(filters),
        },
      },
    });
    const body = await response.json();
    if (accepted) {
      expect(response.status(), name).toBe(200);
      expect(body.result.blocks[0].rows, name).toEqual(rows);
    } else {
      expect([400, 403, 404, 422], name).toContain(response.status());
      expect(JSON.stringify(body), name).not.toMatch(SENTINELS);
    }
  }
});

/** The id of the element that holds the focus. */
const focusedId = (page: Page) => page.evaluate(() => document.activeElement?.id ?? '');

/** Writes `field operator value` into the condition whose field select is `index`. */
async function writeCondition(page: Page, index: number, field: string, value?: string) {
  await page.getByLabel('Campo', { exact: true }).nth(index).selectOption(field);
  if (value !== undefined) await page.getByLabel('Valor', { exact: true }).nth(index).fill(value);
}

async function applyFilters(page: Page) {
  await page.getByRole('button', { name: 'Aplicar filtros' }).click();
  await expect.poll(() => focusedId(page)).toBe('filters-applied');
  await expect(page.locator('[aria-live="polite"]').first()).toHaveAttribute('aria-busy', 'false');
}

/**
 * No figure of `other`'s rows nor any restricted text on the page. Its region's
 * name may appear: the person typed it as a filter value (#265).
 */
async function noFiguresOf(page: Page, other: typeof NORTE) {
  const html = await page.content();
  expect(html).not.toMatch(SENTINELS);
  for (const figure of [other.amount, other.shown]) expect(html).not.toContain(figure);
  expect((await bodyRows(page).catch(() => [])).every((row) => row[0] !== other.region)).toBe(true);
}

test('row filters: applied explicitly, exact under the row policy, cleared, by keyboard and axe', async ({
  page,
}) => {
  // #265 on the real server: drafts ask nothing, «Aplicar» asks the total and
  // the breakdown with the filter, and no filter frees a row of another region.
  test.setTimeout(150_000);
  const network = watch(page);
  const queries = () => network.sent.filter((s) => s === 'POST /api/v1/semantic/query').length;
  await signIn(page, NORTE.user, NORTE.name);
  await openExplore(page);
  await chooseVentas(page);
  await askByRegion(page, NORTE);

  // A draft asks nothing.
  const before = queries();
  await page.getByRole('button', { name: 'Añadir condición', exact: true }).click();
  await writeCondition(page, 0, 'ventas.region', 'Sur');
  expect(queries()).toBe(before);
  await expect(page.getByText('Hay cambios sin aplicar.')).toBeVisible();

  // Sur under Norte's policy: an empty breakdown, a NULL sum and a zero count.
  await applyFilters(page);
  await expect(kpi(page, 'filas')).toHaveText('0');
  await expect(kpi(page, 'importe_total')).toHaveText('Sin dato');
  await expect.poll(() => bodyRows(page)).toEqual([]);
  expect(queries()).toBe(before + 2);

  // El Alto alone: its exact amount.
  await writeCondition(page, 0, 'ventas.ciudad', 'El Alto');
  await applyFilters(page);
  await expect(kpi(page, 'importe_total')).toHaveText('0,20');
  await expect.poll(() => bodyRows(page)).toEqual([['Norte', '1', '0,20']]);

  // OR with Sur, then NOT: still only Norte's rows. The second condition by keyboard.
  await page.getByRole('button', { name: 'Añadir otra condición' }).click();
  await expect.poll(() => focusedId(page)).toMatch(/^filters-n\d+-field$/);
  await page.keyboard.press('Tab');
  await expect.poll(() => focusedId(page)).toMatch(/^filters-n\d+-operator$/);
  await writeCondition(page, 1, 'ventas.region', 'Sur');
  await page.getByLabel('Tipo de grupo', { exact: true }).selectOption('or');
  await applyFilters(page);
  await expect.poll(() => bodyRows(page)).toEqual([['Norte', '1', '0,20']]);
  await page.getByRole('button', { name: 'Negar esta condición' }).first().click();
  await applyFilters(page);
  await expect(page.locator('#filters-applied')).toHaveText(
    'Filtros aplicados: antes de agregar, (no ventas.ciudad es igual a «El Alto» o ventas.region es igual a «Sur»).',
  );
  await expect(kpi(page, 'importe_total')).toHaveText('100,10');
  await expect.poll(() => bodyRows(page)).toEqual([['Norte', '1', '100,10']]);
  await noFiguresOf(page, SUR);
  // Nested groups, with the keyboard: add a group inside and reach it by Enter.
  await page.getByRole('button', { name: 'Añadir grupo dentro' }).focus();
  await page.keyboard.press('Enter');
  await expect.poll(() => focusedId(page)).toMatch(/^filters-n\d+-field$/);
  await axe(page);

  // An empty group is an error: said, focused, never asked.
  const asked = queries();
  await page.getByRole('button', { name: 'Quitar condición' }).last().click();
  await page.getByRole('button', { name: 'Aplicar filtros' }).click();
  await expect.poll(() => focusedId(page)).toBe('filters-notice');
  await expect(page.locator('#filters-notice')).toHaveText(/Corrige los errores/);
  expect(queries()).toBe(asked);

  // «Limpiar»: the person's filters only; Norte's own totals again.
  await page.getByRole('button', { name: 'Limpiar filtros' }).click();
  await expect.poll(() => focusedId(page)).toBe('filters-add');
  await expect(kpi(page, 'importe_total')).toHaveText(NORTE.shown);
  await expect.poll(() => bodyRows(page)).toEqual([[NORTE.region, '2', NORTE.shown]]);
  await expect(page.getByText('Sin filtros aplicados.')).toBeVisible();

  // Sur filtering Norte sees nothing of Norte.
  await signOut(page);
  await signIn(page, SUR.user, SUR.name);
  await openExplore(page);
  await chooseVentas(page);
  await page.getByRole('button', { name: 'Añadir condición', exact: true }).click();
  await writeCondition(page, 0, 'ventas.region', 'Norte');
  await applyFilters(page);
  await expect(kpi(page, 'filas')).toHaveText('0');
  await noFiguresOf(page, NORTE);
});

test('a NULL row filter on a declared test field: Norte 100,10 and Sur -0,20', async ({ page }) => {
  // `observacion` reads the nullable column the restricted `nota` reads (#265).
  test.setTimeout(120_000);
  for (const [who, amount] of [
    [NORTE, '100,10'],
    [SUR, '-0,20'],
  ] as const) {
    await signIn(page, who.user, who.name);
    await openExplore(page);
    await page.getByLabel('Modelo').selectOption('metrics/ventas_notas.yaml');
    await page.getByRole('checkbox', { name: 'importe_total', exact: true }).check();
    await settled(page, 2);
    await page.getByRole('button', { name: 'Añadir condición', exact: true }).click();
    await page.getByLabel('Campo', { exact: true }).selectOption('ventas.observacion');
    await page.getByLabel('Operador', { exact: true }).selectOption('is_null');
    await applyFilters(page);
    await expect(kpi(page, 'importe_total')).toHaveText(amount);
    await expect(kpi(page, 'filas')).toHaveText('1');
    await signOut(page);
  }
});

test('membership revoked before «Aplicar»: the neutral refusal, no figure kept', async ({
  page,
}) => {
  test.setTimeout(120_000);
  const members = fileURLToPath(
    new URL('./semantic/project/policies/members.yaml', import.meta.url),
  );
  const original = readFileSync(members);
  const revoked = original
    .toString('utf8')
    .replace(/^ {2}analysts: \[viewer\]$/m, '  catalog_authors: [viewer]');
  try {
    await signIn(page, NORTE.user, NORTE.name);
    await openExplore(page);
    await chooseVentas(page);
    await page.getByRole('button', { name: 'Añadir condición', exact: true }).click();
    await writeCondition(page, 0, 'ventas.ciudad', 'El Alto');
    writeFileSync(members, revoked);
    await page.getByRole('button', { name: 'Aplicar filtros' }).click();
    await expect(page.getByRole('alert')).toContainText('HTTP 404 · Código: model_not_found');
    await expect(page.locator('dl.kpis')).toHaveCount(0);
    expect(await page.content()).not.toContain('0,20');
    await noLeaks(page, SUR);
  } finally {
    writeFileSync(members, original);
  }
});

/** Rows sorted by their first cell: the server does not promise an order. */
const sorted = (rows: string[][]) => [...rows].sort((a, b) => a[0]!.localeCompare(b[0]!));
const metricFilters = (page: Page) => page.getByRole('region', { name: 'Filtros de métricas' });
const rowFilters = (page: Page) => page.getByRole('region', { name: 'Filtros de filas' });

/** Writes the only condition after aggregating: metric, comparison and value. */
async function writeMetric(page: Page, metric: string, comparison: string, value: string) {
  const editor = metricFilters(page);
  await editor.getByLabel('Métrica', { exact: true }).selectOption(metric);
  await editor.getByLabel('Operador', { exact: true }).selectOption(comparison);
  await editor.getByLabel('Valor', { exact: true }).fill(value);
}

test('metric filters after aggregating: each grain on its own, an empty total, by keyboard and axe', async ({
  page,
}) => {
  // #149 on the real server: `having` on a selected metric, alone and with a
  // row filter; the total and the breakdown, each filtered at its own grain.
  test.setTimeout(180_000);
  const network = watch(page);
  const queries = () => network.sent.filter((s) => s === 'POST /api/v1/semantic/query').length;
  await signIn(page, NORTE.user, NORTE.name);
  await openExplore(page);
  await chooseVentas(page);
  await page.getByRole('checkbox', { name: 'importe_total', exact: true }).check();
  await settled(page, 2);
  await page.getByRole('checkbox', { name: 'ventas.ciudad', exact: true }).check();
  await expect
    .poll(async () => sorted(await bodyRows(page)))
    .toEqual([
      ['El Alto', '1', '0,20'],
      ['La Paz', '1', '100,10'],
    ]);

  // By keyboard: the metric editor starts on its own button, the focus on its control.
  await metricFilters(page).getByRole('button', { name: 'Añadir condición de métrica' }).focus();
  await page.keyboard.press('Enter');
  await expect.poll(() => focusedId(page)).toMatch(/^having-n\d+-field$/);
  const offered = await metricFilters(page)
    .getByLabel('Métrica', { exact: true })
    .locator('option:not([disabled])')
    .allTextContents();
  // The question's metrics, never `margen` (withheld) nor an unselected one.
  expect(offered.map((o) => o.trim())).toEqual(['filas', 'importe_total']);
  const before = queries();
  await writeMetric(page, 'importe_total', '>', '100.00');
  expect(queries()).toBe(before);

  // importe > 100.00: the total 100,30, and La Paz alone by city.
  await applyFilters(page);
  await expect(page.locator('#filters-applied')).toHaveText(
    'Filtros aplicados: después de agregar, importe_total es mayor que 100.00.',
  );
  await expect(kpi(page, 'importe_total')).toHaveText(NORTE.shown);
  await expect(kpi(page, 'filas')).toHaveText('2');
  await expect.poll(() => bodyRows(page)).toEqual([['La Paz', '1', '100,10']]);
  expect(queries()).toBe(before + 2);

  // And WHERE ciudad = El Alto: no total and no city pass, said and not failed.
  await rowFilters(page).getByRole('button', { name: 'Añadir condición', exact: true }).click();
  await writeCondition(page, 0, 'ventas.ciudad', 'El Alto');
  await applyFilters(page);
  await expect(page.locator('#filters-applied')).toHaveText(
    'Filtros aplicados: antes de agregar, ventas.ciudad es igual a «El Alto»; después de agregar, importe_total es mayor que 100.00.',
  );
  await expect(page.getByTestId('kpi-empty')).toHaveText('No hay un total que cumpla este filtro.');
  await expect.poll(() => bodyRows(page)).toEqual([]);
  await expect(page.getByRole('alert')).toHaveCount(0);

  // importe < 100.20 alone: the total is excluded, both cities are not; no sum makes a total.
  await rowFilters(page).getByRole('button', { name: 'Quitar condición', exact: true }).click();
  await writeMetric(page, 'importe_total', '<', '100.20');
  await applyFilters(page);
  await expect(page.getByTestId('kpi-empty')).toBeVisible();
  await expect(page.locator('dl.kpis')).toHaveCount(0);
  await expect
    .poll(async () => sorted(await bodyRows(page)))
    .toEqual([
      ['El Alto', '1', '0,20'],
      ['La Paz', '1', '100,10'],
    ]);
  expect(await page.content()).not.toContain('100,30');
  await noFiguresOf(page, SUR);
  await axe(page);

  // Removing the metric invalidates its clause, visibly: nothing is asked.
  const asked = queries();
  await page.getByRole('checkbox', { name: 'importe_total', exact: true }).uncheck();
  await expect(page.getByRole('alert')).toContainText(
    'La métrica no está seleccionada en esta consulta.',
  );
  await expect(metricFilters(page).getByLabel('Métrica', { exact: true })).toHaveAttribute(
    'aria-invalid',
    'true',
  );
  await page.getByRole('button', { name: 'Aplicar filtros' }).click();
  await expect.poll(() => focusedId(page)).toMatch(/^having-n\d+-field$/);
  await page.waitForTimeout(500);
  expect(queries()).toBe(asked);
  // Back: the same clause, on the same metric, asked again.
  await page.getByRole('checkbox', { name: 'importe_total', exact: true }).check();
  await expect.poll(() => bodyRows(page).then(sorted)).toHaveLength(2);
  await expect(page.getByTestId('kpi-empty')).toBeVisible();
  expect(queries()).toBe(asked + 2);
  expect(network.concurrent()).toEqual([]);

  // Sur, the same filter: only its own rows pass.
  await signOut(page);
  await signIn(page, SUR.user, SUR.name);
  await openExplore(page);
  await chooseVentas(page);
  await page.getByRole('checkbox', { name: 'importe_total', exact: true }).check();
  await settled(page, 2);
  await page.getByRole('checkbox', { name: 'ventas.ciudad', exact: true }).check();
  await metricFilters(page).getByRole('button', { name: 'Añadir condición de métrica' }).click();
  await writeMetric(page, 'importe_total', '>', '100.00');
  await applyFilters(page);
  await expect(kpi(page, 'importe_total')).toHaveText(SUR.shown);
  await expect.poll(() => bodyRows(page)).toEqual([['Tarija', '1', '200,20']]);
  await noFiguresOf(page, NORTE);
});

test('metric filters the page never sends: the server refuses them and shows nothing', async ({
  page,
}) => {
  // #149: forged requests on the real server, each outside the contract or
  // the session's rights. None returns a figure or a restricted name.
  await signIn(page, NORTE.user, NORTE.name);
  const origin = new URL(page.url()).origin;
  const selections = (names: string[]) =>
    names.map((name) => ({ id: metricId(name), kind: 'metric', name }));
  const over = (selection: string, value: object = { type: 'number', value: '100.00' }) => ({
    op: 'compare',
    target: { kind: 'metric', selection },
    comparison: '>',
    value,
  });
  const cases: [string, object][] = [
    [
      'a metric not selected',
      { selections: selections(['filas']), having: over(metricId('importe_total')) },
    ],
    [
      'a metric withheld',
      { selections: selections(['filas', 'margen']), having: over(metricId('margen')) },
    ],
    [
      'a dimension after aggregating',
      {
        selections: selections(['filas']),
        having: {
          op: 'compare',
          target: { kind: 'dimension', name: 'ventas.region' },
          comparison: '=',
          value: { type: 'text', value: 'Sur' },
        },
      },
    ],
    [
      'a number as text',
      {
        selections: selections(['importe_total']),
        having: over(metricId('importe_total'), { type: 'text', value: '100' }),
      },
    ],
    [
      'an exponent',
      {
        selections: selections(['importe_total']),
        having: over(metricId('importe_total'), { type: 'number', value: '1e2' }),
      },
    ],
    [
      'an unknown operator',
      {
        selections: selections(['importe_total']),
        having: { ...over(metricId('importe_total')), op: 'like' },
      },
    ],
    [
      '257 nodes over both',
      {
        selections: selections(['importe_total']),
        filter: {
          op: 'and',
          filters: Array.from({ length: 127 }, () => ({
            op: 'is_not_null',
            target: { kind: 'dimension', name: 'ventas.region' },
          })),
        },
        having: {
          op: 'and',
          filters: Array.from({ length: 128 }, () => over(metricId('importe_total'))),
        },
      },
    ],
    [
      'a forged role on its target',
      {
        selections: selections(['importe_total']),
        having: {
          ...over(metricId('importe_total')),
          target: { kind: 'metric', selection: metricId('importe_total'), roles: ['admin'] },
        },
      },
    ],
    [
      'a forged policy attribute',
      {
        selections: selections(['importe_total']),
        having: over(metricId('importe_total')),
        context: { department: 'Sur' },
      },
    ],
  ];
  for (const [name, query] of cases) {
    const response = await page.request.post('/api/v1/semantic/query', {
      headers: { Origin: origin, Accept: 'application/json' },
      data: { model: 'metrics/ventas.yaml', query },
    });
    expect([400, 403, 404, 422], name).toContain(response.status());
    const body = await response.text();
    expect(body, name).not.toMatch(SENTINELS);
    for (const figure of [NORTE.amount, SUR.amount, '100.10', '200.20']) {
      expect(body, name).not.toContain(figure);
    }
  }
});

test('membership revoked before applying a metric filter: the neutral refusal, no figure kept', async ({
  page,
}) => {
  test.setTimeout(120_000);
  const members = fileURLToPath(
    new URL('./semantic/project/policies/members.yaml', import.meta.url),
  );
  const original = readFileSync(members);
  const revoked = original
    .toString('utf8')
    .replace(/^ {2}analysts: \[viewer\]$/m, '  catalog_authors: [viewer]');
  try {
    await signIn(page, NORTE.user, NORTE.name);
    await openExplore(page);
    await chooseVentas(page);
    await page.getByRole('checkbox', { name: 'importe_total', exact: true }).check();
    await settled(page, 2);
    await metricFilters(page).getByRole('button', { name: 'Añadir condición de métrica' }).click();
    await writeMetric(page, 'importe_total', '>', '100.00');
    writeFileSync(members, revoked);
    await page.getByRole('button', { name: 'Aplicar filtros' }).click();
    await expect(page.getByRole('alert')).toContainText('HTTP 404 · Código: model_not_found');
    await expect(page.locator('dl.kpis')).toHaveCount(0);
    expect(await page.content()).not.toContain(NORTE.shown);
    await noLeaks(page, SUR);
  } finally {
    writeFileSync(members, original);
  }
});
