// pi-acp 0.0.33 ignores session mcpServers. Loaded only into this run's pi
// process via --extension; no project/global settings or packages are changed.
// It also reports provider failures, which pi-acp otherwise ends as a silent
// end_turn with no output.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

export default function (pi) {
  const config = JSON.parse(process.env.ZERON_PI_MCP);
  let child, lines, nextId = 0;
  const pending = new Map();
  function fail(error) {
    for (const request of [...pending.values()]) request.reject(error);
  }
  function close() {
    fail(new Error("Zeron MCP connection closed"));
    lines?.close();
    child?.kill();
    child = undefined;
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
  pi.on("session_start", async () => {
    close();
    child = spawn(config.command, config.args, {
      env: {...process.env, ...config.env}, stdio: ["pipe", "pipe", "inherit"],
    });
    const owned = child;
    const failed = error => { if (child === owned) fail(error); };
    child.on("error", failed);
    child.on("exit", () => failed(new Error("Zeron MCP server exited")));
    child.stdin.on("error", failed);
    lines = createInterface({input: child.stdout});
    lines.on("line", line => {
      let message;
      try { message = JSON.parse(line); } catch { return; }
      const request = pending.get(message.id);
      if (!request) return;
      if (message.error) request.reject(new Error(message.error.message));
      else request.resolve(message.result);
    });
    try {
      await rpc("initialize", {protocolVersion: "2024-11-05", capabilities: {}, clientInfo: {name: "zeron-pi", version: "1"}});
      child.stdin.write(JSON.stringify({jsonrpc: "2.0", method: "notifications/initialized"}) + "\n");
      const {tools} = await rpc("tools/list", {});
      for (const tool of tools) {
        pi.registerTool({
          name: `${config.name}_${tool.name}`, label: `Zeron: ${tool.name}`,
          description: tool.description, parameters: tool.inputSchema,
          async execute(_id, args, signal) {
            const result = await rpc("tools/call", {name: tool.name, arguments: args}, signal);
            if (result.isError) throw new Error(result.content?.filter(c => c.type === "text").map(c => c.text).join("\n") || "Zeron tool failed");
            return {content: result.content, details: result.structuredContent ?? {}};
          },
        });
      }
    } catch (error) {
      close();
      throw error;
    }
  });
  pi.on("message_end", (event, ctx) => {
    const {message} = event;
    if (message?.role === "assistant" && message.stopReason === "error") {
      ctx.ui.notify(message.errorMessage || "Pi request failed", "error");
    }
  });
  pi.on("session_shutdown", close);
  process.once("exit", close);
}
