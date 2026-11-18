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

/** Each tool's `input` by name, as canonical JSON (the last tool of a name,
 * as the engine's map keeps it); from a manifest, or its stored text. */
export const toolInputs = (manifest: JsonObject | string | null | undefined): Map<string, string> => {
  const out = new Map<string, string>();
  let raw: Json | undefined;
  try {
    raw = typeof manifest === "string" ? parseJsonText(manifest) : (manifest ?? undefined);
  } catch {
    return out;
  }
  const tools = isObject(raw) ? raw.get("tools") : undefined;
  for (const t of isArray(tools) ? tools : []) {
    const name = isObject(t) ? t.get("name") : undefined;
    if (isObject(t) && typeof name === "string") out.set(name, t.has("input") ? canonicalJson(t.get("input")!) : NO_INPUT);
  }
  return out;
};

export interface ChangedTool {
  readonly name: string;
  readonly description_before: string;
  readonly description_after: string;
  readonly input_changed: boolean;
}

/**
 * What changed in what it asks, between two versions' items. The tool
 * fields are the engine's (capability.rs `diff`, the app's `digest_diff`):
 * `tools_added` with descriptions, `tools_changed` when a tool's
 * description or input moved (inputs from `inputs`, each version's
 * `toolInputs`), `tools_removed` by name; each by name in UTF-8 order.
 */
export const consentDiff = (
  before: ConsentItem | null,
  after: ConsentItem,
  inputs?: { readonly before: ReadonlyMap<string, string>; readonly after: ReadonlyMap<string, string> }
) => {
  const added = (a: readonly string[], b: readonly string[]) => b.filter((x) => !a.includes(x));
  const was = before ?? { asks: [], commands: [], network_hosts: [], tools: [], ui: { change: [], replace: [] } };
  const byName = (tools: readonly ToolSpec[]) => new Map(tools.map((t) => [t.name, t.description]));
  const [oldTools, newTools] = [byName(was.tools), byName(after.tools)];
  const sorted = (names: Iterable<string>) => [...names].sort(compareUtf8);
  const tools_added: { name: string; description: string }[] = [];
  const tools_changed: ChangedTool[] = [];
  for (const name of sorted(newTools.keys())) {
    const description = newTools.get(name)!;
    if (!oldTools.has(name)) {
      tools_added.push({ name, description });
      continue;
    }
    const descriptionBefore = oldTools.get(name)!;
    const inputChanged = inputs ? (inputs.before.get(name) ?? NO_INPUT) !== (inputs.after.get(name) ?? NO_INPUT) : false;
    if (inputChanged || descriptionBefore !== description) {
      tools_changed.push({ name, description_before: descriptionBefore, description_after: description, input_changed: inputChanged });
    }
  }
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
    ui_replace_added: added(was.ui.replace, after.ui.replace)
  };
};

/** Asks a reviewer looks at twice: reach past the plugin's own window. */
export const RISKY_ASKS = ["native", "commands", "service", "ui.replace", "files", "github.write", "messages.send", "accounts.switch", "account.email", "network"];
