import { useCallback, useEffect, useRef, useState } from "react";
import { Icon } from "@zeron/icons";
import type { AgentAccount, AgentAccountsSnapshot, AgentLoginStart, HarnessId } from "@zeron/proto";
import { useEngineSession } from "../state/session-provider";
import { useNow, useWatchSnapshot } from "../state/hooks";
import { DeviceSwitcher } from "../components/ui/DeviceSwitcher";
import {
  BtnGhost,
  BtnPrimary,
  Dialog,
  DialogBody,
  DialogCard,
  DialogField,
  DialogTitle,
} from "../components/ui/Dialog";
import { SettingsEngineIndicator } from "../components/settings-engine-indicator";
import {
  accountInitial,
  accountLabel,
  activateAgentAccount,
  cancelAgentLogin,
  completeAgentLogin,
  forceUsageFor,
  forgetAgentAccount,
  formatReset,
  listAgentAccounts,
  loginTitle,
  pollAgentLogin,
  providerAccounts,
  providerEmptyCopy,
  PROVIDERS,
  startAgentLogin,
  usageColorVar,
  usageFallback,
  usageLevel,
  type LoadTrigger,
  type ProviderDescriptor,
} from "../lib/accounts";

/**
 * Harness accounts settings (desktop settings/accounts.rs parity): one
 * provider section per harness CLI (Claude Code, Codex, Cursor) with its
 * account rows — email, plan and Active badges, usage meters, Switch and
 * Forget on inactive rows — plus the add-account login flows (paste-code
 * and browser-poll) and the page-header device switcher that retargets
 * every call at another paired device via `targetDeviceId`. The visit's
 * first list forces a usage probe; post-action lists ride the still-warm
 * cache. All RPC failures render inline.
 */

type Loadable = { kind: "loading" } | { kind: "ready"; snapshot: AgentAccountsSnapshot } | { kind: "error"; message: string };

type LoginFlow =
  | { kind: "starting"; harness: HarnessId }
  | { kind: "paste-code"; harness: HarnessId; start: AgentLoginStart; submitting: boolean; error: string | null }
  | { kind: "browser"; harness: HarnessId; start: AgentLoginStart; message: string | null; error: string | null };

