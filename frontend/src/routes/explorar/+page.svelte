<script lang="ts">
  import { goto } from '$app/navigation';
  import { resolve } from '$app/paths';
  import { QueryError, runQuery } from '#lib/api/query.ts';
  import {
    exploreData,
    formatExact,
    regionLabel,
    salesQuery,
    type ExploreData,
  } from '#lib/features/explore/data.ts';
  import RegionChart from '#lib/features/explore/RegionChart.svelte';
  let region = $state('all');
  let regions = $state<(string | null)[]>([]);
  let attempt = $state(0);
  let view = $state<
    { kind: 'loading' } | { kind: 'ready'; data: ExploreData } | { kind: 'error'; error: Error }
  >({ kind: 'loading' });
  // Outside local synthetic development the server closes this route (#118):
  // retrying cannot succeed, so only the explanation is offered.
  const retryable = (error: Error) =>
    !(error instanceof QueryError && error.problem.code === 'legacy_query_disabled');
  $effect(() => {
    const selected: string | null | undefined = region === 'all' ? undefined : JSON.parse(region);
    void attempt;
    const controller = new AbortController();
    view = { kind: 'loading' };
    void Promise.all([
      runQuery(salesQuery(selected, false), controller.signal),
      runQuery(salesQuery(selected, true), controller.signal),
    ])
      .then(([totals, grouped]) => {
        if (!controller.signal.aborted) {
          const data = exploreData(totals, grouped);
          if (selected === undefined) regions = data.rows.map((row) => row.region);
          view = { kind: 'ready', data };
        }
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted) return;
        if (error instanceof QueryError && error.problem.status === 401) {
          controller.abort();
          void goto(resolve('/login'));
          return;
        }
        view = {
          kind: 'error',
          error: error instanceof Error ? error : new Error('No se pudo consultar la API.'),
        };
        controller.abort();
      });
    return () => controller.abort();
  });
</script>

<svelte:head><title>Exploración · bi-platform</title></svelte:head>
<h1>Exploración de ventas</h1>
<p class="mb-6 text-slate-700">
  Ventas sintéticas de desarrollo. Filtra para explorar los importes y las filas por región.
</p>
<div class="panel mb-6 flex flex-wrap items-center gap-3">
  <label for="region" class="font-semibold">Región</label>
  <select id="region" bind:value={region} class="rounded border border-slate-400 bg-white p-2">
    <option value="all">Todas las regiones</option>
    {#each regions as value (value)}<option value={JSON.stringify(value)}
        >{regionLabel(value)}</option
      >{/each}
  </select>
  <button onclick={() => attempt++}>Actualizar resultados</button>
</div>
<div aria-busy={view.kind === 'loading'}>
  {#if view.kind === 'loading'}<p role="status">Consultando ventas…</p>
  {:else if view.kind === 'error'}
    <div class="panel" role="alert">
      <h2>No se pudo consultar ventas</h2>
      <p>{view.error.message}</p>
      {#if view.error instanceof QueryError}<p>
          HTTP {view.error.problem.status} · Código: {view.error.problem.code}
        </p>
        <p>Solicitud: {view.error.problem.request_id}</p>{/if}
      {#if retryable(view.error)}<button class="mt-4" onclick={() => attempt++}>Reintentar</button
        >{/if}
    </div>
  {:else}
    <dl class="kpis mb-6">
      <div class="panel">
        <dt>Filas</dt>
        <dd data-testid="kpi-count">{formatExact(view.data.count)}</dd>
      </div>
      <div class="panel">
        <dt>Suma de importe</dt>
        <dd data-testid="kpi-amount">{formatExact(view.data.amount)}</dd>
      </div>
    </dl>
    {#if view.data.rows.length === 0}<p role="status">No hay ventas para este filtro.</p>
    {:else}
      <section class="panel mb-6" aria-labelledby="chart-title">
        <h2 id="chart-title">Importe por región</h2>
        {#key view.data}<RegionChart rows={view.data.rows} />{/key}
      </section>
      <div class="panel overflow-x-auto">
        <table class="w-full">
          <caption class="mb-3 text-left font-semibold">Resultados por región</caption>
          <thead
            ><tr
              ><th scope="col">Región</th><th scope="col">Filas</th><th scope="col"
                >Suma de importe</th
              ></tr
            ></thead
          >
          <tbody
            >{#each view.data.rows as row (row.region)}<tr
                ><th scope="row">{regionLabel(row.region)}</th><td>{formatExact(row.count)}</td><td
                  >{formatExact(row.amount)}</td
                ></tr
              >{/each}</tbody
          >
        </table>
      </div>
    {/if}
  {/if}
</div>

<style>
  .kpis {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 18rem), 1fr));
    gap: 1rem;
  }
  dt {
    color: #475569;
  }
  dd {
    font-weight: 700;
    font-size: 1.5rem;
    overflow-wrap: anywhere;
    font-variant-numeric: tabular-nums;
  }
  th,
  td {
    padding: 0.75rem;
    border-bottom: 1px solid #e2e8f0;
    text-align: left;
  }
  td {
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
</style>
