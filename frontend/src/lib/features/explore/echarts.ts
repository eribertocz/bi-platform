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
import { formatExact, regionLabel, relativeAmounts, type RegionRow } from './data';
use([BarChart, GridComponent, TooltipComponent, SVGRenderer]);
export function mountChart(element: HTMLElement, rows: RegionRow[]) {
  const chart = init(element, undefined, { renderer: 'svg' });
  try {
    const option: ComposeOption<BarSeriesOption | GridComponentOption | TooltipComponentOption> = {
      animation: false,
      grid: { left: 60, right: 20, bottom: 40, top: 20 },
      xAxis: { type: 'category', data: rows.map((r) => regionLabel(r.region)) },
      yAxis: { type: 'value', axisLabel: { formatter: '{value} %' } },
      tooltip: {
        trigger: 'item',
        renderMode: 'richText',
        formatter: (params) => {
          const p = Array.isArray(params) ? params[0] : params;
          const row = p && rows[p.dataIndex];
          if (!row) return '';
          return `${regionLabel(row.region)}\nImporte: ${formatExact(row.amount)}\nFilas: ${formatExact(row.count)}`;
        },
      },
      series: [
        {
          type: 'bar',
          name: 'Importe relativo',
          data: relativeAmounts(rows),
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
