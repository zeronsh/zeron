import type { WorkspaceEntryKind, WorkspaceFileSearchMatch } from "@zeron/proto";

/**
 * The search-results tree — a port of `crates/ui/src/files/search.rs`'s
 * `SearchTreeModel` (the `rebuild`/`sort_search_paths`/`toggle` trio).
 *
 * Flat search matches are grouped into a synthetic ancestor tree keyed by
 * PATH COMPONENTS, not name prefixes: `THIRD_PARTY_NOTICES/…
 * /THIRD_PARTY_NOTICES.md` nests the match under a real directory row even
 * though its name equals the directory's (the desktop test
 * `search_tree_uses_path_components_instead_of_name_prefixes` guards
 * exactly this). Ancestors that are not themselves matches render as
 * directories; a component that IS a match takes the match's kind, name,
 * and best score.
 *
 * Ordering (`sortSearchPaths`): descendant-propagated `bestScore`
 * descending, then directories before files at equal score, then
 * case-insensitive name, then raw path as the final tiebreak. Toggling a
 * directory node collapses/expands only its own subtree.
 */

export interface SearchTreeNode {
  readonly path: string;
  name: string;
  kind: WorkspaceEntryKind;
  /** The node's own match score, or null for a pure ancestor. */
  score: number | null;
  /** The best score in this node's subtree (propagated by `updateBestScore`). */
  bestScore: number;
  children: string[];
}

export interface SearchTreeRow {
  readonly path: string;
  readonly name: string;
  readonly kind: WorkspaceEntryKind;
  readonly depth: number;
  readonly hasChildren: boolean;
  readonly score: number;
}

export interface SearchTree {
  readonly nodes: ReadonlyMap<string, SearchTreeNode>;
  readonly roots: readonly string[];
  readonly collapsed: ReadonlySet<string>;
  readonly rows: readonly SearchTreeRow[];
}

const NO_SCORE = Number.MIN_SAFE_INTEGER;

/** `SearchTreeModel::rebuild` — build the tree from a flat match list. */
export function buildSearchTree(results: readonly WorkspaceFileSearchMatch[]): SearchTree {
  const nodes = new Map<string, SearchTreeNode>();
  const roots: string[] = [];

  for (const result of results) {
    let parent: string | null = null;
    let path = "";
    for (const component of result.path.split("/").filter((part) => part.length > 0)) {
      path = path.length === 0 ? component : `${path}/${component}`;
      const isMatch = path === result.path;
      const existing = nodes.get(path);
      if (existing !== undefined) {
        if (isMatch) {
          existing.name = result.name;
          existing.kind = result.kind;
          // The max of concurrent matches on one path (the desktop's
          // `score.unwrap_or(i64::MIN).max(result.score)`).
          existing.score = Math.max(existing.score ?? NO_SCORE, result.score);
        }
      } else {
        nodes.set(path, {
          path,
          name: isMatch ? result.name : component,
          kind: isMatch ? result.kind : "directory",
          score: isMatch ? result.score : null,
          bestScore: result.score,
          children: [],
        });
      }
      const parentNode: SearchTreeNode | undefined = parent !== null ? nodes.get(parent) : undefined;
      if (parentNode !== undefined && !parentNode.children.includes(path)) {
        parentNode.children.push(path);
      } else if (parent === null && !roots.includes(path)) {
        roots.push(path);
      }
      parent = path;
    }
  }

  for (const root of [...roots]) {
    updateBestScore(root, nodes);
  }
  sortSearchPaths(roots, nodes);
  for (const root of [...roots]) {
    sortSearchBranch(root, nodes);
  }

  return { nodes, roots, collapsed: new Set<string>(), rows: buildRows(roots, nodes, new Set()) };
}

/** `SearchTreeModel::toggle` — collapse/expand one node's own subtree. */
export function toggleSearchNode(tree: SearchTree, path: string): SearchTree | null {
  const node = tree.nodes.get(path);
  if (node === undefined || node.children.length === 0) {
    return null;
  }
  const collapsed = new Set(tree.collapsed);
  if (!collapsed.delete(path)) {
    collapsed.add(path);
  }
  return { ...tree, collapsed, rows: buildRows(tree.roots, tree.nodes, collapsed) };
}

export function isSearchNodeExpanded(tree: SearchTree, path: string): boolean {
  return !tree.collapsed.has(path);
}

/** `update_best_score` — propagate the subtree maximum up to the root. */
function updateBestScore(path: string, nodes: Map<string, SearchTreeNode>): number {
  const node = nodes.get(path);
  if (node === undefined) {
    return NO_SCORE;
  }
  let bestScore = node.score ?? NO_SCORE;
  for (const child of [...node.children]) {
    bestScore = Math.max(bestScore, updateBestScore(child, nodes));
  }
  node.bestScore = bestScore;
  return bestScore;
}

/** `sort_search_branch` — sort children depth-first, then the node itself. */
function sortSearchBranch(path: string, nodes: Map<string, SearchTreeNode>): void {
  const children = [...(nodes.get(path)?.children ?? [])];
  for (const child of children) {
    sortSearchBranch(child, nodes);
  }
  sortSearchPaths(children, nodes);
  const node = nodes.get(path);
  if (node !== undefined) {
    node.children = children;
  }
}

/**
 * `sort_search_paths`: bestScore descending, directories before files at
 * equal score, case-insensitive name, raw path as the final tiebreak.
 */
export function sortSearchPaths(paths: string[], nodes: ReadonlyMap<string, SearchTreeNode>): void {
  paths.sort((left, right) => {
    const leftNode = nodes.get(left);
    const rightNode = nodes.get(right);
    if (leftNode === undefined || rightNode === undefined) {
      return left < right ? -1 : left > right ? 1 : 0;
    }
    const byScore = rightNode.bestScore - leftNode.bestScore;
    if (byScore !== 0) {
      return byScore;
    }
    const leftDirectory = leftNode.kind === "directory";
    const rightDirectory = rightNode.kind === "directory";
    if (leftDirectory !== rightDirectory) {
      return leftDirectory ? -1 : 1;
    }
    const byName = leftNode.name.toLowerCase().localeCompare(rightNode.name.toLowerCase());
    if (byName !== 0) {
      return byName;
    }
    return left < right ? -1 : left > right ? 1 : 0;
  });
}

/** `append_search_rows` — the depth-first visible-row walk. */
function buildRows(
  roots: readonly string[],
  nodes: ReadonlyMap<string, SearchTreeNode>,
  collapsed: ReadonlySet<string>,
): SearchTreeRow[] {
  const rows: SearchTreeRow[] = [];
  const append = (path: string, depth: number): void => {
    const node = nodes.get(path);
    if (node === undefined) {
      return;
    }
    const hasChildren = node.children.length > 0;
    rows.push({
      path: node.path,
      name: node.name,
      kind: node.kind,
      depth,
      hasChildren,
      score: node.score ?? node.bestScore,
    });
    if (hasChildren && !collapsed.has(path)) {
      for (const child of node.children) {
        append(child, depth + 1);
      }
    }
  };
  for (const root of roots) {
    append(root, 0);
  }
  return rows;
}
