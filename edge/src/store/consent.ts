/**
 * What a plugin asks for, as the consent sheet shows it (contract amendment
 * 9; apps/zgx/src/plugins/consent.rs `bounded`): one item the web and the
 * app both word, computed here from the signed manifest alone.
 *
 * The ratings come from crates/plugin-guard: `commands::risk`,
 * `full_control`, `held_to_its_own_rules` and `net::risky`/`names_exactly`.
 * Their tables are the engine's own, from docs/plugin-store-data.json
 * (data.ts); the `consent_items` vectors hold both sides to them.
 */
import { STORE_DATA } from "./data";
import { type Json, type JsonObject, canonicalJson, compareUtf8, isArray, isObject, parseJsonText } from "./json";
import type { Manifest, ToolSpec } from "./manifest";
import { namesExactly, riskyHost } from "./manifest";

/** The ratings' tables (plugin-guard's), as the engine exports them. */
export const contained: readonly string[] = STORE_DATA.consent.contained;
export const fullControlNames: readonly string[] = STORE_DATA.consent.full_control;
export const heldNames: readonly string[] = STORE_DATA.consent.held_to_its_own_rules;
export const windowsExtensions: readonly string[] = STORE_DATA.consent.windows_extensions;

/** plugin-guard `normalized`: lowercase ASCII, no Windows extension. */
export const normalizedProgram = (program: string): string => {
  const lower = program.replace(/[A-Z]/g, (c) => c.toLowerCase());
  for (const ext of windowsExtensions) if (lower.endsWith(ext)) return lower.slice(0, -ext.length);
  return lower;
};

export const canDoAnything = (program: string): boolean => !contained.includes(normalizedProgram(program));

export const fullControl = (program: string): boolean => {
  const name = normalizedProgram(program);
  return fullControlNames.includes(name) || fullControlNames.includes(name.replace(/[0-9.]+$/, ""));
};

export const heldToItsOwnRules = (program: string): boolean => heldNames.includes(normalizedProgram(program));

export interface ConsentItem {
  readonly asks: readonly string[];
  readonly ui: { readonly change: readonly string[]; readonly replace: readonly string[] };
  readonly commands: readonly string[];
  readonly commands_anything: readonly string[];
  readonly commands_full_control: readonly string[];
  readonly network_hosts: readonly string[];
  readonly network_local: readonly string[];
  readonly network_named: readonly string[];
  readonly tools: readonly ToolSpec[];
}

export const consentItem = (m: Manifest): ConsentItem => ({
  asks: m.permissions,
  ui: { change: m.ui.change, replace: m.ui.replace },
  commands: m.commands,
  commands_anything: m.commands.filter((c) => canDoAnything(c) && !fullControl(c) && !heldToItsOwnRules(c)),
  commands_full_control: m.commands.filter(fullControl),
  network_hosts: m.networkHosts,
  network_local: m.networkHosts.filter((h) => h !== "*" && riskyHost(h)),
  network_named: m.networkHosts.filter((h) => namesExactly(h) && !riskyHost(h)),
  tools: m.tools.map((t) => ({ name: t.name, description: t.description }))
});

/** A tool's input when it names none (manifest.rs). */
const NO_INPUT = '{"type":"object"}';

/** The paths `askParts` reads from a stored manifest, for SQLite's
 * `json_extract(manifest, …ASK_PATHS)` (one small array, so a list never
 * reads whole manifests). */
export const ASK_PATHS = ["$.tools", "$.mcp", "$.service", "$.settings"] as const;

/** Setting kinds that grant something (capability.rs `GRANTING_SETTINGS`). */
const GRANTING_SETTINGS = ["permission", "folder"];

/** The MCP server as the engine's capability record holds it
 * (capability.rs `record`: `{ command, args, env }`, from manifest.rs
 * `McpSpec`; env's non-text values dropped, its names sorted). */
export interface McpRecord {
  readonly command: string;
  readonly args: readonly string[];
  readonly env: Readonly<Record<string, string>>;
}

/** What the consent item doesn't show of a manifest but its consent binds
 * (capability.rs `record`): each tool's input (canonical JSON, by name),
 * the MCP server, the service, and the settings that grant (each as
 * canonical JSON). */
export interface AskParts {
  readonly tools: ReadonlyMap<string, string>;
  readonly mcp: McpRecord | null;
  readonly service: string | null;
  readonly settings: readonly string[];
}

const partsFrom = (tools: Json | undefined, mcp: Json | undefined, service: Json | undefined, settings: Json | undefined): AskParts => {
  const inputs = new Map<string, string>();
  for (const t of isArray(tools) ? tools : []) {
    const name = isObject(t) ? t.get("name") : undefined;
    if (isObject(t) && typeof name === "string") inputs.set(name, t.has("input") ? canonicalJson(t.get("input")!) : NO_INPUT);
  }
  let record: McpRecord | null = null;
  const command = isObject(mcp) ? mcp.get("command") : undefined;
  if (isObject(mcp) && typeof command === "string") {
    const args = mcp.get("args");
    const env = mcp.get("env");
    record = {
      command,
      args: isArray(args) ? args.filter((a): a is string => typeof a === "string") : [],
      env: Object.fromEntries(
        (isObject(env) ? [...env.entries()] : []).filter((e): e is [string, string] => typeof e[1] === "string").sort((a, b) => compareUtf8(a[0], b[0]))
      )
    };
  }
  return {
    tools: inputs,
    mcp: record,
    service: typeof service === "string" ? service : null,
    settings: (isArray(settings) ? settings : []).filter((x) => isObject(x) && GRANTING_SETTINGS.includes(x.get("kind") as string)).map((x) => canonicalJson(x))
  };
};

