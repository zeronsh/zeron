/**
 * What a plugin asks for, as the consent sheet shows it (contract amendment
 * 9; apps/zgx/src/plugins/consent.rs `bounded`): one item the web and the
 * app both word, computed here from the signed manifest alone.
 *
 * The ratings come from crates/plugin-guard: `commands::risk`,
 * `full_control`, `held_to_its_own_rules` and `net::risky`/`names_exactly`.
 * Their tables are copied below until the engine lane's generated
 * docs/plugin-package-vectors.json carries them as data; then this module
 * reads them from there (and its `consent_items` vectors hold both sides).
 */
import type { Manifest, ToolSpec } from "./manifest";
import { namesExactly, riskyHost } from "./manifest";

/** Programs known to neither run others nor write files (plugin-guard
 * `CONTAINED`). */
const CONTAINED = [
  "basename", "cat", "cksum", "cut", "date", "df", "dirname", "du", "echo", "false", "file", "grep", "head",
  "hostname", "id", "jq", "ls", "md5", "md5sum", "nproc", "printenv", "printf", "ps", "pwd", "readlink",
  "realpath", "sha1sum", "sha256sum", "shasum", "sleep", "stat", "sw_vers", "tail", "tr", "true", "uname",
  "uptime", "wc", "which", "whoami"
];

/** Shells and interpreters (plugin-guard `FULL_CONTROL`). */
const FULL_CONTROL = [
  "ash", "bash", "bun", "cmd", "csh", "dash", "deno", "elixir", "erl", "fish", "ksh", "lua", "luajit", "node", "nodejs",
  "osascript", "perl", "php", "powershell", "pwsh", "python", "pypy", "ruby", "sh", "swift", "tclsh",
  "tcsh", "wscript", "cscript", "zsh"
];

/** plugin-guard `normalized`: lowercase ASCII, no Windows extension. */
export const normalizedProgram = (program: string): string => {
  const lower = program.replace(/[A-Z]/g, (c) => c.toLowerCase());
  for (const ext of [".exe", ".cmd", ".bat", ".com"]) if (lower.endsWith(ext)) return lower.slice(0, -ext.length);
  return lower;
};

export const canDoAnything = (program: string): boolean => !CONTAINED.includes(normalizedProgram(program));

export const fullControl = (program: string): boolean => {
  const name = normalizedProgram(program);
  return FULL_CONTROL.includes(name) || FULL_CONTROL.includes(name.replace(/[0-9.]+$/, ""));
};

export const heldToItsOwnRules = (program: string): boolean => {
  const name = normalizedProgram(program);
  return name === "git" || name === "gh";
};

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

/** What changed in what it asks, between two versions' items. */
export const consentDiff = (before: ConsentItem | null, after: ConsentItem) => {
  const added = (a: readonly string[], b: readonly string[]) => b.filter((x) => !a.includes(x));
  const was = before ?? { asks: [], commands: [], network_hosts: [], tools: [], ui: { change: [], replace: [] } };
  return {
    added: added(was.asks, after.asks),
    removed: added(after.asks, was.asks),
    added_commands: added(was.commands, after.commands),
    removed_commands: added(after.commands, was.commands),
    network_added: added(was.network_hosts, after.network_hosts),
    network_removed: added(after.network_hosts, was.network_hosts),
    tools_added: added(was.tools.map((t) => t.name), after.tools.map((t) => t.name)),
    tools_removed: added(after.tools.map((t) => t.name), was.tools.map((t) => t.name)),
    ui_change_added: added(was.ui.change, after.ui.change),
    ui_replace_added: added(was.ui.replace, after.ui.replace)
  };
};

/** Asks a reviewer looks at twice: reach past the plugin's own window. */
export const RISKY_ASKS = ["native", "commands", "service", "ui.replace", "files", "github.write", "messages.send", "accounts.switch", "account.email", "network"];
