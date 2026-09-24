import { useEffect, useRef, useState } from "react";
import { Icon } from "@zeron/icons";
import { RbSwitch } from "../components/base/switch";
import {
  defaultKeymap,
  uiSettings,
  useUiSettings,
  type ComposerSendBehavior,
} from "../state/ui-settings";
import { keymapStore } from "../state/keymap";
import { setKeystrokeIntercept } from "../state/keymap";
import {
  SHORTCUT_GROUPS,
  SHORTCUT_IDS,
  defaultComboOn,
  displayCombo,
  isMacPlatform,
  keymapGet,
  shortcutGroup,
  shortcutLabel,
  type ShortcutId,
} from "../state/shortcuts";
import {
  conflictNotice,
  conflictOwner,
  modifierSendLabel,
  recordKey,
  reservedNotice,
  sendComboIsReserved,
  shortcutDescription,
  shortcutIdEquals,
} from "../lib/shortcuts-editor";

/**
 * The Shortcuts editor (desktop settings/shortcuts.rs parity): the rebind
 * table grouped into cards, the send-behavior segmented control, the
 * escape-behavior toggle, per-row Reset and page-level Restore defaults.
 *
 * Recording installs the web's `cx.intercept_keystrokes` equivalent: the
 * keystroke-intercept registry (the shell's binding dispatch declines to
 * run while it is held) plus one capture-phase window listener that consumes
 * every keystroke, and a blur cancel on the focused recorder element. A
 * reserved or conflicting combo is refused with the exact desktop message
 * and the keymap is left untouched.
 */