/** `AskParts` from D1's `json_extract(manifest, …ASK_PATHS)`, as text. */
export const askParts = (extracted: string | null): AskParts => {
  let parts: Json | undefined;
  try {
    parts = extracted === null ? undefined : parseJsonText(extracted);
  } catch {
    parts = undefined;
  }
  const at = (i: number): Json | undefined => (isArray(parts) ? parts[i] : undefined);
  return partsFrom(at(0), at(1), at(2), at(3));
};

/** `AskParts` of a whole manifest, or its stored text. */
export const askPartsOf = (manifest: JsonObject | string | null | undefined): AskParts => {
  let raw: Json | undefined;
  try {
    raw = typeof manifest === "string" ? parseJsonText(manifest) : (manifest ?? undefined);
  } catch {
    raw = undefined;
  }
  const get = (k: string) => (isObject(raw) ? raw.get(k) : undefined);
  return partsFrom(get("tools"), get("mcp"), get("service"), get("settings"));
};

export interface ChangedTool {
  readonly name: string;
  readonly description_before: string;
  readonly description_after: string;
  readonly input_changed: boolean;
}

/** capability.rs `Before`. */
export interface Before<T> {
  readonly before: T;
  readonly after: T;
}

/**
 * What changed in what it asks, between two versions' items. The fields
 * past the asks are the engine's own (capability.rs `Diff`, a device's
 * `digest_diff`), in its names and shapes:
 * - `tools_added` with descriptions, `tools_changed` when a tool's
 *   description or input moved, `tools_removed` by name; each by name in
 *   UTF-8 order;
 * - `mcp_changed`: the MCP server's `{ before, after }` (each `{ command,
 *   args, env }` or null; env with its values, as the engine shows them),
 *   null when it didn't change;
 * - `service_changed`: the service's `{ before, after }` (a path or null),
 *   null when it didn't change;
 * - `settings_changed`: whether a setting that grants (a permission or a
 *   folder) is new or different; one only dropped isn't (as the engine).
 * Inputs and those three come from `parts`, each version's `AskParts`;
 * without them they read as unchanged.
 */
export const consentDiff = (before: ConsentItem | null, after: ConsentItem, parts?: { readonly before: AskParts | null; readonly after: AskParts }) => {
  const added = (a: readonly string[], b: readonly string[]) => b.filter((x) => !a.includes(x));
  const was = before ?? { asks: [], commands: [], network_hosts: [], tools: [], ui: { change: [], replace: [] } };
  const byName = (tools: readonly ToolSpec[]) => new Map(tools.map((t) => [t.name, t.description]));
  const [oldTools, newTools] = [byName(was.tools), byName(after.tools)];
  const sorted = (names: Iterable<string>) => [...names].sort(compareUtf8);
  const tools_added: { name: string; description: string }[] = [];
  const tools_changed: ChangedTool[] = [];
  const oldParts = parts?.before ?? NO_PARTS;
  for (const name of sorted(newTools.keys())) {
    const description = newTools.get(name)!;
    if (!oldTools.has(name)) {
      tools_added.push({ name, description });
      continue;
    }
    const descriptionBefore = oldTools.get(name)!;
    const inputChanged = parts ? (oldParts.tools.get(name) ?? NO_INPUT) !== (parts.after.tools.get(name) ?? NO_INPUT) : false;
    if (inputChanged || descriptionBefore !== description) {
      tools_changed.push({ name, description_before: descriptionBefore, description_after: description, input_changed: inputChanged });
    }
  }
  const changed = <T>(a: T, b: T): Before<T> | null => (JSON.stringify(a) === JSON.stringify(b) ? null : { before: a, after: b });
  return {
    added: added(was.asks, after.asks),
    removed: added(after.asks, was.asks),
    added_commands: added(was.commands, after.commands),
    removed_commands: added(after.commands, was.commands),
    network_added: added(was.network_hosts, after.network_hosts),
    network_removed: added(after.network_hosts, was.network_hosts),
    tools_added,
    tools_changed,
    tools_removed: sorted([...oldTools.keys()].filter((n) => !newTools.has(n))),
    ui_change_added: added(was.ui.change, after.ui.change),
    ui_replace_added: added(was.ui.replace, after.ui.replace),
    mcp_changed: parts ? changed(oldParts.mcp, parts.after.mcp) : null,
    service_changed: parts ? changed(oldParts.service, parts.after.service) : null,
    settings_changed: parts ? parts.after.settings.some((x) => !oldParts.settings.includes(x)) : false
  };
};
export type ConsentDiff = ReturnType<typeof consentDiff>;

const NO_PARTS: AskParts = { tools: new Map(), mcp: null, service: null, settings: [] };

/**
 * A queue item's `asks_changed` (amendments 40 and 41): whether `diff`
 * names anything but asks that are only new (`added`, shown as
 * `new_asks`). Read off the diff itself, so the submission page always
 * names what set the flag.
 */
export const asksChanged = (diff: ConsentDiff): boolean =>
  diff.removed.length > 0 ||
  diff.added_commands.length > 0 ||
  diff.removed_commands.length > 0 ||
  diff.network_added.length > 0 ||
  diff.network_removed.length > 0 ||
  diff.tools_added.length > 0 ||
  diff.tools_changed.length > 0 ||
  diff.tools_removed.length > 0 ||
  diff.ui_change_added.length > 0 ||
  diff.ui_replace_added.length > 0 ||
  diff.mcp_changed !== null ||
  diff.service_changed !== null ||
  diff.settings_changed;

/** Asks a reviewer looks at twice: reach past the plugin's own window. */
export const RISKY_ASKS = ["native", "commands", "service", "ui.replace", "files", "github.write", "messages.send", "accounts.switch", "account.email", "network"];
