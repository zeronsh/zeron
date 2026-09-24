import { methods, type EngineClient } from "@zeron/engine-client";
import type { AgentLoginPoll, HarnessDescriptor, HarnessId, Model, TitleSettings } from "@zeron/proto";
import type { EngineSession } from "../state/engine-session";
import { descriptorEnabled, offeredHarnesses, visibleHarnesses } from "./model-rows";

/**
 * Settings → Agents ("Harnesses") — the web peer of
 * `crates/ui/src/settings/harnesses.rs`. The pure visibility/enablement
 * helpers (`visible_harnesses`/`offered_harnesses`/`descriptor_enabled`,
 * pickers.rs:4031-4069 + registry.rs:66-70) live in `lib/model-rows.ts`
 * (ticket 10 landed them for the composer); this module re-exports them as
 * the page's named surface and adds what only this page needs: the blurb /
 * CLI-name tables, the title-support set, the RPC wrappers, and the
 * composer-catalog cache-bust (`pickers::bump_harness_catalog`'s purpose).
 */

export { descriptorEnabled, offeredHarnesses, visibleHarnesses };

// ── RPC wrappers ────────────────────────────────────────────────────────
// All four ride the optional `targetDeviceId` passthrough: null (the local
// device) sends nothing — the calls stay direct, exactly as on the desktop.

function targetParams(targetDeviceId: string | null | undefined): Record<string, string> {
  return targetDeviceId == null ? {} : { targetDeviceId };
}

/** `ListHarnesses` — the device's harness catalog (installed probe included). */
export function listHarnesses(
  client: EngineClient,
  targetDeviceId?: string | null,
): Promise<HarnessDescriptor[]> {
  return client.call<HarnessDescriptor[]>(methods.LIST_HARNESSES, targetParams(targetDeviceId));
}

/**
 * `SetHarnessEnabled` — flip one harness; the reply is the fresh catalog, so
 * the page repaints (and a refused/raced toggle self-corrects) in one round
 * trip.
 */
export function setHarnessEnabled(
  client: EngineClient,
  harness: HarnessId,
  enabled: boolean,
  targetDeviceId?: string | null,
): Promise<HarnessDescriptor[]> {
  return client.call<HarnessDescriptor[]>(methods.SET_HARNESS_ENABLED, {
    harness,
    enabled,
    ...targetParams(targetDeviceId),
  });
}

/** `GetTitleSettings` — the device's automatic-title pair. */
export function getTitleSettings(
  client: EngineClient,
  targetDeviceId?: string | null,
): Promise<TitleSettings> {
  return client.call<TitleSettings>(methods.GET_TITLE_SETTINGS, targetParams(targetDeviceId));
}

/**
 * `SetTitleSettings` — the params ARE the settings (`TitleSettings` serializes
 * to `{harness, model}`); the reply is the stored pair, re-read after the
 * engine's own validation.
 */
export function setTitleSettings(
  client: EngineClient,
  settings: TitleSettings,
  targetDeviceId?: string | null,
): Promise<TitleSettings> {
  return client.call<TitleSettings>(
    methods.SET_TITLE_SETTINGS,
    { ...settings, ...targetParams(targetDeviceId) },
  );
}

/** `ListModels` — the picked harness's model catalog (the title-model picker). */
export function listModels(
  client: EngineClient,
  harness: HarnessId,
  targetDeviceId?: string | null,
): Promise<Model[]> {
  return client.call<Model[]>(methods.LIST_MODELS, { harness, ...targetParams(targetDeviceId) });
}

/**
 * `pickers::bump_harness_catalog`, web-shaped: after a toggle lands, poke the
 * composer's per-session catalog to re-fetch its harness list — the pickers
 * read `descriptorEnabled` per render, so a stale-while-revalidate reload is
 * enough (the currently-shown rows stay up while the fresh catalog lands).
 * No-op without a session (nothing is cached yet).
 */
export function bumpHarnessCatalog(session: EngineSession | null): void {
  void session?.catalog.loadHarnesses({ force: true });
}

// ── Page copy (harnesses.rs blurb/cli_name, harness lib.rs supports_titles) ──

