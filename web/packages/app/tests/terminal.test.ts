import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { RpcError, type WatchHandlers, type WatchOptions } from "@zeron/engine-client";
import type { TerminalEvent, TerminalSession } from "@zeron/proto";
import {
  COALESCE_MS,
  RESIZE_DEBOUNCE_MS,
  activeAfterClose,
  activeAfterReorder,
  backoffMs,
  clampTerminalHeight,
  decodeBase64,
  dropIndex,
  encodeBase64,
  exitMessage,
  pasteBytes,
  reorderTabs,
  shellTitle,
  slideOffset,
} from "../src/terminal/tabs";
import { TerminalSessionController, type TerminalSink } from "../src/terminal/session";
import { xtermThemeFromPalette } from "../src/terminal/theme";
import type { TerminalPalette } from "@zeron/theme";

/**
 * The wire boundary for these tests: RPC method names, params, and base64
 * payloads exactly as the engine's terminal endpoints expect them
 * (crates/engine/src/rpc.rs §3.4), and the desktop panel's behavior as the
 * parity reference (crates/ui/src/terminal/panel.rs).
 */

describe("clampTerminalHeight", () => {
  it("clamps to 160 px … 55 % of the viewport", () => {
    expect(clampTerminalHeight(100, 1000)).toBe(160);
    expect(clampTerminalHeight(800, 1000)).toBe(550);
    expect(clampTerminalHeight(280, 1000)).toBe(280);
    expect(clampTerminalHeight(Number.NaN, 1000)).toBe(160);
  });
});

describe("backoffMs", () => {
  it("doubles 500 ms to an 8 s ceiling", () => {
    expect(backoffMs(0)).toBe(500);
    expect(backoffMs(1)).toBe(1000);
    expect(backoffMs(4)).toBe(8000);
    expect(backoffMs(9)).toBe(8000);
  });
});

describe("tab drag math (desktop panel.rs ports)", () => {
  it("dropIndex maps a strip x-offset to a slot", () => {
    expect(dropIndex(0, 118, 3)).toBe(0);
    expect(dropIndex(119, 118, 3)).toBe(1);
    expect(dropIndex(10_000, 118, 3)).toBe(2);
    expect(dropIndex(-5, 118, 3)).toBe(0);
    expect(dropIndex(50, 118, 0)).toBe(0);
  });

  it("reorderTabs moves and preserves identity", () => {
    const tabs = ["a", "b", "c", "d"];
    reorderTabs(tabs, 0, 2);
    expect(tabs).toEqual(["b", "c", "a", "d"]);
    reorderTabs(tabs, 3, 3);
    expect(tabs).toEqual(["b", "c", "a", "d"]);
    reorderTabs(tabs, 9, 0);
    expect(tabs).toEqual(["b", "c", "a", "d"]);
  });

  it("slideOffset shifts the gap toward the dragged tab", () => {
    // Dragging 0 over 2: tabs 1,2 slide one slot left.
    expect(slideOffset(0, 0, 2)).toBe(0);
    expect(slideOffset(1, 0, 2)).toBe(-1);
    expect(slideOffset(2, 0, 2)).toBe(-1);
    // Dragging 2 over 0: tabs 0,1 slide one slot right.
    expect(slideOffset(0, 2, 0)).toBe(1);
    expect(slideOffset(1, 2, 0)).toBe(1);
    expect(slideOffset(2, 2, 0)).toBe(0);
  });

  it("activeAfterReorder keeps the active tab selected", () => {
    expect(activeAfterReorder(0, 0, 2)).toBe(2);
    expect(activeAfterReorder(2, 0, 2)).toBe(1);
    expect(activeAfterReorder(1, 3, 1)).toBe(2);
    expect(activeAfterReorder(3, 0, 2)).toBe(3);
  });

  it("activeAfterClose shifts left of the closed slot", () => {
    expect(activeAfterClose(2, 0, 3)).toBe(1);
    expect(activeAfterClose(1, 2, 2)).toBe(1);
    expect(activeAfterClose(0, 0, 0)).toBe(0);
  });
});

describe("exitMessage / shellTitle (desktop panel.rs ports)", () => {
  it("formats the dim exit trailer", () => {
    expect(exitMessage(0)).toBe("\r\n\x1b[90m[process exited 0]\x1b[0m\r\n");
    expect(exitMessage(137)).toContain("[process exited 137]");
  });

  it("takes the shell basename for the tab label", () => {
    expect(shellTitle("/bin/zsh")).toBe("zsh");
    expect(shellTitle("/usr/local/bin/fish")).toBe("fish");
    expect(shellTitle("C:\\Windows\\System32\\cmd.exe")).toBe("cmd.exe");
    expect(shellTitle("bash")).toBe("bash");
    expect(shellTitle("")).toBe("terminal");
  });
});

