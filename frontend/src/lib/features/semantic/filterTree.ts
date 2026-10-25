import type { CatalogModel } from '#lib/api/semantic.ts';
import { condition, draftOf, type Draft } from './condition';
import { filterProblems, type FilterProblem, type SemanticFilter } from './filters';

/**
 * A `where` being edited, as a tree: conditions written by drafts (#148),
 * AND/OR groups and NOT. A condition no draft writes as it is stays a
 * `fixed` node, kept and shown as it is, never dropped nor rewritten (#264).
 * Keys identify nodes across edits; they are not part of the filter.
 */
export type TreeNode =
  | { key: number; kind: 'condition'; draft: Draft }
  | { key: number; kind: 'fixed'; filter: SemanticFilter }
  | { key: number; kind: 'group'; op: 'and' | 'or'; children: TreeNode[] }
  | { key: number; kind: 'not'; child: TreeNode };

/** What a node may ask of the tree that holds it; the tree applies it. */
export type TreeActions = {
  addCondition: (group: number) => void;
  addGroup: (group: number) => void;
  remove: (key: number) => void;
  negate: (key: number) => void;
  unnegate: (key: number) => void;
  setOp: (group: number, op: 'and' | 'or') => void;
  setDraft: (key: number, draft: Draft) => void;
};

/** What the tree editor reports on every change: the `where` it writes, or
 *  none, and why it cannot be sent, if so. Never results nor credentials. */
export type TreeChange = { filter: SemanticFilter | null; problems: FilterProblem[] };

/** Keys for new nodes, unique within one editor. */
export function keys(start = 0) {
  let next = start;
  return () => next++;
}

/** The filter the tree writes, exactly: order, grouping and every node. */
export function toFilter(node: TreeNode, model: CatalogModel): SemanticFilter {
  switch (node.kind) {
    case 'condition':
      return condition(node.draft, model);
    case 'fixed':
      return node.filter;
    case 'group':
      return { op: node.op, filters: node.children.map((child) => toFilter(child, model)) };
    case 'not':
      return { op: 'not', filter: toFilter(node.child, model) };
  }
}

/** The tree of `filter`, which writes it back exactly (`toFilter`). */
export function fromFilter(
  filter: SemanticFilter,
  model: CatalogModel,
  key: () => number,
): TreeNode {
  switch (filter.op) {
    case 'and':
    case 'or':
      return {
        key: key(),
        kind: 'group',
        op: filter.op,
        children: filter.filters.map((child) => fromFilter(child, model, key)),
      };
    case 'not':
      return { key: key(), kind: 'not', child: fromFilter(filter.filter, model, key) };
    default: {
      // Editable only when its draft writes it back unchanged.
      const draft = draftOf(filter);
      return draft && same(condition(draft, model), filter)
        ? { key: key(), kind: 'condition', draft }
        : { key: key(), kind: 'fixed', filter };
    }
  }
}

/** Structural equality of two JSON values, whatever their key order. */
function same(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const [ka, kb] = [Object.keys(a), Object.keys(b)];
  return (
    ka.length === kb.length &&
    ka.every((k) => same((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]))
  );
}

/** Each node's JSON pointer in the query, as the core reports problems. */
export function paths(root: TreeNode | null, at = '/filter'): Map<number, string> {
  const found = new Map<number, string>();
  const walk = (node: TreeNode, path: string) => {
    found.set(node.key, path);
    if (node.kind === 'group') node.children.forEach((c, i) => walk(c, `${path}/filters/${i}`));
    if (node.kind === 'not') walk(node.child, `${path}/filter`);
  };
  if (root) walk(root, at);
  return found;
}

/** Why the whole tree cannot be sent: one count for the whole, as the server's. */
export const treeProblems = (root: TreeNode | null, model: CatalogModel): FilterProblem[] =>
  root ? filterProblems({ where: toFilter(root, model), having: null }, model, []) : [];

/** `root` with the node `key` replaced by what `change` returns; `null` removes it. */
export function edit(
  root: TreeNode | null,
  key: number,
  change: (node: TreeNode) => TreeNode | null,
): TreeNode | null {
  if (!root) return null;
  if (root.key === key) return change(root);
  if (root.kind === 'group') {
    const children = root.children.flatMap((child) => {
      const next = edit(child, key, change);
      return next ? [next] : [];
    });
    return { ...root, children };
  }
  if (root.kind === 'not') {
    // Removing what a NOT negates removes the NOT: nothing is left to negate.
    const child = edit(root.child, key, change);
    return child ? { ...root, child } : null;
  }
  return root;
}