export function AccountsSettingsPage() {
  const session = useEngineSession();
  const client = session?.client ?? null;
  const snapshot = useWatchSnapshot(session);
  const [target, setTarget] = useState<string | null>(null);
  const [snapshotState, setSnapshot] = useState<Loadable>({ kind: "loading" });
  const [busyAccount, setBusyAccount] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [login, setLogin] = useState<LoginFlow | null>(null);
  const now = useNow(30_000);

  const load = useCallback(
    async (trigger: LoadTrigger) => {
      if (client === null) {
        setSnapshot({ kind: "error", message: "Engine not connected" });
        return;
      }
      setSnapshot({ kind: "loading" });
      try {
        setSnapshot({ kind: "ready", snapshot: await listAgentAccounts(client, forceUsageFor(trigger), target) });
      } catch (cause) {
        setSnapshot({ kind: "error", message: cause instanceof Error ? cause.message : String(cause) });
      }
    },
    [client, target],
  );

  // Mount load (forced usage probe), a retarget, or an engine switch.
  useEffect(() => {
    setBusyAccount(null);
    setActionError(null);
    setLogin(null);
    void load("mount");
  }, [load]);

  // The browser-flow wait loop: poll until the login lands or is dismissed.
  const browserLoginId = login?.kind === "browser" ? login.start.loginId : null;
  useEffect(() => {
    if (client === null || browserLoginId === null) {
      return;
    }
    let active = true;
    void (async () => {
      const poll = await pollAgentLogin(client, browserLoginId, {
        isActive: () => active,
        onPending: (message) => {
          if (active && message !== null) {
            setLogin((current) => (current?.kind === "browser" ? { ...current, message } : current));
          }
        },
      }, target);
      if (!active || poll === null) {
        return;
      }
      if (poll.status === "done") {
        setLogin(null);
        void load("postLogin");
      } else {
        setLogin((current) =>
          current?.kind === "browser" ? { ...current, error: poll.message ?? "Login failed" } : current,
        );
      }
    })();
    return () => {
      active = false;
    };
  }, [client, browserLoginId, load, target]);

  function accountAction(action: "activate" | "forget", account: AgentAccount) {
    if (client === null || busyAccount !== null) {
      return;
    }
    setBusyAccount(account.id);
    setActionError(null);
    void (async () => {
      try {
        if (action === "activate") {
          await activateAgentAccount(client, account, target);
        } else {
          await forgetAgentAccount(client, account, target);
        }
        void load("postAction");
      } catch (cause) {
        setActionError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        setBusyAccount(null);
      }
    })();
  }

  function addAccount(harness: HarnessId) {
    if (client === null || login !== null) {
      return;
    }
    // Pre-open a blank tab inside the click gesture: after the RPC await,
    // popup blockers treat a fresh window.open as un-gestured and may eat
    // it. No `noopener` here — the handle is needed to navigate the tab
    // once the start reply lands (the opener is severed right after).
    const tab = window.open("about:blank", "_blank");
    setActionError(null);
    setLogin({ kind: "starting", harness });
    void (async () => {
      try {
        const start = await startAgentLogin(client, harness, target);
        if (start.cliOpensBrowser) {
          // The engine machine's CLI already opened the page — one tab total.
          tab?.close();
        } else if (tab !== null) {
          tab.location.href = start.url;
          tab.opener = null;
        } else {
          // Hard blocker ate the pre-open — retry the old direct open.
          window.open(start.url, "_blank", "noopener,noreferrer");
        }
        setLogin(
          start.mode === "paste-code"
            ? { kind: "paste-code", harness, start, submitting: false, error: null }
            : { kind: "browser", harness, start, message: null, error: null },
        );
      } catch (cause) {
        tab?.close();
        setLogin(null);
        setActionError(`Login failed to start: ${cause instanceof Error ? cause.message : String(cause)}`);
      }
    })();
  }

  function submitCode(code: string) {
    if (client === null || login?.kind !== "paste-code" || login.submitting) {
      return;
    }
    const trimmed = code.trim();
    if (trimmed.length === 0) {
      return;
    }
    const loginId = login.start.loginId;
    setLogin({ ...login, submitting: true, error: null });
    void (async () => {
      try {
        await completeAgentLogin(client, loginId, trimmed, target);
        setLogin(null);
        void load("postLogin");
      } catch (cause) {
        setLogin((current) =>
          current?.kind === "paste-code"
            ? { ...current, submitting: false, error: cause instanceof Error ? cause.message : String(cause) }
            : current,
        );
      }
    })();
  }

  function dismissLogin() {
    const loginId = login?.kind === "paste-code" || login?.kind === "browser" ? login.start.loginId : null;
    setLogin(null);
    if (client !== null && loginId !== null) {
      void cancelAgentLogin(client, loginId, target).catch(() => {});
    }
  }

  /**
   * `set_target_device` (accounts.rs:242-256): a different device is a
   * different accounts world — drop the in-flight login/action state; the
   * `load` effect reloads with a forced usage probe (the new device's cache
   * is cold).
   */
  function setTargetDevice(next: string | null) {
    if (next === target) {
      return;
    }
    setTarget(next);
    setLogin(null);
    setBusyAccount(null);
    setActionError(null);
  }

  const devices = (snapshot?.devices.rows ?? [])
    .slice()
    .sort((a, b) => (a.createdAt ?? "").localeCompare(b.createdAt ?? "") || a.id.localeCompare(b.id));
  const localDeviceId = session?.client.engineInfo?.deviceId ?? null;
  const refreshing = snapshotState.kind === "loading";
  const accountCount = snapshotState.kind === "ready" && snapshotState.snapshot.accounts.length > 0 ? snapshotState.snapshot.accounts.length : null;

  return (
    <div className="settings-page">
      <div className="settings-title-row">
        <h1 className="settings-title">
          Accounts{accountCount !== null ? <span className="settings-title-count">{accountCount}</span> : null}
        </h1>
        <div className="settings-header-actions">
          <button
            type="button"
            className={`settings-ghost-action settings-refresh ${refreshing ? "settings-refresh-busy" : ""}`}
            onClick={() => void load("refresh")}
          >
            <Icon name="refresh" size={16} />
            Refresh
          </button>
          <DeviceSwitcher
            devices={devices}
            localDeviceId={localDeviceId}
            target={target}
            onTargetChange={setTargetDevice}
          />
        </div>
      </div>
      <p className="settings-subtitle">
        The Claude Code, Codex, and Cursor logins on this device. Zeron detects the live session, keeps each account
        backed up, and can swap between them.
        <SettingsEngineIndicator />
      </p>

      {actionError !== null && (
        <p className="error-strip" role="alert" onClick={() => setActionError(null)}>
          {actionError}
        </p>
      )}

      {snapshotState.kind === "error" ? (
        <p className="error-strip" role="alert" onClick={() => void load("retry")}>
          {snapshotState.message}
          <span className="error-strip-hint">Click to retry</span>
        </p>
      ) : (
        PROVIDERS.map((provider) => (
          <ProviderSection
            key={provider.harness}
            provider={provider}
            loadable={snapshotState}
            busyAccount={busyAccount}
            now={now}
            onAdd={() => addAccount(provider.harness)}
            onSwitch={(account) => accountAction("activate", account)}
            onForget={(account) => accountAction("forget", account)}
          />
        ))
      )}

      <p className="settings-footnote">
        Switching rewrites the CLI’s stored login, so new agent sessions use the selected account immediately. On
        macOS, an already-running Claude Code can hold the previous login for up to ~30 seconds (Keychain cache).
      </p>

      {login !== null && (
        <LoginDialog flow={login} onCancel={dismissLogin} onSubmitCode={submitCode} />
      )}
    </div>
  );
}

