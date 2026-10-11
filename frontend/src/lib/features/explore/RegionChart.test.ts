import { render, screen, waitFor } from '@testing-library/svelte';
import { expect, it, vi } from 'vitest';
import Chart from './RegionChart.svelte';
const mountChart = vi.hoisted(() => vi.fn());
vi.mock('./echarts', () => ({ mountChart }));

it('does not initialize a chart after an immediate unmount during import', async () => {
  const rendered = render(Chart, { rows: [] });
  rendered.unmount();
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(mountChart).not.toHaveBeenCalled();
});
it('reports renderer failures while retaining the accessible fallback', async () => {
  mountChart.mockImplementationOnce(() => {
    throw new Error('renderer unavailable');
  });
  render(Chart, { rows: [] });
  await waitFor(() =>
    expect(screen.getByRole('alert').textContent).toContain('Consulta los valores en la tabla'),
  );
});
