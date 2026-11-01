// Per-run Zeron delegation bridge. No global settings or packages are modified.
// Tools are mcp-tools.json (zeron mcp's tools/list). The server starts on the first call.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { readFileSync } from "node:fs";

const CATALOG = JSON.parse(readFileSync(new URL("./mcp-tools.json", import.meta.url), "utf8"));

export default function (pi) {
  const config = JSON.parse(process.env.ZERON_PI_MCP);
  let child, lines, connecting, nextId = 0, active = false, ready = false;
  const pending = new Map();
  const chatId = config.env?.ZERON_CHAT_ID ?? process.env.ZERON_CHAT_ID;
  const inChat = typeof chatId === "string" && chatId.trim() !== "";
  const omitted = new Set(CATALOG.omitWithoutChat);
  function fail(error) {
    for (const request of [...pending.values()]) request.reject(error);
  }
  function close() {
    const owned = child;
    const reader = lines;
    child = undefined;
    lines = undefined;
    connecting = undefined;
    ready = false;
    fail(new Error("Zeron MCP connection closed"));
    reader?.close();
    owned?.kill();
  }
  function rpc(method, params, signal) {
    if (!child?.stdin.writable) return Promise.reject(new Error("Zeron MCP is not connected"));
    if (signal?.aborted) return Promise.reject(new Error("Zeron MCP request cancelled"));
    return new Promise((resolve, reject) => {
      const id = ++nextId;
      const cleanup = () => {
        clearTimeout(timer);
        signal?.removeEventListener("abort", abort);
        pending.delete(id);
      };
      const abort = () => {
        child?.stdin.write(JSON.stringify({jsonrpc: "2.0", method: "notifications/cancelled", params: {requestId: id}}) + "\n");
        cleanup();
        reject(new Error("Zeron MCP request cancelled"));
      };
      // wait_for_turn can legitimately wait several minutes.
      const timer = setTimeout(() => {
        cleanup();
        reject(new Error(`Zeron MCP ${method} timed out`));
      }, method === "tools/call" ? 660_000 : 15_000);
      pending.set(id, {
        resolve: value => { cleanup(); resolve(value); },
        reject: error => { cleanup(); reject(error); },
      });
      signal?.addEventListener("abort", abort, {once: true});
      child.stdin.write(JSON.stringify({jsonrpc: "2.0", id, method, params}) + "\n", error => {
        if (error) pending.get(id)?.reject(error);
      });
    });
  }
  function connect() {
    if (!active) return Promise.reject(new Error("Zeron MCP is not connected"));
    if (ready && child?.stdin.writable) return Promise.resolve();
    if (connecting) return connecting;
    const owned = spawn(config.command, config.args, {
      env: {...process.env, ...config.env}, stdio: ["pipe", "pipe", "inherit"],
    });
    child = owned;
    const failed = error => {
      if (child !== owned) return;
      ready = false;
      child = undefined;
      const reader = lines;
      lines = undefined;
      reader?.close();
      owned.kill();
      fail(error);
    };
    owned.on("error", failed);
    owned.on("exit", () => failed(new Error("Zeron MCP server exited")));
    owned.stdin.on("error", failed);
    lines = createInterface({input: owned.stdout});
    lines.on("line", line => {
      let message;
      try { message = JSON.parse(line); } catch { return; }
      const request = pending.get(message.id);
      if (!request) return;
      if (message.error) request.reject(new Error(message.error.message));
      else request.resolve(message.result);
    });
    const attempt = (async () => {
      try {
        await rpc("initialize", {protocolVersion: "2024-11-05", capabilities: {}, clientInfo: {name: "zeron-pi", version: "1"}});
        if (child !== owned) throw new Error("Zeron MCP connection closed");
        owned.stdin.write(JSON.stringify({jsonrpc: "2.0", method: "notifications/initialized"}) + "\n");
        ready = true;
      } catch (error) {
        if (child === owned) {
          ready = false;
          child = undefined;
          const reader = lines;
          lines = undefined;
          reader?.close();
          owned.kill();
        }
        throw error;
      }
    })();
    connecting = attempt;
    attempt.then(() => {
      if (connecting === attempt) connecting = undefined;
    }, () => {
      if (connecting === attempt) connecting = undefined;
    });
    return attempt;
  }
  function register() {
    for (const tool of CATALOG.tools) {
      if (!inChat && omitted.has(tool.name)) continue;
      pi.registerTool({
        name: `${config.name}_${tool.name}`, label: `Zeron: ${tool.name}`,
        description: tool.description, parameters: tool.inputSchema,
        async execute(_id, args, signal) {
          if (!active) throw new Error("Zeron MCP is not connected");
          await connect();
          const result = await rpc("tools/call", {name: tool.name, arguments: args}, signal);
          if (result.isError) throw new Error(result.content?.filter(c => c.type === "text").map(c => c.text).join("\n") || "Zeron tool failed");
          return {content: result.content, details: result.structuredContent ?? {}};
        },
      });
    }
  }
  pi.on("session_start", () => {
    close();
    active = true;
    register();
  });
  pi.on("session_shutdown", () => {
    active = false;
    close();
  });
  process.once("exit", close);
}
