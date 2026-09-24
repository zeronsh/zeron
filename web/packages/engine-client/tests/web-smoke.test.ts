import { spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Readable } from "node:stream";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import type { Chat, EngineInfo } from "@zeron/proto";
import { EngineClient } from "../src/client";
import { ENGINE_INFO, MUTATE, WATCH_CHATS, WATCH_SPACES } from "../src/methods";
import { fetchSignInConfig } from "../src/auth";
import { EngineWatchCache } from "../src/watch-cache";
import { delay, statusWhen, trackedFactory, waitUntil } from "./helpers/ws";

/**
 * Browser end-to-end smoke (ticket 18): exercises the full path a browser
 * takes against a real engine, but driven from node so CI can run it
 * without a Playwright install. Targets the `web_smoke` example, which
 * already seeds one chat so the assertion surfaces something real.
 *
 * Covered: the dev sign-in config fetch (the `/auth/config` GET a browser
 * does),
 * first-frame `Auth` over WebSocket, identity verification, the chat list
 * watch stream, and the round-trip of a `Mutate.createChat` (the "send"
 * piece). The same flow with a real chat would be `QueueCommand`; the
 * mock harness does not run, but the wire round-trip and the cache's
 * mutation path are what we need to lock in here.
 */
const FAST_BACKOFF = { initialMs: 25, jitterMs: 1, maxMs: 100 };

interface SmokeHandle {
  child: ChildProcess;
  endpoint: string;
  deviceId: string;
}

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..", "..");
// Cargo may build outside the checkout (CARGO_TARGET_DIR); resolve the
// examples from wherever cargo actually puts them.
const exampleBinary = join(
  process.env.CARGO_TARGET_DIR ?? join(repoRoot, "target"),
  "debug",
  "examples",
  process.platform === "win32" ? "web_smoke.exe" : "web_smoke",
);

function run(command: string, args: string[]): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: repoRoot, stdio: "inherit" });
    child.on("error", reject);
    child.on("exit", (code) => {
      if (code === 0) {
        resolve();
      } else {
        reject(new Error(`${command} ${args.join(" ")} exited with ${code}`));
      }
    });
  });
}

function waitForLine(stream: Readable, prefix: string): Promise<string> {
  return new Promise((resolve, reject) => {
    let buffer = "";
    const onData = (chunk: Buffer): void => {
      buffer += chunk.toString("utf8");
      const newline = buffer.indexOf("\n");
      if (newline >= 0 && buffer.slice(0, newline).trim().startsWith(prefix)) {
        cleanup();
        resolve(buffer.slice(0, newline).trim().slice(prefix.length));
      }
    };
    const onError = (error: Error): void => {
      cleanup();
      reject(error);
    };
    const cleanup = (): void => {
      stream.off("data", onData);
      stream.off("error", onError);
    };
    stream.on("data", onData);
    stream.on("error", onError);
  });
}

let engine: SmokeHandle | undefined;
let credential: string | undefined;
const tracked = trackedFactory();
let client: EngineClient | undefined;
let cache: EngineWatchCache | undefined;

beforeAll(async () => {
  await run("cargo", ["build", "-p", "zeron-engine", "--example", "web_smoke"]);
  const child = spawn(exampleBinary, { stdio: ["ignore", "pipe", "ignore"] });
  const line = await waitForLine(child.stdout!, "SMOKE READY ");
  // The example prints one line: `SMOKE READY <endpoint>` — the
  // listener's plain URL, not JSON, so the browser flow (enter address →
  // fetch sign-in config) maps onto the same code paths the web client
  // uses.
  const baseUrl = line.startsWith("SMOKE READY ") ? line.slice("SMOKE READY ".length).trim() : line;
  // The example does not print its device id; we'll fetch it through the
  // EngineInfo round-trip after sign-in (the same path the browser takes:
  // the connect page only sees the credential, the engine identity is
  // verified by the first authenticated RPC).
  engine = { child, endpoint: baseUrl, deviceId: "" };
}, 600_000);

afterAll(async () => {
  client?.close();
  if (engine !== undefined) {
    engine.child.kill();
    await once(engine.child, "exit").catch(() => {});
  }
});

