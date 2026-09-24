import { describe, expect, it } from "vitest";
import type { EngineClient } from "@zeron/engine-client";
import type { AgentLoginPoll, HarnessDescriptor, Model, TitleSettings } from "@zeron/proto";
import { methods } from "@zeron/engine-client";
import {
  blurb,
  cliName,
  descriptorEnabled,
  getTitleSettings,
  listHarnesses,
  listModels,
  nextSignInPhase,
  notInstalledHint,
  offeredHarnesses,
  setHarnessEnabled,
  setTitleSettings,
  signInFailureLabel,
  signInPendingLabel,
  signsInOnEnable,
  supportsTitles,
  titleHarnessLabel,
  visibleHarnesses,
} from "../src/lib/harnesses";

function descriptor(fields: Partial<HarnessDescriptor>): HarnessDescriptor {
  return {
    id: "claude-code",
    name: "Claude Code",
    supportsSteering: true,
    steeringMode: "step-boundary",
    reasoningLevels: [],
    installed: true,
    ...fields,
  };
}

describe("descriptorEnabled (registry.rs:66-70)", () => {
  it("descriptorEnabledDefaultsToInstalledExceptMock", () => {
    // An explicit flag wins, either way.
    expect(descriptorEnabled(descriptor({ enabled: true }))).toBe(true);
    expect(descriptorEnabled(descriptor({ enabled: false }))).toBe(false);
    // A null flag (an engine predating the setting) falls back to
    // detection — installed and not the mock harness.
    expect(descriptorEnabled(descriptor({ enabled: null, installed: true }))).toBe(true);
    expect(descriptorEnabled(descriptor({ enabled: null, installed: false }))).toBe(false);
    expect(descriptorEnabled(descriptor({ enabled: null, id: "mock", installed: true }))).toBe(false);
    // The mock harness defaults OFF without an explicit flag — the dev rig
    // opts in through the environment, not detection.
    expect(descriptorEnabled(descriptor({ id: "mock", installed: true }))).toBe(false);
  });

  it("antigravityStaysOffUntilTheUserOptsIn", () => {
    // The opt-in fallback (registry.rs opt_in): a null enabled flag reads
    // as OFF — enabling antigravity downloads a large server and runs a
    // browser sign-in, which detection alone must never set off.
    expect(
      descriptorEnabled(descriptor({ id: "antigravity", name: "Antigravity", enabled: null })),
    ).toBe(false);
    // The engine-side opt-in lands as an explicit flag.
    expect(
      descriptorEnabled(descriptor({ id: "antigravity", name: "Antigravity", enabled: true })),
    ).toBe(true);
  });
});

describe("visible/offered harnesses (pickers.rs:4031-4069)", () => {
  it("mockHarnessHiddenUnlessOnlyOption", () => {
    const real = descriptor({ id: "codex", name: "Codex" });
    const mock = descriptor({ id: "mock", name: "Mock", installed: true });
    // Mock hides behind the real harnesses...
    expect(visibleHarnesses([real, mock]).map((d) => d.id)).toEqual(["codex"]);
    // ...but is the only row when it is literally all there is (the smoke
    // engine's registry).
    expect(visibleHarnesses([mock]).map((d) => d.id)).toEqual(["mock"]);
    // Offered additionally requires installed + enabled.
    expect(
      offeredHarnesses([real, descriptor({ id: "codex", name: "Codex", enabled: false }), mock]).map((d) => d.id),
    ).toEqual(["codex"]);
  });
});

describe("page copy tables (harnesses.rs:41-68)", () => {
  it("blurbs and CLI names cover every harness", () => {
    for (const id of [
      "claude-code",
      "codex",
      "cursor",
      "devin",
      "grok",
      "hermes",
      "pi",
      "opencode",
      "antigravity",
      "mock",
    ] as const) {
      expect(blurb(id).length).toBeGreaterThan(0);
      expect(cliName(id).length).toBeGreaterThan(0);
    }
    expect(cliName("cursor")).toBe("cursor-agent");
    expect(cliName("antigravity")).toBe("agy");
    expect(blurb("mock")).toBe("Scripted test harness.");
  });

  it("notInstalledHint swaps wording for the stale-catalog row", () => {
    expect(notInstalledHint("codex", false)).toBe("Install the codex CLI to enable");
    expect(notInstalledHint("codex", true)).toBe("codex CLI not installed — turn it off or install it");
  });

  it("supportsTitles matches harness lib.rs (codex, claude-code, mock)", () => {
    expect(supportsTitles("codex")).toBe(true);
    expect(supportsTitles("claude-code")).toBe(true);
    expect(supportsTitles("mock")).toBe(true);
    expect(supportsTitles("cursor")).toBe(false);
    expect(supportsTitles("opencode")).toBe(false);
    expect(supportsTitles("antigravity")).toBe(false);
  });

  it("titleHarnessLabel names the two supported real agents", () => {
    expect(titleHarnessLabel("claude-code", "whatever")).toBe("Claude Code");
    expect(titleHarnessLabel("codex", "whatever")).toBe("Codex");
    expect(titleHarnessLabel("grok", "Grok")).toBe("Grok");
  });
});

