import { useCallback, useEffect, useRef, useState } from "react";
import type { ReactElement, ReactNode } from "react";
import { Icon, harnessBrandIcon } from "@zeron/icons";
import type { HarnessDescriptor, HarnessId, Model, TitleSettings } from "@zeron/proto";
import { RbSwitch } from "../components/base/switch";
import { DeviceSwitcher } from "../components/ui/DeviceSwitcher";
import { SettingsEngineIndicator } from "../components/settings-engine-indicator";
import { MenuRow } from "../components/ui/MenuRows";
import { PickerCard } from "../components/ui/PickerCard";
import { SkeletonRows } from "../components/ui/Skeleton";
import { useEngineSession } from "../state/session-provider";
import { useWatchSnapshot } from "../state/hooks";
import {
  blurb,
  bumpHarnessCatalog,
  descriptorEnabled,
  getTitleSettings,
  listHarnesses,
  listModels,
  nextSignInPhase,
  notInstalledHint,
  setHarnessEnabled,
  setTitleSettings as saveTitleSettings,
  signInFailureLabel,
  signInPendingLabel,
  signsInOnEnable,
  supportsTitles,
  titleHarnessLabel,
  visibleHarnesses,
  type SignInPhase,
} from "../lib/harnesses";
import { cancelAgentLogin, pollAgentLoginOnce, startAgentLogin } from "../lib/accounts";

/**
 * Agents settings — the desktop's HarnessesPage (nav label "Agents"):
 * per-device harness enablement rows (the composer offers what is on here),
 * the page-header device switcher, and the session-title pickers. Every
 * write is engine-side (`harness-prefs.json` on the target device); the
 * `SetHarnessEnabled` reply carries the fresh catalog, so the rows repaint
 * in one round trip, and the toggle ends with the composer-catalog bump so
 * the pickers re-fetch their harness list.
 */

type Loadable<T> =
  | { kind: "loading" }
  | { kind: "ready"; value: T }
  | { kind: "error"; message: string };

type TitleModels = Loadable<readonly Model[]>;

/** An enable-with-sign-in in flight (harnesses.rs `SignIn`). */
interface SignInState {
  readonly harness: HarnessId;
  /** Known once the engine accepted the start. */
  loginId: string | null;
  message: string | null;
  phase: SignInPhase;
}

/** A sign-in that failed at a phase (harnesses.rs `SignInFailure`). */
interface SignInFailure {
  readonly harness: HarnessId;
  readonly message: string;
  readonly phase: SignInPhase;
}

