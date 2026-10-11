import { expect, test } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

test('real project: login, valid metadata, Spanish file error and accessible reload', async ({
  page,
  request,
}) => {
  await expect
    .poll(async () => (await request.get('/health/ready')).status(), { timeout: 120_000 })
    .toBe(200);
  expect((await request.get('/api/v1/models')).status()).toBe(401);
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/modelos');
  await expect(page).toHaveURL('/login');
  await page.getByRole('button', { name: 'Entrar', exact: true }).click();
  await page.getByRole('link', { name: 'Modelos', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Modelos', exact: true })).toBeVisible();
  await expect(page.getByRole('list', { name: 'Modelos del proyecto' })).toContainText('Válido');
  await page.getByRole('button', { name: 'Ventas', exact: true }).click();
  const detail = page.getByRole('region', { name: 'Ventas', exact: true });
  await expect(detail).toContainText('importe_total');
  await expect(detail).toContainText('DECIMAL(18,2)');
  await expect(detail).toContainText('sales.region');
  await expect(detail).toContainText('Admite valores sin dato (NULL)');
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await page.getByRole('button', { name: 'Origen inexistente', exact: true }).click();
  const invalid = page.getByRole('region', { name: 'Origen inexistente', exact: true });
  await expect(invalid).toContainText('El origen no existe');
  await expect(invalid).toContainText('metrics/origen-inexistente.yaml');
  await invalid.getByText('Ubicación y código del diagnóstico', { exact: true }).click();
  await expect(invalid).toContainText('/datasets/0/source');
  await expect(invalid).toContainText('unknown_source');
  await expect(invalid.getByRole('heading', { name: 'Métricas', exact: true })).toHaveCount(0);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await page.getByRole('button', { name: 'Actualizar modelos' }).click();
  await expect(invalid).toContainText('El origen no existe');
  await page.reload();
  await expect(page.getByRole('button', { name: 'Ventas', exact: true })).toBeVisible();
  expect(errors).toEqual([]);
});
