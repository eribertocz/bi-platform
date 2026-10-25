<script lang="ts">
  import type { CatalogModel } from '#lib/api/semantic.ts';
  import ConditionEditor from './ConditionEditor.svelte';
  import FilterNode from './FilterNode.svelte';
  import type { FilterProblem } from './filters';
  import type { TreeActions, TreeNode } from './filterTree';

  let {
    node,
    model,
    id,
    paths,
    problems,
    actions,
  }: {
    node: TreeNode;
    model: CatalogModel;
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
</script>

{#if node.kind === 'condition'}
  <div class="grid gap-1">
    <ConditionEditor
      {model}
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
  >
    <p class="text-sm">
      Condición guardada que este editor no puede modificar sin cambiarla; se conserva tal cual.
    </p>
    <code class="text-sm break-all">{JSON.stringify(node.filter)}</code>
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
      <FilterNode node={child} {model} {id} {paths} {problems} {actions} />
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
    <FilterNode node={node.child} {model} {id} {paths} {problems} {actions} />
    {#if here.length > 0}<p class="text-sm text-red-800" role="status">{here.join(' ')}</p>{/if}
    <button type="button" class="justify-self-start" onclick={() => actions.unnegate(node.key)}>
      Quitar negación
    </button>
  </fieldset>
{/if}
{#if node.kind !== 'group' && node.kind !== 'not' && here.length > 0}
  <p class="text-sm text-red-800" role="status">{here.join(' ')}</p>
{/if}