function ProviderSection({
  provider,
  loadable,
  busyAccount,
  now,
  onAdd,
  onSwitch,
  onForget,
}: {
  readonly provider: ProviderDescriptor;
  readonly loadable: Loadable;
  readonly busyAccount: string | null;
  readonly now: number;
  readonly onAdd: () => void;
  readonly onSwitch: (account: AgentAccount) => void;
  readonly onForget: (account: AgentAccount) => void;
}) {
  const loading = loadable.kind === "loading";
  const accounts = loadable.kind === "ready" ? providerAccounts(loadable.snapshot, provider.harness) : [];
  const warnings = loadable.kind === "ready" ? loadable.snapshot.warnings.filter((w) => w.harness === provider.harness) : [];
  return (
    <section className="settings-provider">
      <div className="settings-section-header">
        <h2>{provider.name}</h2>
        {!loading && (
          <button type="button" className="btn btn-ghost" onClick={onAdd}>
            Add account
          </button>
        )}
      </div>
      {warnings.map((warning, index) => (
        <p className="warning-strip" role="status" key={index}>
          {warning.message}
        </p>
      ))}
      <div className="settings-card">
        {loading ? (
          <>
            <SkeletonRow />
            <SkeletonRow dim />
          </>
        ) : accounts.length === 0 ? (
          <p className="settings-empty settings-empty-center">{providerEmptyCopy(provider)}</p>
        ) : (
          accounts.map((account) => (
            <AccountRow
              key={account.id}
              account={account}
              busy={busyAccount === account.id}
              now={now}
              onSwitch={() => onSwitch(account)}
              onForget={() => onForget(account)}
            />
          ))
        )}
      </div>
    </section>
  );
}

function AccountRow({
  account,
  busy,
  now,
  onSwitch,
  onForget,
}: {
  readonly account: AgentAccount;
  readonly busy: boolean;
  readonly now: number;
  readonly onSwitch: () => void;
  readonly onForget: () => void;
}) {
  return (
    <div className="settings-row settings-account-row">
      <span className="account-avatar" aria-hidden>
        {accountInitial(account)}
      </span>
      <div className="settings-row-main">
        <span className="settings-row-title">{accountLabel(account)}</span>
        {account.usageWindows.length === 0 ? (
          <span className="account-usage-fallback">{usageFallback(account)}</span>
        ) : (
          <span className="usage-list">
            {account.usageWindows.map((window, index) => (
              <UsageMeter key={index} label={window.label} usedFraction={window.usedFraction} resetsAt={window.resetsAt} now={now} />
            ))}
          </span>
        )}
      </div>
      <div className="account-side">
        <span className="account-badges">
          {account.active && <span className="badge badge-active">Active</span>}
          {account.planLabel != null && <span className="badge">{account.planLabel}</span>}
        </span>
        {!account.active && (
          <span className="account-actions">
            <button type="button" className="btn btn-danger-ghost" disabled={busy} onClick={onForget}>
              Forget
            </button>
            {account.switchable && (
              <button type="button" className="btn btn-solid" disabled={busy} onClick={onSwitch}>
                {busy ? "Switching…" : "Switch"}
              </button>
            )}
          </span>
        )}
      </div>
    </div>
  );
}

function UsageMeter({
  label,
  usedFraction,
  resetsAt,
  now,
}: {
  readonly label: string;
  readonly usedFraction: number;
  readonly resetsAt: string | null;
  readonly now: number;
}) {
  const fraction = Math.min(1, Math.max(0, usedFraction));
  const level = usageLevel(fraction);
  const reset = formatReset(resetsAt, now);
  return (
    <span className="usage-meter">
      <span className="usage-label">{label}</span>
      <span className="usage-track">
        {fraction > 0 && (
          <span
            className="usage-fill"
            style={{ width: `${Math.max(fraction, 0.015) * 100}%`, background: usageColorVar(level) }}
          />
        )}
      </span>
      <span className="usage-percent">{Math.round(fraction * 100)}% used</span>
      {reset !== null && <span className="usage-reset">{reset}</span>}
    </span>
  );
}

