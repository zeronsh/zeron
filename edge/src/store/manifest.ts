/**
 * `zeron-plugin.json`, held to the engine's rules
 * (crates/engine/src/plugins/manifest.rs, with crates/plugin-guard's
 * `commands::valid_name` and `net::HostRule`): a manifest the engine would
 * call broken, the store refuses, with the engine's words.
 *
 * Every Rust string operation here is ported for what it does on Unicode:
 * `len()` counts UTF-8 bytes, `trim()` and `is_whitespace()` are Unicode
 * White_Space (which, unlike JavaScript's, has U+0085 and not U+FEFF),
 * `is_control()` is category Cc.
 */

import { LIMITS, STORE_DATA } from "./data";
import { type Json, type JsonObject, isArray, isObject } from "./json";

export const MANIFEST_FILE = "zeron-plugin.json";

/** Every permission a manifest may ask for (manifest.rs `KNOWN_PERMISSIONS`,
 * the data file's `known_permissions`): any other is the manifest's problem. */
export const KNOWN_PERMISSIONS: readonly string[] = STORE_DATA.known_permissions;

/** Ids no plugin may have at all (manifest.rs `RESERVED_IDS`): a device
 * refuses them too. The store's own further words (built-in plugins' ids,
 * the site's pages) are listing rules, in listing.ts. */
export const RESERVED_IDS: readonly string[] = STORE_DATA.reserved_ids;
export const RESERVED_ID_ERROR = "Zeron already uses that name";

const encoder = new TextEncoder();
const byteLength = (s: string): number => encoder.encode(s).length;

/** Unicode White_Space, as Rust's `char::is_whitespace`. */
const WHITE_SPACE = new Set([
  0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x20, 0x85, 0xa0, 0x1680, 0x2000, 0x2001, 0x2002, 0x2003, 0x2004, 0x2005, 0x2006, 0x2007,
  0x2008, 0x2009, 0x200a, 0x2028, 0x2029, 0x202f, 0x205f, 0x3000
]);
export const isRustWhitespace = (ch: string): boolean => WHITE_SPACE.has(ch.codePointAt(0)!);
const isControl = (ch: string): boolean => {
  const c = ch.codePointAt(0)!;
  return c <= 0x1f || (c >= 0x7f && c <= 0x9f);
};
export const rustTrimStart = (s: string): string => {
  const chars = [...s];
  let i = 0;
  while (i < chars.length && isRustWhitespace(chars[i]!)) i++;
  return chars.slice(i).join("");
};
export const rustTrimEnd = (s: string): string => {
  const chars = [...s];
  let j = chars.length;
  while (j > 0 && isRustWhitespace(chars[j - 1]!)) j--;
  return chars.slice(0, j).join("");
};
export const rustTrim = (s: string): string => rustTrimEnd(rustTrimStart(s));

/** manifest.rs `check_id`: why `id` can't be a plugin's id, if it can't. */
export const idProblem = (id: string): string | undefined => {
  if (!/^[a-z0-9][a-z0-9-]{1,39}$/.test(id)) {
    return `"${id}" can't be a plugin's id: use 2 to 40 lowercase letters, digits and dashes, starting with a letter or digit`;
  }
  return RESERVED_IDS.includes(id) ? RESERVED_ID_ERROR : undefined;
};

/** plugin-guard `commands::valid_name`. */
export const validCommand = (name: string): boolean => /^[A-Za-z0-9][A-Za-z0-9._+-]{0,63}$/.test(name);

export const validServicePath = (path: string): boolean =>
  path.endsWith(".luau") && !path.startsWith("/") && !path.includes("\\") && path.split("/").every((p) => p !== "" && !p.startsWith("."));

/** manifest.rs `valid_tool_name`. */
export const validToolName = (name: string): boolean => /^[a-z][a-z0-9_]{0,40}$/.test(name) && !name.includes("__");