export function ShortcutsSettingsPage() {
  const settings = useUiSettings();
  const keymap = settings.keymap;
  const isMac = isMacPlatform();
  const [recording, setRecording] = useState<ShortcutId | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const keymapRef = useRef(keymap);
  keymapRef.current = keymap;
  const focusRef = useRef<HTMLSpanElement | null>(null);

  // The recorder: intercept + consume every keystroke while one row records.
  useEffect(() => {
    if (recording === null) {
      return;
    }
    setKeystrokeIntercept("shortcuts-recorder", true);
    const onKeyDown = (event: KeyboardEvent): void => {
      // The recorder owns the keystroke outright — nothing else may react.
      event.preventDefault();
      event.stopPropagation();
      const id = recording;
      if (id === null) {
        return;
      }
      const outcome = recordKey(event, isMacPlatform());
      if (outcome.kind === "ignored") {
        return;
      }
      if (outcome.kind === "cancelled") {
        stopRecording();
        return;
      }
      const combo = outcome.combo;
      // Reserved first (mod-enter belongs to the composer on every send
      // mode), then conflicts; both refuse without touching the keymap.
      if (sendComboIsReserved(combo)) {
        setNotice(reservedNotice(combo, isMacPlatform()));
        stopRecording();
        return;
      }
      const owner = conflictOwner(keymapRef.current, id, combo);
      if (owner !== null) {
        setNotice(conflictNotice(combo, owner, isMacPlatform()));
        stopRecording();
        return;
      }
      commitCombo(id, combo);
      setNotice(null);
      stopRecording();
    };
    const stopRecording = (): void => {
      setRecording(null);
    };
    window.addEventListener("keydown", onKeyDown, { capture: true });
    focusRef.current?.focus({ preventScroll: true });
    return () => {
      setKeystrokeIntercept("shortcuts-recorder", false);
      window.removeEventListener("keydown", onKeyDown, { capture: true });
    };
    // `recording` alone: the handler reads the keymap through the ref, so a
    // mid-recording rebind elsewhere cannot desync the conflict checks.
  }, [recording]);

  /** Patch one id's combo through the keymap store (keymap.set + commit). */
  function commitCombo(id: ShortcutId, combo: string) {
    if (typeof id === "object") {
      const jumpSession = [...keymapStore.get().jumpSession];
      jumpSession[id.jumpSession] = combo;
      keymapStore.update({ jumpSession });
      return;
    }
    keymapStore.update({ [id]: combo });
  }

  function resetCombo(id: ShortcutId) {
    commitCombo(id, defaultComboOn(id, isMac));
  }

  function restoreDefaults() {
    uiSettings.updateImmediate({
      keymap: defaultKeymap(isMac),
      escapeStopsActiveAgent: false,
      composerSendBehavior: "enter",
    });
    setNotice(null);
    setRecording(null);
  }

  function setSendBehavior(behavior: ComposerSendBehavior) {
    if (settings.composerSendBehavior === behavior) {
      return;
    }
    setNotice(null);
    uiSettings.updateImmediate({ composerSendBehavior: behavior });
  }

  // `self.keymap != KeymapConfig::default()` is a VALUE comparison on the
  // desktop (derived PartialEq); `defaultKeymap()` mints a fresh object per
  // call, so the web compares the serialized shapes.
  const customized =
    JSON.stringify(keymap) !== JSON.stringify(defaultKeymap(isMac)) ||
    settings.escapeStopsActiveAgent ||
    settings.composerSendBehavior !== "enter";
  const helper =
    recording !== null
      ? "Press Escape to cancel."
      : (notice ?? "Shortcuts must be unique.");

  return (
    <div className="settings-page">
      {/* The recorder's focus target — focused while recording so a blur
          anywhere cancels (cx.on_blur's equivalent). */}
      <span ref={focusRef} tabIndex={-1} className="recorder-focus" onBlur={() => setRecording(null)} />
      <div className="shortcuts-header">
        <div>
          <h1 className="settings-title">Keyboard shortcuts</h1>
          <p className="settings-subtitle">
            Click a binding, then press the key combination you want to use. Changes apply immediately
            and stay on this device.
          </p>
        </div>
        <button
          type="button"
          className={`btn btn-ghost shortcuts-restore ${!customized || recording !== null ? "shortcuts-restore-inert" : ""}`}
          disabled={!customized || recording !== null}
          onClick={restoreDefaults}
        >
          <Icon name="restart" size={14} />
          Restore defaults
        </button>
      </div>

      <section className="settings-card shortcuts-card-mt32">
        <div className="settings-row settings-row-min84">
          <div className="settings-row-main">
            <span className="settings-row-title">Send messages with</span>
            <span className="shortcuts-row-description">
              Choose whether Enter sends immediately or starts a new paragraph. Cmd/Ctrl+Enter always
              submits; with an empty composer it sends the most recently queued message. Shift+Enter
              always inserts a line break.
            </span>
          </div>
          <div className="send-behavior-control">
            {settings.composerSendBehavior !== "enter" && (
              <button
                type="button"
                className="send-behavior-reset"
                aria-label="Reset send behavior to Enter"
                onClick={() => setSendBehavior("enter")}
              >
                <Icon name="restart" size={13} />
              </button>
            )}
            <div className="segmented-control" role="radiogroup" aria-label="Send messages with">
              {(["enter", "modEnter"] as const).map((behavior) => {
                const selected = settings.composerSendBehavior === behavior;
                const label = behavior === "enter" ? "Enter" : modifierSendLabel(isMac);
                return selected ? (
                  <span
                    key={behavior}
                    role="radio"
                    aria-checked={true}
                    className="segmented-option segmented-option-selected"
                  >
                    {label}
                  </span>
                ) : (
                  <button
                    key={behavior}
                    type="button"
                    role="radio"
                    aria-checked={false}
                    className="segmented-option"
                    onClick={() => setSendBehavior(behavior)}
                  >
                    {label}
                  </button>
                );
              })}
            </div>
          </div>
        </div>
      </section>

      <div className="shortcut-groups">
        {SHORTCUT_GROUPS.filter((name) => name !== "Appshots").map((name) => (
          <div key={name} className="shortcut-group">
            <span className="settings-field-label">{name}</span>
            <section className="settings-card">
              {SHORTCUT_IDS.filter((id) => shortcutGroup(id) === name).map((id) => (
                <ShortcutRow
                  key={typeof id === "object" ? `jump-${id.jumpSession}` : id}
                  id={id}
                  combo={keymapGet(keymap, id)}
                  recording={recording !== null && shortcutIdEquals(recording, id)}
                  isMac={isMac}
                  onRecord={() => {
                    setNotice(null);
                    setRecording(id);
                  }}
                  onReset={() => resetCombo(id)}
                />
              ))}
            </section>
          </div>
        ))}
      </div>

      <p className="shortcuts-helper">{helper}</p>

      <section className="settings-card">
        <div className="settings-row settings-row-min84">
          <div className="settings-row-main">
            <span className="settings-row-title">Stop active agent with Escape</span>
            <span className="shortcuts-row-description">
              When no dialog, menu, picker, or terminal handles Escape, stop the agent in the active
              session.
            </span>
          </div>
          <RbSwitch
            checked={settings.escapeStopsActiveAgent}
            onCheckedChange={() =>
              uiSettings.updateImmediate({ escapeStopsActiveAgent: !settings.escapeStopsActiveAgent })
            }
            aria-label="Stop active agent with Escape"
          />
        </div>
      </section>
    </div>
  );
}

function ShortcutRow(props: {
  readonly id: ShortcutId;
  readonly combo: string;
  readonly recording: boolean;
  readonly isMac: boolean;
  readonly onRecord: () => void;
  readonly onReset: () => void;
}) {
  const nonDefault = props.combo !== defaultComboOn(props.id, props.isMac);
  return (
    <div className="shortcut-row">
      <div className="shortcut-row-copy">
        <span className="shortcut-row-label">{shortcutLabel(props.id)}</span>
        <span className="shortcut-row-description">{shortcutDescription(props.id)}</span>
      </div>
      <div className="shortcut-row-controls">
        {nonDefault && !props.recording && (
          <button
            type="button"
            className="shortcut-reset"
            aria-label={`Reset ${shortcutLabel(props.id)} shortcut`}
            onClick={props.onReset}
          >
            Reset
          </button>
        )}
        <button
          type="button"
          className={`combo-chip ${props.recording ? "combo-chip-recording" : ""}`}
          aria-label={`Change ${shortcutLabel(props.id)} shortcut: ${displayCombo(props.combo, props.isMac)}`}
          onClick={props.onRecord}
        >
          {props.recording ? "Press keys…" : displayCombo(props.combo, props.isMac)}
        </button>
      </div>
    </div>
  );
}
