import { condition, draftOf, OPERATOR_LABELS, type Domain, type Draft } from './condition';
import {
  type FilterProblem,
  type SemanticFilter,
  type SemanticTarget,
  type SemanticValue,
} from './filters';

/**
 * A `where` or a `having` being edited, as a tree: conditions written by drafts (#148),
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

/** What the tree editor reports on every change: the filter it writes, or
 *  none, and why it cannot be sent, if so. Never results nor credentials. */
export type TreeChange = { filter: SemanticFilter | null; problems: FilterProblem[] };

/** Keys for new nodes, unique within one editor. */
export function keys(start = 0) {
  let next = start;
  return () => next++;
}

/** The filter the tree writes, exactly: order, grouping and every node. */
export function toFilter(node: TreeNode, domain: Domain): SemanticFilter {
  switch (node.kind) {
    case 'condition':
      return condition(node.draft, domain);
    case 'fixed':
      return node.filter;
    case 'group':
      return { op: node.op, filters: node.children.map((child) => toFilter(child, domain)) };
    case 'not':
      return { op: 'not', filter: toFilter(node.child, domain) };
  }
}

/** The tree of `filter`, which writes it back exactly (`toFilter`). */
export function fromFilter(filter: SemanticFilter, domain: Domain, key: () => number): TreeNode {
  switch (filter.op) {
    case 'and':
    case 'or':
      return {
        key: key(),
        kind: 'group',
        op: filter.op,
        children: filter.filters.map((child) => fromFilter(child, domain, key)),
      };
    case 'not':
      return { key: key(), kind: 'not', child: fromFilter(filter.filter, domain, key) };
    default: {
      // Editable only when its draft writes it back unchanged.
      const draft = draftOf(filter, domain);
      return draft && same(condition(draft, domain), filter)
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
export const treeProblems = (root: TreeNode | null, domain: Domain): FilterProblem[] =>
  root ? domain.problems(toFilter(root, domain)) : [];

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

/**
 * A filter as a sentence, to say what is applied: never evaluated. A target
 * is named as `domain` names it, else by what it carries.
 */
export function summarize(filter: SemanticFilter, domain?: Domain): string {
  const name = (target: SemanticTarget) =>
    domain?.name(target) ??
    (target.kind === 'dimension' ? target.name : target.selection.replace(/^m:/, ''));
  const value = (v: SemanticValue) =>
    v.type === 'null'
      ? 'NULL'
      : v.type === 'boolean'
        ? v.value
          ? 'verdadero'
          : 'falso'
        : v.type === 'text'
          ? `«${v.value}»`
          : v.value;
  switch (filter.op) {
    case 'and':
    case 'or':
      return `(${filter.filters.map((f) => summarize(f, domain)).join(filter.op === 'and' ? ' y ' : ' o ')})`;
    case 'not':
      return `no ${summarize(filter.filter, domain)}`;
    case 'is_null':
    case 'is_not_null':
      return `${name(filter.target)} ${OPERATOR_LABELS[filter.op]}`;
    case 'between':
      return `${name(filter.target)} está entre ${value(filter.low)} y ${value(filter.high)}`;
    case 'in':
      return `${name(filter.target)} es uno de ${filter.values.map(value).join(', ')}`;
    case 'compare':
      return `${name(filter.target)} ${OPERATOR_LABELS[filter.comparison]} ${value(filter.value)}`;
  }
}