export function AgentsSettingsPage() {
  const session = useEngineSession();
  const client = session?.client ?? null;
  const snapshot = useWatchSnapshot(session);
  const [target, setTarget] = useState<string | null>(null);
  const [harnesses, setHarnesses] = useState<Loadable<readonly HarnessDescriptor[]>>({ kind: "loading" });
  const [titleSettings, setTitleSettings] = useState<Loadable<TitleSettings>>({ kind: "loading" });
  const [titleModels, setTitleModels] = useState<TitleModels>({ kind: "loading" });
  /** null closed; false = harness picker, true = model picker. */
  const [titleMenu, setTitleMenu] = useState<boolean | null>(null);
  const [titleSaving, setTitleSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** An enable-with-sign-in in flight (antigravity). */
  const [signIn, setSignIn] = useState<SignInState | null>(null);
  const [signInFailure, setSignInFailure] = useState<SignInFailure | null>(null);
  /** Invalidates the poll loop of a cancelled/superseded sign-in. */
  const signInSeq = useRef(0);

  const devices = (snapshot?.devices.rows ?? [])
    .slice()
    .sort((a, b) => (a.createdAt ?? "").localeCompare(b.createdAt ?? "") || a.id.localeCompare(b.id));
  const localDeviceId = session?.client.engineInfo?.deviceId ?? null;

  const loadTitles = useCallback(
    async (save: TitleSettings | null) => {
      if (client === null) {
        return;
      }
      const saving = save !== null;
      setTitleMenu(null);
      setTitleSaving(saving);
      try {
        const settings = saving
          ? await saveTitleSettings(client, save, target)
          : await getTitleSettings(client, target);
        setTitleSettings({ kind: "ready", value: settings });
        setTitleSaving(false);
        setError(null);
        if (settings.harness !== null) {
          setTitleModels({ kind: "loading" });
          try {
            setTitleModels({ kind: "ready", value: await listModels(client, settings.harness, target) });
          } catch (cause) {
            setTitleModels({ kind: "error", message: describe(cause) });
          }
        } else {
          setTitleModels({ kind: "loading" });
        }
      } catch (cause) {
        // A save failure surfaces in the page's error strip; a load failure
        // in the titles card itself (load_titles' split, harnesses.rs:192-207).
        setTitleSaving(false);
        if (saving) {
          setError(describe(cause));
        } else {
          setTitleSettings({ kind: "error", message: describe(cause) });
        }
      }
    },
    [client, target],
  );

  const load = useCallback(async () => {
    if (client === null) {
      return;
    }
    setError(null);
    setHarnesses({ kind: "loading" });
    setTitleSettings({ kind: "loading" });
    setTitleModels({ kind: "loading" });
    setTitleMenu(null);
    try {
      setHarnesses({ kind: "ready", value: await listHarnesses(client, target) });
    } catch (cause) {
      setHarnesses({ kind: "error", message: describe(cause) });
      return;
    }
    await loadTitles(null);
  }, [client, target, loadTitles]);

  // Mount load, and a full drop-and-reload on every retarget (set_target_device).
  useEffect(() => {
    void load();
  }, [load]);

  // Unmount drops any in-flight sign-in poll loop (the cancel itself is
  // best-effort and only logged; harnesses.rs drop semantics).
  useEffect(() => {
    return () => {
      signInSeq.current += 1;
    };
  }, []);

  function toggle(harness: HarnessId, enabled: boolean) {
    if (client === null) {
      return;
    }
    if (enabled && signsInOnEnable(harness)) {
      startSignIn(harness);
      return;
    }
    setError(null);
    void (async () => {
      try {
        const fresh = await setHarnessEnabled(client, harness, enabled, target);
        setHarnesses({ kind: "ready", value: fresh });
        bumpHarnessCatalog(session);
      } catch (cause) {
        setError(describe(cause));
      }
    })();
  }

  /**
   * Sign in first, then switch on (harnesses.rs `start_sign_in`):
   * StartAgentLogin, then PollAgentLogin until the engine reports the
   * outcome, opening the sign-in page the first time a poll names it.
   */
  function startSignIn(harness: HarnessId) {
    if (client === null) {
      return;
    }
    if (target !== null) {
      // The sign-in redirect lands on a loopback port of the device running
      // the agent, which a browser here can't reach.
      setError("Turn this agent on from its own device to sign in.");
      return;
    }
    setError(null);
    setSignInFailure(null);
    setSignIn({ harness, loginId: null, message: null, phase: "starting" });
    const seq = signInSeq.current + 1;
    signInSeq.current = seq;
    // The loop's own phase tracker (state reads inside the async closure
    // would see the phase at start time, not the current one).
    let phase: SignInPhase = "starting";
    const failure = (message: string) => {
      if (signInSeq.current !== seq) {
        return;
      }
      setSignIn(null);
      setSignInFailure({ harness, message, phase });
    };
    void (async () => {
      let loginId: string;
      try {
        const start = await startAgentLogin(client, harness, target);
        if (signInSeq.current !== seq) {
          return;
        }
        loginId = start.loginId;
        phase = "installing";
        setSignIn((current) =>
          current === null || current.harness !== harness
            ? current
            : { ...current, loginId, phase },
        );
      } catch (cause) {
        failure(`Sign-in failed to start: ${describe(cause)}`);
        return;
      }
      // Pre-open a blank tab inside the click gesture so the poll-carried
      // url can navigate it (popup blockers eat post-await opens; the CLI's
      // own open is suppressed engine-side, settings-accounts parity).
      const tab = window.open("about:blank", "_blank");
      let opened = false;
      for (;;) {
        await new Promise((resolve) => setTimeout(resolve, 1000));
        if (signInSeq.current !== seq) {
          tab?.close();
          return;
        }
        let poll;
        try {
          poll = await pollAgentLoginOnce(client, loginId, target);
        } catch (cause) {
          tab?.close();
          failure(describe(cause));
          return;
        }
        if (signInSeq.current !== seq) {
          tab?.close();
          return;
        }
        if (poll.status === "pending") {
          if (!opened && poll.url != null) {
            opened = true;
            if (tab !== null) {
              tab.location.href = poll.url;
              tab.opener = null;
            } else {
              window.open(poll.url, "_blank", "noopener,noreferrer");
            }
          }
          phase = nextSignInPhase(poll) ?? phase;
          setSignIn((current) =>
            current === null || current.harness !== harness
              ? current
              : { ...current, phase, message: poll.message ?? current.message },
          );
          continue;
        }
        if (poll.status === "done") {
          tab?.close();
          // The toggle itself, now that the sign-in succeeded — a failure
          // here lands as an Enable failure with the fresh catalog intact.
          phase = "enabling";
          setSignIn((current) =>
            current === null || current.harness !== harness
              ? current
              : { ...current, phase, message: null },
          );
          try {
            const fresh = await setHarnessEnabled(client, harness, true, target);
            if (signInSeq.current !== seq) {
              return;
            }
            setHarnesses({ kind: "ready", value: fresh });
            setSignIn(null);
            setSignInFailure(null);
            bumpHarnessCatalog(session);
          } catch (cause) {
            failure(describe(cause));
          }
          return;
        }
        tab?.close();
        failure(poll.message ?? "Unknown error");
        return;
      }
    })();
  }

  function cancelSignIn() {
    const current = signIn;
    if (current === null || client === null) {
      return;
    }
    signInSeq.current += 1;
    setSignIn(null);
    if (current.loginId !== null) {
      // Best-effort; the desktop only debug-logs a failure.
      void cancelAgentLogin(client, current.loginId, target).catch(() => undefined);
    }
  }

  function setTargetDevice(next: string | null) {
    if (next === target) {
      return;
    }
    // A retarget drops any in-flight sign-in (set_target_device cancels).
    signInSeq.current += 1;
    setSignIn(null);
    setSignInFailure(null);
    setTarget(next);
    setTitleMenu(null);
    setTitleSaving(false);
    setError(null);
  }

  return (
    <div className="settings-page">
      <div className="settings-title-row">
        <h1 className="settings-title">Agents</h1>
        <DeviceSwitcher
          devices={devices}
          localDeviceId={localDeviceId}
          target={target}
          onTargetChange={setTargetDevice}
        />
      </div>
      <p className="settings-subtitle">
        Choose which coding agents the composer offers. The setting is per device — switch devices in the
        header. Agents whose CLI isn't installed on a device can't be enabled there.
        <SettingsEngineIndicator />
      </p>

      {error !== null && (
        <p className="error-strip" role="alert" onClick={() => setError(null)}>
          {error}
        </p>
      )}

      {harnesses.kind === "loading" ? (
        <section className="settings-card harnesses-skeleton">
          <SkeletonRows count={4} />
        </section>
      ) : harnesses.kind === "error" ? (
        <div className="settings-error-retry">
          <p className="error-strip" role="alert">
            {harnesses.message}
          </p>
          <button type="button" className="btn btn-ghost" onClick={() => void load()}>
            Retry
          </button>
        </div>
      ) : (
        <section className="settings-card">
          <HarnessRows
            list={harnesses.value}
            onToggle={toggle}
            signIn={signIn}
            signInFailure={signInFailure}
            onCancelSignIn={cancelSignIn}
            onRetrySignIn={startSignIn}
          />
        </section>
      )}

      <TitleSettingsCard
        titleSettings={titleSettings}
        titleModels={titleModels}
        harnesses={harnesses.kind === "ready" ? harnesses.value : []}
        titleMenu={titleMenu}
        titleSaving={titleSaving}
        onSetMenu={(open, isModel) => setTitleMenu(open ? isModel : null)}
        onChoose={(choice) => void loadTitles(choice)}
      />
    </div>
  );
}

function HarnessRows(props: {
  readonly list: readonly HarnessDescriptor[];
  readonly onToggle: (harness: HarnessId, enabled: boolean) => void;
  readonly signIn: SignInState | null;
  readonly signInFailure: SignInFailure | null;
  readonly onCancelSignIn: () => void;
  readonly onRetrySignIn: (harness: HarnessId) => void;
}) {
  const descriptors = visibleHarnesses(props.list);
  const enabledCount = descriptors.filter((descriptor) => descriptorEnabled(descriptor)).length;
  return (
    <>
      {descriptors.map((descriptor, ix) => {
        const enabled = descriptorEnabled(descriptor);
        const installed = descriptor.installed;
        const signingIn =
          props.signIn !== null && props.signIn.harness === descriptor.id ? props.signIn : null;
        const signInFailure =
          props.signInFailure !== null && props.signInFailure.harness === descriptor.id
            ? props.signInFailure
            : null;
        const signInCancellable = signingIn !== null && signingIn.phase !== "enabling";
        // The one enabled harness left can't be switched off — the composer
        // needs something to run — but only when it could actually run; and
        // turning OFF never needs the CLI, turning ON still does. A row
        // mid-sign-in (or showing its failure) is inert until it resolves.
        const lastEnabled = enabled && enabledCount === 1 && installed;
        const interactive =
          signingIn === null && signInFailure === null && !lastEnabled && (enabled || installed);
        const brand = harnessBrandIcon(descriptor.id);
        return (
          <div
            key={descriptor.id}
            className={`settings-row harness-row ${!installed ? "harness-row-uninstalled" : ""} ${
              signingIn !== null ? "harness-row-signing-in" : ""
            }`}
          >
            <div className="row-tile harness-tile" aria-hidden="true">
              <Icon
                name={brand.name}
                size={16}
                className="row-tile-icon"
                style={brand.tint === null ? undefined : { color: brand.tint }}
              />
            </div>
            <div className="settings-row-main">
              <span className="settings-row-title">{descriptor.name}</span>
              <span className="settings-meta-line">
                {blurb(descriptor.id)}
                {signingIn !== null && (
                  <>
                    <span className="settings-meta-dot" aria-hidden="true">·</span>
                    <span className="harness-sign-in-status">
                      {signingIn.message ?? signInPendingLabel(signingIn.phase)}
                    </span>
                  </>
                )}
                {signInFailure !== null && (
                  <>
                    <span className="settings-meta-dot" aria-hidden="true">·</span>
                    <span className="harness-sign-in-failure">
                      {signInFailureLabel(signInFailure.phase)} — {signInFailure.message}
                    </span>
                  </>
                )}
                {!installed && (
                  <>
                    <span className="settings-meta-dot" aria-hidden="true">·</span>
                    <span className="harness-hint">{notInstalledHint(descriptor.id, enabled)}</span>
                  </>
                )}
              </span>
            </div>
            {signInCancellable && (
              <button
                type="button"
                className="btn btn-ghost harness-sign-in-cancel"
                onClick={props.onCancelSignIn}
              >
                Cancel
              </button>
            )}
            {signInFailure !== null && (
              <button
                type="button"
                className="btn btn-ghost harness-sign-in-retry"
                onClick={() => props.onRetrySignIn(descriptor.id)}
              >
                Retry
              </button>
            )}
            {interactive ? (
              <RbSwitch
                checked={enabled}
                onCheckedChange={() => props.onToggle(descriptor.id, !enabled)}
                aria-label={descriptor.name}
              />
            ) : (
              <InertSwitch on={enabled} label={descriptor.name} />
            )}
          </div>
        );
      })}
    </>
  );
}

/** The inert toggle twin: the same 32×18 switch with no handlers at all. */
function InertSwitch(props: { readonly on: boolean; readonly label: string }) {
  return (
    <span
      className={`toggle rb-switch-inert ${props.on ? "toggle-on" : ""}`}
      role="switch"
      aria-checked={props.on}
      aria-label={props.label}
    >
      <span className="toggle-thumb" />
    </span>
  );
}

function TitleSettingsCard(props: {
  readonly titleSettings: Loadable<TitleSettings>;
  readonly titleModels: TitleModels;
  readonly harnesses: readonly HarnessDescriptor[];
  readonly titleMenu: boolean | null;
  readonly titleSaving: boolean;
  readonly onSetMenu: (open: boolean, isModel: boolean) => void;
  readonly onChoose: (choice: TitleSettings) => void;
}) {
  const settings = props.titleSettings;
  if (settings.kind !== "ready") {
    const message = settings.kind === "error" ? settings.message : "Loading title settings…";
    return (
      <section className="settings-card settings-titles-card">
        <span className="settings-row-title">Session titles</span>
        <p className="settings-titles-loading">{message}</p>
      </section>
    );
  }
  const value = settings.value;
  const harnessName = (id: HarnessId): string => {
    const descriptor = props.harnesses.find((entry) => entry.id === id);
    return titleHarnessLabel(id, descriptor?.name ?? id);
  };
  const harnessLabel =
    value.harness !== null
      ? harnessName(value.harness)
      : "Automatic (session agent when supported)";
  const modelLabel =
    value.model !== null
      ? props.titleModels.kind === "ready"
        ? (props.titleModels.value.find((model) => model.id === value.model)?.label ?? value.model)
        : value.model
      : "Automatic (cheapest model)";

  const harnessChoices: readonly { readonly label: string; readonly value: TitleSettings }[] = [
    { label: "Automatic", value: { harness: null, model: null } },
    ...props.harnesses
      .filter(
        (descriptor) =>
          descriptorEnabled(descriptor) &&
          descriptor.installed &&
          supportsTitles(descriptor.id) &&
          descriptor.id !== "mock",
      )
      .map((descriptor) => ({
        label: descriptor.name,
        value: { harness: descriptor.id, model: null } satisfies TitleSettings,
      })),
  ];
  const modelChoices: readonly { readonly label: string; readonly value: TitleSettings }[] = [
    { label: "Automatic", value: { harness: value.harness, model: null } },
    ...(props.titleModels.kind === "ready"
      ? props.titleModels.value.map((model) => ({
          label: model.label,
          value: { harness: value.harness, model: model.id } satisfies TitleSettings,
        }))
      : []),
  ];

  const choiceRow = (choice: { readonly label: string; readonly value: TitleSettings }): ReactElement => (
    <MenuRow
      key={choice.label}
      fadeKey={choice.label}
      selected={choice.value.harness === value.harness && choice.value.model === value.model}
      onClick={() => props.onChoose(choice.value)}
    >
      {choice.label}
    </MenuRow>
  );

  return (
    <section className="settings-card settings-titles-card">
      <span className="settings-row-title">Session titles</span>
      <p className="settings-titles-subtitle">
        Choose the agent and model for automatic titles on this device. Claude Code and Codex support
        restricted title generation.
      </p>
      <TitlePickerRow
        label="Title harness"
        display={harnessLabel}
        interactive={!props.titleSaving}
        open={props.titleMenu === false}
        onOpenChange={(next) => props.onSetMenu(next, false)}
      >
        {harnessChoices.map(choiceRow)}
      </TitlePickerRow>
      <TitlePickerRow
        label="Title model"
        display={modelLabel}
        interactive={!props.titleSaving && value.harness !== null}
        open={props.titleMenu === true}
        onOpenChange={(next) => props.onSetMenu(next, true)}
      >
        {modelChoices.map(choiceRow)}
      </TitlePickerRow>
      {props.titleModels.kind === "error" && (
        <p className="error-strip" role="alert">
          {props.titleModels.message}
        </p>
      )}
    </section>
  );
}

/** One picker row (harnesses.rs:286-316) — exported for the mounted test. */
export function TitlePickerRow(props: {
  readonly label: string;
  readonly display: string;
  readonly interactive: boolean;
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly children: ReactNode;
}) {
  // Ticket 18: the choices ride `PickerCard` — the settings-appearance
  // pattern (the theme variant picker) instead of the old inline
  // `.title-picker-options` expander. Desktop: the portaled card below the
  // trigger; phone: the shared bottom sheet (where the old inline expander
  // stacked full-width rows). `initialFocus: false` keeps focus put, the
  // shared contract for pickers driven from a settings row.
  return (
    <div className={`settings-row title-picker-row ${props.interactive ? "" : "title-picker-inert"}`}>
      <span className="settings-row-title title-picker-label">{props.label}</span>
      <PickerCard
        open={props.open}
        onOpenChange={props.onOpenChange}
        placement="anchorBelow"
        cardClassName="popover-card title-picker-menu"
        role="menu"
        ariaLabel={props.label}
        width={260}
        initialFocus={false}
        trigger={
          <button
            type="button"
            className="btn btn-ghost title-picker-trigger"
            disabled={!props.interactive}
          >
            {props.display}
          </button>
        }
      >
        {props.children}
      </PickerCard>
    </div>
  );
}

function describe(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
