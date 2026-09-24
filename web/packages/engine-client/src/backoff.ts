/**
 * Reconnect backoff, mirroring the desktop engine registry's supervise loop
 * (crates/ui/src/engine_registry.rs): start at `initialMs`, double per retry,
 * cap at `maxMs`, add uniform jitter up to `jitterMs` (the desktop derives it
 * from one UUID byte — 0..=255ms), and reset to `initialMs` after a
 * connection that lived at least `resetAfterMs`.
 */
export interface BackoffOptions {
  readonly initialMs?: number;
  readonly maxMs?: number;
  readonly jitterMs?: number;
  readonly resetAfterMs?: number;
  readonly random?: () => number;
}

const DEFAULT_INITIAL_MS = 500;
const DEFAULT_MAX_MS = 15_000;
const DEFAULT_JITTER_MS = 256;
const DEFAULT_RESET_AFTER_MS = 10_000;

export class ReconnectBackoff {
  readonly #initialMs: number;
  readonly #maxMs: number;
  readonly #jitterMs: number;
  readonly #resetAfterMs: number;
  readonly #random: () => number;
  #delayMs: number;

  constructor(options: BackoffOptions = {}) {
    this.#initialMs = options.initialMs ?? DEFAULT_INITIAL_MS;
    this.#maxMs = options.maxMs ?? DEFAULT_MAX_MS;
    this.#jitterMs = options.jitterMs ?? DEFAULT_JITTER_MS;
    this.#resetAfterMs = options.resetAfterMs ?? DEFAULT_RESET_AFTER_MS;
    this.#random = options.random ?? Math.random;
    this.#delayMs = this.#initialMs;
  }

  /**
   * Delay to sleep before the next dial, given how long the last connection
   * lived. A connection that outlived `resetAfterMs` restarts the curve —
   * a stable engine should not inherit the penalty of a flappy one.
   */
  nextDelayMs(lifetimeMs: number): number {
    if (lifetimeMs > this.#resetAfterMs) {
      this.#delayMs = this.#initialMs;
    }
    const delay = this.#delayMs + Math.floor(this.#random() * this.#jitterMs);
    this.#delayMs = Math.min(this.#delayMs * 2, this.#maxMs);
    return delay;
  }
}
