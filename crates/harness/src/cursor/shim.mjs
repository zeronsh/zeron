// zeron cursor shim — a thin, zeron-owned wrapper around the PINNED
// @cursor/sdk (see CURSOR_SDK_PIN in crates/harness/src/cursor/mod.rs; the
// SDK is public beta with ~weekly releases — expect churn, revalidate on
// every bump). Materialized by zeron into the SDK's managed install dir so
// `import("@cursor/sdk")` resolves from the sibling node_modules.
//
// Why a shim at all: @cursor/sdk does NOT wrap the cursor-agent binary — it
// is Cursor's agent runtime bundled in-process, speaking proprietary
// protobuf/ConnectRPC to Cursor's backend, with client-side tool execution.
// There is no stdio wire to drive from Rust; this file IS the wire.
//
// Protocol: JSONL, one frame per line.
//   stdin  (engine → shim):
//     {"op":"run","prompt","cwd","model"?,"modelOptions"?,"resume"?}   start / first turn
//     {"op":"user","prompt"}                            explicit next turn
//     {"op":"steer","prompt"}                           native mid-turn input
//     {"op":"interrupt"}                                cancel the live run
//   stdout (shim → engine):
//     {"ev":"ready","agentId","model"?}
//     {"ev":"steered"}                                   input consumed
//     {"ev":"text","text","parent"?}        parent = spawning task callId
//     {"ev":"thinking","text","parent"?}
//     {"ev":"tool","phase":"start"|"end","id","name","args"?,"error"?,"parent"?}
//     {"ev":"usage","input","output"}
//     {"ev":"turn","status":"finished"|"error"|"cancelled","error"?}
//     {"ev":"fatal","message"}              unrecoverable (auth, SDK init)
//
// Models mode (`node <shim> models`): prints {"ev":"models","items":[…]} —
// the live `Cursor.models.list()` catalog — and exits.
//
// Login mode (`node <shim> login <store-path>`): no stdin protocol — runs the
// SDK's PKCE browser flow (`Cursor.auth.login`) writing the minted key to
// <store-path> instead of the live `~/.cursor/sdk/auth.json` (the engine
// snapshots it as an account slot, mirroring codex's throwaway CODEX_HOME).
//   stdout: {"ev":"auth-url","url"}         open/show this to the user
//           {"ev":"logged-in","email"?,"expiresAtMs"?}   then exit 0
//           {"ev":"fatal","message"}                     then exit 1
//
// Unknown SDK update kinds are ignored (never fatal): the SDK is beta and
// its delta union grows; the engine treats a degraded stream as chip-only.

import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import readline from "node:readline";

let beforeExit = async () => {};
const out = (o) => process.stdout.write(JSON.stringify(o) + "\n");
// process.exit() does not drain a pipe. Wait for all queued frames, including
// multi-megabyte catalogs, before stopping SDK background handles.
const exitAfterFlush = async (code) => {
  await beforeExit();
  await new Promise((resolve, reject) => {
    process.stdout.write("", (error) => error ? reject(error) : resolve());
  });
  process.exit(code);
};
let fatalStarted = false;
const fatal = async (message) => {
  if (fatalStarted) return;
  fatalStarted = true;
  out({ ev: "fatal", message: formatError(message) });
  await exitAfterFlush(1);
};

// SDK background promises can fail outside send()/wait(). Report the actual
// error over the protocol before shutting down, rather than letting Node dump
// a multi-megabyte minified source line and obscure the exception.
for (const event of ["uncaughtException", "unhandledRejection"]) {
  process.on(event, (error) => {
    const deadline = setTimeout(() => process.exit(1), 2000);
    deadline.unref();
    void fatal(error).catch(() => process.exit(1));
  });
}

// Only copy explicit diagnostic fields, never cause/config/headers or the
// full SDK object (which can carry credentials and request bodies).
function formatError(error, requestId) {
  let text = String(error?.message ?? error ?? "Unknown Cursor SDK error");
  const details = [];
  for (const [key, value] of [["code", error?.code], ["requestId", error?.requestId ?? requestId]]) {
    if (typeof value === "string" && /^[a-zA-Z0-9_.:-]{1,128}$/.test(value)) details.push(`${key}=${value}`);
  }
  if (details.length) text += ` [${details.join(", ")}]`;
  return text;
}

