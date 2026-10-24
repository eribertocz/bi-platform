import { init, use } from 'echarts/core';
import { BarChart, type BarSeriesOption } from 'echarts/charts';
import {
  GridComponent,
  TooltipComponent,
  type GridComponentOption,
  type TooltipComponentOption,
} from 'echarts/components';
import { SVGRenderer } from 'echarts/renderers';
import type { ComposeOption } from 'echarts/core';
import type { Bars } from './exact';
use([BarChart, GridComponent, TooltipComponent, SVGRenderer]);

/** Draws `projection` in `element`; the returned function disposes it. */
export function mountBars(element: HTMLElement, projection: Bars, labels: [string, string]) {
  const chart = init(element, undefined, { renderer: 'svg' });
  try {
    const [dimension, metric] = labels;
    const option: ComposeOption<BarSeriesOption | GridComponentOption | TooltipComponentOption> = {
      animation: false,
      grid: { left: 60, right: 20, bottom: 40, top: 20 },
      xAxis: { type: 'category', data: projection.categories },
      yAxis: { type: 'value', axisLabel: { formatter: '{value} %' } },
      tooltip: {
        trigger: 'item',
        renderMode: 'richText',
        formatter: (params) => {
          const p = Array.isArray(params) ? params[0] : params;
          if (!p || projection.categories[p.dataIndex] === undefined) return '';
          return `${dimension}: ${projection.categories[p.dataIndex]}\n${metric}: ${projection.exact[p.dataIndex]}`;
        },
      },
      series: [
        {
          type: 'bar',
          name: `${metric} relativo`,
          data: projection.heights,
          itemStyle: { color: '#1e40af' },
        },
      ],
    };
    chart.setOption(option);
    const observer = new ResizeObserver(() => chart.resize());
    observer.observe(element);
    return () => {
      observer.disconnect();
      chart.dispose();
    };
  } catch (error) {
    chart.dispose();
    throw error;
  }
}
