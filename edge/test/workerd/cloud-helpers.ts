import { SELF } from "cloudflare:test";
import { expect } from "vitest";
import { b64urlEncode, runnerTokenMessage } from "../../src/cloud/crypto";

export interface TestUser {
  readonly userId: string;
  readonly orgId: string;
}

export const newUser = (prefix = "u"): TestUser => ({
  userId: `${prefix}-${crypto.randomUUID().slice(0, 8)}`,
  orgId: "org1"
});

export const userBearer = (u: TestUser) => `Bearer ${u.userId}@${u.orgId}`;

export const call = (
  method: string,
  path: string,
  bearer?: string,
  body?: unknown,
  headers: Record<string, string> = {}
) =>
  SELF.fetch(`https://edge.test${path}`, {
    method,
    headers: {
      ...(bearer ? { authorization: bearer } : {}),
      ...(body === undefined ? {} : { "content-type": "application/json" }),
      ...headers
    },
    body: body === undefined ? undefined : JSON.stringify(body)
  });

export interface FakeState {
  sandboxes: {
    id: string;
    state: string;
    env: Record<string, string>;
    ttlSeconds: number | null;
    deleted: boolean;
    createdAt: string;
  }[];
  calls: {
    method: string;
    path: string;
    sandboxId?: string;
    query?: Record<string, string>;
    body?: Record<string, unknown>;
    headers: Record<string, string>;
  }[];
}

export const boatState = async (): Promise<FakeState> => (await fetch("https://boat.test/__state")).json();

export const boatControl = (path: "__fail" | "__set" | "__extra", body: unknown) =>
  fetch(`https://boat.test/${path}`, { method: "POST", body: JSON.stringify(body) });

export interface AccountStatus {
  state: string;
  deviceId?: string;
  error?: string;
  failedAction?: string;
  awakeSessions: number;
  maxAwakeSessions: number;
  available: boolean;
}

export interface SessionView {
  chatId: string;
  deviceId: string;
  spaceId: string;
  repo: string;
  path: string;
  state: string;
  error?: string;
  failedAction?: string;
  sandbox?: { provider: string; id: string };
}

export const status = async (u: TestUser) =>
  (await call("GET", `/cloud/${u.orgId}`, userBearer(u))).json() as Promise<AccountStatus>;

export const sessions = async (u: TestUser) =>
  ((await (await call("GET", `/cloud/${u.orgId}/sessions`, userBearer(u))).json()) as { sessions: SessionView[] }).sessions;

export const session = async (u: TestUser, chatId: string) => (await sessions(u)).find((s) => s.chatId === chatId);

export const waitForState = async (u: TestUser, state: string, timeout = 15_000) => {
  await expect.poll(async () => (await status(u)).state, { timeout, interval: 50 }).toBe(state);
  return status(u);
};

/** Poll one session's state (`gone` = the session no longer exists). */
export const waitForSession = async (u: TestUser, chatId: string, state: string, timeout = 15_000) => {
  await expect
    .poll(async () => (await session(u, chatId))?.state ?? "gone", { timeout, interval: 50 })
    .toBe(state);
  return session(u, chatId);
};

/** The GitHub repository the test project's origin names. */
export const REPO = { fullName: "acme/app", cloneUrl: "https://github.com/acme/app.git", defaultBranch: "main" };

/** Turn Cloud on; returns the logical device id and a project (a laptop's
 * space) whose chats run on Cloud. */
export const enableCloud = async (u: TestUser, spaceId = `sp-${crypto.randomUUID().slice(0, 8)}`) => {
  const enabled = await call("POST", `/cloud/${u.orgId}/enable`, userBearer(u), {});
  expect(enabled.status, await enabled.clone().text()).toBe(200);
  const { deviceId: accountDeviceId } = (await enabled.json()) as { deviceId: string };
  return { accountDeviceId, spaceId };
};

export const createSession = (
  u: TestUser,
  chatId: string,
  spaceId: string,
  branch?: string,
  repo: Record<string, unknown> = REPO
) =>
  call("POST", `/cloud/${u.orgId}/sessions`, userBearer(u), {
    chatId,
    spaceId,
    repo,
    ...(branch !== undefined ? { branch } : {})
  });

/** The sandbox the fake created for this device (env carries the enrollment). */
export const sandboxFor = async (deviceId: string) => {
  let found: FakeState["sandboxes"][number] | undefined;
  await expect
    .poll(
      async () => {
        found = (await boatState()).sandboxes.find((s) => s.env.ZERON_RUNNER_ENROLL?.includes(`.${deviceId}.`));
        return found !== undefined;
      },
      { timeout: 10_000, interval: 50 }
    )
    .toBe(true);
  return found!;
};

export const enrollCodeFor = async (deviceId: string) =>
  (await sandboxFor(deviceId)).env.ZERON_RUNNER_ENROLL!.split(".").at(-1)!;

export interface RunnerKey {
  readonly publicKey: string;
  sign(message: string): Promise<string>;
}

export const runnerKey = async (): Promise<RunnerKey> => {
  const pair = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
  const raw = (await crypto.subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer;
  return {
    publicKey: b64urlEncode(new Uint8Array(raw)),
    sign: async (message) =>
      b64urlEncode(new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, pair.privateKey, new TextEncoder().encode(message))))
  };
};

export const enroll = (u: TestUser, deviceId: string, code: string, publicKey: string) =>
  call("POST", "/runner/enroll", undefined, { ...u, deviceId, code, publicKey });

export const requestToken = async (u: TestUser, deviceId: string, key: RunnerKey, ts = Date.now()) =>
  call("POST", "/runner/token", undefined, {
    ...u,
    deviceId,
    ts,
    sig: await key.sign(runnerTokenMessage(u.orgId, u.userId, deviceId, ts))
  });

/** A chat on a Cloud project → its own sandbox (against the fake) → enroll →
 * ready. Enables Cloud and adds a project first unless `project` is given. */
export const readySession = async (
  u: TestUser,
  chatId = `chat-${crypto.randomUUID().slice(0, 8)}`,
  project?: { accountDeviceId: string; spaceId: string }
) => {
  const { accountDeviceId, spaceId } = project ?? (await enableCloud(u));
  const created = await createSession(u, chatId, spaceId);
  expect(created.status, await created.clone().text()).toBe(200);
  const { deviceId } = (await created.json()) as SessionView;
  const code = await enrollCodeFor(deviceId);
  const key = await runnerKey();
  const enrolled = await enroll(u, deviceId, code, key.publicKey);
  expect(enrolled.status, await enrolled.clone().text()).toBe(200);
  const ready = (await waitForSession(u, chatId, "ready"))!;
  return { chatId, spaceId, accountDeviceId, deviceId, code, key, sandboxId: ready.sandbox!.id };
};

export const runnerToken = async (u: TestUser, deviceId: string, key: RunnerKey) => {
  const reply = await requestToken(u, deviceId, key);
  expect(reply.status, await reply.clone().text()).toBe(200);
  return ((await reply.json()) as { accessToken: string }).accessToken;
};
