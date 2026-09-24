import { afterEach, describe, expect, test } from "vitest";
import { encodeScopedId, type ChatStatus } from "@zeron/engine-client";
import type { SessionStatus } from "@zeron/proto";
import {
  AttentionSoundGate,
  COALESCE_MS,
  ConnectivityNotificationState,
  STARTUP_QUIET_MS,
  chatBannerTexts,
  connectivityBannerTexts,
  connectivitySoundSince,
  echoSendPending,
  notificationPermission,
  onChatNotificationClick,
  postBanner,
  requestNotificationPermission,
  resetPermissionRequestState,
  sessionNotificationState,
  shouldRequestPermission,
  soundSince,
  type SessionNotificationState,
} from "../src/lib/notifications";
import { parseKillSwitch, sessionSoundEnabled } from "../src/lib/sounds";
import { appAttentionGate, resetAppAttentionGate } from "../src/state/attention-gate";
import { EchoStore, UNDELIVERED_GRACE_MS, type PendingSend } from "../src/state/transcript-store";

const NOW = Date.parse("2026-09-16T12:00:00Z");

/**
 * `sound.rs`'s test `baseline` helper: a hand-built state, fresh by default
 * (the Rust struct is private-field-set in tests the same way).
 */
function baseline(indicator: SessionNotificationState["indicator"], turn: string | null): SessionNotificationState {
  return { indicator, lastCompletedTurn: turn, fresh: true };
}

function statusRow(fields: Partial<ChatStatus>): ChatStatus {
  return {
    chatId: "chat",
    deviceId: "device-1",
    status: "working" as SessionStatus,
    startedAt: null,
    updatedAt: "2026-09-16T11:59:30Z",
    lastCompletedTurn: null,
    ...fields,
  };
}

// ---------------------------------------------------------------------------
// The ported desktop checklist (sound.rs tests, exact names)
// ---------------------------------------------------------------------------

test("interruptedAndExpiredActivityNeverChime", () => {
  const working = baseline("working", "old");
  const idle = baseline("none", "old");
  expect(soundSince(idle, working, false)).toBeNull();
  // An older host without explicit completion metadata is silent too.
  expect(soundSince(baseline("none", null), baseline("working", null), false)).toBeNull();
});

test("aRunErrorChimesOnceAndNeverMasqueradesAsCompletion", () => {
  const working = baseline("working", "old");
  const errored = baseline("errored", "failed");
  expect(soundSince(errored, working, false)).toBe("attention");
  expect(soundSince(errored, errored, false)).toBeNull();
});

test("durableConnectivityDegradationChimesOncePerOutage", () => {
  expect(connectivitySoundSince("connected", "disabled")).toBeNull();
  expect(connectivitySoundSince("reconnecting", "connected")).toBe("attention");
  expect(connectivitySoundSince("offline", "reconnecting")).toBeNull();
  expect(connectivitySoundSince("connected", "offline")).toBeNull();
  expect(connectivitySoundSince("offline", "connected")).toBe("attention");
});

test("ordinaryQueueCompletionsSurviveCoalescedWorkingStates", () => {
  const first = baseline("working", null);
  const second = baseline("working", "first");
  expect(soundSince(second, first, false)).toBe("done");
  expect(soundSince(second, second, false)).toBeNull();
  const last = baseline("none", "second");
  expect(soundSince(last, second, false)).toBe("done");
  expect(soundSince(last, last, false)).toBeNull();
});

test("pendingSendConsumesCompletionButPreservesInputRequests", () => {
  const before = baseline("working", null);
  const settled = baseline("none", "first");
  expect(soundSince(settled, before, true)).toBeNull();
  expect(soundSince(settled, settled, false)).toBeNull();
  const question = baseline("awaitingInput", "first");
  expect(soundSince(question, settled, true)).toBe("request");
  expect(soundSince(question, question, false)).toBeNull();
});

test("staleCompletionIsConsumedWithoutReplayingOnAHeartbeat", () => {
  const before = baseline("working", null);
  const stale: SessionNotificationState = { ...baseline("none", "old"), fresh: false };
  expect(soundSince(stale, before, false)).toBeNull();
  const refreshed = baseline("working", "old");
  expect(soundSince(refreshed, stale, false)).toBeNull();
});

// ---------------------------------------------------------------------------
// The driver's send_pending probe (the echo overlay's id namespace)
// ---------------------------------------------------------------------------

