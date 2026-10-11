import { expect, it, vi } from 'vitest';
const chart = vi.hoisted(() => ({ setOption: vi.fn(), resize: vi.fn(), dispose: vi.fn() }));
vi.mock('echarts/core', () => ({ init: vi.fn(() => chart), use: vi.fn() }));
import { mountChart } from './echarts';

it('disconnects resize observation and disposes the ECharts instance', () => {
  const observe = vi.fn();
  const disconnect = vi.fn();
  let resize!: () => void;
  vi.stubGlobal(
    'ResizeObserver',
    class {
      constructor(callback: () => void) {
        resize = callback;
      }
      observe = observe;
      disconnect = disconnect;
    },
  );
  const element = document.createElement('div');
  const destroy = mountChart(element, [
    { region: 'east', count: '9007199254740993', amount: '9999999999999999.99' },
  ]);
  expect(observe).toHaveBeenCalledWith(element);
  resize();
  expect(chart.resize).toHaveBeenCalledOnce();
  const option = chart.setOption.mock.calls[0]![0];
  expect(option.tooltip.renderMode).toBe('richText');
  expect(option.tooltip.formatter({ dataIndex: 0 })).toContain('9.999.999.999.999.999,99');
  expect(option.series[0].data).toEqual([100]);
  destroy();
  expect(disconnect).toHaveBeenCalledOnce();
  expect(chart.dispose).toHaveBeenCalledOnce();
});
it('disposes a partially initialized chart if setup fails', () => {
  chart.setOption.mockImplementationOnce(() => {
    throw new Error('renderer failed');
  });
  expect(() => mountChart(document.createElement('div'), [])).toThrow('renderer failed');
  expect(chart.dispose).toHaveBeenCalledOnce();
});
