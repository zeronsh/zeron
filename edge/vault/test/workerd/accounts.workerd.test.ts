import { env } from "cloudflare:workers";
import { runInDurableObject } from "cloudflare:test";
import { afterEach, describe, expect, it, vi } from "vitest";
import { accountStub } from "../../src/names";
import { CLAUDE_USAGE_URL, CODEX_USAGE_URL } from "../../src/providers/usage";
import { fakeJwt } from "../support";
import { DAY, codexMaterial, json, mockUpstream, runnerCaller, sec, setup, signedGrant, value, vault } from "./helpers";

afterEach(() => vi.restoreAllMocks());

/** A `codex login` for `email`, valid for days. */
const codexLogin = (email: string) => ({
  id_token: fakeJwt({ email, nonce: crypto.randomUUID() }),
  access_token: fakeJwt({ exp: sec(Date.now() + 5 * DAY), jti: crypto.randomUUID() }),
  refresh_token: `rt-${crypto.randomUUID()}`,
  account_id: `acct-${email}`
});

const putCodex = async (ctx: Awaited<ReturnType<typeof setup>>, tokens: ReturnType<typeof codexLogin>) =>
  value(
    await vault().putCredential(ctx.caller, "codex", {
      material: codexMaterial(tokens),
      authorizedDevices: [ctx.device.deviceId]
    })
  );

const grantedToken = async (ctx: Awaited<ReturnType<typeof setup>>) =>
  vault().grant(ctx.caller, await signedGrant(ctx.device, ctx.userId, "codex"));

