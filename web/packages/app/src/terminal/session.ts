import type { EngineClient, WatchHandle } from "@zeron/engine-client";
import { methods } from "@zeron/engine-client";
import type { TerminalEvent, TerminalSession } from "@zeron/proto";
import { COALESCE_MS, RESIZE_DEBOUNCE_MS, decodeBase64, encodeBase64, exitMessage } from "./tabs";

/**
 * The engine half of one terminal tab, mirroring the desktop panel's
 * per-tab data path (`crates/ui/src/terminal/panel.rs`):
 *
 * - `OpenTerminal {chatId, cols, rows}` → `TerminalSession`; a failure lands
 *   in the emulator as a red one-liner and a dead tab (exited = -1).
 * - `SubscribeTerminal {terminalId, afterSeq}` streams replay-then-live-tail
 *   `TerminalEvent`s; `seq` is tracked so engine-client's automatic
 *   re-subscribe after a reconnect resumes instead of replaying (the params
 *   object below is serialized at subscribe time, so mutating `afterSeq`
 *   carries the resume cursor across reconnects — emulator content survives
 *   a drop exactly like the desktop's).
 * - Keyboard input coalesces for 12 ms before `WriteTerminal` (base64).
 * - Resizes debounce 80 ms before `ResizeTerminal`; the emulator itself is
 *   resized immediately by the caller (desktop: prepaint resizes the grid,
 *   the RPC lags).
 * - `Exit` feeds the dim `[process exited N]` trailer and stops the stream;
 *   detaching (tab closed here = `CloseTerminal`) kills the PTY.
 *
 * DOM-free by design: output lands on the `TerminalSink`, which the xterm
 * adapter implements.
 */
export interface TerminalSink {
  /** Raw PTY output bytes (a decoded base64 `Data` frame). */
  write(bytes: Uint8Array): void;
  /** The shell exited; `code` is the process exit code. */
  exited(code: number): void;
}

/** The slice of `EngineClient` a terminal session drives. */
export type TerminalRpc = Pick<EngineClient, "call" | "watch">;

export interface TerminalSessionOptions {
  readonly client: TerminalRpc;
  readonly chatId: string;
  readonly sink: TerminalSink;
}

type Timer = ReturnType<typeof setTimeout> | undefined;

export class TerminalSessionController {
  readonly #client: TerminalRpc;
  readonly #chatId: string;
  readonly #sink: TerminalSink;

  #terminalId: string | null = null;
  #shell: string | null = null;
  #exitCode: number | null = null;
  #watch: WatchHandle | null = null;
  #lastSeq = 0;
  #cols = 0;
  #rows = 0;
  #pendingResize: { cols: number; rows: number } | null = null;
  #resizeTimer: Timer;
  #inputChunks: string[] = [];
  #flushTimer: Timer;
  #closed = false;

  constructor(options: TerminalSessionOptions) {
    this.#client = options.client;
    this.#chatId = options.chatId;
    this.#sink = options.sink;
  }

  /** The engine-side terminal id once `OpenTerminal` has answered. */
  get terminalId(): string | null {
    return this.#terminalId;
  }

  /** The shell basename reported by `OpenTerminal` (for the tab label). */
  get shell(): string | null {
    return this.#shell;
  }

  /** The exit code once the shell exited; `-1` when the open itself failed. */
  get exitCode(): number | null {
    return this.#exitCode;
  }

  get exited(): boolean {
    return this.#exitCode !== null;
  }