describe("base64 wire encoding", () => {
  it("round-trips multibyte UTF-8", () => {
    const text = "héllo → 世界";
    const bytes = new TextEncoder().encode(text);
    expect(new TextDecoder().decode(decodeBase64(encodeBase64(bytes)))).toBe(text);
  });

  it("decodes unpadded input and drops garbage", () => {
    expect(new TextDecoder().decode(decodeBase64("aGVsbG8"))).toBe("hello");
    expect(decodeBase64("!!!not base64!!!").length).toBe(0);
  });
});

describe("pasteBytes (desktop paste_bytes, view.rs:309-319)", () => {
  it("paste wraps when bracketed", () => {
    // Wraps in the bracketed-paste markers iff the mode is on…
    expect(pasteBytes("ls\r\n", true)).toBe("\x1b[200~ls\r\n\x1b[201~");
    expect(pasteBytes("ls\r\n", false)).toBe("ls\r\n");
    // …and strips an injected end-marker from the pasted text first.
    expect(pasteBytes("a\x1b[201~b", true)).toBe("\x1b[200~ab\x1b[201~");
    expect(pasteBytes("a\x1b[201~b", false)).toBe("ab");
  });
});

describe("xtermThemeFromPalette", () => {
  it("maps the theme's terminal roles onto xterm's ANSI slots", () => {
    const ansi = Array.from({ length: 16 }, (_, i) => `#0000${i.toString(16).padStart(2, "0")}`);
    const palette: TerminalPalette = {
      background: "#101010",
      foreground: "#e0e0e0",
      selection: "#303030",
      ansi,
    };
    const theme = xtermThemeFromPalette(palette, "#e8e8ea66");
    expect(theme.background).toBe("#101010");
    expect(theme.foreground).toBe("#e0e0e0");
    expect(theme.selectionBackground).toBe("#303030");
    expect(theme.black).toBe(ansi[0]);
    expect(theme.red).toBe(ansi[1]);
    expect(theme.white).toBe(ansi[7]);
    expect(theme.brightBlack).toBe(ansi[8]);
    expect(theme.brightWhite).toBe(ansi[15]);
    // The cursor is its own translucent role (`--rb-cursor`); the glyph
    // under it repaints in the normal foreground (cursorAccent), so the
    // block reads as an overlay, not an inversion (view.rs:548-550).
    expect(theme.cursor).toBe("#e8e8ea66");
    expect(theme.cursorAccent).toBe("#e0e0e0");
    // The scrollbar thumb defaults to a dim terminal foreground when no UI
    // role was read (currentTerminalTheme passes text-faint itself).
    expect(theme.scrollbarSliderBackground).toBe("#e0e0e085");
  });
});

// ── Session controller against a scripted RPC fake ─────────────────────

interface FakeCall {
  method: string;
  params: Record<string, unknown>;
  resolve: (value: unknown) => void;
  reject: (error: unknown) => void;
}

class FakeRpc {
  readonly calls: FakeCall[] = [];
  watchMethod: string | null = null;
  watchParams: { terminalId: string; afterSeq: number } | null = null;
  watchHandlers: WatchHandlers<TerminalEvent> | null = null;
  watchOptions: WatchOptions | undefined;
  watchCancelled = 0;

  call<T>(method: string, params: unknown = {}): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      this.calls.push({ method, params: params as Record<string, unknown>, resolve: (v) => resolve(v as T), reject });
    });
  }

  watch<T>(method: string, params: unknown, handlers: WatchHandlers<T>, options?: WatchOptions) {
    this.watchMethod = method;
    this.watchParams = params as { terminalId: string; afterSeq: number };
    this.watchHandlers = handlers as unknown as WatchHandlers<TerminalEvent>;
    this.watchOptions = options;
    return {
      method,
      cancel: () => {
        this.watchCancelled += 1;
      },
    };
  }

  callsFor(method: string): FakeCall[] {
    return this.calls.filter((call) => call.method === method);
  }

  resolveLast(method: string, value: unknown): void {
    const call = this.callsFor(method).at(-1);
    if (call === undefined) {
      throw new Error(`no call to ${method}`);
    }
    call.resolve(value);
  }
}

class FakeSink implements TerminalSink {
  readonly chunks: Uint8Array[] = [];
  exitCode: number | null = null;
  write(bytes: Uint8Array): void {
    this.chunks.push(bytes);
  }
  exited(code: number): void {
    this.exitCode = code;
  }
  text(): string {
    return new TextDecoder().decode(concat(this.chunks));
  }
}

function concat(chunks: Uint8Array[]): Uint8Array {
  const total = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
}

