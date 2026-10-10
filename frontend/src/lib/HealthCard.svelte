<script lang="ts">
  import type { HealthResult, HealthStatus } from '#lib/api/health.ts';

  let {
    title,
    endpoint,
    result,
  }: { title: string; endpoint: string; result: HealthResult | null } = $props();
  const labels: Record<HealthStatus, string> = {
    ok: 'OK',
    degraded: 'Degradado',
    unavailable: 'No disponible',
  };
</script>

<section class="panel" aria-label={title} aria-busy={result === null}>
  <h2 class="mb-1">{title}</h2>
  <p class="mb-4 text-sm text-slate-600"><code>{endpoint}</code></p>
  <div aria-live="polite" aria-atomic="true">
    {#if result === null}
      <p>Consultando…</p>
    {:else if result.kind === 'error'}
      <p class="font-semibold text-red-800">Error de consulta</p>
      <p class="mt-2">{result.message}</p>
    {:else}
      <p
        class="font-semibold"
        class:text-green-800={result.status === 200 && result.health.status === 'ok'}
        class:text-amber-900={result.status === 503 || result.health.status !== 'ok'}
      >
        {result.status === 503 ? 'No listo' : labels[result.health.status]}
        <span class="text-sm font-normal text-slate-600">(HTTP {result.status})</span>
      </p>
      {#if result.health.checks.length > 0}
        <ul class="mt-4 space-y-2" aria-label="Dependencias">
          {#each result.health.checks as check, index (index)}
            <li class="flex flex-wrap justify-between gap-2">
              <span>{check.name}</span><span class="font-medium">{labels[check.status]}</span>
            </li>
          {/each}
        </ul>
      {/if}
    {/if}
  </div>
</section>
