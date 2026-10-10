<script lang="ts">
  import { onMount } from 'svelte';
  import HealthCard from '#lib/HealthCard.svelte';
  import { getHealth, type HealthResult } from '#lib/api/health.ts';

  let live = $state<HealthResult | null>(null);
  let ready = $state<HealthResult | null>(null);
  let loading = $state(true);
  let controller: AbortController | undefined;

  async function refresh() {
    controller?.abort();
    const current = new AbortController();
    controller = current;
    loading = true;
    live = null;
    ready = null;
    await Promise.all([
      getHealth('/health/live', current.signal).then((result) => {
        if (!current.signal.aborted) live = result;
      }),
      getHealth('/health/ready', current.signal).then((result) => {
        if (!current.signal.aborted) ready = result;
      }),
    ]);
    if (!current.signal.aborted) loading = false;
  }

  onMount(() => {
    void refresh();
    return () => controller?.abort();
  });
</script>

<svelte:head
  ><title>Estado del sistema · bi-platform</title><meta
    name="description"
    content="Estado actual de la API y sus dependencias."
  /></svelte:head
>
<div class="mb-6 flex flex-wrap items-start justify-between gap-4">
  <div>
    <h1>Estado del sistema</h1>
    <p class="text-slate-700">Disponibilidad de la API y sus dependencias en este momento.</p>
  </div>
  <button onclick={() => void refresh()} disabled={loading}>Actualizar estado</button>
</div>
<div class="grid gap-4 lg:grid-cols-2">
  <HealthCard title="Proceso de la API" endpoint="/health/live" result={live} />
  <HealthCard title="Listo para servir" endpoint="/health/ready" result={ready} />
</div>