let sdk;
try {
  sdk = await import("@cursor/sdk");
} catch (e) {
  await fatal(`@cursor/sdk failed to load: ${e?.message ?? e}`);
}
const { Agent, Cursor, FileCredentialStore, JsonlLocalAgentStore } = sdk;

// ---- per-run agent store --------------------------------------------------
// The SDK's default local store is SQLite keyed by WORKSPACE
// (`getDefaultSdkStateRoot(cwd)`), designed for one process per workspace.
// Zeron runs concurrent shims on the same cwd as a matter of course — the
// chat turn and its title generation start together, and two chats can share
// a worktree — and the second `Agent.create` dies with "database is locked"
// (reproduced live, 1.0.28). The shared-root JSONL backend is no better: its
// writes are only serialized process-WIDE and updates rewrite whole files.
//
// So each run gets its OWN JsonlLocalAgentStore directory (one agent per
// store — the "huge catalog" caveat never applies), and a one-writer marker
// file maps agentId → store dir so `Agent.resume` from a later process finds
// it. Agents created before this scheme have no marker and fall back to the
// SDK default store, which is where they live.
const STATE_BASE =
  process.env.ZERON_CURSOR_STATE_DIR || path.join(os.homedir(), ".zeron", "cursor-state");

function agentDirMarker(agentId) {
  return path.join(STATE_BASE, "by-agent", String(agentId));
}

function newRunStore(storeDir) {
  const dir = storeDir || path.join(STATE_BASE, "agents", crypto.randomUUID());
  fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
  return { dir, store: new JsonlLocalAgentStore(dir) };
}

function storeForResume(agentId) {
  let dir;
  try { dir = fs.readFileSync(agentDirMarker(agentId), "utf8").trim(); }
  catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
  if (!dir || !fs.existsSync(dir)) throw new Error("Cursor conversation storage is missing; restore its state directory before resuming.");
  return { dir, store: new JsonlLocalAgentStore(dir) };
}

function rememberAgentDir(agentId, dir) {
  const marker = agentDirMarker(agentId);
  fs.mkdirSync(path.dirname(marker), { recursive: true, mode: 0o700 });
  const tmp = `${marker}.tmp-${process.pid}`;
  fs.writeFileSync(tmp, dir, {mode: 0o600});
  fs.renameSync(tmp, marker);
}

