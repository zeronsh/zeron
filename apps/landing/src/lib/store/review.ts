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

/**
 * The manifest, as a reviewer reads it among the files. A package carries
 * it beside its files, not as one of them (no file may have its name), so
 * the edge sends it apart from the file list: here it is put back in that
 * list as the text a reviewer reads, with its own hash so two versions with
 * the same manifest show no change.
 */
export const MANIFEST_FILE = "zeron-plugin.json";

export const manifestText = (manifest: Record<string, unknown>): string => `${JSON.stringify(manifest, null, 2)}\n`;

export async function manifestFile(text: string): Promise<PackageFile> {
  const bytes = new TextEncoder().encode(text);
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return { path: MANIFEST_FILE, bytes: bytes.length, sha256: [...digest].map((b) => b.toString(16).padStart(2, "0")).join(""), binary: false };
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

export interface ManifestItem {
  item: string;
  /** It widens what the plugin can do. */
  risky: boolean;
}

export interface ManifestChange {
  key: string;
  /** Compact JSON either side (null: not there), for the record. */
  before: string | null;
  after: string | null;
  /** For a list (or an object of lists): what came and went, item by item. */
  added: ManifestItem[] | null;
  removed: ManifestItem[] | null;
  /** The field decides what it can reach. */
  reach: boolean;
}

/** Manifest fields that decide what a plugin can reach. */
const REACH = new Set(["permissions", "commands", "network", "mcp", "service", "ui", "tools", "entry"]);
/** Written by signing, and different every time: not a change anyone made. */
const IGNORED = new Set(["signature", "version"]);
/** Permissions that let it act beyond its own space (the consent sheet's risky asks). */
const RISKY_ASKS = new Set(["ui.replace", "commands", "native", "service", "accounts.switch", "accounts.pin", "models.set", "messages.send", "devices"]);

/** A field as items, when it is a list (or an object of lists, or a list of named things); else null. */
function items(key: string, value: unknown): string[] | null {
  if (value === undefined) return [];
  if (Array.isArray(value)) {
    if (value.every((v) => typeof v === "string" || typeof v === "number")) return value.map(String);
    if (value.every((v) => v && typeof v === "object" && typeof (v as { name?: unknown }).name === "string")) return value.map((v) => String((v as { name: string }).name));
    return null;
  }
  if (value && typeof value === "object" && key !== "settings") {
    const out: string[] = [];
    for (const [k, v] of Object.entries(value)) {
      if (!Array.isArray(v) || !v.every((x) => typeof x === "string")) return null;
      for (const x of v) out.push(`${k}: ${x}`);
    }
    return out;
  }
  return null;
}

/** Whether adding `item` to `key` lets it do more than it could. */
const riskyItem = (key: string, item: string) => (key === "permissions" ? RISKY_ASKS.has(item) : key === "commands" || key === "network" || key === "tools" || (key === "ui" && item.startsWith("replace:")));

/** Each top-level manifest field that differs; the ones that decide its reach first. */
export function manifestChanges(now: Record<string, unknown>, before: Record<string, unknown>): ManifestChange[] {
  const keys = [...new Set([...Object.keys(before), ...Object.keys(now)])].filter((k) => !IGNORED.has(k)).sort();
  const out: ManifestChange[] = [];
  for (const key of keys) {
    const a = key in before ? stable(before[key]) : null;
    const b = key in now ? stable(now[key]) : null;
    if (a === b) continue;
    const was = items(key, before[key]);
    const is = items(key, now[key]);
    const listed = was !== null && is !== null;
    const wasSet = new Set(was ?? []);
    const isSet = new Set(is ?? []);
    out.push({
      key,
      before: a,
      after: b,
      added: listed ? is.filter((x) => !wasSet.has(x)).map((item) => ({ item, risky: riskyItem(key, item) })) : null,
      removed: listed ? was.filter((x) => !isSet.has(x)).map((item) => ({ item, risky: false })) : null,
      reach: REACH.has(key),
    });
  }
  const weight = (c: ManifestChange) => (c.added?.some((i) => i.risky) ? 2 : c.reach ? 1 : 0);
  return out.sort((x, y) => weight(y) - weight(x));
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

/**
 * The most a line diff may cost. Its work grows with the lines times the
 * edits between them, so two texts longer than MAX_LINES, or differing by
 * more than MAX_EDITS, are "too different to show line by line": a
 * submitter could otherwise make the page hang. (Edits, not time, are the
 * bound: in a Worker the clock doesn't move during work, so a timeout
 * can't stop it.)
 */
export const DIFF_BUDGET = { MAX_LINES: 6000, MAX_EDITS: 1500 } as const;

const lineCount = (text: string) => {
  let n = 1;
  for (let i = text.indexOf("\n"); i !== -1; i = text.indexOf("\n", i + 1)) n++;
  return n;
};

/** A unified diff of two texts, as rows to draw, three lines of context; null when it costs too much. */
export function diffRows(before: string, after: string): DiffRow[] | null {
  if (lineCount(before) > DIFF_BUDGET.MAX_LINES || lineCount(after) > DIFF_BUDGET.MAX_LINES) return null;
  const patch = structuredPatch("before", "after", before, after, "", "", { context: 3, maxEditLength: DIFF_BUDGET.MAX_EDITS });
  if (!patch) return null;
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
/**
 * What the site shows as a picture (amendments 14, 22): PNG and JPEG
 * pictures and the SVG icon, by extension, never by what a file claims.
 * The files proxy serves these and nothing else; the review shows anything
 * else as text or as "not text".
 */
export const PICTURE_TYPES: ReadonlyMap<string, string> = new Map([
  ["png", "image/png"],
  ["jpg", "image/jpeg"],
  ["jpeg", "image/jpeg"],
  ["svg", "image/svg+xml"],
]);

/** A path's picture type, if the site shows it as a picture. */
export const pictureType = (path: string): string | undefined => {
  const dot = path.lastIndexOf(".");
  return dot === -1 || path.lastIndexOf("/") > dot ? undefined : PICTURE_TYPES.get(path.slice(dot + 1).toLowerCase());
};

export const isPicture = (path: string) => pictureType(path) !== undefined;

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
