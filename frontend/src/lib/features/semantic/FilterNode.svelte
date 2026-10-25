<script lang="ts">
  import type { Domain } from './condition';
  import ConditionEditor from './ConditionEditor.svelte';
  import FilterNode from './FilterNode.svelte';
  import type { FilterProblem } from './filters';
  import type { TreeActions, TreeNode } from './filterTree';

  let {
    node,
    domain,
    id,
    paths,
    problems,
    actions,
  }: {
    node: TreeNode;
    domain: Domain;
    id: string;
    paths: Map<number, string>;
    problems: FilterProblem[];
    actions: TreeActions;
  } = $props();

  const nodeId = $derived(`${id}-n${node.key}`);
  // Only what is reported at this very node: a condition's own literals
  // are shown by its editor.
  const here = $derived(
    problems.filter((p) => p.path === paths.get(node.key)).map((p) => p.message),
  );
  // A saved condition has no editor to show its literals: every problem at
  // it or inside it, by exact path segments (`/filters/1` never takes `/10`).
  const saved = $derived.by(() => {
    const at = paths.get(node.key) ?? '';
    return problems
      .filter((p) => p.path === at || p.path.startsWith(`${at}/`))
      .map((p) => p.message);
  });
</script>

{#if node.kind === 'condition'}
  <div class="grid gap-1">
    <ConditionEditor
      {domain}
      initial={node.draft}
      id={nodeId}
      onchange={(change) => actions.setDraft(node.key, change.draft)}
      onremove={() => actions.remove(node.key)}
    />
    <button type="button" class="justify-self-start" onclick={() => actions.negate(node.key)}>
      Negar esta condición
    </button>
  </div>
{:else if node.kind === 'fixed'}
  <div
    class="grid gap-1 rounded border border-slate-300 p-3"
    role="group"
    aria-label="Condición guardada"
    aria-describedby={saved.length > 0 ? `${nodeId}-errors` : undefined}
  >
    <p class="text-sm">
      Condición guardada que este editor no puede modificar sin cambiarla; se conserva tal cual.
    </p>
    <code class="text-sm break-all">{JSON.stringify(node.filter)}</code>
    {#if saved.length > 0}
      <ul id="{nodeId}-errors" class="text-sm text-red-800">
        {#each saved as message, i (i)}<li>{message}</li>{/each}
      </ul>
    {/if}
    <button type="button" class="justify-self-start" onclick={() => actions.remove(node.key)}>
      Quitar condición guardada
    </button>
  </div>
{:else if node.kind === 'group'}
  <fieldset class="grid gap-3 rounded border border-slate-400 p-3">
    <legend class="font-semibold">
      {node.op === 'and' ? 'Grupo Y: se cumplen todas' : 'Grupo O: se cumple alguna'}
    </legend>
    <div class="flex flex-wrap items-center gap-2">
      <label for="{nodeId}-op">Tipo de grupo</label>
      <select
        id="{nodeId}-op"
        class="rounded border border-slate-400 bg-white p-2"
        value={node.op}
        onchange={(event) => actions.setOp(node.key, event.currentTarget.value as 'and' | 'or')}
      >
        <option value="and">Y: se cumplen todas</option>
        <option value="or">O: se cumple alguna</option>
      </select>
    </div>
    {#each node.children as child (child.key)}
      <FilterNode node={child} {domain} {id} {paths} {problems} {actions} />
    {/each}
    {#if here.length > 0}<p class="text-sm text-red-800" role="status">{here.join(' ')}</p>{/if}
    <div class="flex flex-wrap gap-2">
      <button type="button" id="{nodeId}-add" onclick={() => actions.addCondition(node.key)}>
        Añadir condición al grupo
      </button>
      <button type="button" onclick={() => actions.addGroup(node.key)}>Añadir grupo dentro</button>
      <button type="button" onclick={() => actions.negate(node.key)}>Negar grupo</button>
      <button type="button" onclick={() => actions.remove(node.key)}>Quitar grupo</button>
    </div>
  </fieldset>
{:else}
  <fieldset class="grid gap-3 rounded border border-slate-400 p-3">
    <legend class="font-semibold">NO: se niega lo siguiente</legend>
    <FilterNode node={node.child} {domain} {id} {paths} {problems} {actions} />
    {#if here.length > 0}<p class="text-sm text-red-800" role="status">{here.join(' ')}</p>{/if}
    <button type="button" class="justify-self-start" onclick={() => actions.unnegate(node.key)}>
      Quitar negación
    </button>
  </fieldset>
{/if}
{#if node.kind === 'condition' && here.length > 0}
  <p class="text-sm text-red-800" role="status">{here.join(' ')}</p>
{/if}