const SESSION: TerminalSession = { id: "term-1", cwd: "/work", shell: "/bin/zsh" };

describe("TerminalSessionController", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  async function openController(rpc: FakeRpc, sink: FakeSink): Promise<TerminalSessionController> {
    const controller = new TerminalSessionController({ client: rpc, chatId: "chat-1", sink });
    const pending = controller.open(80, 24);
    expect(rpc.callsFor("OpenTerminal")).toHaveLength(1);
    expect(rpc.callsFor("OpenTerminal")[0]!.params).toEqual({ chatId: "chat-1", cols: 80, rows: 24 });
    rpc.resolveLast("OpenTerminal", SESSION);
    await pending;
    return controller;
  }

  it("open → subscribe round-trips, with afterSeq resume and no ack barrier", async () => {
    const rpc = new FakeRpc();
    const sink = new FakeSink();
    const controller = await openController(rpc, sink);

    expect(controller.terminalId).toBe("term-1");
    expect(controller.shell).toBe("/bin/zsh");
    expect(rpc.watchMethod).toBe("SubscribeTerminal");
    expect(rpc.watchParams).toEqual({ terminalId: "term-1", afterSeq: 0 });
    // No readiness frame on this stream; the ack barrier would fire on an
    // idle shell after a reconnect (desktop subscribes without one).
    expect(rpc.watchOptions?.ackTimeoutMs).toBe(0);

    // Output streams: base64 Data frames land decoded on the emulator sink,
    // and the resume cursor tracks seq for the next reconnect.
    rpc.watchHandlers!.onItem({ type: "data", seq: 1, data: encodeBase64(new TextEncoder().encode("$ ls\r\n")) }, { generation: 1 });
    expect(sink.text()).toBe("$ ls\r\n");
    expect(rpc.watchParams!.afterSeq).toBe(1);
  });

  it("input reaches the shell, coalesced into one WriteTerminal per 12 ms window", async () => {
    const rpc = new FakeRpc();
    const sink = new FakeSink();
    const controller = await openController(rpc, sink);

    controller.input("l");
    controller.input("s");
    vi.advanceTimersByTime(COALESCE_MS);
    expect(rpc.callsFor("WriteTerminal")).toHaveLength(1);
    const data = rpc.callsFor("WriteTerminal")[0]!.params.data as string;
    expect(new TextDecoder().decode(decodeBase64(data))).toBe("ls");

    controller.input(" -la");
    vi.advanceTimersByTime(COALESCE_MS * 2);
    expect(rpc.callsFor("WriteTerminal")).toHaveLength(2);
  });

  it("buffers input typed while OpenTerminal is in flight", async () => {
    const rpc = new FakeRpc();
    const sink = new FakeSink();
    const controller = new TerminalSessionController({ client: rpc, chatId: "chat-1", sink });
    const pending = controller.open(80, 24);

    controller.input("top\n");
    vi.advanceTimersByTime(COALESCE_MS * 5);
    expect(rpc.callsFor("WriteTerminal")).toHaveLength(0);

    rpc.resolveLast("OpenTerminal", SESSION);
    await pending;
    vi.advanceTimersByTime(COALESCE_MS * 2);
    expect(rpc.callsFor("WriteTerminal")).toHaveLength(1);
    const data = rpc.callsFor("WriteTerminal")[0]!.params.data as string;
    expect(new TextDecoder().decode(decodeBase64(data))).toBe("top\n");
  });

  it("resize debounces 80 ms to the latest grid size", async () => {
    const rpc = new FakeRpc();
    const sink = new FakeSink();
    const controller = await openController(rpc, sink);

    controller.resize(100, 30);
    vi.advanceTimersByTime(RESIZE_DEBOUNCE_MS / 2);
    controller.resize(120, 40);
    vi.advanceTimersByTime(RESIZE_DEBOUNCE_MS);
    expect(rpc.callsFor("ResizeTerminal")).toHaveLength(1);
    expect(rpc.callsFor("ResizeTerminal")[0]!.params).toEqual({ terminalId: "term-1", cols: 120, rows: 40 });

    // A no-op resize (same size) sends nothing.
    controller.resize(120, 40);
    vi.advanceTimersByTime(RESIZE_DEBOUNCE_MS * 2);
    expect(rpc.callsFor("ResizeTerminal")).toHaveLength(1);
  });

  it("exit feeds the trailer, stops the stream, and drops further input", async () => {
    const rpc = new FakeRpc();
    const sink = new FakeSink();
    const controller = await openController(rpc, sink);

    rpc.watchHandlers!.onItem({ type: "exit", seq: 9, exitCode: 3 }, { generation: 1 });
    expect(controller.exitCode).toBe(3);
    expect(sink.exitCode).toBe(3);
    expect(sink.text()).toContain("[process exited 3]");
    expect(rpc.watchCancelled).toBe(1);

    controller.input("x");
    vi.advanceTimersByTime(COALESCE_MS * 2);
    expect(rpc.callsFor("WriteTerminal")).toHaveLength(0);
  });

  it("a failed open lands in the emulator as a red one-liner on a dead tab", async () => {
    const rpc = new FakeRpc();
    const sink = new FakeSink();
    const controller = new TerminalSessionController({ client: rpc, chatId: "chat-1", sink });
    const pending = controller.open(80, 24);
    rpc.callsFor("OpenTerminal")[0]!.reject(new Error("engine offline"));
    await pending;

    expect(controller.exitCode).toBe(-1);
    expect(sink.exitCode).toBe(-1);
    expect(sink.text()).toContain("failed to open terminal: engine offline");
    expect(rpc.watchMethod).toBeNull();
  });

  it("close kills the PTY and cancels the stream", async () => {
    const rpc = new FakeRpc();
    const sink = new FakeSink();
    const controller = await openController(rpc, sink);

    controller.close();
    expect(rpc.callsFor("CloseTerminal")).toHaveLength(1);
    expect(rpc.callsFor("CloseTerminal")[0]!.params).toEqual({ terminalId: "term-1" });
    expect(rpc.watchCancelled).toBe(1);

    controller.input("x");
    vi.advanceTimersByTime(COALESCE_MS * 2);
    expect(rpc.callsFor("WriteTerminal")).toHaveLength(0);
  });

  it("a tab closed mid-open still releases the freshly opened PTY", async () => {
    const rpc = new FakeRpc();
    const sink = new FakeSink();
    const controller = new TerminalSessionController({ client: rpc, chatId: "chat-1", sink });
    const pending = controller.open(80, 24);
    controller.close();
    rpc.resolveLast("OpenTerminal", SESSION);
    await pending;

    expect(rpc.callsFor("CloseTerminal")).toHaveLength(1);
    expect(rpc.callsFor("CloseTerminal")[0]!.params).toEqual({ terminalId: "term-1" });
    expect(rpc.watchMethod).toBeNull();
  });

  it("a stream error on a live tab is surfaced without killing scrollback", async () => {
    const rpc = new FakeRpc();
    const sink = new FakeSink();
    const controller = await openController(rpc, sink);

    rpc.watchHandlers!.onItem({ type: "data", seq: 4, data: encodeBase64(new TextEncoder().encode("old output")) }, { generation: 1 });
    rpc.watchHandlers!.onEnd!(new RpcError("failed", "Terminal not found"));
    expect(controller.exitCode).toBe(-1);
    expect(sink.text()).toContain("old output");
    expect(sink.text()).toContain("terminal stream lost");
    expect(rpc.watchCancelled).toBe(1);
  });

  // ── The Actions attach path (`attach_reserved_session`, panel.rs:536) ──

  it("attach adopts the engine-opened PTY without an OpenTerminal call", async () => {
    const rpc = new FakeRpc();
    const sink = new FakeSink();
    const controller = new TerminalSessionController({ client: rpc, chatId: "chat-1", sink });

    controller.attach({ id: "run-9", cwd: "/work", shell: "bash" });
    // No OpenTerminal — the run RPC already opened the PTY on the owner.
    expect(rpc.callsFor("OpenTerminal")).toHaveLength(0);
    expect(controller.terminalId).toBe("run-9");
    expect(controller.shell).toBe("bash");
    expect(rpc.watchMethod).toBe("SubscribeTerminal");
    expect(rpc.watchParams).toEqual({ terminalId: "run-9", afterSeq: 0 });

    // The replay-then-live stream feeds the tab like an opened one.
    rpc.watchHandlers!.onItem({ type: "data", seq: 2, data: encodeBase64(new TextEncoder().encode("remote-action")) }, { generation: 1 });
    expect(sink.text()).toBe("remote-action");
    expect(rpc.watchParams!.afterSeq).toBe(2);
  });

  it("attach onto a closed tab releases the engine-opened PTY", async () => {
    const rpc = new FakeRpc();
    const sink = new FakeSink();
    const controller = new TerminalSessionController({ client: rpc, chatId: "chat-1", sink });
    controller.close();

    controller.attach({ id: "run-9", cwd: "/work", shell: "bash" });
    // The tab closed while the action's run was in flight — the PTY the
    // engine opened for it is released, never streamed.
    expect(rpc.callsFor("CloseTerminal")).toHaveLength(1);
    expect(rpc.callsFor("CloseTerminal")[0]!.params).toEqual({ terminalId: "run-9" });
    expect(rpc.watchMethod).toBeNull();
  });
});
