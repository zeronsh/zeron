import { describe, expect, it } from "vitest";
import type { EngineClient } from "@zeron/engine-client";
import type { AgentAccount, AgentAccountsSnapshot, AgentLoginPoll } from "@zeron/proto";
import {
  accountInitial,
  accountLabel,
  activateAgentAccount,
  cancelAgentLogin,
  completeAgentLogin,
  forceUsageFor,
  forgetAgentAccount,
  formatReset,
  listAgentAccounts,
  loginTitle,
  pollAgentLogin,
  providerAccounts,
  providerEmptyCopy,
  PROVIDERS,
  startAgentLogin,
  usageFallback,
  usageLevel,
  USAGE_CRITICAL_FRACTION,
  USAGE_WARN_FRACTION,
} from "../src/lib/accounts";

const NOW = Date.parse("2026-09-16T12:00:00Z");

describe("usageLevel (accounts.rs thresholds)", () => {
  it("classifies against the 80/95 cuts", () => {
    expect(USAGE_WARN_FRACTION).toBe(0.8);
    expect(USAGE_CRITICAL_FRACTION).toBe(0.95);
    expect(usageLevel(0)).toBe("normal");
    expect(usageLevel(0.79)).toBe("normal");
    expect(usageLevel(0.8)).toBe("warn");
    expect(usageLevel(0.94)).toBe("warn");
    expect(usageLevel(0.95)).toBe("critical");
    expect(usageLevel(1)).toBe("critical");
  });
});

describe("forceUsageFor (LoadTrigger policy)", () => {
  it("forces on the visit's first list, refresh, and post-login only", () => {
    expect(forceUsageFor("mount")).toBe(true);
    expect(forceUsageFor("retry")).toBe(true);
    expect(forceUsageFor("refresh")).toBe(true);
    expect(forceUsageFor("postLogin")).toBe(true);
    expect(forceUsageFor("postAction")).toBe(false);
  });
});

describe("formatReset", () => {
  it("is null without a reset moment", () => {
    expect(formatReset(null, NOW)).toBe(null);
    expect(formatReset("not a date", NOW)).toBe(null);
  });

  it("uses a clock time within ~22h, a weekday within a week, then month+day", () => {
    const soon = formatReset(new Date(NOW + 2 * 3_600_000).toISOString(), NOW);
    expect(soon).toMatch(/^resets \d{1,2}:\d{2}/);
    const days = formatReset(new Date(NOW + 3 * 86_400_000).toISOString(), NOW);
    expect(days).toMatch(/^resets (Mon|Tue|Wed|Thu|Fri|Sat|Sun)$/);
    const weeks = formatReset(new Date(NOW + 30 * 86_400_000).toISOString(), NOW);
    expect(weeks).toMatch(/^resets [A-Z][a-z]{2} \d{1,2}$/);
  });
});

describe("providers", () => {
  it("lists Claude Code, Codex, Cursor in the desktop's order", () => {
    expect(PROVIDERS.map((provider) => provider.name)).toEqual(["Claude Code", "Codex", "Cursor"]);
    expect(PROVIDERS.map((provider) => provider.cli)).toEqual(["claude", "codex", "cursor-agent"]);
  });

  it("names the CLI in the empty-state copy, except Cursor", () => {
    expect(providerEmptyCopy(PROVIDERS[0]!)).toContain("claude");
    expect(providerEmptyCopy(PROVIDERS[2]!)).toContain("isn't connected");
    expect(providerEmptyCopy(PROVIDERS[2]!)).not.toContain("cursor-agent login — sign in");
  });
});

function account(fields: Partial<AgentAccount>): AgentAccount {
  return {
    id: "a1",
    harness: "claude-code",
    email: "user@example.com",
    planLabel: null,
    active: false,
    usageWindows: [],
    switchable: true,
    ...fields,
  };
}

function snapshotOf(accounts: AgentAccount[]): AgentAccountsSnapshot {
  return { accounts, warnings: [] };
}

describe("providerAccounts", () => {
  it("filters by harness in engine order — no active-first re-sort", () => {
    const snapshot = snapshotOf([
      account({ id: "c1", harness: "claude-code", active: false }),
      account({ id: "x1", harness: "codex" }),
      account({ id: "c2", harness: "claude-code", active: true }),
    ]);
    expect(providerAccounts(snapshot, "claude-code").map((a) => a.id)).toEqual(["c1", "c2"]);
    expect(providerAccounts(snapshot, "codex").map((a) => a.id)).toEqual(["x1"]);
    expect(providerAccounts(snapshot, "cursor")).toEqual([]);
  });
});

describe("account labels", () => {
  it("prefers email, then display name, then the fallback", () => {
    expect(accountLabel(account({ email: "a@b.c", displayName: "Name" }))).toBe("a@b.c");
    expect(accountLabel(account({ email: null, displayName: "Name" }))).toBe("Name");
    expect(accountLabel(account({ email: null, displayName: null }))).toBe("Unknown account");
  });

  it("derives the avatar initial", () => {
    expect(accountInitial(account({ email: "user@example.com" }))).toBe("U");
    expect(accountInitial(account({ email: null, displayName: null }))).toBe("U");
  });

  it("reserves 'Usage unavailable' for switchable accounts", () => {
    expect(usageFallback(account({ switchable: true }))).toBe("Usage unavailable");
    expect(usageFallback(account({ switchable: false }))).toBe("Credentials unavailable");
  });
});

