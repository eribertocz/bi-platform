<script lang="ts">
  import { checkBlock, columnLabel, shown, shownText, type SemanticBlock } from './exact';
  let {
    block,
    caption,
    labels = {},
  }: { block: SemanticBlock; caption: string; labels?: Record<string, string> } = $props();
  // Checked as a whole first: an incompatible block shows no figure at all.
  const checked = $derived.by(() => {
    try {
      return { ok: true as const, block: checkBlock(block) };
    } catch (error) {
      return { ok: false as const, message: (error as Error).message };
    }
  });
</script>

{#if !checked.ok}<p role="alert">{checked.message}</p>
{:else}
  {@const result = checked.block}
  <!-- A wide table scrolls: the keyboard reaches it, named by its caption
       (WAI scrollable-region pattern, required by axe scrollable-region-focusable). -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="overflow-x-auto focus-visible:outline-2 focus-visible:outline-offset-2"
    tabindex="0"
    role="region"
    aria-label={`${caption}, desplazable`}
  >
    <table class="w-full">
      <caption class="mb-3 text-left font-semibold">{caption}</caption>
      <thead>
        <tr>
          {#each result.columns as column (column.selection_id)}
            <th scope="col" class:text-right={column.kind === 'metric'}>
              {columnLabel(column, labels)}
              {#if column.timezone}<span class="block text-xs font-normal text-slate-600"
                  >Zona horaria: {column.timezone}</span
                >{/if}
              {#if column.masked}<span class="block text-xs font-normal text-slate-600"
                  >Valores ocultos por tus permisos</span
                >{/if}
            </th>
          {/each}
        </tr>
      </thead>
      <tbody>
        {#each result.rows as row, r (r)}
          <tr>
            {#each result.columns as column, i (column.selection_id)}
              {@const value = shown(column, row[i] ?? null)}
              <td
                class:text-right={value.kind === 'value' && value.numeric}
                class:italic={value.kind !== 'value'}
                class:text-slate-600={value.kind !== 'value'}
                data-kind={value.kind}
              >
                {#if value.kind === 'masked'}<span aria-label="Valor oculto">Oculto</span
                  >{:else}{shownText(value)}{/if}
              </td>
            {/each}
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  {#if result.row_count === 0}<p role="status">La consulta no devolvió filas.</p>{/if}
  {#if result.limit !== null}
    <p class="mt-2 text-sm text-slate-700" data-testid="applied-limit">
      Límite aplicado: {result.limit}
      {result.limit === 1 ? 'fila' : 'filas'}{#if result.row_count === result.limit}; puede haber
        más filas que no se muestran{/if}.
    </p>
  {/if}
{/if}