/**
 * A ghost account row (`render_skeleton_row`, accounts.rs:1114-1193): avatar,
 * email line, two usage-meter ghosts, a badge — same geometry as the real row
 * so loaded data lands without a layout jump. `dim` fades row two; the inner
 * block rides the shared skeleton pulse.
 */
function SkeletonRow({ dim = false }: { readonly dim?: boolean }) {
  return (
    <div className={`skeleton-account-row ${dim ? "skeleton-account-row-dim" : ""}`} aria-hidden>
      <div className="skeleton-account-inner">
        <span className="skeleton-avatar" />
        <div className="skeleton-account-main">
          <span className="skeleton-ghost skeleton-email-line" />
          <div className="skeleton-meters">
            {[0, 1].map((ix) => (
              <div className="skeleton-meter-row" key={ix}>
                <span className="skeleton-ghost skeleton-meter-label" />
                <span className="skeleton-meter-track" />
                <span className="skeleton-ghost skeleton-meter-percent" />
              </div>
            ))}
          </div>
        </div>
        <span className="skeleton-ghost skeleton-badge" />
      </div>
    </div>
  );
}

/**
 * The login flow's modal (accounts.rs:1114's dialog over `popover::modal`).
 * Exported for the mounted family test (tests/settings-dialogs.test.ts).
 */
export function LoginDialog({
  flow,
  onCancel,
  onSubmitCode,
}: {
  readonly flow: LoginFlow;
  readonly onCancel: () => void;
  readonly onSubmitCode: (code: string) => void;
}) {
  const [code, setCode] = useState("");
  const inputRef = useRef<HTMLInputElement | null>(null);
  // The shared `ui/Dialog` family (ticket 18): the scrim/trap/Escape ride
  // `RbResponsiveDialog` — the phone arm comes with it (the bottom sheet,
  // where the old fixed-centered card used to squash at ≤768px). The
  // family's scrim swallows presses (the desktop's `.occlude()`d modal —
  // the old web hand-roll cancelled on backdrop clicks; Cancel/Escape
  // close now) and Escape cancels, matching `popover::modal`'s contract.
  return (
    <Dialog ariaLabel={loginTitle(flow.harness)} onClose={onCancel} initialFocus={inputRef}>
      <DialogCard>
        <DialogTitle>{loginTitle(flow.harness)}</DialogTitle>
        {flow.kind === "starting" && <DialogBody>Starting the login flow…</DialogBody>}
        {flow.kind === "paste-code" && (
          <>
            <DialogBody>
              A browser window opened. Sign in to the account you want to add, approve access, then paste the code
              Anthropic shows you below. Your current login is untouched until you switch.
            </DialogBody>
            <a className="login-dialog-link" href={flow.start.url} target="_blank" rel="noopener noreferrer">
              Reopen the authorization page
            </a>
            <form
              className="dialog-form-rows"
              onSubmit={(event) => {
                event.preventDefault();
                onSubmitCode(code);
              }}
            >
              <DialogField>
                <input
                  ref={inputRef}
                  className="mono"
                  type="text"
                  placeholder="Paste the authorization code"
                  value={code}
                  onChange={(event) => setCode(event.target.value)}
                  autoComplete="off"
                  spellCheck={false}
                  aria-label="Authorization code"
                />
              </DialogField>
              {flow.error !== null && <p className="login-dialog-error">{flow.error}</p>}
              <div className="dialog-actions-row">
                <BtnGhost type="button" onClick={onCancel}>
                  Cancel
                </BtnGhost>
                <BtnPrimary type="submit" disabled={flow.submitting || code.trim().length === 0}>
                  {flow.submitting ? "Verifying…" : "Add account"}
                </BtnPrimary>
              </div>
            </form>
          </>
        )}
        {flow.kind === "browser" && (
          <>
            <DialogBody>
              {flow.harness === "cursor"
                ? "Finish signing in to Cursor in your browser. This mints a zeron-named API key you can revoke any time from Cursor's dashboard — it is separate from `cursor-agent login`."
                : "Finish signing in to OpenAI in your browser. The new login is captured in an isolated profile — your current session is untouched until you switch."}
            </DialogBody>
            <a className="login-dialog-link" href={flow.start.url} target="_blank" rel="noopener noreferrer">
              Reopen the sign-in page
            </a>
            {flow.error === null ? (
              <p className="login-dialog-poll">
                <span className="dot dot-working" /> {flow.message ?? "Waiting for the browser…"}
              </p>
            ) : (
              <p className="login-dialog-error">{flow.error}</p>
            )}
            <div className="login-dialog-actions">
              <BtnGhost type="button" onClick={onCancel}>
                {flow.error !== null ? "Close" : "Cancel"}
              </BtnGhost>
            </div>
          </>
        )}
      </DialogCard>
    </Dialog>
  );
}
