import type { HarnessId, SlashCommand } from "@zeron/proto";
import type { RpcErrorKind } from "@zeron/engine-client";
import { filterIndices } from "./picker-search";
import type { CompletionToken } from "./mentions";

/**
 * The `/` slash-command library — a port of the desktop's composer.rs slash
 * core: `slash_token` (`:3905`), `slash_error_message` (`:3980`), the
 * once-per-harness `slash_cache` shape (`:4026`), and the local per-keystroke
 * re-rank (`refilter_slash`, `:5430`). Slash commands are whole-prompt
 * prefixes, so only the first token triggers, and a query containing another
 * `/` (a typed path) never does.
 */

/**
 * `slash_token` (composer.rs:3905-3925): the text must start with `/`, the
 * cursor must sit inside the first command word, and the query must carry
 * no other `/`. `range` spans the whole command word, `query` is the typed
 * filter.
 */
export function slashToken(text: string, cursor: number): CompletionToken | null {
  if (cursor > text.length || !isCharBoundary(text, cursor) || !text.startsWith("/")) {
    return null;
  }
  let end = text.length;
  for (let ix = 0; ix < text.length; ix += 1) {
    const code = text.charCodeAt(ix);
    if (code === 0x20 || (code >= 0x09 && code <= 0x0d)) {
      end = ix;
      break;
    }
  }
  // Cursor outside the command token (typing the argument): popup closed.
  if (cursor === 0 || cursor > end) {
    return null;
  }
  const query = text.slice(1, cursor);
  if (query.includes("/")) {
    return null;
  }
  return { start: 0, end, query };
}

function isCharBoundary(text: string, index: number): boolean {
  if (index < 0 || index > text.length) {
    return false;
  }
  if (index === text.length) {
    return true;
  }
  const code = text.charCodeAt(index);
  return code < 0xdc00 || code > 0xdfff;
}

/** The cache shape: one `ListCommands` per harness per composer lifetime. */
export type SlashCache = Map<HarnessId, readonly SlashCommand[]>;

/** The result of a local re-rank: ranked indices plus the reset cursor. */
export interface SlashFilterResult {
  /** Indices into the cached command list, filter-ranked for the query. */
  readonly filtered: readonly number[];
  /** `0` when any row matched, else null (composer.rs:5445). */
  readonly active: number | null;
}

/**
 * `refilter_slash` (composer.rs:5430-5449): the pure local filter — prefix
 * before substring, ties by input order (`popover::filter_indices`), the
 * cursor re-entering at 0. No RPC, no debounce, no skeleton churn.
 */
export function refilterSlash(
  query: string,
  commands: readonly SlashCommand[],
): SlashFilterResult {
  const names = commands.map((command) => command.name);
  const filtered = filterIndices(query, names);
  return { filtered, active: filtered.length > 0 ? 0 : null };
}

/**
 * `slash_error_message` (composer.rs:3980): a failed command discovery,
 * translated for the popup. `timeout`/`parked` (web-client-only kinds) read
 * as unreachable.
 */
export function slashErrorMessage(kind: RpcErrorKind): string {
  switch (kind) {
    case "unknown-method":
      return "The session's device runs an older zeron — update it to list commands";
    case "transport":
    case "closed":
    case "timeout":
    case "parked":
      return "The session's device is unreachable";
    default:
      return "Couldn't load this agent's commands";
  }
}

/**
 * A slash row's description (composer.rs:5580-5588): the command's own
 * description, with the input hint folded in — `"<hint>"` alone when there
 * is no description, else `"{description} · <{hint}>"`.
 */
export function slashDescription(command: SlashCommand): string {
  const hint = command.inputHint ?? null;
  if (hint === null || hint.length === 0) {
    return command.description;
  }
  return command.description.length === 0
    ? `<${hint}>`
    : `${command.description} · <${hint}>`;
}

/** Decode a `ListCommands` reply, tolerating a malformed payload. */
export function parseSlashCommands(reply: unknown): readonly SlashCommand[] | null {
  if (!Array.isArray(reply)) {
    return null;
  }
  const commands: SlashCommand[] = [];
  for (const entry of reply) {
    if (typeof entry !== "object" || entry === null) {
      return null;
    }
    const candidate = entry as Record<string, unknown>;
    if (typeof candidate.name !== "string" || typeof candidate.description !== "string") {
      return null;
    }
    const inputHint =
      typeof candidate.inputHint === "string" ? candidate.inputHint : null;
    commands.push({
      name: candidate.name,
      description: candidate.description,
      inputHint: candidate.inputHint === undefined ? null : inputHint,
    });
  }
  return commands;
}
