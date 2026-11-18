// What a reviewer reads: a package's files as a tree, what changed since the
// last approved version (files, manifest, lines), and each file's language.
import { structuredPatch } from "diff";
import type { PackageFile } from "./types.ts";

// ── The file tree ──────────────────────────────────────────────────────

export interface TreeNode {
  name: string;
  /** The file's path; folders have none. */
  path: string | null;
  children: TreeNode[];
  file?: PackageFile;
}

/** The package's files as folders and files, folders first, each by name. */
export function fileTree(files: readonly PackageFile[]): TreeNode[] {
  const root: TreeNode = { name: "", path: null, children: [] };
  for (const file of files) {
    let node = root;
    const parts = file.path.split("/");
    for (const [i, part] of parts.entries()) {
      const last = i === parts.length - 1;
      let next = node.children.find((c) => c.name === part && (c.path === null) === !last);
      if (!next) {
        next = last ? { name: part, path: file.path, children: [], file } : { name: part, path: null, children: [] };
        node.children.push(next);
      }
      node = next;
    }
  }
  const sort = (nodes: TreeNode[]) => {
    nodes.sort((a, b) => (a.path === null) !== (b.path === null) ? (a.path === null ? -1 : 1) : a.name.localeCompare(b.name, "en"));
    for (const n of nodes) sort(n.children);
  };
  sort(root.children);
  return root.children;
}

// ── What changed ───────────────────────────────────────────────────────

export type Change = "added" | "removed" | "modified";

export interface ChangedFile {
  path: string;
  change: Change;
  now?: PackageFile;
  before?: PackageFile;
}

/** Each file added, removed or changed since `base`, by path. */
export function changedFiles(files: readonly PackageFile[], base: readonly PackageFile[]): ChangedFile[] {
  const before = new Map(base.map((f) => [f.path, f]));
  const now = new Map(files.map((f) => [f.path, f]));
  const out: ChangedFile[] = [];
  for (const f of files) {
    const b = before.get(f.path);
    if (!b) out.push({ path: f.path, change: "added", now: f });
    else if (b.sha256 !== f.sha256) out.push({ path: f.path, change: "modified", now: f, before: b });
  }
  for (const b of base) if (!now.has(b.path)) out.push({ path: b.path, change: "removed", before: b });
  return out.sort((a, b) => a.path.localeCompare(b.path, "en"));
}

export interface ManifestChange {
  key: string;
  before: string | null;
  after: string | null;
  /** Something that widens what it can do: called out. */
  risky: boolean;
}

/** Manifest fields that decide what a plugin can reach; a change to one is called out. */
const REACH = new Set(["permissions", "commands", "network", "mcp", "service", "ui", "tools", "entry"]);
/** Written by signing, and different every time: not a change anyone made. */
const IGNORED = new Set(["signature", "version"]);

/** Each top-level manifest field that differs, as compact JSON either side. */
export function manifestChanges(now: Record<string, unknown>, before: Record<string, unknown>): ManifestChange[] {
  const keys = [...new Set([...Object.keys(before), ...Object.keys(now)])].filter((k) => !IGNORED.has(k)).sort();
  const out: ManifestChange[] = [];
  for (const key of keys) {
    const a = key in before ? stable(before[key]) : null;
    const b = key in now ? stable(now[key]) : null;
    if (a !== b) out.push({ key, before: a, after: b, risky: REACH.has(key) });
  }
  return out.sort((x, y) => Number(y.risky) - Number(x.risky));
}

/** JSON with its keys in order, so two equal values read the same. */
function stable(value: unknown): string {
  return JSON.stringify(value, (_, v) => (v && typeof v === "object" && !Array.isArray(v) ? Object.fromEntries(Object.entries(v).sort(([a], [b]) => a.localeCompare(b))) : v));
}

// ── Lines ──────────────────────────────────────────────────────────────

export interface DiffRow {
  kind: "hunk" | "context" | "added" | "removed";
  text: string;
  /** Its line number before and after (none for a hunk's head). */
  old?: number;
  new?: number;
}

/** A unified diff of two texts, as rows to draw, three lines of context. */
export function diffRows(before: string, after: string): DiffRow[] {
  const patch = structuredPatch("before", "after", before, after, "", "", { context: 3 });
  const rows: DiffRow[] = [];
  for (const hunk of patch.hunks) {
    rows.push({ kind: "hunk", text: `@@ -${hunk.oldStart},${hunk.oldLines} +${hunk.newStart},${hunk.newLines} @@` });
    let o = hunk.oldStart;
    let n = hunk.newStart;
    for (const line of hunk.lines) {
      const mark = line[0];
      const text = line.slice(1);
      if (mark === "\\") continue; // "\ No newline at end of file"
      if (mark === "+") rows.push({ kind: "added", text, new: n++ });
      else if (mark === "-") rows.push({ kind: "removed", text, old: o++ });
      else rows.push({ kind: "context", text, old: o++, new: n++ });
    }
  }
  return rows;
}

export const countRows = (rows: readonly DiffRow[]) => ({
  added: rows.filter((r) => r.kind === "added").length,
  removed: rows.filter((r) => r.kind === "removed").length,
});

// ── Languages ──────────────────────────────────────────────────────────

const LANGUAGES: Record<string, string> = {
  luau: "lua", lua: "lua", json: "json", md: "markdown", js: "javascript", mjs: "javascript", cjs: "javascript", ts: "typescript",
  svg: "xml", html: "xml", xml: "xml", css: "css", yml: "yaml", yaml: "yaml", toml: "ini", sh: "bash", py: "python", rs: "rust", go: "go", sql: "sql",
};

/** The highlighter's language for a file, by its extension, or null. */
export function languageOf(path: string): string | null {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return Object.hasOwn(LANGUAGES, ext) ? LANGUAGES[ext]! : null;
}

/** Pictures the viewer shows as pictures. */
export const isPicture = (path: string) => /\.(png|jpe?g|gif|webp|avif|svg)$/i.test(path);

/** The most of one file the viewer reads (bytes); past it, it says so. */
export const VIEW_LIMIT = 512 * 1024;

/** A file's text, or null when it isn't text or is too big to show. */
export async function readText(response: Response, limit = VIEW_LIMIT): Promise<{ text: string } | { binary: true } | { tooBig: true } | null> {
  if (!response.ok) return null;
  const length = Number(response.headers.get("content-length") ?? "0");
  if (length > limit) {
    await response.body?.cancel();
    return { tooBig: true };
  }
  const buffer = await response.arrayBuffer();
  if (buffer.byteLength > limit) return { tooBig: true };
  try {
    const text = new TextDecoder("utf-8", { fatal: true }).decode(buffer);
    // eslint-disable-next-line no-control-regex
    return /[\u0000-\u0008\u000e-\u001f]/.test(text) ? { binary: true } : { text };
  } catch {
    return { binary: true };
  }
}
