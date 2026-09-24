import type { CheckoutDiff } from "@zeron/proto";
import type { EngineClient, WatchHandle } from "@zeron/engine-client";
import { methods, RpcError } from "@zeron/engine-client";
import { diffPhase, resolveDiff, scopeMode, upsertDiffFrame, type DiffPhase, type DiffScope } from "../lib/diff";

/**
 * The Changes store: the live working-tree diff set plus the per-chat scope
 * view (working tree / branch / latest turn / a pinned commit) over a single
 * EngineClient.
 *
 * The `WatchCheckoutDiffs` stream is the source of truth for the
 * working-tree scope; one-shot `GetCheckoutDiff` captures back the branch,
 * latest-turn, and commit scopes. The store resolves a per-chat diff through
 * `resolveDiff` (checkout id first, then device+cwd, then cwd) and keeps a
 * parse cache keyed on the diff checksum so re-renders are cheap.
 *
 * The watch retries itself: a failed or ended stream sets the banner message
 * and re-subscribes after a flat 2s delay, with the last content staying
 * visible underneath (`spawn_watch`'s loop). Scoped-capture failures land in
 * a SEPARATE `scopedError` — they replace the content area, not the banner,
 * and the two known engine-version/turn-state messages are remapped by the
 * view (`render`, changes.rs:4785-4821).
 *
 * Same React-binding contract as the watch cache: `getSnapshot()` is
 * identity-stable until an actual change, `subscribe` fires once per change.
 */

/** The flat watch retry delay (`spawn_watch`, changes.rs:1822). */
const WATCH_RETRY_MS = 2000;

export interface ScopedDiff {
  readonly diff: CheckoutDiff;
  readonly scope: DiffScope;
  readonly baseRef: string | null;
  readonly commitSha: string | null;
  /** Identifier for the (scope, base, commit, checksum) tuple — supersedes stale fetches. */
  readonly key: string;
}

export interface ChangesSnapshot {
  /** Every working-tree diff currently known to the engine. */
  readonly working: readonly CheckoutDiff[];
  /** The diff for the active (scope, base, commit) tuple, if any. */
  readonly scoped: ScopedDiff | null;
  /** Branches for the current chat's checkout (default branch first). */
  readonly branches: readonly string[];
  /** True once the watch has delivered its first item on this generation. */
  readonly watchLoaded: boolean;
  /** The working-tree diff resolved for the selected chat, or null. */
  readonly resolvedForChat: CheckoutDiff | null;
  /** The phase the resolved diff is in (preparing / clean / list). */
  readonly phase: DiffPhase;
  /** A terminal error from the watch stream — the top banner, content stays. */
  readonly error: string | null;
  /** The last scoped capture's failure — replaces the content area. */
  readonly scopedError: string | null;
  /** The connection generation these rows belong to. */
  readonly generation: number;
}

/** The watch surface the store needs — `EngineClient` satisfies it. */
export interface ChangesClient {
  call<T>(method: string, params?: unknown): Promise<T>;
  watch<T>(method: string, params: unknown, handlers: {
    onItem: (item: T, context: { generation: number }) => void;
    onEnd?: (error: RpcError | undefined) => void;
  }): WatchHandle;
}

interface Target {
  readonly checkoutId: string | null;
  readonly deviceId: string;
  readonly cwd: string | null;
  readonly chatId: string | null;
}

const EMPTY_BRANCHES: readonly string[] = [];

export class ChangesStore {
  readonly #client: ChangesClient;
  readonly #target: Target;
  readonly #log: (message: string, detail?: unknown) => void;
  #working: readonly CheckoutDiff[] = [];
  #scoped: ScopedDiff | null = null;
  #scopedKey: string | null = null;
  #scopedInflight: string | null = null;
  #branches: readonly string[] = EMPTY_BRANCHES;
  #branchesFor: string | null = null;
  #branchesInflight: string | null = null;
  #scope: DiffScope = "workingTree";
  #baseRef: string | null = null;
  #commitSha: string | null = null;
  #watchLoaded = false;
  #error: string | null = null;
  #scopedError: string | null = null;
  #generation = 0;
  #snapshot: ChangesSnapshot;
  #handle: WatchHandle | null = null;
  #retryTimer: ReturnType<typeof setTimeout> | null = null;
  #disposed = false;

  constructor(client: EngineClient | ChangesClient, target: Target, options: { log?: (message: string, detail?: unknown) => void } = {}) {
    this.#client = client;
    this.#target = target;
    this.#log = options.log ?? (() => {});
    this.#snapshot = this.#takeSnapshot();
    this.#subscribe();
  }

  getSnapshot(): ChangesSnapshot {
    return this.#snapshot;
  }

  subscribe(listener: () => void): () => void {
    const wrapped = (): void => listener();
    this.#listeners.add(wrapped);
    return () => {
      this.#listeners.delete(wrapped);
    };
  }