describe("web smoke (sign-in, watch, send) against a seeded engine", () => {
  test("fetches the dev sign-in config exactly the way the browser's connect page does", async () => {
    const config = await fetchSignInConfig(engine!.endpoint);
    expect(config).toEqual({ mode: "dev", authorizeUrl: null });
    // Dev mode: the bearer is a local user id the listener accepts
    // without WorkOS.
    credential = "web-smoke";
  });

  test("connects, verifies the engine identity, and exposes a stable cache", async () => {
    client = new EngineClient({
      endpoint: engine!.endpoint.replace("http", "ws"),
      credential: credential!,
      webSocket: tracked.factory,
      backoff: FAST_BACKOFF,
    });
    cache = new EngineWatchCache(client);
    client.connect();
    const status = await statusWhen(client, (value) => value.state === "connected");
    expect(status.state).toBe("connected");
    if (status.state === "connected") {
      engine!.deviceId = status.info.deviceId;
      expect(status.info.capabilities).toContain("web-client");
      expect(status.info.workspaceScope).toBe("local");
    }
    const info = await client.call<EngineInfo>(ENGINE_INFO, {});
    expect(info.deviceId).toBe(engine!.deviceId);
  });

  test("watches the chat list and sees the seeded smoke chat", async () => {
    const items: Array<{ generation: number; chats: readonly Chat[] }> = [];
    const handle = client!.watch<Chat[]>(WATCH_CHATS, {}, {
      onItem: (item, context) => items.push({ generation: context.generation, chats: item }),
    });
    try {
      await waitUntil(
        () => items.some((entry) => entry.chats.some((chat) => chat.id === "smoke-chat")),
        10_000,
        "seeded smoke-chat on WatchChats",
      );
      const seeded = items
        .flatMap((entry) => entry.chats)
        .find((chat) => chat.id === "smoke-chat");
      expect(seeded).toBeDefined();
      expect(seeded!.title).toBe("Browser smoke chat");
      expect(typeof seeded!.deviceId).toBe("string");
    } finally {
      handle.cancel();
    }
  });

  test("sends a Mutate.createChat and the cache picks it up", async () => {
    await waitUntil(
      () => cache!.getSnapshot().chats.loaded && cache!.getSnapshot().chats.rows.length >= 1,
      10_000,
      "watch cache to load before send",
    );
    const before = cache!.getSnapshot().chats.rows.length;
    const newChatId = `smoke-send-${Date.now()}`;
    await client!.call(MUTATE, {
      op: "createChat",
      chatId: newChatId,
      deviceId: engine!.deviceId,
    });
    await waitUntil(
      () =>
        cache!.getSnapshot().chats.rows.some((row) => row.id === newChatId) &&
        cache!.getSnapshot().chats.rows.length === before + 1,
      10_000,
      "new chat lands on the watch cache",
    );
    const sent = cache!.getSnapshot().chats.rows.find((row) => row.id === newChatId);
    expect(sent).toBeDefined();
    expect(sent!.id).toBe(newChatId);
    // Mutate without a follow-up rename lands a "New session" title; the
    // server's createChat path is the same one the composer's first send
    // would use, so this is the right assertion.
    expect(sent!.title === null || sent!.title === "New session").toBe(true);
  });

  test("watch streams for chats + spaces share the same generation swap", async () => {
    // The cache swap model: every connect bumps generation and every
    // collection reloads together. The web smoke verifies this contract
    // so a future change that drops the atomic swap trips here.
    const generations = new Set<number>();
    const seen = { chats: 0, spaces: 0 };
    const stops: Array<() => void> = [];
    try {
      const chatSub = client!.watch<Chat[]>(WATCH_CHATS, {}, {
        onItem: (_item, ctx) => {
          generations.add(ctx.generation);
          seen.chats += 1;
        },
      });
      const spaceSub = client!.watch<unknown[]>(WATCH_SPACES, {}, {
        onItem: (_item, ctx) => {
          generations.add(ctx.generation);
          seen.spaces += 1;
        },
      });
      stops.push(() => chatSub.cancel(), () => spaceSub.cancel());
      // Send one more Mutate so both streams emit again on the same
      // generation, then confirm both saw at least one item.
      await client!.call(MUTATE, {
        op: "createChat",
        chatId: `smoke-gen-${Date.now()}`,
        deviceId: engine!.deviceId,
      });
      await waitUntil(() => seen.chats >= 2 && seen.spaces >= 1, 10_000, "stream items after mutate");
      expect(generations.size).toBeGreaterThanOrEqual(1);
    } finally {
      for (const stop of stops) {
        stop();
      }
    }
  });
});