/** One-line blurb per agent (harnesses.rs:41-53), verbatim. */
export function blurb(harness: HarnessId): string {
  switch (harness) {
    case "claude-code":
      return "Anthropic's coding agent, driven through the Claude Code CLI.";
    case "codex":
      return "OpenAI's coding agent, driven through the Codex CLI.";
    case "cursor":
      return "Cursor's coding agent, driven through the cursor-agent CLI.";
    case "devin":
      return "Cognition's Devin agent (devin CLI).";
    case "grok":
      return "xAI's Grok Build agent (grok CLI).";
    case "hermes":
      return "Nous Research's Hermes Agent (hermes CLI).";
    case "pi":
      return "The pi coding agent (pi CLI).";
    case "opencode":
      return "SST's opencode agent (opencode CLI).";
    case "antigravity":
      return "Google's Antigravity agent (Antigravity ACP server).";
    case "mock":
      return "Scripted test harness.";
  }
}

/** The CLI named in the not-installed hint (harnesses.rs:56-68), verbatim. */
export function cliName(harness: HarnessId): string {
  switch (harness) {
    case "claude-code":
      return "claude";
    case "codex":
      return "codex";
    case "cursor":
      return "cursor-agent";
    case "devin":
      return "devin";
    case "grok":
      return "grok";
    case "hermes":
      return "hermes";
    case "pi":
      return "pi";
    case "opencode":
      return "opencode";
    case "antigravity":
      return "agy";
    case "mock":
      return "mock";
  }
}

// ── Antigravity sign-in (harnesses.rs SignInPhase + signs_in_on_enable) ──

/**
 * Harnesses whose toggle runs the agent's own sign-in before switching on
 * (`signs_in_on_enable`, harnesses.rs): antigravity only — its ACP server's
 * google sign-in runs from Settings, never mid-chat.
 */
export function signsInOnEnable(harness: HarnessId): boolean {
  return harness === "antigravity";
}

/** The milestones of an enable-with-sign-in (harnesses.rs `SignInPhase`). */
export type SignInPhase = "starting" | "installing" | "authenticating" | "enabling";

/** The in-progress row copy (harnesses.rs `pending_label`), verbatim. */
export function signInPendingLabel(phase: SignInPhase): string {
  switch (phase) {
    case "starting":
      return "Preparing Antigravity…";
    case "installing":
      return "Installing Antigravity…";
    case "authenticating":
      return "Finish signing in in your browser.";
    case "enabling":
      return "Enabling Antigravity…";
  }
}

/** The failure row copy (harnesses.rs `failure_label`), verbatim. */
export function signInFailureLabel(phase: SignInPhase): string {
  switch (phase) {
    case "starting":
      return "Setup failed";
    case "installing":
      return "Installation failed";
    case "authenticating":
      return "Sign-in failed";
    case "enabling":
      return "Enable failed";
  }
}

/**
 * The phase a poll moves an in-flight sign-in to, or `null` to keep the
 * current one (a pending poll without a url). `done` lands as the enabling
 * step — the toggle itself still has to run once the sign-in succeeded.
 */
export function nextSignInPhase(poll: AgentLoginPoll): SignInPhase | null {
  if (poll.status === "done") {
    return "enabling";
  }
  if (poll.status === "pending" && poll.url != null) {
    return "authenticating";
  }
  return null;
}

/**
 * `zeron_harness::supports_titles` (harness/src/lib.rs:306-311): the drivers
 * with a restricted title-generation path — Codex, Claude Code, and the dev
 * rig's Mock.
 */
export function supportsTitles(harness: HarnessId): boolean {
  return harness === "codex" || harness === "claude-code" || harness === "mock";
}

/**
 * The not-installed hint (harnesses.rs:616-624): one wording for the
 * never-enabled row, another for a stale catalog that still stamps an
 * enabled-but-uninstalled row. Both share the `warning_muted.opacity(0.9)`
 * tone, applied by the page's CSS.
 */
export function notInstalledHint(harness: HarnessId, enabled: boolean): string {
  return enabled
    ? `${cliName(harness)} CLI not installed — turn it off or install it`
    : `Install the ${cliName(harness)} CLI to enable`;
}

/**
 * The title-harness picker's display name (harnesses.rs:278-283): the two
 * supported real agents by name; anything else falls back to the caller's
 * spelling of the id.
 */
export function titleHarnessLabel(harness: HarnessId, fallback: string): string {
  switch (harness) {
    case "claude-code":
      return "Claude Code";
    case "codex":
      return "Codex";
    default:
      return fallback;
  }
}