  /**
   * Switch scope / base ref / commit. A context change (scope, base, or
   * commit) clears the cached capture so the pane shows the spinner while
   * the new one loads; `setScope("workingTree")` drops the capture outright.
   */
  setScope(scope: DiffScope, baseRef?: string | null, commitSha?: string | null): void {
    const nextBase = baseRef ?? null;
    const nextSha = commitSha ?? null;
    if (this.#scope === scope && this.#baseRef === nextBase && this.#commitSha === nextSha) {
      return;
    }
    const contextChanged = this.#scope !== scope || this.#baseRef !== nextBase || this.#commitSha !== nextSha;
    this.#scope = scope;
    this.#baseRef = nextBase;
    this.#commitSha = nextSha;
    if (contextChanged) {
      this.#scopedKey = null;
      this.#scoped = null;
      this.#scopedError = null;
    }
    if (scope === "workingTree") {
      this.#scoped = null;
      this.#scopedError = null;
    }
    this.#ensureBranches();
    this.#ensureScoped();
    this.#commit();
  }

  setBaseRef(base: string | null): void {
    if (this.#baseRef === base) {
      return;
    }
    this.#baseRef = base;
    this.#scopedKey = null;
    this.#scoped = null;
    this.#scopedError = null;
    this.#ensureScoped();
    this.#commit();
  }

  /**
   * Drop the watch + every in-flight call, then re-subscribe for a fresh
   * first item — the session-swap path. (The watch's own 2s retry loop makes
   * a manual Retry affordance unnecessary; this stays for retargeting.)
   */
  resubscribe(): void {
    if (this.#disposed) {
      return;
    }
    this.#cancelRetry();
    this.#handle?.cancel();
    this.#handle = null;
    this.#working = [];
    this.#scoped = null;
    this.#scopedKey = null;
    this.#scopedInflight = null;
    this.#watchLoaded = false;
    this.#error = null;
    this.#subscribe();
    this.#ensureBranches();
    this.#ensureScoped();
    this.#commit();
  }

  dispose(): void {
    if (this.#disposed) {
      return;
    }
    this.#disposed = true;
    this.#cancelRetry();
    this.#handle?.cancel();
    this.#handle = null;
    this.#listeners.clear();
  }

  #listeners = new Set<() => void>();

