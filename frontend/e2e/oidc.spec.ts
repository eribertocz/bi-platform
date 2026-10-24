import { expect, test } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

test.beforeEach(async ({ request }) => {
  const config = await request.get('/api/v1/oidc/config');
  expect(config.status()).toBe(200);
  const enabled = (await config.json()).enabled;
  if (process.env.BI_E2E_REQUIRE_OIDC === '1') expect(enabled).toBe(true);
  test.skip(!enabled, 'The dedicated OIDC CI job enables the real provider');
});

test('corporate login with real Keycloak, Strict session and local logout', async ({
  page,
  request,
  baseURL,
}) => {
  expect((await request.get('/api/v1/dev/users')).status()).toBe(404);
  await page.goto('/login');
  const login = page.getByRole('button', { name: 'Entrar con mi empresa' });
  await expect(login).toBeVisible();
  expect(
    (
      await new AxeBuilder({ page })
        .withTags(['wcag2a', 'wcag2aa', 'wcag21aa', 'wcag22aa'])
        .analyze()
    ).violations,
  ).toEqual([]);
  await login.focus();
  await expect(login).toBeFocused();
  await page.keyboard.press('Enter');
  await page.getByLabel('Username or email').fill('oidc-user');
  await page.getByLabel('Password', { exact: true }).fill('oidc-test-only');
  await page.getByRole('button', { name: 'Sign In', exact: true }).click();
  await expect(page).toHaveURL('/');
  await expect(page.getByText('Sesión: Corporate Analyst · Datos sintéticos')).toBeVisible();
  const cookies = await page.context().cookies();
  const session = cookies.find((cookie) => cookie.name === 'bi_session');
  expect(session).toMatchObject({ httpOnly: true, sameSite: 'Strict', secure: false });
  expect(cookies.some((cookie) => cookie.name === 'bi_oidc')).toBe(false);
  expect(await page.evaluate(() => document.cookie)).not.toContain('bi_session');
  await page.reload();
  await expect(page.getByText('Sesión: Corporate Analyst · Datos sintéticos')).toBeVisible();
  expect((await page.request.get('/api/v1/session')).status()).toBe(200);
  await page.getByRole('button', { name: 'Cerrar sesión' }).click();
  await expect(page).toHaveURL('/login');
  expect(
    (
      await request.get('/api/v1/session', { headers: { Cookie: `bi_session=${session?.value}` } })
    ).status(),
  ).toBe(401);
  // Local logout leaves corporate SSO under the provider's control.
  expect(
    (await request.post('/api/v1/oidc/start', { headers: { Origin: baseURL! } })).status(),
  ).toBe(200);
});

test('after corporate login, Explore asks only the semantic catalog and shows what it answers', async ({
  page,
}) => {
  // Observed, never intercepted: the real server decides and answers.
  const sent: string[] = [];
  page.on('request', (request) => {
    const { pathname } = new URL(request.url());
    if (pathname.startsWith('/api/')) sent.push(pathname);
  });
  const catalog = page.waitForResponse((response) =>
    response.url().endsWith('/api/v1/semantic/catalog'),
  );
  await page.goto('/login');
  await page.getByRole('button', { name: 'Entrar con mi empresa' }).click();
  await page.getByLabel('Username or email').fill('oidc-user');
  await page.getByLabel('Password', { exact: true }).fill('oidc-test-only');
  await page.getByRole('button', { name: 'Sign In', exact: true }).click();
  await expect(page).toHaveURL('/');
  await expect(page.getByText('Sesión: Corporate Analyst · Datos sintéticos')).toBeVisible();
  const explore = page.getByRole('link', { name: 'Exploración', exact: true });
  await explore.focus();
  await expect(explore).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL('/explorar');
  const answered = await catalog;
  expect(answered.status()).toBe(200);
  const body = await answered.json();
  // Without a project (the dedicated CI job) the page says so; with the
  // governed project of explore.spec.ts it offers the models it lists.
  if (body.status === 'not_configured') {
    await expect(page.getByRole('status')).toHaveText(
      'Esta instancia todavía no tiene un proyecto configurado.',
    );
    await expect(page.getByLabel('Modelo')).toHaveCount(0);
  } else {
    await expect(page.getByLabel('Modelo').getByRole('option')).toHaveText(
      body.models.map((model: { name: string }) => model.name),
    );
  }
  await expect(page.getByRole('alert')).toHaveCount(0);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  // The G1 routes are gone from this page: no query or model inspection.
  expect(sent.filter((path) => /^\/api\/v1\/(queries|models)(\/|$)/.test(path))).toEqual([]);
});

test('a real provider token without nonce cannot create a session', async ({ page, baseURL }) => {
  await page.goto('/login');
  const start = await page.request.post('/api/v1/oidc/start', { headers: { Origin: baseURL! } });
  expect(start.status()).toBe(200);
  const url = new URL((await start.json()).authorization_url);
  expect(url.searchParams.get('code_challenge_method')).toBe('S256');
  url.searchParams.delete('nonce');
  await page.goto(url.toString());
  await page.getByLabel('Username or email').fill('oidc-user');
  await page.getByLabel('Password', { exact: true }).fill('oidc-test-only');
  const callback = page.waitForResponse((response) =>
    response.url().startsWith(`${baseURL}/api/v1/oidc/callback`),
  );
  await page.getByRole('button', { name: 'Sign In', exact: true }).click();
  expect((await callback).status()).toBe(401);
  expect((await page.request.get('/api/v1/session')).status()).toBe(401);
});