describe("multiple accounts per provider", () => {
  it("keeps each login as its own account; a new sign-in becomes active", async () => {
    const ctx = await setup();
    const ada = codexLogin("ada@example.com");
    const bob = codexLogin("bob@example.com");
    await putCodex(ctx, ada);
    await putCodex(ctx, bob);

    const accounts = value(await vault().accounts(ctx.caller, "codex"));
    expect(accounts.map((a) => [a.account, a.active, a.kind])).toEqual([
      ["ada@example.com", false, "oauth"],
      ["bob@example.com", true, "oauth"]
    ]);
    expect(value(await grantedToken(ctx)).accessToken).toBe(bob.access_token);

    // The connection summary counts them and reads from the active one.
    const status = value(await vault().status(ctx.caller));
    expect(status.connections.find((c) => c.provider === "codex")).toMatchObject({
      account: "bob@example.com",
      accounts: 2,
      hasActive: true
    });
  });

  it("the same login signed in again replaces its own account, not a new one", async () => {
    const ctx = await setup();
    await putCodex(ctx, codexLogin("ada@example.com"));
    await putCodex(ctx, codexLogin("bob@example.com"));
    const again = codexLogin("ada@example.com");
    await putCodex(ctx, again);

    const accounts = value(await vault().accounts(ctx.caller, "codex"));
    expect(accounts).toHaveLength(2);
    expect(accounts.find((a) => a.active)?.account).toBe("ada@example.com");
    const grant = value(await grantedToken(ctx));
    expect(grant.accessToken).toBe(again.access_token);
    // Generations stay unique across accounts.
    expect(grant.generation).toBe(3);
  });

  it("switching picks the account grants come from; none active issues none", async () => {
    const ctx = await setup();
    const ada = codexLogin("ada@example.com");
    await putCodex(ctx, ada);
    await putCodex(ctx, codexLogin("bob@example.com"));
    const adaSlot = value(await vault().accounts(ctx.caller, "codex")).find((a) => a.account === "ada@example.com")!.slot;

    const switched = value(await vault().activateAccount(ctx.caller, "codex", adaSlot));
    expect(switched.find((a) => a.active)?.slot).toBe(adaSlot);
    expect(value(await grantedToken(ctx)).accessToken).toBe(ada.access_token);

    value(await vault().activateAccount(ctx.caller, "codex", null));
    const none = await grantedToken(ctx);
    expect(!none.ok && none.message).toBe("no codex account is active");

    const missing = await vault().activateAccount(ctx.caller, "codex", "a000000000");
    expect(!missing.ok && missing.error).toBe("not_found");
  });

  it("forgetting the active account leaves none active; the others stay", async () => {
    const ctx = await setup();
    await putCodex(ctx, codexLogin("ada@example.com"));
    await putCodex(ctx, codexLogin("bob@example.com"));
    const bobSlot = value(await vault().accounts(ctx.caller, "codex")).find((a) => a.active)!.slot;

    const left = value(await vault().forgetAccount(ctx.caller, "codex", bobSlot));
    expect(left.map((a) => [a.account, a.active])).toEqual([["ada@example.com", false]]);
    // Idempotent.
    expect(value(await vault().forgetAccount(ctx.caller, "codex", bobSlot))).toHaveLength(1);
    const none = await grantedToken(ctx);
    expect(!none.ok && none.error).toBe("not_found");
  });

  it("API keys dedupe by key, never by anything secret in the listing", async () => {
    const ctx = await setup();
    const put = async (key: string) =>
      value(
        await vault().putCredential(ctx.caller, "anthropic-key", {
          material: { key },
          authorizedDevices: [ctx.device.deviceId]
        })
      );
    await put("sk-ant-api03-first-0123456789");
    await put("sk-ant-api03-second-0123456789");
    await put("sk-ant-api03-first-0123456789");
    const accounts = value(await vault().accounts(ctx.caller, "anthropic-key"));
    expect(accounts).toHaveLength(2);
    expect(accounts.every((a) => a.kind === "api-key")).toBe(true);
    expect(JSON.stringify(accounts)).not.toContain("sk-ant-api03");
  });

  it("usage is read by the vault for one login; a rejection never marks it", async () => {
    const ctx = await setup();
    const ada = codexLogin("ada@example.com");
    await putCodex(ctx, ada);
    const slot = value(await vault().accounts(ctx.caller, "codex"))[0]!.slot;

    // Built per call: a Response made outside the account object can't be read in it.
    let answer = () => json({ plan_type: "pro", rate_limit: { primary_window: { used_percent: 12 } } });
    const upstream = mockUpstream({ [CODEX_USAGE_URL]: () => answer() });
    const usage = value(await vault().accountUsage(ctx.caller, "codex", slot));
    expect(usage.body).toMatchObject({ plan_type: "pro" });
    const call = upstream.calls[0]!;
    expect(new Headers(call.init?.headers).get("authorization")).toBe(`Bearer ${ada.access_token}`);
    expect(new Headers(call.init?.headers).get("chatgpt-account-id")).toBe("acct-ada@example.com");

    answer = () => json({ error: "nope" }, 403);
    const refused = await vault().accountUsage(ctx.caller, "codex", slot);
    expect(!refused.ok && refused.error).toBe("upstream");
    expect(value(await vault().accounts(ctx.caller, "codex"))[0]!.status).toBe("connected");

    const keyless = await vault().accountUsage(ctx.caller, "anthropic-key", slot);
    expect(!keyless.ok && keyless.error).toBe("not_found");
    expect(CLAUDE_USAGE_URL).toBe("https://api.anthropic.com/api/oauth/usage");
  });

  it("only users manage accounts", async () => {
    const ctx = await setup();
    const runner = runnerCaller(ctx.userId, ctx.device.deviceId);
    for (const result of [
      await vault().accounts(runner, "codex"),
      await vault().activateAccount(runner, "codex", null),
      await vault().forgetAccount(runner, "codex", "a000000000"),
      await vault().accountUsage(runner, "codex", "a000000000")
    ]) {
      expect(!result.ok && result.error).toBe("forbidden");
    }
  });

  it("the single-record layout migrates into one active account that still opens", async () => {
    const ctx = await setup();
    const ada = codexLogin("ada@example.com");
    await putCodex(ctx, ada);
    // Rewind the object to the old layout: one `record` row, no accounts.
    await runInDurableObject(accountStub(env, ctx.userId, "codex"), (instance, state) => {
      const sql = state.storage.sql;
      const row = sql.exec<Record<string, SqlStorageValue>>("SELECT * FROM accounts").toArray()[0]!;
      sql.exec(
        `CREATE TABLE record (id INTEGER PRIMARY KEY CHECK (id = 1), generation INTEGER NOT NULL, status TEXT NOT NULL,
           account TEXT, authorized_devices TEXT NOT NULL, envelope TEXT NOT NULL, updated_at INTEGER NOT NULL,
           hard_expires_at INTEGER, last_refresh_at INTEGER, last_attempt_at INTEGER, last_error TEXT,
           failures INTEGER NOT NULL DEFAULT 0)`
      );
      sql.exec(
        "INSERT INTO record (id, generation, status, account, authorized_devices, envelope, updated_at) VALUES (1, ?, ?, ?, ?, ?, ?)",
        row.generation,
        row.status,
        row.account,
        row.authorized_devices,
        row.envelope,
        row.updated_at
      );
      sql.exec("DELETE FROM accounts");
      sql.exec("DELETE FROM meta WHERE key = 'active'");
      (instance as unknown as { migrateSingleRecord(): void }).migrateSingleRecord();
      expect(sql.exec("SELECT name FROM sqlite_master WHERE name = 'record'").toArray()).toHaveLength(0);
    });

    const accounts = value(await vault().accounts(ctx.caller, "codex"));
    expect(accounts.map((a) => [a.account, a.active])).toEqual([["ada@example.com", true]]);
    // Same generation and envelope, so the same AAD: it still opens.
    const grant = value(await grantedToken(ctx));
    expect(grant).toMatchObject({ accessToken: ada.access_token, generation: 1 });
  });
});