  #subscribe(): void {
    this.#handle = this.#client.watch<CheckoutDiff | CheckoutDiff[]>(methods.WATCH_CHECKOUT_DIFFS, {}, {
      onItem: (item, { generation }) => this.#onItem(item, generation),
      onEnd: (error) => this.#onEnd(error),
    });
  }

  #onItem(item: CheckoutDiff | CheckoutDiff[], generation: number): void {
    if (this.#disposed || generation < this.#generation) {
      return;
    }
    if (generation > this.#generation) {
      this.#generation = generation;
      this.#working = [];
      this.#scoped = null;
      this.#scopedKey = null;
      this.#scopedInflight = null;
      this.#branches = EMPTY_BRANCHES;
      this.#branchesFor = null;
      this.#branchesInflight = null;
      this.#watchLoaded = false;
    }
    if (Array.isArray(item)) {
      this.#working = item;
    } else {
      this.#upsertWorking(item);
    }
    this.#watchLoaded = true;
    this.#error = null;
    // The watch's checksum rides the scoped key, so a working-tree change
    // (or a commit — HEAD moves the checksum) re-captures the scoped view
    // while the old one stays visible until the new lands.
    this.#ensureScoped();
    this.#commit();
  }

  #upsertWorking(one: CheckoutDiff): void {
    this.#working = upsertDiffFrame(this.#working, one);
  }

  /**
   * The stream ended or failed: banner, keep the last content, retry in 2s
   * (`spawn_watch`'s loop — "Diff stream interrupted — retrying" for a clean
   * end, "Diff watch unavailable: …" for a subscribe failure).
   */
  #onEnd(error: RpcError | undefined): void {
    if (this.#disposed) {
      return;
    }
    if (error !== undefined) {
      this.#error = `Diff watch unavailable: ${error.message}`;
    } else {
      this.#error = "Diff stream interrupted — retrying";
    }
    this.#scheduleRetry();
    this.#commit();
  }

  #scheduleRetry(): void {
    if (this.#retryTimer !== null || this.#disposed) {
      return;
    }
    this.#retryTimer = setTimeout(() => {
      this.#retryTimer = null;
      if (this.#disposed) {
        return;
      }
      this.#handle?.cancel();
      this.#handle = null;
      this.#subscribe();
      this.#commit();
    }, WATCH_RETRY_MS);
  }

  #cancelRetry(): void {
    if (this.#retryTimer !== null) {
      clearTimeout(this.#retryTimer);
      this.#retryTimer = null;
    }
  }

  #ensureBranches(): void {
    if (this.#target.cwd === null) {
      return;
    }
    const key = `${this.#target.deviceId}:${this.#target.cwd}`;
    if (this.#branchesFor === key || this.#branchesInflight === key) {
      return;
    }
    this.#branchesInflight = key;
    this.#client
      .call<string[]>(methods.LIST_BRANCHES, { repoPath: this.#target.cwd, targetDeviceId: this.#target.deviceId })
      .then((branches) => {
        if (this.#disposed || this.#branchesInflight !== key) {
          return;
        }
        this.#branchesInflight = null;
        this.#branchesFor = key;
        this.#branches = Array.isArray(branches) ? branches : [];
        if (this.#scope === "branch" && this.#baseRef === null && this.#branches.length > 0) {
          const first = this.#branches[0]!;
          if (first !== this.#baseRef) {
            this.#baseRef = first;
            this.#scopedKey = null;
            this.#ensureScoped();
          }
        }
        this.#commit();
      })
      .catch((error: unknown) => {
        if (this.#disposed || this.#branchesInflight !== key) {
          return;
        }
        this.#branchesInflight = null;
        this.#log("changes: list branches failed", describeError(error));
        this.#commit();
      });
  }

  #ensureScoped(): void {
    if (this.#scope === "workingTree") {
      this.#scoped = null;
      this.#scopedKey = null;
      this.#scopedInflight = null;
      return;
    }
    if (this.#scope === "branch" && this.#baseRef === null) {
      this.#scoped = null;
      return;
    }
    if (this.#scope === "commit" && this.#commitSha === null) {
      // A commit-pinned pane without its pin never fetches.
      this.#scoped = null;
      return;
    }
    const chat = {
      checkoutId: this.#target.checkoutId,
      deviceId: this.#target.deviceId,
      cwd: this.#target.cwd,
    };
    const watchSum = resolveDiff(this.#working, chat)?.checksum ?? "";
    const key = `${this.#scope}:${this.#baseRef ?? ""}:${this.#commitSha ?? ""}:${this.#target.cwd ?? ""}:${this.#target.chatId ?? ""}:${watchSum}`;
    if (this.#scopedKey === key || this.#scopedInflight === key) {
      return;
    }
    if (
      this.#scoped !== null &&
      this.#scoped.key === key &&
      this.#scoped.scope === this.#scope &&
      this.#scoped.baseRef === this.#baseRef &&
      this.#scoped.commitSha === this.#commitSha
    ) {
      return;
    }
    this.#scopedInflight = key;
    const params: Record<string, unknown> = {
      cwd: this.#target.cwd ?? "",
      mode: scopeMode(this.#scope),
      targetDeviceId: this.#target.deviceId,
    };
    if (this.#baseRef !== null) {
      params.baseRef = this.#baseRef;
    }
    if (this.#commitSha !== null) {
      params.commitSha = this.#commitSha;
    }
    if (this.#target.chatId !== null) {
      params.chatId = this.#target.chatId;
    }
    this.#client
      .call<CheckoutDiff>(methods.GET_CHECKOUT_DIFF, params)
      .then((diff) => {
        if (this.#disposed || this.#scopedInflight !== key) {
          return;
        }
        this.#scopedInflight = null;
        this.#scopedKey = key;
        this.#scoped = { diff, scope: this.#scope, baseRef: this.#baseRef, commitSha: this.#commitSha, key };
        this.#scopedError = null;
        this.#commit();
      })
      .catch((error: unknown) => {
        if (this.#disposed || this.#scopedInflight !== key) {
          return;
        }
        this.#scopedInflight = null;
        this.#scopedKey = key;
        this.#scoped = null;
        this.#scopedError = describeError(error);
        this.#commit();
      });
  }

  #takeSnapshot(): ChangesSnapshot {
    const chat = {
      checkoutId: this.#target.checkoutId,
      deviceId: this.#target.deviceId,
      cwd: this.#target.cwd,
    };
    const resolved = resolveDiff(this.#working, chat);
    return {
      working: this.#working,
      scoped: this.#scoped,
      branches: this.#branches,
      watchLoaded: this.#watchLoaded,
      resolvedForChat: resolved,
      phase: diffPhase(resolved),
      error: this.#error,
      scopedError: this.#scopedError,
      generation: this.#generation,
    };
  }

  #commit(): void {
    this.#snapshot = this.#takeSnapshot();
    for (const listener of this.#listeners) {
      try {
        listener();
      } catch (error) {
        this.#log("changes store listener threw", describeError(error));
      }
    }
  }
}

function describeError(error: unknown): string {
  if (error instanceof RpcError) {
    if (error.kind === "transport") {
      return "Engine is offline; reconnecting";
    }
    return error.message;
  }
  return error instanceof Error ? error.message : String(error);
}