describe("loginTitle", () => {
  it("matches the desktop dialog titles", () => {
    expect(loginTitle("codex")).toBe("Add Codex account");
    expect(loginTitle("cursor")).toBe("Connect Cursor");
    expect(loginTitle("claude-code")).toBe("Add Claude account");
  });
});

/** A scripted call stub — pollAgentLogin's protocol boundary. */
function scriptedClient(replies: (AgentLoginPoll | Error)[]) {
  const calls: { method: string; params: unknown }[] = [];
  const client = {
    async call(method: string, params: unknown): Promise<unknown> {
      calls.push({ method, params });
      const next = replies.length > 0 ? replies.shift()! : { status: "pending" };
      if (next instanceof Error) {
        throw next;
      }
      return next;
    },
  } as unknown as EngineClient;
  return { client, calls };
}

const NO_SLEEP = () => Promise.resolve();

describe("pollAgentLogin (the browser-flow wait loop)", () => {
  it("polls until done, surfacing pending messages", async () => {
    const { client, calls } = scriptedClient([
      { status: "pending", message: "Opening the browser…" },
      { status: "pending", message: null },
      { status: "done" },
    ]);
    const messages: (string | null)[] = [];
    const poll = await pollAgentLogin(client, "login-1", {
      isActive: () => true,
      onPending: (message) => messages.push(message),
      sleep: NO_SLEEP,
    });
    expect(poll).toEqual({ status: "done" });
    expect(messages).toEqual(["Opening the browser…", null]);
    expect(calls.every((call) => call.method === "PollAgentLogin")).toBe(true);
    expect(calls[0]!.params).toEqual({ loginId: "login-1" });
  });

  it("resolves an error poll when the flow fails engine-side", async () => {
    const { client } = scriptedClient([{ status: "error", message: "denied" }]);
    const poll = await pollAgentLogin(client, "login-1", { isActive: () => true, onPending: () => {}, sleep: NO_SLEEP });
    expect(poll).toEqual({ status: "error", message: "denied" });
  });

  it("turns a failed poll call into an error poll", async () => {
    const { client } = scriptedClient([new Error("transport: gone")]);
    const poll = await pollAgentLogin(client, "login-1", { isActive: () => true, onPending: () => {}, sleep: NO_SLEEP });
    expect(poll?.status).toBe("error");
    expect(poll?.message).toContain("Poll failed: transport: gone");
  });

  it("stops polling when the dialog is dismissed", async () => {
    const { client, calls } = scriptedClient([{ status: "pending" }, { status: "done" }]);
    let active = true;
    const poll = await pollAgentLogin(client, "login-1", {
      isActive: () => active,
      onPending: () => {
        active = false; // dismissed after the first pending note
      },
      sleep: NO_SLEEP,
    });
    expect(poll).toBe(null);
    expect(calls).toHaveLength(1);
  });
});

describe("account RPC wrappers", () => {
  function fakeClient(reply: unknown) {
    const calls: { method: string; params: unknown }[] = [];
    const client = {
      async call(method: string, params: unknown): Promise<unknown> {
        calls.push({ method, params });
        return reply;
      },
    } as unknown as EngineClient;
    return { client, calls };
  }

  it("lists with the forceUsage flag", async () => {
    const { client, calls } = fakeClient(snapshotOf([]));
    await listAgentAccounts(client, true);
    expect(calls).toEqual([{ method: "ListAgentAccounts", params: { forceUsage: true } }]);
  });

  it("activates and forgets with the desktop's tolerant param shape", async () => {
    const { client, calls } = fakeClient(snapshotOf([]));
    const target = account({ id: "a1", harness: "codex" });
    await activateAgentAccount(client, target);
    await forgetAgentAccount(client, target);
    expect(calls).toEqual([
      { method: "ActivateAgentAccount", params: { id: "a1", accountId: "a1", harness: "codex" } },
      { method: "ForgetAgentAccount", params: { id: "a1", accountId: "a1", harness: "codex" } },
    ]);
  });

  it("runs the login flow verbs with loginId/code params", async () => {
    const { client, calls } = fakeClient({ loginId: "l1", url: "https://example.com", mode: "paste-code" });
    await startAgentLogin(client, "claude-code");
    await completeAgentLogin(client, "l1", "code-123");
    await cancelAgentLogin(client, "l1");
    expect(calls.map((call) => call.method)).toEqual(["StartAgentLogin", "CompleteAgentLogin", "CancelAgentLogin"]);
    expect(calls[0]!.params).toEqual({ harness: "claude-code" });
    expect(calls[1]!.params).toEqual({ loginId: "l1", code: "code-123" });
    expect(calls[2]!.params).toEqual({ loginId: "l1" });
  });
});
