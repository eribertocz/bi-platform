<script lang="ts">
  import { tick } from 'svelte';
  import type { CatalogModel } from '#lib/api/semantic.ts';
  import { filterable, type Draft } from './condition';
  import type { SemanticFilter } from './filters';
  import {
    edit,
    fromFilter,
    keys,
    paths,
    toFilter,
    treeProblems,
    type TreeActions,
    type TreeChange,
    type TreeNode,
  } from './filterTree';
  import FilterNode from './FilterNode.svelte';

  let {
    model,
    initial,
    id,
    onchange,
  }: {
    model: CatalogModel;
    initial: SemanticFilter | null;
    /** Unique prefix of this editor's element ids. */
    id: string;
    onchange: (change: TreeChange) => void;
  } = $props();

  const key = keys();
  // svelte-ignore state_referenced_locally
  let root = $state<TreeNode | null>(initial ? fromFilter(initial, model, key) : null);
  let announcement = $state('');

  const nodePaths = $derived(paths(root));
  const problems = $derived(treeProblems(root, model));

  const fresh = (): TreeNode => ({
    key: key(),
    kind: 'condition',
    draft: { field: filterable(model)[0]?.name ?? '', operator: '=', values: [''] },
  });

  /** Applies one edit, reports it, says what changed and moves the focus. */
  async function apply(next: TreeNode | null, said: string, focus?: string) {
    root = next;
    const snapshot = $state.snapshot(root) as TreeNode | null;
    onchange({
      filter: snapshot ? toFilter(snapshot, model) : null,
      problems: treeProblems(snapshot, model),
    });
    announcement = said;
    await tick();
    document.getElementById(focus ?? `${id}-add`)?.focus();
  }

  function parentOf(node: TreeNode | null, child: number): TreeNode | null {
    if (!node) return null;
    if (node.kind === 'group') {
      if (node.children.some((c) => c.key === child)) return node;
      for (const c of node.children) {
        const found = parentOf(c, child);
        if (found) return found;
      }
    }
    if (node.kind === 'not') return node.child.key === child ? node : parentOf(node.child, child);
    return null;
  }
  /** Where the focus goes once `key` is gone: its group's «add», or the start. */
  function afterRemoving(key: number) {
    let parent = parentOf(root, key);
    while (parent && parent.kind === 'not') parent = parentOf(root, parent.key);
    return parent ? `${id}-n${parent.key}-add` : `${id}-add`;
  }

  const actions: TreeActions = {
    addCondition: (group) => {
      const node = fresh();
      void apply(
        edit(root, group, (g) =>
          g.kind === 'group' ? { ...g, children: [...g.children, node] } : g,
        ),
        'Condición añadida al grupo.',
        `${id}-n${node.key}-field`,
      );
    },
    addGroup: (group) => {
      const node = fresh();
      const inner: TreeNode = { key: key(), kind: 'group', op: 'and', children: [node] };
      void apply(
        edit(root, group, (g) =>
          g.kind === 'group' ? { ...g, children: [...g.children, inner] } : g,
        ),
        'Grupo Y añadido dentro del grupo.',
        `${id}-n${node.key}-field`,
      );
    },
    remove: (target) => {
      const focus = afterRemoving(target);
      void apply(
        edit(root, target, () => null),
        'Elemento quitado del filtro.',
        focus,
      );
    },
    negate: (target) =>
      void apply(
        edit(root, target, (n) => ({ key: key(), kind: 'not', child: n })),
        'Negación añadida.',
      ),
    unnegate: (target) =>
      void apply(
        edit(root, target, (n) => (n.kind === 'not' ? n.child : n)),
        'Negación quitada.',
      ),
    setOp: (group, op) =>
      void apply(
        edit(root, group, (g) => (g.kind === 'group' ? { ...g, op } : g)),
        op === 'and' ? 'El grupo ahora exige todas.' : 'El grupo ahora exige alguna.',
        `${id}-n${group}-op`,
      ),
    setDraft: (target, draft: Draft) => {
      root = edit(root, target, (n) => (n.kind === 'condition' ? { ...n, draft } : n));
      const snapshot = $state.snapshot(root) as TreeNode | null;
      onchange({
        filter: snapshot ? toFilter(snapshot, model) : null,
        problems: treeProblems(snapshot, model),
      });
    },
  };

  function start(kind: 'condition' | 'group') {
    const node = fresh();
    const next: TreeNode =
      kind === 'condition' ? node : { key: key(), kind: 'group', op: 'and', children: [node] };
    void apply(
      next,
      kind === 'condition' ? 'Condición añadida.' : 'Grupo Y añadido.',
      `${id}-n${node.key}-field`,
    );
  }

  /** Wraps the single condition at the root in a group, to add another. */
  function group() {
    if (!root) return;
    const inner = fresh();
    void apply(
      { key: key(), kind: 'group', op: 'and', children: [root, inner] },
      'Grupo Y creado con la condición anterior y una nueva.',
      `${id}-n${inner.key}-field`,
    );
  }
</script>

<div class="grid gap-3">
  {#if root}
    <FilterNode node={root} {model} {id} paths={nodePaths} {problems} {actions} />
    {#if root.kind !== 'group'}
      <button type="button" id="{id}-add" class="justify-self-start" onclick={group}>
        Añadir otra condición
      </button>
    {/if}
  {:else}
    <p>Sin filtros: se consultan todos los datos que puedes ver.</p>
    <div class="flex flex-wrap gap-2">
      <button type="button" id="{id}-add" onclick={() => start('condition')}
        >Añadir condición</button
      >
      <button type="button" onclick={() => start('group')}>Añadir grupo</button>
    </div>
  {/if}
  <!-- `status` is a polite live region; the page keeps `aria-live` for its own answer. -->
  <p class="sr-only" role="status">{announcement}</p>
</div>