describe("echoSendPending", () => {
  const ENGINE = "http://engine-a:27699";
  const RAW = "chat-1";

  function send(overrides: Partial<PendingSend> = {}): PendingSend {
    return {
      messageId: "msg-1",
      chatId: encodeScopedId(ENGINE, RAW),
      startedAtMs: 0,
      text: "hello",
      attachmentPaths: [],
      ...overrides,
    };
  }

  test("an echo recorded under the scoped page id suppresses the driver's raw status row", () => {
    const echoes = new EchoStore();
    echoes.pushEcho(send());
    // The composer publishes under the scoped PAGE id; the driver's status
    // row carries the engine's RAW chat id — the probe bridges the two.
    expect(echoSendPending(echoes, ENGINE, RAW, 1_000)).toBe(true);
    // The suppression is honest: past the grace window nothing is pending.
    expect(echoSendPending(echoes, ENGINE, RAW, UNDELIVERED_GRACE_MS + 1)).toBe(false);
  });

  test("a raw-keyed echo (the pre-fix namespace) never matches the probe", () => {
    const echoes = new EchoStore();
    echoes.pushEcho(send({ chatId: RAW }));
    expect(echoSendPending(echoes, ENGINE, RAW, 1_000)).toBe(false);
  });

  test("another engine's scope never matches", () => {
    const echoes = new EchoStore();
    echoes.pushEcho(send());
    expect(echoSendPending(echoes, "http://engine-b:27699", RAW, 1_000)).toBe(false);
  });
});

test("connectivityBootOutagesSeedSilentlyThenLaterOutagesAlert", () => {
  const t0 = 0;

  // Warm daemon: the first authoritative snapshot is already degraded.
  const warm = new ConnectivityNotificationState();
  expect(warm.update("disabled", false, t0)).toBeNull();
  expect(warm.update("offline", true, t0)).toBeNull();
  expect(warm.update("connected", true, t0 + 6_000)).toBeNull();
  expect(warm.update("offline", true, t0 + 7_000)).toBe("attention");

  // Cold daemon: the engine's grace initially masks the existing outage.
  const cold = new ConnectivityNotificationState();
  expect(cold.update("connected", true, t0)).toBeNull();
  expect(cold.update("offline", true, t0 + 4_000)).toBeNull();
  expect(cold.update("connected", true, t0 + 5_000)).toBeNull();
  expect(cold.update("reconnecting", true, t0 + 6_000)).toBe("attention");

  // A quiet healthy boot may not publish another frame until the first
  // genuine outage; elapsed time arms that transition itself.
  const healthy = new ConnectivityNotificationState();
  expect(healthy.update("connected", true, t0)).toBeNull();
  expect(healthy.update("offline", true, t0 + 6_000)).toBe("attention");

  // Replacing the runtime returns to bootstrap semantics even if the
  // prior runtime had already armed alerts.
  expect(healthy.update("disabled", false, t0 + 7_000)).toBeNull();
  expect(healthy.update("offline", true, t0 + 7_000)).toBeNull();
});

test("attentionGateCoalescesSessionAndConnectivityWatchCallbacks", () => {
  const t0 = 0;
  const gate = new AttentionSoundGate();
  expect(gate.shouldPlay(t0)).toBe(true);
  expect(gate.shouldPlay(t0)).toBe(false);
  expect(gate.shouldPlay(t0 + 200)).toBe(false);
  expect(gate.shouldPlay(t0 + 250)).toBe(true);
});

test("theAppGlobalGateCoalescesAttentionAcrossEnginesDrivers", () => {
  // The hoisting fix (shell.rs:1147/1482: ONE gate on the shell, never one
  // per watch): two drivers — one per engine — both earn "attention" in the
  // same burst, and the shared instance lets only the first chime through.
  resetAppAttentionGate();
  const t0 = 10_000;
  expect(appAttentionGate.shouldPlay(t0)).toBe(true);
  expect(appAttentionGate.shouldPlay(t0)).toBe(false);
  expect(appAttentionGate.shouldPlay(t0 + COALESCE_MS)).toBe(true);
  resetAppAttentionGate();
});

// ---------------------------------------------------------------------------
// Baseline construction (SessionNotificationState::new)
// ---------------------------------------------------------------------------