/** manifest.rs `change_target_problem`. */
export const changeTargetProblem = (target: string): string | undefined => {
  const refuse = (why: string) => `It can't change "${target}": ${why}`;
  const notSelector = "it isn't a selector (#id, .class, hook:name, role:name or \"name\")";
  if (byteLength(target) > 200 || [...target].some(isControl)) return refuse("a selector is one short line");
  if (rustTrimStart(target).startsWith("@")) {
    return refuse("\"ui.change\" names parts of the screen, not modules (that's \"ui.replace\")");
  }
  for (const piece of target.split(">>")) {
    let rest = rustTrim(piece);
    if (rest === "") return refuse(notSelector);
    while (rest !== "") {
      if (rest.startsWith('"')) {
        const after = rest.slice(1);
        const end = after.indexOf('"');
        if (end < 0) return refuse("an unclosed quote");
        rest = rustTrimStart(after.slice(end + 1));
        continue;
      }
      const isClass = rest.startsWith(".");
      const prefix = isClass || rest.startsWith("#") ? 1 : rest.startsWith("hook:") || rest.startsWith("role:") ? 5 : 0;
      const tail = rest.slice(prefix);
      let nameEnd = rest.length;
      let offset = 0;
      for (const ch of tail) {
        if (isRustWhitespace(ch) || (isClass && ch === ".")) {
          nameEnd = prefix + offset;
          break;
        }
        offset += ch.length;
      }
      const name = rest.slice(prefix, nameEnd);
      rest = rustTrimStart(rest.slice(nameEnd));
      if (name === "" || /[@*[\]~"#]/.test(name) || name === "row") return refuse(notSelector);
      if (isClass && name.startsWith("plugin-")) return refuse("plugins' tabs, pages and badges are their own and Zeron's");
    }
  }
  return undefined;
};

// ── network hosts (plugin-guard net.rs) ─────────────────────────────────────

export const MAX_HOSTS = LIMITS.max_hosts;

/** Rust's `Ipv4Addr::from_str`: four decimal octets, no leading zeros. */
export const parseIpv4 = (s: string): number[] | undefined => {
  const parts = s.split(".");
  if (parts.length !== 4) return undefined;
  const octets: number[] = [];
  for (const part of parts) {
    if (!/^[0-9]{1,3}$/.test(part) || (part.length > 1 && part[0] === "0")) return undefined;
    const value = Number(part);
    if (value > 255) return undefined;
    octets.push(value);
  }
  return octets;
};

/** Rust's `Ipv6Addr::from_str` (std's parser: groups of 1–4 hex digits, one
 * `::`, a trailing embedded IPv4 address, no zone), as 8 groups. */
export const parseIpv6 = (s: string): number[] | undefined => {
  let at = 0;
  const hexGroup = (): number | undefined => {
    const m = /^[0-9A-Fa-f]{1,4}/.exec(s.slice(at));
    if (!m) return undefined;
    // std reads digits up to 4 and fails on a fifth.
    if (/^[0-9A-Fa-f]/.test(s.slice(at + m[0].length))) return undefined;
    at += m[0].length;
    return parseInt(m[0], 16);
  };
  /** std's `read_ipv4_addr`: four octets, each read greedily (more than
   * three digits, a leading zero or past 255 fails it). */
  const ipv4 = (): number[] | undefined => {
    const start = at;
    const octets: number[] = [];
    for (let i = 0; i < 4; i++) {
      if (i > 0) {
        if (s[at] !== ".") break;
        at++;
      }
      const m = /^[0-9]+/.exec(s.slice(at));
      if (!m || m[0].length > 3 || (m[0].length > 1 && m[0][0] === "0") || Number(m[0]) > 255) break;
      at += m[0].length;
      octets.push(Number(m[0]));
    }
    if (octets.length === 4) return octets;
    at = start;
    return undefined;
  };
  /** std's `read_groups`. */
  const readGroups = (limit: number): { groups: number[]; ipv4: boolean } => {
    const groups: number[] = [];
    for (let i = 0; i < limit; i++) {
      if (i < limit - 1) {
        const start = at;
        if (i === 0 || s[at] === ":") {
          if (i > 0) at++;
          const v4 = ipv4();
          if (v4) {
            groups.push((v4[0]! << 8) | v4[1]!, (v4[2]! << 8) | v4[3]!);
            return { groups, ipv4: true };
          }
        }
        at = start;
      }
      const start = at;
      if (i > 0) {
        if (s[at] !== ":") return { groups, ipv4: false };
        at++;
      }
      const group = hexGroup();
      if (group === undefined) {
        at = start;
        return { groups, ipv4: false };
      }
      groups.push(group);
    }
    return { groups, ipv4: false };
  };
  const head = readGroups(8);
  if (head.groups.length === 8) return at === s.length ? head.groups : undefined;
  if (head.ipv4) return undefined;
  if (s.slice(at, at + 2) !== "::") return undefined;
  at += 2;
  const tail = readGroups(8 - (head.groups.length + 1));
  if (at !== s.length) return undefined;
  const zeros = new Array<number>(8 - head.groups.length - tail.groups.length).fill(0);
  return [...head.groups, ...zeros, ...tail.groups];
};

export const isIpAddress = (s: string): boolean => parseIpv4(s) !== undefined || parseIpv6(s) !== undefined;

/** Rust's `u16::from_str`: an optional `+`, then decimal digits. */
const parsePort = (s: string): number | undefined => {
  const m = /^\+?([0-9]+)$/.exec(s);
  if (!m) return undefined;
  const value = Number(m[1]);
  return value <= 65535 ? value : undefined;
};

export interface HostRule {
  readonly host: string;
  readonly wildcard: boolean;
  readonly port: number | null;
}

/** net.rs `HostRule::parse`. */
export const parseHostRule = (raw: string): HostRule | string => {
  const bad = `"${raw}" isn't a host (api.example.com, *.example.org, localhost:8080)`;
  const entry = rustTrim(raw);
  if (entry === "" || byteLength(entry) > 253 || /[/@?# ]/.test(entry)) return bad;
  let host: string;
  let port: number | null = null;
  if (entry.startsWith("[")) {
    const rest = entry.slice(1);
    const close = rest.indexOf("]");
    if (close < 0) return bad;
    host = rest.slice(0, close);
    if (!parseIpv6(host)) return bad;
    const after = rest.slice(close + 1);
    if (after.startsWith(":")) {
      const p = parsePort(after.slice(1));
      if (p === undefined) return bad;
      port = p;
    } else if (after !== "") return bad;
  } else {
    const colon = entry.lastIndexOf(":");
    if (colon >= 0 && !entry.slice(0, colon).includes(":")) {
      const p = parsePort(entry.slice(colon + 1));
      if (p === undefined) return bad;
      host = entry.slice(0, colon);
      port = p;
    } else if (colon >= 0) {
      if (!parseIpv6(entry)) return bad;
      host = entry;
    } else {
      host = entry;
    }
  }
  host = host.replace(/[A-Z]/g, (c) => c.toLowerCase());
  const wildcard = host.startsWith("*.");
  const name = wildcard ? host.slice(2) : host;
  const labelOk = (l: string) => l.length > 0 && l.length <= 63 && /^[A-Za-z0-9-]+$/.test(l) && !l.startsWith("-") && !l.endsWith("-");
  const isIp = isIpAddress(name);
  if (!(isIp || name.split(".").every(labelOk)) || (wildcard && (isIp || !name.includes(".")))) return bad;
  return { host: name, wildcard, port };
};

/** net.rs `place` for an address: public, private (this device or your
 * network), or never reachable. */
const placeV4 = ([a, b, c, d]: number[]): "public" | "private" | "never" => {
  if ((a === 169 && b === 254) || (a === 0 && b === 0 && c === 0 && d === 0) || (a === 255 && b === 255 && c === 255 && d === 255)) return "never";
  if (a! >= 224 && a! <= 239) return "never";
  if (a === 0) return "never";
  if ((a === 192 && b === 0 && c === 2) || (a === 198 && b === 51 && c === 100) || (a === 203 && b === 0 && c === 113)) return "never";
  if ((a === 198 && (b! & 0xfe) === 18) || (a === 192 && b === 0 && c === 0) || a! >= 240) return "never";
  if (a === 100 && b === 100 && c === 100 && d === 200) return "never";
  if (a === 127 || a === 10 || (a === 172 && b! >= 16 && b! <= 31) || (a === 192 && b === 168) || (a === 100 && (b! & 0xc0) === 64)) return "private";
  return "public";
};

const placeV6 = (s: number[]): "public" | "private" | "never" => {
  const v4 = (hi: number, lo: number) => [hi >> 8, hi & 0xff, lo >> 8, lo & 0xff];
  const zero6 = s.slice(0, 6).every((g) => g === 0);
  const loopback = zero6 && s[6] === 0 && s[7] === 1;
  const unspecified = s.every((g) => g === 0);
  // IPv4-mapped (::ffff:a.b.c.d).
  if (s.slice(0, 5).every((g) => g === 0) && s[5] === 0xffff) return placeV4(v4(s[6]!, s[7]!));
  if (zero6 && !loopback && !unspecified) return placeV4(v4(s[6]!, s[7]!));
  if (s[0] === 0x64 && s[1] === 0xff9b && s.slice(2, 6).every((g) => g === 0)) return placeV4(v4(s[6]!, s[7]!));
  if (s[0] === 0x2002) return placeV4(v4(s[1]!, s[2]!));
  if (unspecified || (s[0]! & 0xff00) === 0xff00) return "never";
  if ((s[0]! & 0xffc0) === 0xfe80 || (s[0]! & 0xffc0) === 0xfec0 || (s[0] === 0x2001 && s[1] === 0x0db8)) return "never";
  if (s[0] === 0x2001 && s[1] === 0) return "never";
  if (loopback || (s[0]! & 0xfe00) === 0xfc00) return "private";
  return "public";
};

export const place = (ip: string): "public" | "private" | "never" | undefined => {
  const v4 = parseIpv4(ip);
  if (v4) return placeV4(v4);
  const v6 = parseIpv6(ip);
  return v6 ? placeV6(v6) : undefined;
};

/** net.rs `risky`: an entry that reaches this device or your network. */
export const riskyHost = (entry: string): boolean => {
  const rule = parseHostRule(entry);
  if (typeof rule === "string") return true;
  const where = place(rule.host);
  if (where !== undefined) return where !== "public";
  const host = rule.host;
  const last = host.replace(/\.+$/, "").split(".").pop() ?? host;
  const hex = last.startsWith("0x") ? /^[0-9A-Fa-f]*$/.test(last.slice(2)) : undefined;
  const endsInNumber = hex ?? (last !== "" && /^[0-9]+$/.test(last));
  return (
    host === "localhost" ||
    !host.includes(".") ||
    endsInNumber ||
    [".localhost", ".local", ".internal", ".lan", ".home.arpa", ".intranet", ".corp"].some((t) => host.endsWith(t))
  );
};

/** net.rs `names_exactly`. */
export const namesExactly = (entry: string): boolean => entry !== "*" && !rustTrimStart(entry).startsWith("*.");

// ── the manifest ────────────────────────────────────────────────────────────

export interface ToolSpec {
  readonly name: string;
  readonly description: string;
}

/** A manifest as the engine reads it, with its first problem. */
export interface Manifest {
  readonly raw: JsonObject;
  readonly id: string;
  readonly name: string;
  readonly version: string;
  readonly description: string;
  readonly entry: string;
  readonly permissions: readonly string[];
  readonly ui: { readonly change: readonly string[]; readonly replace: readonly string[] };
  readonly tools: readonly ToolSpec[];
  readonly mcp: boolean;
  readonly commands: readonly string[];
  readonly service: string | null;
  readonly networkHosts: readonly string[];
  readonly problem: string | undefined;
}

const stringList = (value: Json | undefined, what: string, problems: string[]): string[] => {
  if (value === undefined || value === null) return [];
  if (isArray(value) && value.every((v) => typeof v === "string")) return value as string[];
  problems.push(`Its ${what} must be a list of text`);
  return [];
};

/** manifest.rs `Manifest::from_value` and `rules`. */
export const readManifest = (raw: JsonObject): Manifest => {
  const problems: string[] = [];
  const text = (key: string, required: boolean): string => {
    const value = raw.get(key);
    if (typeof value === "string" && (rustTrim(value) !== "" || !required)) return value;
    if ((value === undefined || value === null) && !required) return "";
    problems.push(required ? `Its manifest is missing "${key}"` : `Its "${key}" must be text`);
    return "";
  };
  const id = text("id", true);
  const name = text("name", true);
  const version = text("version", true);
  const entry = text("entry", true);
  const description = text("description", false);
  const permissions = stringList(raw.get("permissions"), "permissions", problems);
  let ui = { change: [] as string[], replace: [] as string[] };
  const uiValue = raw.get("ui");
  if (isObject(uiValue)) {
    ui = { change: stringList(uiValue.get("change"), "ui.change", problems), replace: stringList(uiValue.get("replace"), "ui.replace", problems) };
  } else if (uiValue !== undefined && uiValue !== null) {
    problems.push('Its "ui" must be an object');
  }
  const tools: ToolSpec[] = [];
  const toolsValue = raw.get("tools");
  if (isArray(toolsValue)) {
    for (const item of toolsValue) {
      const toolName = isObject(item) ? item.get("name") : undefined;
      if (isObject(item) && typeof toolName === "string") {
        const d = item.get("description");
        tools.push({ name: toolName, description: typeof d === "string" ? d : "" });
      } else {
        problems.push('Each of its tools needs a "name"');
      }
    }
  } else if (toolsValue !== undefined && toolsValue !== null) {
    problems.push('Its "tools" must be a list');
  }
  let mcp = false;
  const mcpValue = raw.get("mcp");
  if (isObject(mcpValue)) {
    const command = mcpValue.get("command");
    if (typeof command === "string" && rustTrim(command) !== "") {
      mcp = true;
      stringList(mcpValue.get("args"), "mcp.args", problems);
    } else {
      problems.push('Its "mcp" needs a "command"');
    }
  } else if (mcpValue !== undefined && mcpValue !== null) {
    problems.push('Its "mcp" must be an object');
  }
  const commands = stringList(raw.get("commands"), '"commands"', problems);
  let service: string | null = null;
  const serviceValue = raw.get("service");
  if (typeof serviceValue === "string") service = serviceValue;
  else if (serviceValue !== undefined && serviceValue !== null) problems.push('Its "service" must be the path of a .luau file in it');
  let networkHosts: string[] = [];
  const network = raw.get("network");
  if (isObject(network)) networkHosts = stringList(network.get("hosts"), "network.hosts", problems);
  else if (network !== undefined && network !== null) problems.push('Its "network" must be an object ("hosts": ["api.example.com"])');

  const manifest = { raw, id, name, version, description, entry, permissions, ui, tools, mcp, commands, service, networkHosts };
  return { ...manifest, problem: problems[0] ?? rules(manifest) };
};

const rules = (m: Omit<Manifest, "problem">): string | undefined => {
  const unknown = m.permissions.find((p) => !KNOWN_PERMISSIONS.includes(p));
  if (unknown !== undefined) return `It asks for "${unknown}", which isn't a permission Zeron knows`;
  const asks = (p: string) => m.permissions.includes(p);
  if (m.networkHosts.length > 0 && !asks("network")) return 'It names hosts to reach but doesn\'t ask for "network"';
  if (m.networkHosts.length > MAX_HOSTS) return `A plugin names ${MAX_HOSTS} hosts at most`;
  for (const host of m.networkHosts) {
    const rule = parseHostRule(host);
    if (typeof rule === "string") return rule;
  }
  if (m.tools.length > 0 && !asks("tools")) return 'It lists tools but doesn\'t ask for the "tools" permission';
  if (m.mcp && !asks("native")) return 'It has an MCP server but doesn\'t ask for the "native" permission';
  if (m.commands.length > 0 && !asks("commands")) return 'It names programs to run but doesn\'t ask for "commands"';
  if (asks("commands") && m.commands.length === 0) return 'It asks for "commands" but names no program to run';
  const badCommand = m.commands.find((c) => !validCommand(c));
  if (badCommand !== undefined) return `"${badCommand}" isn't a program's name: name it as it's found on the PATH`;
  if (m.service !== null && !asks("service")) return 'It has a service but doesn\'t ask for "service"';
  if (m.service === null && asks("service")) return 'It asks for "service" but has none ("service": "service.luau")';
  if (m.service !== null && !validServicePath(m.service)) return `Its service "${m.service}" must be a .luau file inside it, by a relative path`;
  if (m.ui.change.length > 0 && !asks("ui.change")) return 'It changes parts of Zeron but doesn\'t ask for "ui.change"';
  for (const target of m.ui.change) {
    const problem = changeTargetProblem(target);
    if (problem) return problem;
  }
  if (m.ui.replace.length > 0 && !asks("ui.replace")) return 'It replaces screens but doesn\'t ask for "ui.replace"';
  for (const screen of m.ui.replace) {
    if (!screen.startsWith("@app/screens/")) return `It can only replace Zeron's screens, not "${screen}"`;
    if (["@app/screens/plugins", "@app/screens/shell/settings"].some((p) => screen === p || screen.startsWith(`${p}/`))) {
      return `"${screen}" can't be replaced by a plugin`;
    }
  }
  const badTool = m.tools.find((t) => !validToolName(t.name));
  if (badTool) return `Its tool "${badTool.name}" needs a name of lowercase letters, digits and underscores`;
  return undefined;
};
