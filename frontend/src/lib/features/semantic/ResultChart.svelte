<script lang="ts">
  import { bars, columnLabel, type SemanticBlock } from './exact';
  let {
    block,
    metric,
    labels = {},
  }: { block: SemanticBlock; metric?: string; labels?: Record<string, string> } = $props();
  let element = $state<HTMLDivElement>();
  let failed = $state(false);
  const projection = $derived.by(() => {
    try {
      return bars(block, metric);
    } catch {
      return null;
    }
  });
  const names = $derived<[string, string] | null>(
    projection && [
      columnLabel(projection.dimension, labels),
      columnLabel(projection.metric, labels),
    ],
  );
  // Every new projection disposes the chart of the previous one before
  // drawing: a former question is never left on screen.
  $effect(() => {
    const [target, current, axes] = [element, projection, names];
    failed = false;
    if (!target || !current || !axes) return;
    let disposed = false;
    let cleanup: (() => void) | undefined;
    void import('./bars')
      .then(({ mountBars }) => {
        if (!disposed) cleanup = mountBars(target, current, axes);
      })
      .catch(() => {
        if (!disposed) failed = true;
      });
    return () => {
      disposed = true;
      cleanup?.();
    };
  });
</script>

{#if !projection || !names}
  <p class="text-sm text-slate-600" data-testid="chart-omitted">
    Este resultado no se puede representar como gráfico sin deformarlo. Consulta la tabla.
  </p>
{:else}
  {#if failed}<p role="alert">
      No se pudo cargar el gráfico. Consulta los valores en la tabla.
    </p>{/if}
  <div
    bind:this={element}
    class="chart"
    role="img"
    aria-label={`${names[1]} por ${names[0]}. Los valores exactos están en la tabla de resultados.`}
  ></div>
  <p class="text-sm text-slate-600">
    Alturas relativas al mayor valor absoluto (100 %). Consulta el valor exacto en la tabla; los
    valores muy pequeños pueden no ser visibles.
  </p>
{/if}

<style>
  .chart {
    width: 100%;
    height: 22rem;
  }
</style>
