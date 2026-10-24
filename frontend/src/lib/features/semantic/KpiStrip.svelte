<script lang="ts">
  import { columnLabel, kpis, shownText, type SemanticRun } from './exact';
  let { run, labels = {} }: { run: SemanticRun; labels?: Record<string, string> } = $props();
  // Only a total the server aggregated: never a sum of the rows of a table.
  const values = $derived.by(() => {
    try {
      return { ok: true as const, items: kpis(run) };
    } catch (error) {
      return { ok: false as const, message: (error as Error).message };
    }
  });
</script>

{#if !values.ok}<p role="alert">{values.message}</p>
{:else}
  <dl class="kpis">
    {#each values.items as { column, value } (column.selection_id)}
      <div class="panel">
        <dt>{columnLabel(column, labels)}</dt>
        <dd data-kind={value.kind} data-testid={`kpi-${column.selection_id}`}>
          {shownText(value)}
        </dd>
      </div>
    {/each}
  </dl>
{/if}
