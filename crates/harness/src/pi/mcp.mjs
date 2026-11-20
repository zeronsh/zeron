// Per-run Zeron MCP bridge. Tool names and the stdio cancellation contract stay stable.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

// Computer results are MCP envelopes so codemode can inspect handles and forward images.
// Ordinary chat tools keep their existing text return type unless the server declares a schema.
const COMPUTER_RESULT_SCHEMA = {
  type: "object",
  properties: {
    content: {type: "array", items: {type: "object", additionalProperties: true}},
    structuredContent: {},
    isError: {type: "boolean"},
  },
  required: ["content", "isError"],
  additionalProperties: true,
};
const COMPUTER_SCRIPT_GUIDANCE = "In codemode, computer tools return {content, structuredContent, isError}. " +
  "Check isError; forward text blocks with text() and image blocks with image(). Never print the whole result or image data.";

function qualifyInstructions(text, tools, prefix) {
  const names = new Set(tools.map(tool => tool.name));
  return text.replace(/\b[A-Za-z_][A-Za-z0-9_]*\b/g, (name, offset) =>
    names.has(name) || name === "computer_" || (name.startsWith("computer_") && text[offset + name.length] === "*")
      ? `${prefix}_${name}` : name);
}

export default function (pi) {
  const config = JSON.parse(process.env.ZERON_PI_MCP);
  // Native MCP arrived with Pi's structured tool contract. Older Pi keeps the text/error path.
  const structuredTools = typeof pi.registerMcpServer === "function";
  const section = `zeron_mcp_${config.name.replace(/[^A-Za-z0-9_]/g, "_")}`;
  let child, lines, nextId = 0, instructions = "";
  const pending = new Map();
  function fail(error) {
    for (const request of [...pending.values()]) request.reject(error);
  }
  function close() {
    instructions = "";
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
  pi.on("before_agent_start", event => {
    if (event.systemPromptOptions) {
      const sections = event.systemPromptOptions.sections ??= {};
      if (instructions) sections[section] = instructions;
      else delete sections[section];
    } else if (instructions) {
      // Pi >= 0.85.1 predates structured prompt sections; do not replace other guidance.
      return {systemPrompt: `${event.systemPrompt}\n\n<${section}>\n${instructions}\n</${section}>`};
    }
  });
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
      const initialized = await rpc("initialize", {protocolVersion: "2024-11-05", capabilities: {}, clientInfo: {name: "zeron-pi", version: "1"}});
      child.stdin.write(JSON.stringify({jsonrpc: "2.0", method: "notifications/initialized"}) + "\n");
      const {tools} = await rpc("tools/list", {});
      instructions = qualifyInstructions(initialized.instructions ?? "", tools, config.name);
      if (structuredTools && tools.some(tool => tool.name.startsWith("computer_"))) {
        instructions = [instructions, COMPUTER_SCRIPT_GUIDANCE].filter(Boolean).join("\n");
      }
      const namespace = {name: config.name, description: `${config.name} MCP tools`, instructions};
      const discoveryTools = structuredTools &&
        typeof pi.getAllTools === "function" &&
        typeof pi.getActiveTools === "function" &&
        typeof pi.setActiveTools === "function";
      const allTools = discoveryTools ? (pi.getAllTools() ?? []) : [];
      // Pi 1.5.2 exposes reachesTools as a function on getAllTools(), while mocks may use true.
      const reaches = name => allTools.some(tool =>
        tool.name === name && (typeof tool.reachesTools === "function" || tool.reachesTools === true));
      const hasCodemode = reaches("codemode");
      const hasToolSearch = reaches("tool_search");
      const canDefer = hasCodemode || hasToolSearch;
      const hasDeferred = canDefer && tools.some(tool => tool.name !== "wait_on_background");
      if (hasDeferred) {
        const via = [
          hasCodemode && `codemode (tools.${config.name}_*, searchTools, describeTool)`,
          hasToolSearch && "tool_search",
        ].filter(Boolean).join(" or ");
        instructions = [instructions, `${config.name}_* tools are deferred: call via ${via}.`].filter(Boolean).join("\n");
      }
      for (const tool of tools) {
        const computerResult = structuredTools && tool.name.startsWith("computer_");
        const outputSchema = computerResult ? COMPUTER_RESULT_SCHEMA : tool.outputSchema;
        pi.registerTool({
          name: `${config.name}_${tool.name}`, label: `${config.name}: ${tool.name}`,
          description: tool.description, parameters: tool.inputSchema,
          ...(tool.annotations ? {annotations: tool.annotations} : {}),
          ...(canDefer ? {exposure: tool.name === "wait_on_background" ? "direct" : "deferred"} : {}),
          ...(structuredTools ? {namespace} : {}),
          ...(outputSchema ? {outputSchema} : {}),
          async execute(_id, args, signal) {
            const result = await rpc("tools/call", {name: tool.name, arguments: args}, signal);
            const isError = result.isError === true || result.structuredContent?.status === "refused";
            if (isError && !(structuredTools && outputSchema)) {
              throw new Error(result.content?.filter(c => c.type === "text").map(c => c.text).join("\n") || "Zeron tool failed");
            }
            return {
              content: result.content, details: result.structuredContent ?? {},
              ...(computerResult ? {structuredContent: {...result, isError}} :
                result.structuredContent === undefined ? {} : {structuredContent: result.structuredContent}),
              ...(isError ? {isError: true} : {}),
            };
          },
        });
      }
      if (hasDeferred) {
        const active = pi.getActiveTools() ?? [];
        if (!active.includes("codemode") && !active.includes("tool_search")) {
          pi.setActiveTools([...active, hasCodemode ? "codemode" : "tool_search"]);
        }
      }
    } catch (error) {
      close();
      throw error;
    }
  });
  pi.on("session_shutdown", close);
  process.once("exit", close);
}
