/**
 * Models a Cloud machine discovered with Cloud's credentials
 * (`POST /runner/models`), kept per account and served by
 * `GET /cloud/{orgId}/models` while every machine sleeps.
 */
import { describe, expect, it } from "vitest";
import { MAX_PUBLISHED_MODELS, parsePublishedModels } from "../../src/cloud/models";
import { call, newUser, readySession, runnerToken, userBearer } from "./cloud-helpers";

type Listed = Record<string, { id: string; label: string; reasoningLevels?: string[]; description?: string }[]>;

const listed = async (bearer: string): Promise<Listed> =>
  ((await (await call("GET", "/cloud/org1/models", bearer)).json()) as { models: Listed }).models;

const piModels = [
  { id: "openai-codex/gpt-6.1-sol", label: "GPT-6.1-Sol", provider: { id: "openai-codex", name: "ChatGPT" }, reasoningLevels: ["low", "high", "someday"], options: [] },
  { id: "anthropic/claude-opus-5-5", label: "Opus 5.5", reasoningLevels: [], options: [], default: true }
];

describe("published catalogs", () => {
  it("a machine's catalog is served for its agent beside the hand-kept ones, to its own user only", async () => {
    const u = newUser();
    const { deviceId, key } = await readySession(u);
    const runner = `Bearer ${await runnerToken(u, deviceId, key)}`;

    expect(Object.keys(await listed(userBearer(u))).sort()).toEqual(["claude-code", "codex"]);
    const publish = (bearer: string, body: unknown) => call("POST", "/runner/models", bearer, body);
    expect((await publish(runner, { harness: "pi", models: piModels, credential: "codex:3,openrouter-key:4" })).status).toBe(200);

    const models = await listed(userBearer(u));
    expect(Object.keys(models).sort()).toEqual(["claude-code", "codex", "pi"]);
    // A reasoning level an older client couldn't parse is dropped, not the catalog.
    expect(models.pi?.map((m) => [m.id, m.reasoningLevels])).toEqual([
      ["openai-codex/gpt-6.1-sol", ["low", "high"]],
      ["anthropic/claude-opus-5-5", []]
    ]);
    // Another user sees none of it.
    expect(Object.keys(await listed(userBearer(newUser())))).toEqual(["claude-code", "codex"]);

    // The latest publish wins.
    expect((await publish(runner, { harness: "pi", models: [piModels[1]], credential: "codex:5" })).status).toBe(200);
    expect((await listed(userBearer(u))).pi?.map((m) => m.id)).toEqual(["anthropic/claude-opus-5-5"]);
  });

  it("runners only, for an agent Cloud runs, with models that parse; the hand-kept lists stay", async () => {
    const u = newUser();
    const { deviceId, key } = await readySession(u);
    const runner = `Bearer ${await runnerToken(u, deviceId, key)}`;
    const ok = { harness: "hermes", models: piModels, credential: "anthropic-key:1" };
    expect((await call("POST", "/runner/models", userBearer(u), ok)).status).toBe(403);
    expect((await call("POST", "/runner/models", runner, { ...ok, harness: "acp.gemini" })).status).toBe(400);
    expect((await call("POST", "/runner/models", runner, { ...ok, credential: "" })).status).toBe(400);
    expect((await call("POST", "/runner/models", runner, { ...ok, models: [{ label: "no id" }] })).status).toBe(400);
    // A machine can't replace the hand-kept lists.
    expect((await call("POST", "/runner/models", runner, { ...ok, harness: "codex" })).status).toBe(200);
    const models = await listed(userBearer(u));
    expect(models.codex?.some((m) => m.id === "openai-codex/gpt-6.1-sol")).toBe(false);
    expect(models.hermes).toBeUndefined();
  });
});

type Catalogs = Record<string, { accounts: string; models: { id: string }[]; at: number }[]>;

const catalogs = async (bearer: string): Promise<Catalogs> =>
  ((await (await call("GET", "/cloud/org1/models", bearer)).json()) as { catalogs: Catalogs }).catalogs;

