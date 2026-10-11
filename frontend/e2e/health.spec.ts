import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';

const cwd = fileURLToPath(new URL('../..', import.meta.url));
const compose = (action: 'stop' | 'start') => {
  execFileSync(process.env.BI_COMPOSE_COMMAND ?? 'docker', ['compose', action, 'postgres'], {
    cwd,
    stdio: 'pipe',
    timeout: 30_000,
  });
};

test('Estado del sistema reflects real PostgreSQL availability and recovery', async ({
  page,
  request,
}) => {
  // First start: DuckDB downloads its extensions and loads the synthetic fixture.
  await expect
    .poll(async () => (await request.get('/health/ready')).status(), { timeout: 120_000 })
    .toBe(200);
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));

  // Direct navigation exercises Axum's 200.html fallback and the built JS assets.
  const navigation = await page.goto('/estado');
  expect(navigation?.status()).toBe(200);
  await expect(
    page.getByRole('heading', { name: 'Estado del sistema', exact: true }),
  ).toBeVisible();
  const live = page.getByRole('region', { name: 'Proceso de la API' });
  const ready = page.getByRole('region', { name: 'Listo para servir' });
  await expect(live.getByText('OK (HTTP 200)', { exact: true })).toBeVisible();
  await expect(ready.getByText('OK (HTTP 200)', { exact: true })).toBeVisible();
  await expect(ready.getByText('postgres', { exact: true })).toBeVisible();
  await expect(ready.getByText('migrations', { exact: true })).toBeVisible();

  await page.getByRole('link', { name: 'Inicio', exact: true }).click();
  await expect(page).toHaveURL('/login');
  await page.getByRole('link', { name: 'Estado del sistema', exact: true }).click();
  await expect(ready.getByText('OK (HTTP 200)', { exact: true })).toBeVisible();

  for (const path of ['/api/unknown', '/health/unknown', '/missing.js']) {
    const response = await request.get(path, { headers: { accept: 'text/html' } });
    expect(response.status()).toBe(404);
    expect(await response.text()).not.toContain('<html');
  }

  try {
    compose('stop');
    await expect.poll(async () => (await request.get('/health/ready')).status()).toBe(503);
    await page.getByRole('button', { name: 'Actualizar estado' }).click();
    await expect(ready.getByText('No listo (HTTP 503)', { exact: true })).toBeVisible();
    await expect(ready.getByText('No disponible', { exact: true })).toBeVisible();
    await expect(live.getByText('OK (HTTP 200)', { exact: true })).toBeVisible();
  } finally {
    compose('start');
  }

  await expect.poll(async () => (await request.get('/health/ready')).status()).toBe(200);
  await page.getByRole('button', { name: 'Actualizar estado' }).click();
  await expect(ready.getByText('OK (HTTP 200)', { exact: true })).toBeVisible();
  expect(errors).toEqual([]);
});
