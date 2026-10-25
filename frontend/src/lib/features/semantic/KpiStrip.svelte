<script lang="ts">
  import { columnLabel, shownText, totals, type SemanticRun } from './exact';
  let {
    run,
    labels = {},
    emptyAllowed = false,
  }: {
    run: SemanticRun;
    labels?: Record<string, string>;
    /** The question has a filter after aggregating that may exclude the total. */
    emptyAllowed?: boolean;
  } = $props();
  // Only a total the server aggregated: never a sum of the rows of a table.
  const values = $derived.by(() => {
    try {
      return { ok: true as const, totals: totals(run, emptyAllowed) };
    } catch (error) {
      return { ok: false as const, message: (error as Error).message };
    }
  });
</script>

{#if !values.ok}<p role="alert">{values.message}</p>
{:else if values.totals.kind === 'empty'}
  <p class="panel" data-testid="kpi-empty">No hay un total que cumpla este filtro.</p>
{:else}
  <dl class="kpis">
    {#each values.totals.items as { column, value } (column.selection_id)}
      <div class="panel">
        <dt>{columnLabel(column, labels)}</dt>
        <dd data-kind={value.kind} data-testid={`kpi-${column.selection_id}`}>
          {shownText(value)}
        </dd>
      </div>
    {/each}
  </dl>
{/if}
