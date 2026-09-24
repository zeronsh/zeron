import { RowTile } from "../components/settings-widgets";
import { RbSwitch } from "../components/base/switch";
import { uiSettings, useUiSettings } from "../state/ui-settings";

/**
 * Files settings (desktop settings/files.rs parity): local preferences for
 * workspace-file editing — autosave and its delay pills, word wrap, and
 * show-all. Every control commits immediately through ticket 03's settings
 * store; the live-apply side (file surfaces reading the store) already
 * rides `useUiSettings` in the viewers. The editor font size moved to
 * Appearance when the code font became its own setting (upstream #374).
 */

/** `DELAY_OPTIONS` (files.rs:8) — the autosave delay pills, in ms. */
const DELAY_OPTIONS: readonly number[] = [300, 600, 900, 1_500, 3_000];

function delayLabel(ms: number): string {
  return ms >= 1_000 ? `${ms / 1_000} s` : `${ms} ms`;
}

export function FilesSettingsPage() {
  const settings = useUiSettings();
  return (
    <div className="settings-page">
      <h1 className="settings-title">Files</h1>
      <p className="settings-subtitle">Control how workspace files are displayed and saved while you edit.</p>

      <section className="settings-card">
        <div className="settings-row settings-files-row settings-files-nosep">
          <RowTile icon="folder" />
          <div className="settings-row-main">
            <span className="settings-row-title">Autosave</span>
            <span className="settings-row-meta">Save edited workspace files to disk automatically.</span>
          </div>
          <RbSwitch
            checked={settings.filesAutosaveEnabled}
            onCheckedChange={() => uiSettings.updateImmediate({ filesAutosaveEnabled: !settings.filesAutosaveEnabled })}
            aria-label="Autosave"
          />
        </div>
        {settings.filesAutosaveEnabled && (
          <div className="settings-row settings-files-row settings-files-nosep settings-files-pills">
            <RowTile icon="folder" />
            <div className="settings-row-main">
              <span className="settings-row-title">Autosave delay</span>
              <span className="settings-row-meta">Save files after editing has been idle for this long.</span>
              <div className="pill-row" role="radiogroup" aria-label="Autosave delay">
                {DELAY_OPTIONS.map((ms) => (
                  <Pill
                    key={ms}
                    label={delayLabel(ms)}
                    selected={settings.filesAutosaveDelayMs === ms}
                    onSelect={() => uiSettings.updateImmediate({ filesAutosaveDelayMs: ms })}
                  />
                ))}
              </div>
            </div>
          </div>
        )}
        <div className="settings-row settings-files-row settings-files-nosep">
          <RowTile icon="list" />
          <div className="settings-row-main">
            <span className="settings-row-title">Word wrap</span>
            <span className="settings-row-meta">Wrap long lines in every workspace file.</span>
          </div>
          <RbSwitch
            checked={settings.filesWordWrap}
            onCheckedChange={() => uiSettings.updateImmediate({ filesWordWrap: !settings.filesWordWrap })}
            aria-label="Word wrap"
          />
        </div>
        <div className="settings-row">
          <RowTile icon="eye" />
          <div className="settings-row-main">
            <span className="settings-row-title">Show all files</span>
            <span className="settings-row-meta">Include hidden and ignored files in every file tree.</span>
          </div>
          <RbSwitch
            checked={settings.filesShowAll}
            onCheckedChange={() => uiSettings.updateImmediate({ filesShowAll: !settings.filesShowAll })}
            aria-label="Show all files"
          />
        </div>
      </section>
    </div>
  );
}

/** One pill button (files.rs:73-107): 28px tall, 10px sides, radius 7. */
function Pill(props: { readonly label: string; readonly selected: boolean; readonly onSelect: () => void }) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={props.selected}
      className={`pill ${props.selected ? "pill-selected" : ""}`}
      onClick={props.onSelect}
    >
      {props.label}
    </button>
  );
}