// The Rust harness holds an OS lock for this store until the child is reaped.
// A PID marker additionally prevents a new engine from reclaiming an orphaned
// but still-live shim after its parent engine crashes.
let ownedStore = null;
let ownerPath = null;
async function claimStore(local) {
  const marker = path.join(local.dir, ".zeron-owner.json");
  let previous;
  try { previous = JSON.parse(fs.readFileSync(marker, "utf8")); }
  catch (error) { if (error.code !== "ENOENT") throw error; }
  if (previous?.pid && previous.pid !== process.pid) {
    const alive = (pid) => {
      try { process.kill(pid, 0); return true; }
      catch (error) { if (error.code === "ESRCH") return false; throw error; }
    };
    // A parent that died releases its OS lease immediately. Allow its shim's
    // parent watchdog to finish bounded cleanup before reclaiming the store.
    if (previous.parentPid && !alive(previous.parentPid)) {
      const deadline = Date.now() + 3500;
      while (fs.existsSync(marker) && alive(previous.pid) && Date.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
    }
    if (fs.existsSync(marker) && alive(previous.pid)) {
      throw new Error("This Cursor conversation is still running in another process. Stop that run before retrying.");
    }
  }
  const temporary = `${marker}.${process.pid}`;
  fs.writeFileSync(temporary, JSON.stringify({pid: process.pid, parentPid: process.ppid}), {mode: 0o600});
  fs.renameSync(temporary, marker);
  ownerPath = marker;
  ownedStore = local.store;
}

async function recoverInterruptedRun(agentId) {
  if (!ownedStore) return; // Legacy SDK-default stores aren't ours to edit.
  const document = await ownedStore.agents.get({agentId});
  if (!document || (!document.activeRunId && document.status !== "running")) return;
  const interruptedRun = document.activeRunId
    ? await ownedStore.runs.get({agentId, runId: document.activeRunId}) : null;
  if (interruptedRun && ["queued", "running"].includes(interruptedRun.status)) {
    await ownedStore.runs.update({run: {
      ...interruptedRun, status: "cancelled", endedAt: Date.now(), updatedAt: Date.now(),
      error: "The previous Zeron process stopped before completing this turn.",
    }});
  }
  // Preserve the newest available conversation checkpoint; never start a new
  // conversation or replay the interrupted prompt during recovery.
  await ownedStore.agents.update({agent: {
    ...document, status: "idle", activeRunId: null, updatedAt: Date.now(),
    latestCheckpoint: interruptedRun?.latestCheckpointRef ?? document.latestCheckpoint,
  }});
}

// Persist the user text BEFORE announcing readiness. Cursor can cancel a run
// before saving its first checkpoint; resume alone then silently loses that
// message. This receipt is owned by the same exclusive store lease as the SDK.
// Only text absent from the saved conversation is carried into the next explicit
// user request as interrupted history. Never execute/retry an old run on its own.
let receiptPath = null;
async function savedUserMessages() {
  const document = await ownedStore.agents.get({agentId: agent.agentId});
  if (!document?.latestCheckpoint) return [];
  const texts = [];
  for (let offset = 0; ; offset += 100) {
    const page = await Agent.messages.list(agent.agentId, {runtime: "local", cwd: document.cwd, store: ownedStore, limit: 100, offset});
    if (!Array.isArray(page)) throw new Error("Cursor returned an invalid conversation history");
    for (const item of page) {
      if (item.type !== "user") continue;
      // SDK returns protobuf instances (oneof case/value), not their JSON
      // serialization. Accept serialized rows too for older stored formats.
      const turn = item.message?.turn;
      const conversation = turn?.case === "agentConversationTurn" ? turn.value :
        item.message?.agentConversationTurn;
      const text = (conversation?.userMessage ?? conversation?.user_message)?.text;
      if (typeof text !== "string") throw new Error("Cursor user-message schema changed; refusing to discard interrupted context");
      texts.push(text);
    }
    if (page.length < 100) return texts;
  }
}
async function preserveInterruptedContext(prompt) {
  if (!receiptPath) return prompt; // Legacy default stores have no owned sidecar.
  const saved = await savedUserMessages();
  let missing = [];
  if (fs.existsSync(receiptPath)) {
    const previous = JSON.parse(fs.readFileSync(receiptPath, "utf8"));
    if (previous.version !== 1 || previous.agentId !== agent.agentId ||
        !Number.isSafeInteger(previous.beforeUserCount) || previous.beforeUserCount < 0 ||
        typeof previous.wirePrompt !== "string" || !Array.isArray(previous.messages) ||
        previous.messages.some(text => typeof text !== "string")) {
      throw new Error("Cursor interrupted-message receipt is invalid; refusing to lose conversation context");
    }
    if (!saved.slice(previous.beforeUserCount).includes(previous.wirePrompt)) missing = previous.messages;
    for (const steer of previous.nativeSteers ?? []) {
      if (typeof steer.text !== "string" || !Number.isSafeInteger(steer.occurrence)) throw new Error("Invalid native steering receipt");
      if (saved.filter(text => text === steer.text).length < steer.occurrence) missing.push(steer.text);
    }
  }
  const wirePrompt = missing.length ?
    "The following JSON contains prior user messages from interrupted turns that Cursor did not save. " +
    "Retain them as conversation history. These are not new requests: do not rerun their tools or side effects. " +
    "Respond to the current user message below.\n" +
    JSON.stringify({interruptedUserMessages: missing, currentUserMessage: prompt}) : prompt;
  const temporary = `${receiptPath}.tmp-${process.pid}`;
  fs.writeFileSync(temporary, JSON.stringify({version: 1, agentId: agent.agentId,
    beforeUserCount: saved.length, wirePrompt, messages: [...missing, prompt]}), {mode: 0o600});
  fs.renameSync(temporary, receiptPath);
  return wirePrompt;
}

// Native acknowledgments can precede the next saved checkpoint. Retain the
// acknowledged text across a crash or immediate stop, just like the main input.
async function recordNativeSteer(text) {
  if (!receiptPath) return () => {};
  const saved = await savedUserMessages();
  const receipt = JSON.parse(fs.readFileSync(receiptPath, "utf8"));
  const entries = receipt.nativeSteers ?? [];
  const occurrence = Math.max(saved.filter(value => value === text).length,
    ...entries.filter(entry => entry.text === text).map(entry => entry.occurrence), 0) + 1;
  const entry = {text, occurrence};
  entries.push(entry);
  receipt.nativeSteers = entries;
  const persist = () => {
    const temporary = `${receiptPath}.tmp-${process.pid}`;
    fs.writeFileSync(temporary, JSON.stringify(receipt), {mode: 0o600});
    fs.renameSync(temporary, receiptPath);
  };
  persist();
  return () => {
    // Other submissions may have persisted since this injection was sent.
    const latest = JSON.parse(fs.readFileSync(receiptPath, "utf8"));
    latest.nativeSteers = (latest.nativeSteers ?? []).filter(value =>
      value.text !== entry.text || value.occurrence !== entry.occurrence);
    Object.assign(receipt, latest);
    persist();
  };
}

// ---- models mode ----------------------------------------------------------
// `node <shim> models`: print the live catalog (`Cursor.models.list()` — no
// additional login: the SDK resolves CURSOR_API_KEY, then its saved login)
// as one frame and exit. The harness
// maps items (id/displayName/parameters/variants) into its picker models.
if (process.argv[2] === "models") {
  try {
    const listed = await Cursor.models.list();
    const catalog = Array.isArray(listed) ? listed : (listed?.items ?? []);
    // The picker consumes parameter definitions and the default variant only,
    // not the combinatorial list of every purchasable variant.
    const items = catalog.map(({id, displayName, description, parameters, variants}) => ({
      id, displayName, description, parameters,
      variants: (variants ?? []).filter((variant) => variant.isDefault).slice(0, 1),
    }));
    out({ ev: "models", items });
    await exitAfterFlush(0);
  } catch (e) {
    await fatal(`cursor model discovery failed: ${e?.message ?? e}`);
  }
}

// ---- login mode -----------------------------------------------------------
// The browser flow never opens a browser itself (the engine may be headless;
// the URL can be opened on whichever device is viewing the app), and the
// minted key lands in the engine-chosen store file, never the live login.
if (process.argv[2] === "login") {
  const storePath = process.argv[3];
  if (!storePath) await fatal("login mode needs a store path");
  try {
    const result = await Cursor.auth.login({
      openBrowser: false,
      onLoginUrl: (url) => out({ ev: "auth-url", url }),
      store: new FileCredentialStore(storePath),
      apiKeyName: `zeron — ${os.hostname()}`,
    });
    out({
      ev: "logged-in",
      ...(result?.email ? { email: result.email } : {}),
      ...(result?.apiKeyExpiresAtMs ? { expiresAtMs: result.apiKeyExpiresAtMs } : {}),
    });
    await exitAfterFlush(0);
  } catch (e) {
    await fatal(`cursor login failed: ${e?.message ?? e}`);
  }
}

let agent = null;
let run = null;
let interrupted = false;
let closing = false;
let cleanupPromise;
beforeExit = () => cleanupPromise ??= (async () => {
  closing = true;
  // Cancellation may itself wedge on a dead transport. Keep teardown bounded;
  // next resume repairs a leftover activeRunId after acquiring ownership.
  if (run) {
    await Promise.race([
      run.cancel().catch(() => {}),
      new Promise((resolve) => setTimeout(resolve, 1000)),
    ]);
  }
  try { agent?.close(); } catch {}
  if (ownerPath) {
    try { fs.unlinkSync(ownerPath); } catch {}
    ownerPath = null;
  }
})();
for (const signal of ["SIGTERM", "SIGINT"]) {
  process.on(signal, () => { void exitAfterFlush(0); });
}
// If the engine itself crashes there is no EOF guarantee (a leaked descriptor
// may keep stdin open). Do not leave a detached SDK run owning this conversation.
const parentPid = process.ppid;
setInterval(() => {
  if (process.ppid !== parentPid) { void exitAfterFlush(0); return; }
  try { process.kill(parentPid, 0); }
  catch (error) { if (error.code === "ESRCH") void exitAfterFlush(0); }
}, 1000).unref();

// One InteractionUpdate → zero or one frame. `parent` attributes nested
// subagent traffic (tool-call-delta carries the child's updates tagged by
// the spawning task's callId — the full subagent transcript, live).
const activeTools = new Set();
function mapUpdate(u, parent) {
  if (!u || typeof u !== "object") return;
  const tag = parent ? { parent } : {};
  switch (u.type) {
    case "text-delta":
      if (u.text) out({ ev: "text", text: u.text, ...tag });
      break;
    case "thinking-delta":
      if (u.text) out({ ev: "thinking", text: u.text, ...tag });
      break;
    case "tool-call-started":
      if (!parent) activeTools.add(u.callId);
      out({
        ev: "tool",
        phase: "start",
        id: u.callId,
        name: u.toolCall?.type ?? "tool",
        args: u.toolCall?.args ?? null,
        ...tag,
      });
      break;
    case "tool-call-completed": {
      if (!parent) {
        activeTools.delete(u.callId);
        queueMicrotask(() => { void pumpSteers().catch(fatal); });
      }
      const r = u.toolCall?.result;
      const failed =
        r?.status === "error" ||
        (r?.status === "success" &&
          typeof r?.value?.exitCode === "number" &&
          r.value.exitCode !== 0);
      out({
        ev: "tool",
        phase: "end",
        id: u.callId,
        name: u.toolCall?.type ?? "tool",
        args: u.toolCall?.args ?? null,
        error: Boolean(failed),
        ...tag,
      });
      break;
    }
    case "tool-call-delta":
      // Nested subagent update, tagged by the spawning task call.
      if (u.taskUpdate) mapUpdate(u.taskUpdate, u.callId);
      break;
    case "turn-ended":
      if (u.usage) {
        out({
          ev: "usage",
          input: u.usage.inputTokens ?? 0,
          output: u.usage.outputTokens ?? 0,
        });
      }
      break;
    default:
      // step-*/summary-*/token-delta/partial-tool-call/…: no consumer.
      break;
  }
}

// Auth errors surface at RUN time (Agent.create succeeds unauthed —
// verified live: the turn fails with "[unknown] Invalid User API Key").
// Attach the exact fix, because the SDK's credentials are separate from
// `cursor-agent login`.
function withAuthHint(message) {
  const text = String(message ?? "");
  if (/api key|not authenticated|unauthorized/i.test(text)) {
    return (
      text +
      " — connect Cursor in Settings → Accounts (its login is separate from " +
      "`cursor-agent login`), or set CURSOR_API_KEY from cursor.com/settings."
    );
  }
  return text;
}

// Keep ownership until Cursor confirms that the active turn appended the text.
// A boundary race returns revert_to_followup; only those messages start a turn.
const pendingSteers = [];
const steerDeliveries = new Set();
let steerPump = null;
let turnActive = false;
// Immediate steering: Cursor's native steer lands only at a step boundary, so
// a plain text answer would run to completion first. With no tool running, a
// steer cancels the streaming run and continues as the follow-up turn (the
// way Codex turn/steer behaves). A running tool is never cancelled: the
// native steer injects at its boundary instead.
let preempting = false;
// The run a steer cancelled: its late stream updates must not leak into the
// steer's reply.
let preemptedRun = null;
// Tests of the native SDK steering path disable preemption.
const NATIVE_STEER_ONLY = process.env.ZERON_CURSOR_NATIVE_STEER_ONLY === "1";
function preemptForSteer() {
  if (NATIVE_STEER_ONLY) return false;
  if (!turnActive || !run || activeTools.size || preempting || interrupted || closing) return false;
  preempting = true;
  preemptedRun = run;
  run.cancel().catch(() => {});
  return true;
}
// The preempted run ended: its steers continue the conversation as one turn.
async function continueAfterPreempt() {
  preempting = false;
  activeTools.clear();
  await Promise.all(steerDeliveries);
  run = null;
  turnActive = false;
  if (!pendingSteers.length) {
    out({ ev: "turn", status: "finished" });
    return;
  }
  chain = chain.then(followupSteers).catch(fatal);
}
function pumpSteers() {
  if (steerPump) return steerPump;
  if (preempting) return Promise.resolve();
  if (!run || !pendingSteers.length || activeTools.size) return Promise.resolve();
  const target = run;
  steerPump = (async () => {
    while (pendingSteers.length && run === target && !activeTools.size && !interrupted && !closing) {
      if (typeof target.steer !== "function") {
        throw new Error("Cursor SDK lacks native steering; update the managed SDK");
      }
      const message = pendingSteers.find(message => !message.submitted && message.revertedRun !== target);
      if (!message) return;
      const reverted = await recordNativeSteer(message.prompt);
      // History lookup can yield while another tool starts. Never let native
      // steering cancel its shell/process tree; inject at the tool boundary.
      if (activeTools.size) { reverted(); return; }
      message.submitted = true;
      // Cursor's client submits each injection independently. Awaiting its
      // terminal delivery acknowledgment here serializes model responses.
      const delivery = target.steer(message.prompt).then(outcome => {
        if (outcome === "revert_to_followup") {
          message.revertedRun = target;
          reverted();
        } else if (outcome === "complete_delivered") {
          message.delivered = true;
        } else {
          throw new Error(`Unknown Cursor steering acknowledgment: ${outcome}`);
        }
        // The engine owns an ordered mailbox even when SDK acks arrive out of order.
        while (pendingSteers[0]?.delivered) {
          pendingSteers.shift();
          out({ev: "steered"});
        }
      });
      steerDeliveries.add(delivery);
      void delivery.catch(fatal).finally(() => steerDeliveries.delete(delivery));
    }
  })().finally(() => { steerPump = null; });
  return steerPump;
}
async function followupSteers() {
  while (pendingSteers.length && !interrupted && !closing) {
    const batch = pendingSteers.splice(0);
    // A later input can be delivered before an earlier one is rejected.
    // Never replay the delivered input when draining the rejected prefix.
    const undelivered = batch.filter(message => !message.delivered);
    const prompt = undelivered.length === 1 ? undelivered[0].prompt :
      "These user messages arrived together. Address them together in order:\n" +
      JSON.stringify(undelivered.map(message => message.prompt));
    await runTurn(prompt, undefined, () => {
      for (const message of batch) out({ev: "steered"});
    });
  }
}
function acceptSteer(message) {
  pendingSteers.push(message);
  if (turnActive) {
    if (!preemptForSteer()) void pumpSteers().catch(fatal);
  } else {
    chain = chain.then(followupSteers).catch(fatal);
  }
}

async function runTurn(prompt, ready, accepted) {
  turnActive = true;
  if (closing) return;
  try {
    prompt = await preserveInterruptedContext(prompt);
    ready?.();
    if (interrupted || closing) {
      out({ev: "turn", status: "cancelled"});
      return;
    }
    accepted?.();
    let thisRun = null;
    run = thisRun = await agent.send(prompt, {
      onDelta: ({ update }) => {
        if (thisRun && thisRun === preemptedRun) return;
        try {
          mapUpdate(update);
        } catch {
          // A malformed update must never kill the turn.
        }
      },
    });
  } catch (e) {
    out({ ev: "turn", status: "error", error: withAuthHint(formatError(e, run?.requestId)) });
    run = null;
    turnActive = false;
    return;
  }
  // A steer that arrived while send() was creating the run preempts now.
  if (!pendingSteers.length || !preemptForSteer()) void pumpSteers().catch(fatal);
  // Interrupt may arrive while send() is still creating the run.
  if (interrupted || closing) await run.cancel().catch(() => {});
  let result;
  try {
    result = await run.wait();
  } catch (e) {
    if (preempting && !interrupted && !closing) return continueAfterPreempt();
    out({
      ev: "turn",
      status: interrupted ? "cancelled" : "error",
      error: withAuthHint(formatError(e, run?.requestId)),
    });
    return;
  }
  if (preempting && !interrupted && !closing) return continueAfterPreempt();
  activeTools.clear();
  await pumpSteers();
  await Promise.all(steerDeliveries);
  run = null;
  turnActive = false;
  out({
    ev: "turn",
    status: result?.status ?? "finished",
    ...(result?.error?.message ? { error: withAuthHint(formatError(result.error, result.requestId)) } : {}),
  });
  if (pendingSteers.length) chain = chain.then(followupSteers).catch(fatal);
}

// The run's ModelSelection: id + typed parameter values (thinking / context /
// effort / fast / optimize_for — ids from the discovered catalog). Legacy
// sessions may still carry the pre-discovery static option ("optimizeFor",
// choice "speed") — translate rather than send an id the backend never knew.
function modelSelection(msg) {
  const id = msg.model || "auto";
  const params = [];
  for (const [key, value] of Object.entries(msg.modelOptions ?? {})) {
    if (typeof value !== "string" || !value) continue;
    if (key === "optimizeFor") {
      params.push({ id: "optimize_for", value: value === "speed" ? "cost" : value });
    } else {
      params.push({ id: key, value });
    }
  }
  return params.length ? { id, params } : { id };
}

async function start(msg) {
  const model = modelSelection(msg);
  const local = { cwd: msg.cwd || process.cwd(), enableAgentRetries: true };
  // Isolated per-run store (see the header above). Resume looks the agent's
  // store up by marker; a markerless (pre-isolation) agent resumes from the
  // SDK's default store exactly as before.
  let runDir = null;
  if (msg.resume) {
    const found = storeForResume(msg.resume);
    if (found) {
      await claimStore(found);
      await recoverInterruptedRun(msg.resume);
      local.store = found.store;
      runDir = found.dir;
    }
  } else {
    const fresh = newRunStore(msg.storeDir);
    await claimStore(fresh);
    local.store = fresh.store;
    runDir = fresh.dir;
  }
  const options = {
    model,
    ...(msg.mcp ? { mcpServers: { [msg.mcp.name]: {
      type: "stdio", command: msg.mcp.command, args: msg.mcp.args, env: msg.mcp.env,
    } } } : {}),
    // askQuestion has no public answer channel in this SDK (SDKRequestMessage
    // carries only a request id) — a question would block the run forever.
    // generateImage has nowhere to land in a zeron session (ACP parity).
    disallowedTools: ["askQuestion", "generateImage"],
    local,
  };
  try {
    // SDK startup validates the model via get_models. Rapid process resumes
    // can hit its 30/minute limit. Retry ONLY this pre-send discovery failure;
    // never replay a run or retry authentication / arbitrary provider errors.
    for (let attempt = 0; ; attempt++) {
      try {
        agent = msg.resume
          ? await Agent.resume(msg.resume, options)
          : await Agent.create(options);
        break;
      } catch (error) {
        if (attempt >= 6 || interrupted || closing ||
            !/rate limit.*get_models|get_models.*rate limit/i.test(String(error?.message ?? error))) throw error;
        await new Promise(resolve => setTimeout(resolve, 1000 * 2 ** attempt));
      }
    }
  } catch (e) {
    // Auth is the common cause: the SDK's credentials are SEPARATE from
    // `cursor-agent login` (verified) — name the fix precisely.
    const auth = await Cursor.auth.status().catch(() => null);
    if (!process.env.CURSOR_API_KEY && auth?.status !== "logged-in") {
      await fatal(
        "Cursor is not connected (its login is separate from " +
          "`cursor-agent login`): connect it in Settings → Accounts, or set " +
          `CURSOR_API_KEY from cursor.com/settings, then retry. (${e?.message ?? e})`,
      );
    }
    await fatal(`cursor agent failed to start: ${e?.message ?? e}`);
  }
  if (runDir) {
    rememberAgentDir(agent.agentId, runDir);
    receiptPath = path.join(runDir, ".zeron-user-receipt.json");
  }
  await runTurn(msg.prompt ?? "", () => {
    out({ ev: "ready", agentId: agent.agentId, model: agent.model?.id ?? model.id });
  });
}

const rl = readline.createInterface({ input: process.stdin });
let chain = Promise.resolve();
rl.on("line", (line) => {
  line = line.trim();
  if (!line) return;
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    return;
  }
  switch (msg.op) {
    case "run":
      chain = chain.then(() => start(msg)).catch((e) => fatal(e));
      break;
    case "steer":
      acceptSteer(msg);
      break;
    case "user":
      chain = chain
        .then(() => (agent ? runTurn(msg.prompt ?? "") : undefined))
        .catch((e) => fatal(e));
      break;
    case "interrupt":
      interrupted = true;
      if (run) run.cancel().catch(() => {});
      break;
    default:
      break;
  }
});
rl.on("close", () => {
  // Parent teardown owns the lifetime; do not keep a run alive after EOF.
  void exitAfterFlush(0);
});
