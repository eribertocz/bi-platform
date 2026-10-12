import { expect, test } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

test('real project: login, valid metadata, Spanish file error and accessible reload', async ({
  page,
  request,
}) => {
  test.setTimeout(150_000);
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
  await page.getByRole('button', { name: 'Actualizar modelos' }).focus();
  await page.keyboard.press('Tab');
  await expect(page.getByRole('button', { name: 'Origen inexistente', exact: true })).toBeFocused();
  await page.keyboard.press('Tab');
  const ventas = page.getByRole('button', { name: 'Ventas', exact: true });
  await expect(ventas).toBeFocused();
  expect(await ventas.evaluate((element) => getComputedStyle(element).outlineStyle)).not.toBe(
    'none',
  );
  await page.keyboard.press('Enter');
  const detail = page.getByRole('region', { name: 'Ventas', exact: true });
  await expect(detail).toContainText('importe_total');
  await expect(detail).toContainText('DECIMAL(18,2)');
  await expect(detail).toContainText('sales.region');
  await expect(detail).toContainText('Admite valores sin dato (NULL)');
  await expect(detail).toContainText('Suma exacta del importe de las ventas.');
  await expect(detail).toContainText('Sinónimos para IA: facturación');
  await expect(detail).toContainText('Región comercial');
  await expect(detail).toContainText('Sinónimos para IA: zona comercial');
  await expect(detail).toContainText('Dimensión de tiempo');
  await expect(detail).toContainText('Fecha del pedido');
  await expect(detail).toContainText('TIMESTAMPTZ (instante)');
  const metadata = await page.evaluate(async () => (await fetch('/api/v1/models')).json());
  const source = metadata.models.find((m: { file: string }) => m.file === 'metrics/ventas.yaml');
  expect(
    source.datasets[0].fields.find((f: { name: string }) => f.name === 'region').annotation
      .ai_context.vendor_hint,
  ).toEqual({ category: 'geography' });
  const declared = detail.getByText('Datasets y campos declarados', { exact: true });
  await declared.click();
  await expect(detail.getByText('Una fila por pedido.', { exact: true })).toBeVisible();
  await expect(detail.getByText('Sinónimos para IA: pedidos', { exact: true })).toBeVisible();
  expect(
    (
      await new AxeBuilder({ page })
        .withTags(['wcag2a', 'wcag2aa', 'wcag21aa', 'wcag22aa', 'best-practice'])
        .analyze()
    ).violations,
  ).toEqual([]);
  await declared.click();
  const fingerprint = detail.getByText('Huella de esta definición', { exact: true });
  await fingerprint.focus();
  await page.keyboard.press('Enter');
  expect(await detail.ariaSnapshot()).toContain('heading "Métricas" [level=3]');
  expect(await detail.ariaSnapshot()).toContain('DECIMAL(18,2)');
  expect(await detail.ariaSnapshot()).toContain('heading "Dimensiones" [level=3]');
  await expect(detail).toHaveAttribute('aria-live', 'polite');
  expect(
    (
      await new AxeBuilder({ page })
        .withTags(['wcag2a', 'wcag2aa', 'wcag21aa', 'wcag22aa', 'best-practice'])
        .analyze()
    ).violations,
  ).toEqual([]);
  await page.keyboard.press('Shift+Tab');
  await expect(detail.getByText('Datasets y campos declarados', { exact: true })).toBeFocused();
  await page.keyboard.press('Shift+Tab');
  await expect(ventas).toBeFocused();
  await page.keyboard.press('Shift+Tab');
  await expect(page.getByRole('button', { name: 'Origen inexistente', exact: true })).toBeFocused();
  await page.keyboard.press('Enter');
  const invalid = page.getByRole('region', { name: 'Origen inexistente', exact: true });
  await expect(invalid).toContainText('El origen no existe');
  await expect(invalid).toContainText('metrics/origen-inexistente.yaml');
  await page.keyboard.press('Tab');
  await expect(ventas).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(
    invalid.getByText('Ubicación y código del diagnóstico', { exact: true }),
  ).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(invalid).toContainText('/datasets/0/source');
  await expect(invalid).toContainText('unknown_source');
  await expect(invalid.getByRole('heading', { name: 'Métricas', exact: true })).toHaveCount(0);
  expect(await invalid.ariaSnapshot()).toContain('El origen no existe');
  expect(
    (
      await new AxeBuilder({ page })
        .withTags(['wcag2a', 'wcag2aa', 'wcag21aa', 'wcag22aa', 'best-practice'])
        .analyze()
    ).violations,
  ).toEqual([]);
  await page.getByRole('button', { name: 'Actualizar modelos' }).click();
  await expect(invalid).toContainText('El origen no existe');
  await page.reload();
  await expect(page.getByRole('button', { name: 'Ventas', exact: true })).toBeVisible();
  const file = fileURLToPath(
    new URL('../../examples/models-project/metrics/ventas.yaml', import.meta.url),
  );
  const original = readFileSync(file, 'utf8');
  await page.getByRole('button', { name: 'Ventas', exact: true }).click();
  try {
    writeFileSync(
      file,
      original.replace('dimension: {}', 'datatype: String\n        dimension: {is_time: true}'),
    );
    await page.getByRole('button', { name: 'Actualizar modelos' }).click();
    await expect(detail).toContainText(
      'Este campo tiene is_time: true, pero su tipo no es temporal.',
    );
    await expect(detail).toContainText('Aviso:');
    await page.setViewportSize({ width: 320, height: 800 });
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
    ).toBe(true);
    expect(
      (
        await new AxeBuilder({ page })
          .withTags(['wcag2a', 'wcag2aa', 'wcag21aa', 'wcag22aa', 'best-practice'])
          .analyze()
      ).violations,
    ).toEqual([]);
    await page.setViewportSize({ width: 1280, height: 720 });
    writeFileSync(file, 'version: [\n');
    await page.getByRole('button', { name: 'Actualizar modelos' }).click();
    const broken = page.getByRole('region', { name: 'ventas.yaml', exact: true });
    await expect(broken).toContainText('El archivo no es un documento YAML o JSON válido');
    await expect(broken).toContainText('Línea');
    await expect(broken).toContainText('columna');
    await expect(broken.getByRole('heading', { name: 'Métricas', exact: true })).toHaveCount(0);
    expect(
      (
        await new AxeBuilder({ page })
          .withTags(['wcag2a', 'wcag2aa', 'wcag21aa', 'wcag22aa', 'best-practice'])
          .analyze()
      ).violations,
    ).toEqual([]);
  } finally {
    writeFileSync(file, original);
  }
  await page.getByRole('button', { name: 'Actualizar modelos' }).click();
  await expect(page.getByRole('region', { name: 'Ventas', exact: true })).toContainText(
    'importe_total',
  );
  expect(errors).toEqual([]);
});
