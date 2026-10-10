import { expect, test } from '@playwright/test';

test('real login, protected navigation, logout and revoked cookie', async ({
  page,
  request,
  baseURL,
}) => {
  await expect.poll(async () => (await request.get('/health/ready')).status()).toBe(200);
  const anonymous = await request.get('/api/v1/session');
  expect(anonymous.status()).toBe(401);
  expect(anonymous.headers()['content-type']).toContain('application/problem+json');
  expect((await anonymous.json()).request_id).toBe(anonymous.headers()['x-request-id']);

  await page.goto('/');
  await expect(page).toHaveURL('/login');
  await page.getByLabel('Usuario de desarrollo').selectOption('synthetic-analyst');
  await page.getByRole('button', { name: 'Entrar', exact: true }).click();
  await expect(page).toHaveURL('/');
  await expect(page.getByRole('heading', { name: 'Inicio', exact: true })).toBeVisible();
  await expect(page.getByText('Sesión: Analista sintético · Datos sintéticos')).toBeVisible();
  const cookies = await page.context().cookies();
  const cookie = cookies.find((item) => item.name === 'bi_session');
  expect(cookie).toMatchObject({ httpOnly: true, sameSite: 'Strict', secure: false });
  expect(await page.evaluate(() => document.cookie)).not.toContain('bi_session');
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Inicio', exact: true })).toBeVisible();
  const stolen = `bi_session=${cookie?.value}`;
  const csrf = await request.post('/api/v1/logout', {
    headers: { Cookie: stolen, Origin: 'http://evil.test' },
  });
  expect(csrf.status()).toBe(403);
  await page.getByRole('button', { name: 'Cerrar sesión' }).click();
  await expect(page).toHaveURL('/login');
  expect((await request.get('/api/v1/session', { headers: { Cookie: stolen } })).status()).toBe(
    401,
  );
  expect((await page.context().cookies()).some((item) => item.name === 'bi_session')).toBe(false);
  await page.goto('/');
  await expect(page).toHaveURL('/login');

  // Revoke the browser session through a separate real HTTP request.
  await page.getByRole('button', { name: 'Entrar', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Inicio', exact: true })).toBeVisible();
  const renewed = (await page.context().cookies()).find((item) => item.name === 'bi_session');
  expect(
    (
      await request.post('/api/v1/logout', {
        headers: { Cookie: `bi_session=${renewed?.value}`, Origin: baseURL! },
      })
    ).status(),
  ).toBe(204);
  await page.reload();
  await expect(page).toHaveURL('/login');
});
