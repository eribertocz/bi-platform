<script lang="ts">
  import { untrack } from 'svelte';
  import { listCatalog, type SemanticCatalog } from '#lib/api/semantic.ts';
  import {
    answer,
    askable,
    consistency,
    failure,
    groupable,
    initialChoice,
    MANY_METRICS,
    MAX_DIMENSIONS,
    MAX_METRICS,
    reconcile,
    requests,
    type Answer,
    type Choice,
    type Failure,
  } from '#lib/features/semantic/explore.ts';
  import KpiStrip from '#lib/features/semantic/KpiStrip.svelte';
  import ReceiptPanel from '#lib/features/semantic/ReceiptPanel.svelte';
  import ResultChart from '#lib/features/semantic/ResultChart.svelte';
  import ResultTable from '#lib/features/semantic/ResultTable.svelte';

  let catalog = $state<
    | { kind: 'loading' }
    | { kind: 'ready'; value: SemanticCatalog }
    | { kind: 'error'; failure: Failure }
  >({ kind: 'loading' });
  let catalogRevision = $state(0);
  let choice = $state<Choice | null>(null);
  let attempt = $state(0);
  let view = $state<
    | { kind: 'idle' }
    | { kind: 'loading' }
    | { kind: 'ready'; answer: Answer; labels: Record<string, string> }
    | { kind: 'error'; failure: Failure }
  >({ kind: 'idle' });

  const model = $derived(
    catalog.kind === 'ready'
      ? catalog.value.models.find((m) => m.model === choice?.model)
      : undefined,
  );
  // The question as data: a change of selection or model is a new question.
  const question = $derived(askable(choice) ? JSON.stringify(choice) : null);

  // The catalog first, and again only on an explicit refresh: while it
  // loads, no question runs and the previous answer is withdrawn.
  $effect(() => {
    void catalogRevision;
    const controller = new AbortController();
    catalog = { kind: 'loading' };
    view = { kind: 'idle' };
    void listCatalog(controller.signal)
      .then((value) => {
        if (controller.signal.aborted) return;
        catalog = { kind: 'ready', value };
        choice = reconcile($state.snapshot(choice), value);
      })
      .catch((error: unknown) => {
        if (!controller.signal.aborted) catalog = { kind: 'error', failure: failure(error) };
      });
    return () => controller.abort();
  });

  // One attempt per question: replacing or leaving it aborts what is
  // pending, withdraws its answer and discards anything that arrives late.
  $effect(() => {
    void attempt;
    const asked = question;
    if (catalog.kind !== 'ready' || asked === null) {
      // A failure stays on screen until the person changes the question.
      if (untrack(() => view.kind) !== 'error') view = { kind: 'idle' };
      return;
    }
    const current: Choice = JSON.parse(asked);
    const controller = new AbortController();
    view = { kind: 'loading' };
    void answer(current, controller.signal)
      .then((value) => {
        if (!controller.signal.aborted)
          view = { kind: 'ready', answer: value, labels: requests(current).labels };
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted) return;
        const failed = failure(error);
        // No longer usable: the selection goes with its answer.
        if (failed.gone) choice = { model: current.model, metrics: [], dimensions: [] };
        view = { kind: 'error', failure: failed };
      });
    return () => controller.abort();
  });

  function pick(next: Choice) {
    if (view.kind === 'error') view = { kind: 'idle' };
    choice = next;
  }
  /** Whether the change was taken; a refused one leaves the box as it was. */
  function toggle(list: 'metrics' | 'dimensions', name: string, on: boolean): boolean {
    if (!choice) return false;
    const names = choice[list].filter((n) => n !== name);
    // The same limits as the controls, held by the state itself.
    const limit = list === 'metrics' ? MAX_METRICS : MAX_DIMENSIONS;
    if (on && names.length >= limit) return false;
    pick({ ...choice, [list]: on ? [...names, name] : names });
    return true;
  }
</script>

<svelte:head><title>Exploración · bi-platform</title></svelte:head>
<h1>Exploración</h1>
<p class="mb-6 text-slate-700">
  Elige métricas y, si quieres, hasta {MAX_DIMENSIONS} campos para desglosarlas. Solo aparece lo que tu
  sesión puede consultar.
</p>

