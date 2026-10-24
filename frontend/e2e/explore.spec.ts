import { expect, test, type Page, type Request } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

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
// running semantic/ventas.sql. With oidc.spec.ts a run signs in eight times: the server's
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