describe("catalogs per account", () => {
  it("each set of accounts keeps its own catalog, the newest first, at most eight an agent", async () => {
    const u = newUser();
    const { deviceId, key } = await readySession(u);
    const runner = `Bearer ${await runnerToken(u, deviceId, key)}`;
    const publish = (body: unknown) => call("POST", "/runner/models", runner, body);
    const grok = (id: string) => [{ id, label: id, reasoningLevels: [], options: [] }];
    // A free account's one model, then a paid one's four: both kept, by account.
    expect((await publish({ harness: "grok", models: grok("grok-4.7"), credential: "grok@free:1", accounts: "grok@free" })).status).toBe(200);
    expect((await publish({ harness: "grok", models: grok("grok-4.7-build-fast"), credential: "grok@paid:2", accounts: "grok@paid" })).status).toBe(200);
    const kept = (await catalogs(userBearer(u))).grok;
    expect(kept?.map((c) => [c.accounts, c.models.map((m) => m.id)])).toEqual([
      ["grok@paid", ["grok-4.7-build-fast"]],
      ["grok@free", ["grok-4.7"]]
    ]);
    // The newest is still what an older client reads.
    expect((await listed(userBearer(u))).grok?.map((m) => m.id)).toEqual(["grok-4.7-build-fast"]);
    // A refresh of one account's catalog replaces it in place.
    expect((await publish({ harness: "grok", models: grok("grok-4.8"), credential: "grok@free:3", accounts: "grok@free" })).status).toBe(200);
    expect((await catalogs(userBearer(u))).grok?.map((c) => [c.accounts, c.models[0]?.id])).toEqual([
      ["grok@free", "grok-4.8"],
      ["grok@paid", "grok-4.7-build-fast"]
    ]);
    for (let i = 0; i < 10; i++) {
      expect((await publish({ harness: "grok", models: grok(`m${i}`), credential: `grok@s${i}:1`, accounts: `grok@s${i}` })).status).toBe(200);
    }
    const bounded = (await catalogs(userBearer(u))).grok ?? [];
    expect(bounded.length).toBe(8);
    expect(bounded[0]?.accounts).toBe("grok@s9");
    // Another user sees none of it.
    expect(await catalogs(userBearer(newUser()))).toEqual({});
  });

  it("the account's own device publishes what it discovered signing an account in; accounts are well-formed", async () => {
    const u = newUser();
    await readySession(u);
    const body = { harness: "antigravity", models: [{ id: "gemini-4-neon", label: "Gemini 4 Neon", reasoningLevels: ["low", "xhigh"], options: [] }], accounts: "antigravity@g1" };
    expect((await call("POST", "/cloud/org1/models", userBearer(u), body)).status).toBe(200);
    expect((await catalogs(userBearer(u))).antigravity?.map((c) => c.accounts)).toEqual(["antigravity@g1"]);
    // With no machine's yet, it's also the newest.
    expect((await listed(userBearer(u))).antigravity?.map((m) => m.id)).toEqual(["gemini-4-neon"]);
    for (const accounts of ["", "antigravity", "b@x,a@y", "antigravity@g 1", 7]) {
      expect((await call("POST", "/cloud/org1/models", userBearer(u), { ...body, accounts })).status, String(accounts)).toBe(400);
    }
    expect((await call("POST", "/cloud/org1/models", userBearer(u), { ...body, harness: "acp.gemini" })).status).toBe(400);
    const { deviceId, key } = await readySession(u);
    const runner = `Bearer ${await runnerToken(u, deviceId, key)}`;
    expect((await call("POST", "/runner/models", runner, { ...body, credential: "x:1", accounts: "Bad@" })).status).toBe(400);
  });
});

describe("parsePublishedModels", () => {
  it("keeps what parses, distinct, bounded in count and size", () => {
    const many = Array.from({ length: MAX_PUBLISHED_MODELS + 50 }, (_, i) => ({
      id: `m-${i}`,
      label: `Model ${i}`,
      description: "d".repeat(400),
      reasoningLevels: [],
      options: []
    }));
    const kept = parsePublishedModels([...many, { id: "m-1", label: "dup" }])!;
    expect(kept.length).toBeLessThanOrEqual(MAX_PUBLISHED_MODELS);
    expect(JSON.stringify(kept).length).toBeLessThanOrEqual(100 * 1024);
    // Too big with descriptions: descriptions go before models do.
    expect(kept.every((m) => m.description === undefined)).toBe(true);
    expect(kept.length).toBe(MAX_PUBLISHED_MODELS);
    expect(parsePublishedModels([{ id: "x", label: "X", options: [{ id: "o", label: "O", choices: [{ id: 1 }], defaultChoice: "a" }] }]))
      .toEqual([{ id: "x", label: "X", reasoningLevels: [], options: [] }]);
    expect(parsePublishedModels([])).toBeUndefined();
    expect(parsePublishedModels("nope")).toBeUndefined();
  });
});
