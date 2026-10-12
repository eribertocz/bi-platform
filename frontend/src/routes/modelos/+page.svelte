<script lang="ts">
  import { goto } from '$app/navigation';
  import { resolve } from '$app/paths';
  import {
    listModels,
    ModelsError,
    type ModelsResponse,
    type ModelMember,
  } from '#lib/api/models.ts';
  let attempt = $state(0);
  let selected = $state<string | null>(null);
  let view = $state<
    | { kind: 'loading' }
    | { kind: 'ready'; data: ModelsResponse }
    | { kind: 'error'; message: string }
  >({ kind: 'loading' });
  const active = $derived(
    view.kind === 'ready' ? view.data.models.find((m) => m.file === selected) : undefined,
  );
  $effect(() => {
    void attempt;
    const controller = new AbortController();
    view = { kind: 'loading' };
    void listModels(controller.signal)
      .then((data) => {
        if (!controller.signal.aborted) view = { kind: 'ready', data };
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted) return;
        if (error instanceof ModelsError && error.status === 401) {
          void goto(resolve('/login'));
          return;
        }
        view = {
          kind: 'error',
          message: error instanceof Error ? error.message : 'No se pudo cargar los modelos.',
        };
      });
    return () => controller.abort();
  });
</script>

{#snippet members(title: string, items: ModelMember[])}
  <h3 class="mt-5 mb-2 font-semibold">{title}</h3>
  {#if items.length === 0}<p>No hay {title.toLowerCase()} en este modelo.</p>
  {:else}<ul class="space-y-3">
      {#each items as item (item.id)}<li>
          <strong>{item.name}</strong> · <span>{item.data_type}</span>
          {#if item.nullable === true}<span> · Admite valores sin dato (NULL)</span>
          {:else if item.nullable === false}<span> · No admite valores sin dato</span>
          {:else}<span> · Nulabilidad no determinada</span>{/if}
        </li>{/each}
    </ul>{/if}
{/snippet}

<svelte:head><title>Modelos · bi-platform</title></svelte:head>
<h1>Modelos</h1>
<p class="mb-5 text-slate-700">
  Aquí puedes revisar las métricas y dimensiones de tu proyecto y los archivos que necesitan
  correcciones.
</p>
<button class="mb-5" onclick={() => attempt++}>Actualizar modelos</button>
<div aria-busy={view.kind === 'loading'}>
  {#if view.kind === 'loading'}<p role="status">Cargando modelos…</p>
  {:else if view.kind === 'error'}<div class="panel" role="alert">
      <h2>No se pudo cargar los modelos</h2>
      <p>{view.message}</p>
      <button class="mt-4" onclick={() => attempt++}>Reintentar</button>
    </div>
  {:else}
    {#if view.data.project_status === 'not_configured'}<p role="status">
        No hay un proyecto configurado. La persona que administra la plataforma puede seleccionar su
        carpeta de proyecto para mostrar los modelos.
      </p>
    {:else if view.data.project_status === 'ready' && view.data.models.length === 0}<p
        role="status"
      >
        No hay modelos en la carpeta metrics del proyecto. Añade un archivo YAML o JSON y pulsa
        «Actualizar modelos».
      </p>{/if}
    {#each view.data.diagnostics as diagnostic, i (i)}<p class="panel mb-3" role="alert">
        {diagnostic.message}
      </p>{/each}
    {#if view.data.models.length > 0}
      <ul class="space-y-3 mb-6" aria-label="Modelos del proyecto">
        {#each view.data.models as model (model.file)}<li class="panel">
            <button
              class="text-left"
              aria-pressed={selected === model.file}
              onclick={() => (selected = model.file)}>{model.name}</button
            >
            <p class="mt-2">
              {model.status === 'valid' ? 'Válido' : 'Necesita correcciones'} · {model.file}
            </p>
          </li>{/each}
      </ul>
    {/if}
    {#if active}
      <section class="panel" aria-labelledby="model-detail-title" aria-live="polite">
        <h2 id="model-detail-title">{active.name}</h2>
        <p class="mb-4">Archivo: {active.file}</p>
        {#if active.status === 'valid'}
          {@render members('Métricas', active.metrics)}
          {@render members('Dimensiones', active.dimensions)}
          <details class="mt-5">
            <summary>Huella de esta definición</summary>
            <p class="break-all mt-2">{active.fingerprint}</p>
          </details>
        {:else}<p class="mb-3">
            Corrige estos errores en el archivo y pulsa «Actualizar modelos».
          </p>{/if}
        {#if active.diagnostics.length > 0}<h3 class="mt-4 mb-2 font-semibold">
            Avisos del archivo
          </h3>
          <ul class="space-y-4">
            {#each active.diagnostics as diagnostic, i (i)}<li>
                <p>
                  <strong>{diagnostic.severity === 'error' ? 'Error' : 'Aviso'}:</strong>
                  {diagnostic.message}
                </p>
                <p>
                  {active.file}{#if diagnostic.line != null}
                    · Línea {diagnostic.line}{/if}{#if diagnostic.column != null}, columna {diagnostic.column}{/if}
                </p>
                <details>
                  <summary>Ubicación y código del diagnóstico</summary>
                  <p>Ruta del documento: {diagnostic.path || '/'} · Código: {diagnostic.code}</p>
                </details>
              </li>{/each}
          </ul>{/if}
      </section>
    {/if}
  {/if}
</div>