  /** Open the PTY and start streaming. The grid size is the initial size. */
  async open(cols: number, rows: number): Promise<void> {
    this.#cols = cols;
    this.#rows = rows;
    let opened: TerminalSession;
    try {
      opened = await this.#client.call<TerminalSession>(methods.OPEN_TERMINAL, {
        chatId: this.#chatId,
        cols,
        rows,
      });
    } catch (error) {
      this.#exitCode = -1;
      const message = error instanceof Error ? error.message : String(error);
      this.#sink.write(new TextEncoder().encode(`\x1b[31mfailed to open terminal: ${message}\x1b[0m\r\n`));
      this.#sink.exited(-1);
      return;
    }
    if (this.#closed) {
      // The tab closed while the open was in flight — release the PTY
      // (desktop spawn_session's !attached path).
      void this.#client.call(methods.CLOSE_TERMINAL, { terminalId: opened.id }).catch(() => {});
      return;
    }
    this.#terminalId = opened.id;
    this.#shell = opened.shell;
    this.#subscribe();
  }

  /**
   * Adopt a PTY the engine already opened (project Actions' `RunProjectAction`
   * reply — desktop `attach_reserved_session`, panel.rs:536): no
   * `OpenTerminal` call, just the id/shell and the replay-then-live stream.
   * A tab closed while the run was in flight releases the PTY instead.
   */
  attach(opened: TerminalSession): void {
    if (this.#closed) {
      void this.#client.call(methods.CLOSE_TERMINAL, { terminalId: opened.id }).catch(() => {});
      return;
    }
    this.#terminalId = opened.id;
    this.#shell = opened.shell;
    this.#subscribe();
  }

  /**
   * Queue keyboard bytes (xterm's `onData` payload). Coalesces for 12 ms
   * before the `WriteTerminal` flush; input on an exited tab is dropped
   * (desktop queue_input). A flush while `OpenTerminal` is still in flight
   * keeps the buffer and retries shortly, so fast typists lose nothing.
   */
  input(data: string): void {
    if (this.#closed || this.exited) {
      return;
    }
    this.#inputChunks.push(data);
    this.#flushTimer ??= setTimeout(() => this.#flushInput(), COALESCE_MS);
  }

  /**
   * The emulator resized (already applied locally): debounce 80 ms, then
   * send the *latest* size — desktop on_grid_metrics semantics.
   */
  resize(cols: number, rows: number): void {
    if (this.#closed || this.exited || (cols === this.#cols && rows === this.#rows)) {
      return;
    }
    this.#cols = cols;
    this.#rows = rows;
    this.#pendingResize = { cols, rows };
    this.#resizeTimer ??= setTimeout(() => this.#flushResize(), RESIZE_DEBOUNCE_MS);
  }

  /**
   * Close the tab: kill the PTY (fire-and-forget `CloseTerminal`), stop the
   * stream, drop pending input. Idempotent.
   */
  close(): void {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    clearTimeout(this.#flushTimer);
    this.#flushTimer = undefined;
    clearTimeout(this.#resizeTimer);
    this.#resizeTimer = undefined;
    this.#inputChunks = [];
    this.#watch?.cancel();
    this.#watch = null;
    const id = this.#terminalId;
    this.#terminalId = null;
    // Desktop close_tab sends CloseTerminal whenever the id resolved —
    // exited sessions linger 30 min server-side and this reaps them early.
    if (id !== null) {
      void this.#client.call(methods.CLOSE_TERMINAL, { terminalId: id }).catch(() => {});
    }
  }

  #subscribe(): void {
    const id = this.#terminalId;
    if (id === null || this.#closed) {
      return;
    }
    // Serialized at subscribe time by the codec: mutating `afterSeq` as
    // items arrive makes engine-client's reconnect re-subscribe resume from
    // the cursor (desktop: `SubscribeTerminal {terminalId, afterSeq}` in the
    // pump loop) instead of replaying the bounded window over live content.
    const params: { terminalId: string; afterSeq: number } = { terminalId: id, afterSeq: this.#lastSeq };
    this.#watch = this.#client.watch<TerminalEvent>(
      methods.SUBSCRIBE_TERMINAL,
      params,
      {
        onItem: (event) => this.#onEvent(params, event),
        onEnd: (error) => this.#onStreamEnd(error),
      },
      // No readiness frame on this stream and an idle shell can be silent
      // for minutes after an afterSeq resume — the ack barrier would fire
      // spuriously (desktop subscribes without one).
      { ackTimeoutMs: 0 },
    );
  }

  #onEvent(params: { terminalId: string; afterSeq: number }, event: TerminalEvent): void {
    if (event.type === "data") {
      this.#lastSeq = event.seq;
      params.afterSeq = event.seq;
      this.#sink.write(decodeBase64(event.data));
      return;
    }
    this.#lastSeq = event.seq;
    params.afterSeq = event.seq;
    this.#exitCode = event.exitCode;
    this.#sink.write(new TextEncoder().encode(exitMessage(event.exitCode)));
    this.#sink.exited(event.exitCode);
    // The stream ends here; cancel so a later reconnect does not resubscribe
    // a dead PTY (its replay buffer expires after 30 min anyway).
    this.#watch?.cancel();
    this.#watch = null;
  }

  #onStreamEnd(error: unknown): void {
    if (this.#closed || this.exited || error === undefined) {
      return;
    }
    // A same-connection stream failure (engine restart loses the PTY;
    // reconnect re-subscribe then gets "Terminal not found"). Surface it
    // like the open failure; the tab stays for its scrollback.
    const message = error instanceof Error ? error.message : String(error);
    this.#exitCode = -1;
    this.#sink.write(new TextEncoder().encode(`\r\n\x1b[31m[terminal stream lost: ${message}]\x1b[0m\r\n`));
    this.#sink.exited(-1);
    this.#watch?.cancel();
    this.#watch = null;
  }

  #flushInput(): void {
    this.#flushTimer = undefined;
    const id = this.#terminalId;
    if (id === null) {
      // OpenTerminal still in flight — keep the buffer, retry shortly.
      if (!this.#closed && !this.exited && this.#inputChunks.length > 0) {
        this.#flushTimer = setTimeout(() => this.#flushInput(), COALESCE_MS);
      }
      return;
    }
    const text = this.#inputChunks.join("");
    this.#inputChunks = [];
    if (text.length === 0) {
      return;
    }
    const data = encodeBase64(new TextEncoder().encode(text));
    void this.#client.call(methods.WRITE_TERMINAL, { terminalId: id, data }).catch(() => {
      // Offline mid-keystroke: the desktop fire-and-forgets too (the error
      // is logged there; here the drop is silent and the shell stays put).
    });
  }

  #flushResize(): void {
    this.#resizeTimer = undefined;
    const id = this.#terminalId;
    const pending = this.#pendingResize;
    this.#pendingResize = null;
    if (id === null || pending === null || this.#closed || this.exited) {
      return;
    }
    void this.#client
      .call(methods.RESIZE_TERMINAL, { terminalId: id, cols: pending.cols, rows: pending.rows })
      .catch(() => {});
  }
}