describe("sessionNotificationState", () => {
  test("builds the staleness-gated indicator, marker, and freshness flag", () => {
    const fresh = sessionNotificationState(
      statusRow({ status: "awaitingInput", updatedAt: "2026-09-16T11:59:40Z", lastCompletedTurn: "turn-1" }),
      NOW,
    );
    expect(fresh).toEqual({ indicator: "awaitingInput", lastCompletedTurn: "turn-1", fresh: true });

    // Errored is exempt from the staleness rule (view.rs).
    const errored = sessionNotificationState(statusRow({ status: "errored", updatedAt: "2026-09-16T11:00:00Z" }), NOW);
    expect(errored.indicator).toBe("errored");
    expect(errored.fresh).toBe(false);

    // A stale working row is dead — never an eternal Working, never fresh.
    const stale = sessionNotificationState(statusRow({ updatedAt: "2026-09-16T11:00:00Z" }), NOW);
    expect(stale.indicator).toBe("none");
    expect(stale.fresh).toBe(false);
  });

  test("freshness uses the shared SESSION_STALE_MS window boundary", () => {
    const at = (offsetMs: number): string => new Date(NOW - offsetMs).toISOString();
    expect(sessionNotificationState(statusRow({ updatedAt: at(45_000) }), NOW).fresh).toBe(true);
    expect(sessionNotificationState(statusRow({ updatedAt: at(45_001) }), NOW).fresh).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Settings gate and kill-switch (settings.rs:1138, sound.rs:19 / notify.rs:27)
// ---------------------------------------------------------------------------

describe("sessionSoundEnabled", () => {
  const on = {
    soundEnabled: true,
    soundCompletionEnabled: true,
    soundInputEnabled: true,
    soundAttentionEnabled: true,
  };

  test("the master toggle mutes every kind", () => {
    for (const sound of ["done", "request", "attention"] as const) {
      expect(sessionSoundEnabled({ ...on, soundEnabled: false }, sound)).toBe(false);
    }
  });

  test("each kind answers to its own toggle", () => {
    expect(sessionSoundEnabled({ ...on, soundCompletionEnabled: false }, "done")).toBe(false);
    expect(sessionSoundEnabled({ ...on, soundCompletionEnabled: false }, "request")).toBe(true);
    expect(sessionSoundEnabled({ ...on, soundInputEnabled: false }, "request")).toBe(false);
    expect(sessionSoundEnabled({ ...on, soundAttentionEnabled: false }, "attention")).toBe(false);
    expect(sessionSoundEnabled({ ...on, soundAttentionEnabled: false }, "done")).toBe(true);
  });
});

describe("parseKillSwitch", () => {
  test("absent flags leave both features enabled", () => {
    expect(parseKillSwitch("")).toEqual({ sound: false, notifications: false });
    expect(parseKillSwitch("?other=1")).toEqual({ sound: false, notifications: false });
  });

  test("query flags disable each half independently", () => {
    expect(parseKillSwitch("?disableSound=1")).toEqual({ sound: true, notifications: false });
    expect(parseKillSwitch("?disableNotifications=1")).toEqual({ sound: false, notifications: true });
    expect(parseKillSwitch("?disableSound=1&disableNotifications=1")).toEqual({
      sound: true,
      notifications: true,
    });
  });

  test("explicitly-off values do not disable", () => {
    expect(parseKillSwitch("?disableSound=0").sound).toBe(false);
    expect(parseKillSwitch("?disableSound=false").sound).toBe(false);
    expect(parseKillSwitch("?disableSound=").sound).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Banner texts (§2.6, verbatim) and the Notification wrapper
// ---------------------------------------------------------------------------

describe("banner texts", () => {
  test("chat banners carry the chat title or the New session fallback", () => {
    expect(chatBannerTexts("done", "Fix the parser")).toEqual({ title: "Fix the parser", body: "Run finished" });
    expect(chatBannerTexts("request", null)).toEqual({ title: "New session", body: "Waiting on your input" });
    expect(chatBannerTexts("attention", null)).toEqual({ title: "New session", body: "Run failed" });
  });

  test("connectivity banners split offline from reconnecting", () => {
    expect(connectivityBannerTexts("offline")).toEqual({
      title: "Connection unavailable",
      body: "Your device is offline",
    });
    expect(connectivityBannerTexts("reconnecting")).toEqual({
      title: "Connection unavailable",
      body: "Zeron is trying to reconnect",
    });
  });
});

describe("the Notification wrapper", () => {
  interface FakeBanner {
    readonly title: string;
    readonly options: { body?: string; data?: unknown };
    readonly closed: { value: boolean };
    onclick: ((event: Event) => void) | null;
    fireClick(): void;
  }

  interface InstalledApi {
    readonly banners: FakeBanner[];
    readonly requests: { value: number };
    setPermission(permission: string): void;
  }

  function installNotificationApi(permission: string): InstalledApi {
    const banners: FakeBanner[] = [];
    const requests = { value: 0 };
    let currentPermission = permission;
    class FakeNotification {
      onclick: ((event: Event) => void) | null = null;
      readonly title: string;
      readonly options: { body?: string; data?: unknown };
      readonly closed = { value: false };
      constructor(title: string, options: { body?: string; data?: unknown }) {
        this.title = title;
        this.options = options;
        banners.push(this);
      }
      close(): void {
        this.closed.value = true;
      }
      fireClick(): void {
        this.onclick?.({ target: this } as unknown as Event);
      }
    }
    const constructor = FakeNotification as unknown as {
      permission: string;
      requestPermission: () => Promise<string>;
    };
    Object.defineProperty(constructor, "permission", {
      get: () => currentPermission,
    });
    constructor.requestPermission = () => {
      requests.value += 1;
      return Promise.resolve(currentPermission);
    };
    (globalThis as { Notification?: unknown }).Notification = FakeNotification;
    (globalThis as { window?: unknown }).window = { focus: () => {} };
    return {
      banners,
      requests,
      setPermission: (next: string) => {
        currentPermission = next;
      },
    };
  }

  afterEach(() => {
    delete (globalThis as { Notification?: unknown }).Notification;
    delete (globalThis as { window?: unknown }).window;
    resetPermissionRequestState();
  });

  test("posts with the chat id payload and routes clicks to the chat handler", () => {
    const api = installNotificationApi("granted");
    const clicks: string[] = [];
    const uninstall = onChatNotificationClick((chatId) => clicks.push(chatId));

    postBanner("Fix the parser", "Run finished", "chat-42");
    expect(api.banners).toHaveLength(1);
    expect(api.banners[0]!.title).toBe("Fix the parser");
    expect(api.banners[0]!.options.body).toBe("Run finished");
    expect(api.banners[0]!.options.data).toEqual({ chatId: "chat-42" });

    // Simulate the banner click: the banner closes, the app focuses, and the
    // click routes to the chat handler.
    api.banners[0]!.fireClick();
    expect(api.banners[0]!.closed.value).toBe(true);
    expect(clicks).toEqual(["chat-42"]);
    uninstall();
  });

  test("connectivity banners carry no chat id and never route", () => {
    const api = installNotificationApi("granted");
    const clicks: string[] = [];
    const uninstall = onChatNotificationClick((chatId) => clicks.push(chatId));

    postBanner("Connection unavailable", "Your device is offline");
    expect(api.banners).toHaveLength(1);
    expect(api.banners[0]!.options.data).toBeUndefined();
    api.banners[0]!.fireClick();
    expect(clicks).toEqual([]);
    uninstall();
  });

  test("silently no-ops without permission or the API", () => {
    const api = installNotificationApi("denied");
    postBanner("Fix the parser", "Run finished", "chat-42");
    expect(api.banners.length).toBe(0);

    delete (globalThis as { Notification?: unknown }).Notification;
    expect(notificationPermission()).toBeNull();
    postBanner("Fix the parser", "Run finished", "chat-42");
    expect(api.banners.length).toBe(0);
  });

  test("requestNotificationPermission asks exactly once per session and never re-prompts settled answers", async () => {
    const api = installNotificationApi("default");
    expect(shouldRequestPermission("default", false)).toBe(true);
    expect(shouldRequestPermission("default", true)).toBe(false);
    expect(shouldRequestPermission("granted", false)).toBe(false);
    expect(shouldRequestPermission("denied", false)).toBe(false);
    expect(shouldRequestPermission(null, false)).toBe(false);

    await requestNotificationPermission();
    await requestNotificationPermission();
    // Only one browser request for two toggle-ons.
    expect(api.requests.value).toBe(1);

    // A settled answer short-circuits without a browser request.
    api.setPermission("granted");
    await requestNotificationPermission();
    expect(api.requests.value).toBe(1);
  });
});

// ---------------------------------------------------------------------------
// Exported constants match the desktop's numbers
// ---------------------------------------------------------------------------

test("the coalescing and quiet-period constants match sound.rs", () => {
  expect(COALESCE_MS).toBe(250);
  expect(STARTUP_QUIET_MS).toBe(5_000);
});
