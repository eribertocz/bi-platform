<script lang="ts">
  import { onMount } from 'svelte';
  import type { RegionRow } from './data';
  let { rows }: { rows: RegionRow[] } = $props();
  let element: HTMLDivElement;
  let error = $state(false);
  onMount(() => {
    let disposed = false;
    let cleanup: (() => void) | undefined;
    void import('./echarts')
      .then(({ mountChart }) => {
        if (!disposed) cleanup = mountChart(element, rows);
      })
      .catch(() => {
        if (!disposed) error = true;
      });
    return () => {
      disposed = true;
      cleanup?.();
    };
  });
</script>

{#if error}<p role="alert">No se pudo cargar el gráfico. Consulta los valores en la tabla.</p>{/if}
<div
  bind:this={element}
  class="chart"
  role="img"
  aria-label="Importe por región. Los valores exactos están en la tabla de resultados."
></div>
<p class="text-sm text-slate-600">
  Alturas relativas al mayor importe absoluto (100 %). Consulta el importe exacto en la tabla; los
  importes muy pequeños pueden no ser visibles.
</p>

<style>
  .chart {
    width: 100%;
    height: 22rem;
  }
</style>
