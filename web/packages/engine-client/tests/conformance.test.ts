import { spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Readable } from "node:stream";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import type { Chat, EngineInfo } from "@zeron/proto";
import { EngineClient } from "../src/client";
import { ENGINE_INFO, MUTATE, WATCH_CHATS } from "../src/methods";
import { fetchSignInConfig } from "../src/auth";
import { EngineWatchCache } from "../src/watch-cache";
import { delay, statusWhen, trackedFactory, waitUntil } from "./helpers/ws";

const FAST_BACKOFF = { initialMs: 25, jitterMs: 1, maxMs: 100 };

interface EngineHandle {
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
  process.platform === "win32" ? "web_conformance.exe" : "web_conformance",
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

let engine: EngineHandle | undefined;
let credential: string | undefined;
const tracked = trackedFactory();
let client: EngineClient | undefined;

beforeAll(async () => {
  await run("cargo", ["build", "-p", "zeron-engine", "--example", "web_conformance"]);
  const child = spawn(exampleBinary, { stdio: ["ignore", "pipe", "ignore"] });
  const line = await waitForLine(child.stdout!, "CONFORMANCE ");
  const info = JSON.parse(line) as { endpoint: string; deviceId: string };
  engine = { child, ...info };
}, 600_000);

afterAll(async () => {
  client?.close();
  if (engine !== undefined) {
    engine.child.kill();
    await once(engine.child, "exit").catch(() => {});
  }
});

describe("conformance against a real engine", () => {
  test("fetches the dev sign-in config over HTTP", async () => {
    const config = await fetchSignInConfig(engine!.endpoint);
    expect(config).toEqual({ mode: "dev", authorizeUrl: null });
    // Dev mode: the bearer is a local user id the listener accepts
    // without WorkOS.
    credential = "conformance";
  });

  test("connects with first-frame auth, verifies identity, and makes typed calls", async () => {
    client = new EngineClient({
      endpoint: engine!.endpoint.replace("http", "ws"),
      credential: credential!,
      expectedDeviceId: engine!.deviceId,
      webSocket: tracked.factory,
      backoff: FAST_BACKOFF,
    });
    client.connect();
    const status = await statusWhen(client, (value) => value.state === "connected");
    expect(status).toMatchObject({ state: "connected", info: { deviceId: engine!.deviceId } });
    expect(client.generation).toBe(1);
    expect(tracked.sockets).toHaveLength(1);

    const info = await client.call<EngineInfo>(ENGINE_INFO, {});
    expect(info.deviceId).toBe(engine!.deviceId);
    expect(info.workspaceScope).toBe("local");
    expect(info.capabilities).toContain("web-client");
  });

  test("watches the chat list stream", async () => {
    const items: Array<{ item: Chat[]; generation: number }> = [];
    client!.watch<Chat[]>(WATCH_CHATS, {}, {
      onItem: (item, context) => items.push({ item, generation: context.generation }),
    });
    await waitUntil(() => items.length >= 1, 10_000, "first WatchChats item");
    expect(items[0]!.generation).toBe(1);
    for (const chat of items[0]!.item) {
      expect(typeof chat.id).toBe("string");
      expect(typeof chat.deviceId).toBe("string");
      expect(typeof chat.archived).toBe("boolean");
      expect(typeof chat.createdAt).toBe("string");
    }
  });

  test("a hard drop reconnects, re-verifies identity, and resubscribes", async () => {
    tracked.sockets[0]!.terminate();
    await statusWhen(client!, (value) => value.state === "connected" && value.generation === 2);
    expect(tracked.sockets).toHaveLength(2);
    const info = await client!.call<EngineInfo>(ENGINE_INFO, {});
    expect(info.deviceId).toBe(engine!.deviceId);

    const items: Array<number> = [];
    const handle = client!.watch<Chat[]>(WATCH_CHATS, {}, {
      onItem: (_item, context) => items.push(context.generation),
    });
    await waitUntil(() => items.includes(2), 10_000, "resubscribed item on generation 2");
    handle.cancel();
  });
});

describe("conformance against a real engine: the watch cache", () => {
  test("populates every collection, applies mutations incrementally, and swaps on reconnect without ghost rows", async () => {
    const own = trackedFactory();
    const cacheClient = new EngineClient({
      endpoint: engine!.endpoint.replace("http", "ws"),
      credential: credential!,
      expectedDeviceId: engine!.deviceId,
      webSocket: own.factory,
      backoff: FAST_BACKOFF,
    });
    try {
      const cache = new EngineWatchCache(cacheClient);
      cacheClient.connect();
      await statusWhen(cacheClient, (value) => value.state === "connected");
      await waitUntil(
        () => {
          const snapshot = cache.getSnapshot();
          return (
            snapshot.chats.loaded &&
            snapshot.spaces.loaded &&
            snapshot.devices.loaded &&
            snapshot.statuses.loaded
          );
        },
        10_000,
        "every collection loaded",
      );
      const first = cache.getSnapshot();
      expect(first.generation).toBe(1);
      expect(first.capabilities).toContain("web-client");
      expect(cache.supports("web-client")).toBe(true);

      await cacheClient.call(MUTATE, { op: "createChat", chatId: "watch-cache-a", deviceId: engine!.deviceId });
      await cacheClient.call(MUTATE, { op: "createChat", chatId: "watch-cache-b", deviceId: engine!.deviceId });
      await waitUntil(
        () => cache.getSnapshot().chats.rows.length === 2,
        10_000,
        "created chats arrive",
      );
      const before = cache.getSnapshot().chats.rows;

      await cacheClient.call(MUTATE, { op: "renameChat", chatId: "watch-cache-b", title: "Renamed" });
      await waitUntil(
        () =>
          cache
            .getSnapshot()
            .chats.rows.some((row) => row.id === "watch-cache-b" && row.title === "Renamed"),
        10_000,
        "rename lands",
      );
      const after = cache.getSnapshot().chats.rows;
      expect(after.find((row) => row.id === "watch-cache-a")).toBe(
        before.find((row) => row.id === "watch-cache-a"),
      );

      // A drop mid-stream, then server-side changes while the client is
      // offline: chat-b is deleted, chat-c is created by another client.
      own.sockets[0]!.terminate();
      const other = trackedFactory();
      const second = new EngineClient({
        endpoint: engine!.endpoint.replace("http", "ws"),
        credential: credential!,
        expectedDeviceId: engine!.deviceId,
        webSocket: other.factory,
        backoff: FAST_BACKOFF,
      });
      second.connect();
      await statusWhen(second, (value) => value.state === "connected");
      await second.call(MUTATE, { op: "deleteChat", chatId: "watch-cache-b" });
      await second.call(MUTATE, { op: "createChat", chatId: "watch-cache-c", deviceId: engine!.deviceId });
      second.close();

      await statusWhen(cacheClient, (value) => value.state === "connected" && value.generation === 2);
      await waitUntil(
        () => {
          const snapshot = cache.getSnapshot();
          // statuses must be part of the wait: the assertions below read it
          // immediately after, and the collections refill independently —
          // chats landing first raced the statuses watch (the intermittent
          // CI failure at the swapped.statuses.loaded assertion).
          return (
            snapshot.generation === 2 &&
            snapshot.chats.loaded &&
            snapshot.statuses.loaded &&
            snapshot.chats.rows.length === 2
          );
        },
        10_000,
        "cache refilled on generation 2",
      );
      const swapped = cache.getSnapshot();
      expect(swapped.chats.rows.map((row) => row.id).sort()).toEqual(["watch-cache-a", "watch-cache-c"]);
      expect(swapped.statuses.loaded).toBe(true);
      expect(swapped.capabilities).toContain("web-client");
      expect(swapped.chats.rows.some((row) => row.id === "watch-cache-b")).toBe(false);
    } finally {
      cacheClient.close();
    }
  }, 60_000);
});