describe("antigravity sign-in (harnesses.rs SignInPhase)", () => {
  it("onlyAntigravitySignsInOnEnable", () => {
    expect(signsInOnEnable("antigravity")).toBe(true);
    for (const id of ["claude-code", "codex", "cursor", "devin", "grok", "hermes", "pi", "opencode"] as const) {
      expect(signsInOnEnable(id)).toBe(false);
    }
  });

  it("setupCopyMatchesEachPhase", () => {
    expect(signInPendingLabel("starting")).toBe("Preparing Antigravity…");
    expect(signInFailureLabel("starting")).toBe("Setup failed");
    expect(signInPendingLabel("installing")).toBe("Installing Antigravity…");
    expect(signInFailureLabel("installing")).toBe("Installation failed");
    expect(signInPendingLabel("authenticating")).toBe("Finish signing in in your browser.");
    expect(signInFailureLabel("authenticating")).toBe("Sign-in failed");
    expect(signInPendingLabel("enabling")).toBe("Enabling Antigravity…");
    expect(signInFailureLabel("enabling")).toBe("Enable failed");
  });

  it("nextSignInPhaseTracksThePoll", () => {
    const poll = (fields: Partial<AgentLoginPoll>): AgentLoginPoll => ({
      status: "pending",
      message: null,
      url: null,
      ...fields,
    });
    // A pending poll without a url keeps the current phase (null).
    expect(nextSignInPhase(poll({ message: "Downloading Antigravity." }))).toBeNull();
    // The first poll that names the sign-in page moves to authenticating.
    expect(nextSignInPhase(poll({ url: "https://accounts.google.com/o/oauth2" }))).toBe(
      "authenticating",
    );
    // Done hands over to the enabling step; error never moves the phase —
    // the failure label names where it stopped.
    expect(nextSignInPhase(poll({ status: "done" }))).toBe("enabling");
    expect(nextSignInPhase(poll({ status: "error" }))).toBeNull();
  });
});

/** A call-recording fake EngineClient — the protocol-boundary check. */
function fakeClient(replies: Record<string, unknown>) {
  const calls: { method: string; params: unknown }[] = [];
  const client = {
    async call(method: string, params: unknown): Promise<unknown> {
      calls.push({ method, params });
      const reply = replies[method];
      if (reply === undefined) {
        throw new Error(`unknown method: ${method}`);
      }
      return reply;
    },
  } as unknown as EngineClient;
  return { client, calls };
}

const CATALOG: HarnessDescriptor[] = [descriptor({})];
const TITLES: TitleSettings = { harness: "claude-code", model: "opus" };
const MODELS: Model[] = [
  { id: "opus", label: "Opus", reasoningLevels: [], options: [] },
];

describe("harnesses RPC wrappers", () => {
  it("lists harnesses with the targetDeviceId passthrough only when set", async () => {
    const { client, calls } = fakeClient({ ListHarnesses: CATALOG });
    expect(await listHarnesses(client, null)).toEqual(CATALOG);
    expect(await listHarnesses(client, "dev-2")).toEqual(CATALOG);
    expect(calls[0]).toEqual({ method: "ListHarnesses", params: {} });
    expect(calls[1]).toEqual({ method: "ListHarnesses", params: { targetDeviceId: "dev-2" } });
  });

  it("toggles through SetHarnessEnabled and returns the fresh catalog", async () => {
    const { client, calls } = fakeClient({ SetHarnessEnabled: CATALOG });
    expect(await setHarnessEnabled(client, "codex", false, "dev-2")).toEqual(CATALOG);
    expect(calls).toEqual([
      { method: "SetHarnessEnabled", params: { harness: "codex", enabled: false, targetDeviceId: "dev-2" } },
    ]);
    expect(methods.SET_HARNESS_ENABLED).toBe("SetHarnessEnabled");
  });

  it("reads and writes the title pair (params ARE the settings)", async () => {
    const { client, calls } = fakeClient({ GetTitleSettings: TITLES, SetTitleSettings: TITLES });
    expect(await getTitleSettings(client)).toEqual(TITLES);
    expect(await setTitleSettings(client, { harness: null, model: null })).toEqual(TITLES);
    expect(calls).toEqual([
      { method: "GetTitleSettings", params: {} },
      { method: "SetTitleSettings", params: { harness: null, model: null } },
    ]);
    expect(methods.GET_TITLE_SETTINGS).toBe("GetTitleSettings");
    expect(methods.SET_TITLE_SETTINGS).toBe("SetTitleSettings");
  });

  it("lists models for the picked harness", async () => {
    const { client, calls } = fakeClient({ ListModels: MODELS });
    expect(await listModels(client, "claude-code", "dev-2")).toEqual(MODELS);
    expect(calls).toEqual([{ method: "ListModels", params: { harness: "claude-code", targetDeviceId: "dev-2" } }]);
  });
});