{#snippet problem(failed: Failure, title: string)}
  <div class="panel mb-6" role="alert">
    <h2>{title}</h2>
    <p>{failed.message}</p>
    {#if failed.status}<p>
        {failed.code ? `HTTP ${failed.status} · Código: ${failed.code}` : `HTTP ${failed.status}`}
      </p>{/if}
    {#if failed.requestId}<p>Solicitud: {failed.requestId}</p>{/if}
  </div>
{/snippet}

{#if catalog.kind === 'loading'}<p role="status">Cargando el catálogo…</p>
{:else if catalog.kind === 'error'}
  {@render problem(catalog.failure, 'No se pudo cargar el catálogo')}
  {#if catalog.failure.retry}<button onclick={() => catalogRevision++}>Reintentar</button>{/if}
{:else if catalog.value.status === 'not_configured'}
  <p role="status">Esta instancia todavía no tiene un proyecto configurado.</p>
{:else if catalog.value.models.length === 0}
  <p role="status">Tu sesión no tiene métricas disponibles para consultar.</p>
  <button onclick={() => catalogRevision++}>Actualizar catálogo</button>
{:else}
  <form class="panel mb-6 grid gap-4" onsubmit={(event) => event.preventDefault()}>
    <div class="flex flex-wrap items-center gap-3">
      <label for="model" class="font-semibold">Modelo</label>
      <select
        id="model"
        class="rounded border border-slate-400 bg-white p-2"
        value={choice?.model}
        onchange={(event) => {
          if (catalog.kind !== 'ready') return;
          const next = catalog.value.models.find((m) => m.model === event.currentTarget.value);
          if (next) pick(initialChoice({ status: 'ready', models: [next] })!);
        }}
      >
        {#each catalog.value.models as option (option.model)}
          <option value={option.model}>{option.name} · {option.model}</option>
        {/each}
      </select>
      <button type="button" onclick={() => catalogRevision++}>Actualizar catálogo</button>
    </div>
    {#if model && choice}
      <fieldset aria-describedby="metrics-limit">
        <legend class="font-semibold">Métricas (1 a {MAX_METRICS})</legend>
        {#each model.metrics as metric (metric.name)}
          {@const on = choice.metrics.includes(metric.name)}
          <label class="mr-4 inline-flex items-center gap-2">
            <input
              type="checkbox"
              checked={on}
              disabled={metric.capabilities.select === 'masked' ||
                (!on && choice.metrics.length >= MAX_METRICS)}
              onchange={(event) => {
                const box = event.currentTarget;
                if (!toggle('metrics', metric.name, box.checked)) box.checked = !box.checked;
              }}
            />
            {metric.name}
            {#if metric.capabilities.select === 'masked'}<span class="text-sm text-slate-600"
                >(solo valores ocultos: todavía no se puede consultar)</span
              >{/if}
            {#if metric.lifecycle.state === 'deprecated'}<span class="text-sm text-slate-600"
                >(obsoleta)</span
              >{/if}
          </label>
        {/each}
      </fieldset>
      <div id="metrics-limit">
        {#if choice.metrics.length >= MAX_METRICS}
          <p class="text-sm text-slate-700">
            Máximo {MAX_METRICS} métricas: quita una para agregar otra.
          </p>
        {/if}
        <!-- Announced once when crossing the threshold: the region stays mounted. -->
        <div role="status">
          {#if choice.metrics.length >= MANY_METRICS}
            <p class="text-sm text-slate-700">
              Muchas métricas pueden dificultar la lectura. Puedes quitar algunas para comparar
              mejor.
            </p>
          {/if}
        </div>
      </div>
      <fieldset>
        <legend class="font-semibold">Desglosar por (0 a {MAX_DIMENSIONS})</legend>
        {#each groupable(model) as field (field.name)}
          {@const on = choice.dimensions.includes(field.name)}
          <label class="mr-4 inline-flex items-center gap-2">
            <input
              type="checkbox"
              checked={on}
              disabled={!on && choice.dimensions.length >= MAX_DIMENSIONS}
              onchange={(event) => {
                const box = event.currentTarget;
                if (!toggle('dimensions', field.name, box.checked)) box.checked = !box.checked;
              }}
            />
            {field.name}
          </label>
        {:else}<p class="text-sm text-slate-600">Este modelo no tiene campos para desglosar.</p>
        {/each}
      </fieldset>
    {/if}
  </form>

  <div aria-busy={view.kind === 'loading'} aria-live="polite">
    {#if view.kind === 'idle'}
      {#if !askable(choice)}<p role="status">Elige al menos una métrica.</p>{/if}
    {:else if view.kind === 'loading'}<p role="status">Consultando…</p>
    {:else if view.kind === 'error'}
      {@render problem(view.failure, 'No se pudo responder la consulta')}
      {#if view.failure.retry}<button onclick={() => attempt++}>Reintentar</button>{/if}
      {#if view.failure.gone}<button onclick={() => catalogRevision++}>Actualizar catálogo</button
        >{/if}
    {:else}
      {@const result = view.answer}
      <section class="mb-6" aria-labelledby="totals-title">
        <h2 id="totals-title">Totales</h2>
        <p class="mb-3 text-sm text-slate-700">
          Calculados por el servidor sobre todos los datos que puedes ver, sin desglose; no son la
          suma de las filas de la tabla.
        </p>
        <KpiStrip run={result.kpi} labels={view.labels} />
      </section>
      {#if result.table}
        {@const block = result.table.result.blocks[0]!}
        {#if consistency(result) === 'diverged'}
          <p class="panel mb-6" role="note">
            Los totales y el desglose leyeron versiones distintas de los datos, del modelo o de las
            políticas: pueden no coincidir.
          </p>
        {:else}
          <p class="mb-6 text-sm text-slate-700" role="note">
            Los totales y el desglose son dos lecturas, cada una con su recibo.
          </p>
        {/if}
        <section class="panel mb-6" aria-labelledby="chart-title">
          <h2 id="chart-title">Gráfico</h2>
          <ResultChart {block} labels={view.labels} />
        </section>
        <section class="panel mb-6" aria-label="Desglose">
          <ResultTable {block} caption="Desglose" labels={view.labels} />
        </section>
        <ReceiptPanel
          receipt={result.table.receipt}
          notices={result.table.notices}
          title="Recibo del desglose"
        />
      {/if}
      <div class="mt-6">
        <ReceiptPanel
          receipt={result.kpi.receipt}
          notices={result.kpi.notices}
          title="Recibo de los totales"
        />
      </div>
    {/if}
  </div>
{/if}
