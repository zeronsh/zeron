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
