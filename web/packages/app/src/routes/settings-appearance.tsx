import { useEffect, useMemo, useRef, useState, useSyncExternalStore, type CSSProperties } from "react";
import {
  accentForVariant,
  accentPresets,
  type Appearance,
  type ThemeVariant,
} from "@zeron/theme";
import { Icon } from "@zeron/icons";
import { PickerCard } from "../components/ui/PickerCard";
import { MenuHeading, MenuRow } from "../components/ui/MenuRows";
import { Dialog, BtnPrimary } from "../components/ui/Dialog";
import {
  RbSelect,
  RbSelectItem,
  RbSelectPopup,
  RbSelectPortal,
  RbSelectPositioner,
  RbSelectTrigger,
} from "../components/base/select";
import { PalettePreview, ThemeMiniature, ThemeModePreview } from "../components/theme-preview";
import { CompactAction, CompactActionDanger, MetaLine, RowTile } from "../components/settings-widgets";
import { appearanceStore, useAppearance, useSystemAppearance } from "../state/appearance";
import {
  normalizeTranscriptWidth,
  TRANSCRIPT_WIDTH_DEFAULT,
  TRANSCRIPT_WIDTH_MAX,
  TRANSCRIPT_WIDTH_MIN,
  TRANSCRIPT_WIDTH_STEP,
  uiSettings,
  useUiSettings,
  UI_FONT_SIZES,
  type NewThreadBackgroundEffect,
} from "../state/ui-settings";
import {
  accentHelper,
  APPEARANCE_MODES,
  appearanceModeLabel,
  CODE_FONT_CHOICES,
  DEFAULT_APPEARANCE,
  effectiveCodeFontFamily,
  effectiveTerminalFontFamily,
  effectiveUiFontFamily,
  fontFamilyLabel,
  fontSizePxLabel,
  MONO_FONT_SIZES,
  nearestMonoFontSize,
  resolveAppearance,
  TERMINAL_FONT_CHOICES,
  UI_FONT_CHOICES,
  variantChoices,
  type AccentSelection,
  type UiFontChoice,
} from "../lib/appearance-store";
import {
  findVariantAnywhere,
  parseThemeSource,
  reportLines,
  reportSummary,
  slug,
  sourceName,
  themeLibrary,
  type ThemeCompilation,
  type ThemeImportReport,
  type ThemeLibraryEntry,
} from "../lib/theme-library";
import {
  backgroundRowState,
  installNewThreadBackground,
  removeNewThreadBackground,
  resolveActiveNewThreadBackground,
  type ResolvedNewThreadBackground,
} from "../lib/new-thread-background";
import { idbBackgroundBlobStore } from "../lib/background-blob-store";

/**
 * Appearance settings (desktop settings/appearance.rs parity): the
 * appearance-mode option cards with live theme miniatures, the independent
 * light/dark theme-family popover selectors, the accent swatch row, the
 * new-thread composer background and its effect pills, the interface
 * font/size pickers, and the custom theme library (snapshot import only).
 * Everything applies live and persists device-locally through ticket 03's
 * settings store.
 *
 * Web deviations (both product decisions, recorded in the ticket's
 * Comments): the Glass surface row does not render — frosted is removed and
 * `resolveSurfaceTreatment` is forced opaque; and the theme-library's
 * Link/Reveal/Reload actions are absent (no linkable OS path in a browser).
 */

/** One shared blob store for the page's background installs. */
const backgroundBlobs = idbBackgroundBlobStore();

/** `NewThreadBackgroundEffect::ALL` with labels + descriptions (settings.rs). */
const BACKGROUND_EFFECTS: readonly {
  readonly id: NewThreadBackgroundEffect;
  readonly label: string;
  readonly description: string;
}[] = [
  { id: "none", label: "None", description: "Shows the original artwork." },
  { id: "dither", label: "Dither", description: "Rebuilds the artwork with a dithered color palette." },
  { id: "ascii", label: "ASCII", description: "Recreates the artwork with colored characters on black." },
  { id: "halftone", label: "Halftone", description: "Recreates the artwork with colored print dots on black." },
  { id: "scanlines", label: "Scanlines", description: "Adds a pronounced horizontal display-line texture." },
];

export function AppearanceSettingsPage() {
  const preferences = useAppearance();
  const system = useSystemAppearance();
  const resolved = resolveAppearance(preferences.mode, system);
  const settings = useUiSettings();
  const [openMenu, setOpenMenu] = useState<Appearance | null>(null);
  const [backgroundError, setBackgroundError] = useState<string | null>(null);
  const [activeBackground, setActiveBackground] = useState<ResolvedNewThreadBackground | null>(null);
  const [libraryError, setLibraryError] = useState<string | null>(null);
  const [importState, setImportState] = useState<ImportDialogState | null>(null);
  const [reviewEntryId, setReviewEntryId] = useState<string | null>(null);
  const libraryEntries = useSyncExternalStore(
    themeLibrary.subscribe,
    themeLibrary.getSnapshot,
    themeLibrary.getSnapshot,
  );
  const libraryWarning = themeLibrary.getLoadWarning();

  const lightVariant =
    findVariantAnywhere(preferences.lightVariant) ?? findVariantAnywhere(DEFAULT_APPEARANCE.lightVariant)!;
  const darkVariant =
    findVariantAnywhere(preferences.darkVariant) ?? findVariantAnywhere(DEFAULT_APPEARANCE.darkVariant)!;
  const pageVariant = resolved === "dark" ? darkVariant : lightVariant;
  // The row keys off the RESOLVED background (ticket 48), never the raw
  // field: nothing stored resolves the bundled default the same way the
  // canvas painter already does.
  const backgroundRow = backgroundRowState(
    settings.newThreadComposerBackground,
    activeBackground,
  );
  const backgroundInstalled = backgroundRow.installed;
  const backgroundAvailable = backgroundRow.available;

  // The stored entry only counts while its blob still resolves; nothing
  // stored resolves the bundled default (the desktop's
  // `active_new_thread_background`).
  useEffect(() => {
    let cancelled = false;
    void resolveActiveNewThreadBackground(settings.newThreadComposerBackground).then((resolved) => {
      if (!cancelled) {
        setActiveBackground(resolved);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [settings.newThreadComposerBackground?.path, settings.newThreadComposerBackground?.name]);

  const backgroundInputRef = useRef<HTMLInputElement>(null);
  const onPickBackgroundFile = (file: File): void => {
    // `choose_new_thread_background` clears the stale error before staging.
    setBackgroundError(null);
    void installNewThreadBackground(file, { settings: uiSettings, blobs: backgroundBlobs }).then(
      (error) => {
        setBackgroundError(error);
      },
    );
  };
  const onRemoveBackground = (): void => {
    setBackgroundError(null);
    void removeNewThreadBackground({ settings: uiSettings, blobs: backgroundBlobs }).then((error) => {
      setBackgroundError(error);
    });
  };

  const onMenuOpenChange = (appearance: Appearance) => (next: boolean) => {
    setOpenMenu((current) => {
      if (next) {
        return appearance;
      }
      return current === appearance ? null : current;
    });
  };

  const backgroundMeta = backgroundRow.meta;

  const libraryAction = (action: () => void): void => {
    try {
      action();
      setLibraryError(null);
    } catch (error) {
      setLibraryError(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <div className="settings-page">
      <h1 className="settings-title">Appearance</h1>
      <p className="settings-subtitle">Choose how Zeron looks. These settings stay in this browser.</p>

      <div className="settings-mode-block">
        <span className="settings-field-label">Appearance</span>
        <div className="settings-option-row" role="radiogroup" aria-label="Appearance mode">
          {APPEARANCE_MODES.map((mode) => (
            <button
              key={mode}
              type="button"
              role="radio"
              aria-checked={preferences.mode === mode}
              className={`option-card ${preferences.mode === mode ? "option-card-selected" : ""}`}
              onClick={() => appearanceStore.setMode(mode)}
            >
              <span className="option-card-frame">
                <ThemeModePreview mode={mode} lightVariant={lightVariant} darkVariant={darkVariant} />
              </span>
              <span className="option-card-caption">{appearanceModeLabel(mode)}</span>
            </button>
          ))}
        </div>
      </div>

      <section className="settings-card">
        <ThemeSelectorRow
          appearance="light"
          label="Light theme"
          value={preferences.lightVariant}
          open={openMenu === "light"}
          onOpenChange={onMenuOpenChange("light")}
        />
        <ThemeSelectorRow
          appearance="dark"
          label="Dark theme"
          value={preferences.darkVariant}
          open={openMenu === "dark"}
          onOpenChange={onMenuOpenChange("dark")}
        />
        <div className="settings-row">
          <RowTile icon="tuning" />
          <div className="settings-row-main">
            <span className="settings-row-title">Accent color</span>
            <MetaLine fragments={[accentHelper(preferences.accent)]} />
          </div>
          <div className="swatch-row" role="radiogroup" aria-label="Accent color">
            <AccentSwatch choice="themeDefault" selected={preferences.accent === "themeDefault"} variant={pageVariant} />
            {accentPresets.map((preset) => (
              <AccentSwatch
                key={preset.id}
                choice={preset.id}
                selected={preferences.accent === preset.id}
                variant={pageVariant}
              />
            ))}
          </div>
        </div>
        <div className="settings-row">
          {activeBackground !== null ? (
            <div className="background-tile">
              <img className="background-tile-img" src={activeBackground.url} alt="" />
            </div>
          ) : (
            <RowTile icon="fileImage" />
          )}
          <div className="settings-row-main">
            <span className="settings-row-title">New thread composer background</span>
            <MetaLine fragments={backgroundMeta} />
          </div>
          <div className="settings-row-actions">
            {backgroundInstalled ? (
              <>
                <CompactAction onClick={() => backgroundInputRef.current?.click()}>Replace image</CompactAction>
                <CompactActionDanger onClick={onRemoveBackground}>Remove</CompactActionDanger>
              </>
            ) : (
              <CompactAction onClick={() => backgroundInputRef.current?.click()}>Choose image</CompactAction>
            )}
          </div>
          <input
            ref={backgroundInputRef}
            type="file"
            accept="image/*"
            hidden
            onChange={(event) => {
              const file = event.target.files?.[0];
              event.target.value = "";
              if (file !== undefined) {
                onPickBackgroundFile(file);
              }
            }}
          />
        </div>
        {backgroundAvailable && (
          <div className="settings-row">
            <RowTile icon="tuning" />
            <div className="settings-row-main">
              <span className="settings-row-title">Background effect</span>
              <MetaLine
                fragments={[
                  BACKGROUND_EFFECTS.find((effect) => effect.id === settings.newThreadBackgroundEffect)
                    ?.description ?? "",
                ]}
              />
            </div>
            <div className="settings-effect-choices" role="radiogroup" aria-label="Background effect">
              {BACKGROUND_EFFECTS.map((effect) => (
                <button
                  key={effect.id}
                  type="button"
                  role="radio"
                  aria-checked={settings.newThreadBackgroundEffect === effect.id}
                  className={`choice choice-effect ${settings.newThreadBackgroundEffect === effect.id ? "choice-selected" : ""}`}
                  onClick={() => uiSettings.updateImmediate({ newThreadBackgroundEffect: effect.id })}
                >
                  {effect.label}
                </button>
              ))}
            </div>
          </div>
        )}
        {backgroundError !== null && (
          <div className="settings-row-error">
            <p className="error-strip">
              <Icon name="dangerTriangle" size={16} className="error-strip-icon" />
              {backgroundError}
            </p>
          </div>
        )}
        <div className="settings-row">
          <RowTile icon="folderWithFiles" />
          <div className="settings-row-main">
            <span className="settings-row-title">Theme library</span>
            <MetaLine fragments={["Import custom themes."]} />
          </div>
          <div className="settings-row-actions">
            <BtnPrimary onClick={() => setImportState({ fileName: null, compilation: null, selected: new Set(), detailsVariant: null, error: null })}>
              Add theme
            </BtnPrimary>
          </div>
        </div>
        {libraryEntries.length > 0 && <div className="library-group-header">IMPORTED</div>}
        {libraryEntries.map((entry) => (
          <LibraryEntryRow
            key={entry.id}
            entry={entry}
            onReview={() => setReviewEntryId(entry.id)}
            onDuplicate={() => libraryAction(() => themeLibrary.duplicate(entry.id))}
            onRemove={() => libraryAction(() => themeLibrary.remove(entry.id))}
          />
        ))}
      </section>

      <InterfaceFontBlock settings={settings} />
      <MonoFontBlock kind="terminal" settings={settings} />
      <MonoFontBlock kind="code" settings={settings} />
      <ConversationWidthBlock settings={settings} />

      {(libraryError ?? libraryWarning) !== null && (
        <p className="library-warning">{libraryError ?? libraryWarning}</p>
      )}

      {importState !== null && (
        <ThemeImportDialog
          state={importState}
          setState={setImportState}
          onClose={() => setImportState(null)}
        />
      )}
      {reviewEntryId !== null && (
        <ThemeReviewDialog
          entry={libraryEntries.find((entry) => entry.id === reviewEntryId) ?? null}
          onClose={() => setReviewEntryId(null)}
        />
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// The theme-family selector (appearance.rs:1028-1194)
// ---------------------------------------------------------------------------

function ThemeSelectorRow(props: {
  readonly appearance: Appearance;
  readonly label: string;
  readonly value: string;
  readonly open: boolean;
  readonly onOpenChange: (next: boolean) => void;
}) {
  const choices = variantChoices(props.appearance);
  const selectedVariant =
    choices.find((variant) => variant.id === props.value) ?? choices[0];
  if (selectedVariant === undefined) {
    return null;
  }
  const heading = props.appearance === "light" ? "Light themes" : "Dark themes";
  return (
    <div className="settings-row">
      <RowTile icon="tuning" />
      <div className="settings-row-main">
        <span className="settings-row-title">{props.label}</span>
        <MetaLine fragments={["Used whenever this appearance is active."]} />
      </div>
      <PickerCard
        open={props.open}
        onOpenChange={props.onOpenChange}
        placement="anchorBelow"
        cardClassName="popover-card theme-select-menu"
        role="menu"
        ariaLabel={heading}
        width={260}
        initialFocus={false}
        trigger={
          <button
            type="button"
            className={`theme-select-trigger ${props.open ? "theme-select-trigger-open" : ""}`}
            aria-haspopup="menu"
            aria-label={props.label}
          >
            <PalettePreview variant={selectedVariant} />
            <span className="theme-select-label">{selectedVariant.name}</span>
            <Icon name="sortVertical" size={14} className="theme-select-caret" />
          </button>
        }
      >
        <MenuHeading>{heading}</MenuHeading>
        {choices.map((variant) => (
          <MenuRow
            key={variant.id}
            fadeKey={variant.id}
            selected={variant.id === props.value}
            onClick={() => {
              appearanceStore.setVariant(props.appearance, variant.id);
              props.onOpenChange(false);
            }}
          >
            <PalettePreview variant={variant} />
            <span className="theme-select-row-label">{variant.name}</span>
            {variant.id === props.value && <Icon name="check" size={14} className="theme-select-check" />}
          </MenuRow>
        ))}
      </PickerCard>
    </div>
  );
}

// ---------------------------------------------------------------------------
// The accent swatches (appearance.rs:931-1001)
// ---------------------------------------------------------------------------

function AccentSwatch(props: {
  readonly choice: AccentSelection;
  readonly selected: boolean;
  readonly variant: ThemeVariant;
}) {
  const roles = accentForVariant(props.variant, props.choice);
  const label =
    props.choice === "themeDefault"
      ? "Theme default"
      : (accentPresets.find((preset) => preset.id === props.choice)?.label ?? props.choice);
  return (
    <button
      type="button"
      role="radio"
      aria-checked={props.selected}
      title={label}
      aria-label={label}
      className={`accent-swatch ${props.selected ? "accent-swatch-selected" : ""}`}
      onClick={() => appearanceStore.setAccent(props.choice)}
    >
      <span className="accent-swatch-chip">
        {props.choice === "themeDefault" ? (
          <span className="accent-sample-glyph" style={{ background: roles.wash }}>
            <span style={{ background: roles.glyph[0] }} />
            <span style={{ background: roles.glyph[1] }} />
            <span style={{ background: roles.glyph[2] }} />
          </span>
        ) : (
          <span className="accent-sample-flat" style={{ background: roles.primary }} />
        )}
      </span>
    </button>
  );
}

// ---------------------------------------------------------------------------
// The interface font/size pickers (appearance.rs:2295-2554)
// ---------------------------------------------------------------------------

function InterfaceFontBlock(props: {
  readonly settings: ReturnType<typeof useUiSettings>;
}) {
  const effectiveFont = effectiveUiFontFamily(props.settings.uiFontFamily);
  return (
    <div className="settings-font-block">
      <div className="settings-font-row">
        <div className="settings-font-copy">
          <span className="settings-field-label">Interface font</span>
          <p className="settings-font-description">
            Menus, sidebars, and conversation text.
          </p>
        </div>
        <div className="settings-font-controls">
          <FontFamilySelect
            value={effectiveFont}
            choices={UI_FONT_CHOICES}
            ariaLabel="Interface font"
            onCommit={(family) => uiSettings.updateImmediate({ uiFontFamily: family })}
          />
          <FontSizeSelect
            value={props.settings.uiFontSize}
            sizes={UI_FONT_SIZES}
            ariaLabel="Interface font size"
            onCommit={(size) => uiSettings.updateImmediate({ uiFontSize: size })}
          />
        </div>
      </div>
      {props.settings.uiFontFamily !== effectiveFont && (
        <p className="error-strip font-error-strip">
          <Icon name="dangerTriangle" size={16} className="error-strip-icon" />
          This font could not be loaded. Zeron is using Geist.
        </p>
      )}
    </div>
  );
}

/**
 * The terminal and code/diff slots (settings/appearance.rs FontKind::Terminal
 * / ::Code): independent families and sizes on the shared mono ladder. The
 * terminal's catalog is the fixed-width subset only.
 */
function MonoFontBlock(props: {
  readonly kind: "terminal" | "code";
  readonly settings: ReturnType<typeof useUiSettings>;
}) {
  const terminal = props.kind === "terminal";
  const requested = terminal
    ? props.settings.terminalFontFamily
    : props.settings.codeFontFamily;
  const effectiveFont = terminal
    ? effectiveTerminalFontFamily(requested)
    : effectiveCodeFontFamily(requested);
  const size = terminal ? props.settings.terminalFontSize : props.settings.codeFontSize;
  const choices = terminal ? TERMINAL_FONT_CHOICES : CODE_FONT_CHOICES;
  return (
    <div className="settings-font-block">
      <div className="settings-font-row">
        <div className="settings-font-copy">
          <span className="settings-field-label">
            {terminal ? "Terminal font" : "Code & diff font"}
          </span>
          <p className="settings-font-description">
            {terminal
              ? "Terminal panes and shell output. Fixed-width families only."
              : "Code blocks, diffs, and workspace file editors."}
          </p>
        </div>
        <div className="settings-font-controls">
          <FontFamilySelect
            value={effectiveFont}
            choices={choices}
            ariaLabel={terminal ? "Terminal font" : "Code font"}
            onCommit={(family) =>
              uiSettings.updateImmediate(
                terminal ? { terminalFontFamily: family } : { codeFontFamily: family },
              )
            }
          />
          <FontSizeSelect
            value={nearestMonoFontSize(size)}
            sizes={MONO_FONT_SIZES}
            ariaLabel={terminal ? "Terminal font size" : "Code font size"}
            onCommit={(next) =>
              uiSettings.updateImmediate(
                terminal ? { terminalFontSize: next } : { codeFontSize: next },
              )
            }
          />
        </div>
      </div>
      {requested !== effectiveFont && (
        <p className="error-strip font-error-strip">
          <Icon name="dangerTriangle" size={16} className="error-strip-icon" />
          {terminal
            ? `Proportional fonts can't drive the terminal grid. Zeron is using ${fontFamilyLabel(effectiveFont)}.`
            : `This font could not be loaded. Zeron is using ${fontFamilyLabel(effectiveFont)}.`}
        </p>
      )}
    </div>
  );
}

function FontFamilySelect(props: {
  readonly value: UiFontChoice;
  readonly choices: readonly UiFontChoice[];
  readonly ariaLabel: string;
  readonly onCommit: (family: UiFontChoice) => void;
}) {
  return (
    <RbSelect<UiFontChoice>
      value={props.value}
      onValueChange={(next) => {
        if (next !== null) {
          props.onCommit(next);
        }
      }}
      overlaySource="settings-font-family"
    >
      <RbSelectTrigger className="settings-select-trigger font-trigger" aria-label={props.ariaLabel}>
        <span className="settings-select-label">{fontFamilyLabel(props.value)}</span>
        <Icon name="altArrowDown" size={14} className="settings-select-caret" />
      </RbSelectTrigger>
      <RbSelectPortal>
        <RbSelectPositioner>
          <RbSelectPopup className="popover-card settings-select-menu font-menu">
            {props.choices.map((family) => (
              <RbSelectItem key={family} value={family} className="settings-select-item">
                <span className="settings-select-item-label">{fontFamilyLabel(family)}</span>
                <span className="settings-select-check">
                  {family === props.value && <Icon name="check" size={14} />}
                </span>
              </RbSelectItem>
            ))}
          </RbSelectPopup>
        </RbSelectPositioner>
      </RbSelectPortal>
    </RbSelect>
  );
}

function FontSizeSelect(props: {
  readonly value: number;
  readonly sizes: readonly number[];
  readonly ariaLabel: string;
  readonly onCommit: (size: number) => void;
}) {
  return (
    <RbSelect<number>
      value={props.value}
      onValueChange={(next) => {
        if (next !== null) {
          props.onCommit(next);
        }
      }}
      overlaySource="settings-font-size"
    >
      <RbSelectTrigger className="settings-select-trigger size-trigger" aria-label={props.ariaLabel}>
        <span className="settings-select-label">{fontSizePxLabel(props.value)}</span>
        <Icon name="altArrowDown" size={14} className="settings-select-caret" />
      </RbSelectTrigger>
      <RbSelectPortal>
        <RbSelectPositioner>
          <RbSelectPopup className="popover-card settings-select-menu size-menu">
            {props.sizes.map((size) => (
              <RbSelectItem key={size} value={size} className="settings-select-item">
                <span className="settings-select-item-label">{fontSizePxLabel(size)}</span>
                <span className="settings-select-check">
                  {size === props.value && <Icon name="check" size={14} />}
                </span>
              </RbSelectItem>
            ))}
          </RbSelectPopup>
        </RbSelectPositioner>
      </RbSelectPortal>
    </RbSelect>
  );
}

// ---------------------------------------------------------------------------
// The conversation-width slider (appearance.rs render_transcript_width,
// upstream cbf2ad84)
// ---------------------------------------------------------------------------

/**
 * The conversation column's maximum width: a 240px slider on the 560–1200
 * ladder with a 16px step. Drag samples write through the debounced policy —
 * the snapshot moves synchronously so the transcript reflows live under the
 * pointer, while one coalesced write reaches storage; releasing the pointer
 * flushes it. The value/Reset row and the scale labels share the surrounding
 * whitespace (invisible, never reflowing) and reveal on hover, drag, and
 * keyboard focus, as on the desktop.
 */
function ConversationWidthBlock(props: {
  readonly settings: ReturnType<typeof useUiSettings>;
}) {
  const width = props.settings.transcriptWidth;
  const fraction =
    (width - TRANSCRIPT_WIDTH_MIN) / (TRANSCRIPT_WIDTH_MAX - TRANSCRIPT_WIDTH_MIN);
  const sliderRef = useRef<HTMLDivElement | null>(null);
  const [hovered, setHovered] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [keyboard, setKeyboard] = useState(false);
  const [focused, setFocused] = useState(false);
  const showDetails = hovered || dragging || (keyboard && focused);

  /** `drag_width` — the pointer's x, mapped across the 7px end insets. */
  const widthFromPointerX = (x: number): number => {
    const bounds = sliderRef.current?.getBoundingClientRect();
    if (bounds === undefined) {
      return width;
    }
    const raw = (x - bounds.left - 7) / Math.max(bounds.width - 14, 1);
    const clamped = Math.min(Math.max(raw, 0), 1);
    return normalizeTranscriptWidth(
      TRANSCRIPT_WIDTH_MIN + clamped * (TRANSCRIPT_WIDTH_MAX - TRANSCRIPT_WIDTH_MIN),
    );
  };

  return (
    <div className="settings-font-block">
      <div className="settings-font-row">
        <div className="settings-font-copy">
          <span className="settings-field-label">Conversation width</span>
          <p className="settings-font-description">
            Maximum width of messages. Adapts to smaller windows.
          </p>
        </div>
        <div
          className="settings-width-control"
          onPointerEnter={() => setHovered(true)}
          onPointerLeave={() => setHovered(false)}
        >
          <div className="settings-width-details" data-hidden={showDetails ? undefined : "hidden"}>
            <span>{`${Math.round(width)} px`}</span>
            <button
              type="button"
              className="settings-width-reset"
              onClick={() => uiSettings.updateImmediate({ transcriptWidth: TRANSCRIPT_WIDTH_DEFAULT })}
            >
              Reset
            </button>
          </div>
          <div
            ref={sliderRef}
            className="settings-width-slider"
            role="slider"
            tabIndex={0}
            aria-label="Conversation width"
            aria-valuemin={TRANSCRIPT_WIDTH_MIN}
            aria-valuemax={TRANSCRIPT_WIDTH_MAX}
            aria-valuenow={width}
            aria-valuetext={`${Math.round(width)} px`}
            onPointerDown={(event) => {
              event.currentTarget.setPointerCapture(event.pointerId);
              setDragging(true);
              setKeyboard(false);
              uiSettings.updateDebounced({
                transcriptWidth: widthFromPointerX(event.clientX),
              });
            }}
            onPointerMove={(event) => {
              if (dragging) {
                uiSettings.updateDebounced({
                  transcriptWidth: widthFromPointerX(event.clientX),
                });
              }
            }}
            onLostPointerCapture={() => {
              setDragging(false);
              uiSettings.flush();
            }}
            onKeyDown={(event) => {
              const next =
                event.key === "left" || event.key === "down"
                  ? width - TRANSCRIPT_WIDTH_STEP
                  : event.key === "right" || event.key === "up"
                    ? width + TRANSCRIPT_WIDTH_STEP
                    : event.key === "home"
                      ? TRANSCRIPT_WIDTH_MIN
                      : event.key === "end"
                        ? TRANSCRIPT_WIDTH_MAX
                        : null;
              if (next === null) {
                return;
              }
              setKeyboard(true);
              event.preventDefault();
              uiSettings.updateDebounced({ transcriptWidth: next });
            }}
            onFocus={() => setFocused(true)}
            onBlur={() => setFocused(false)}
          >
            <div className="settings-width-rail">
              <div className="settings-width-fill" style={{ width: `${fraction * 100}%` }} />
              <div
                className="settings-width-knob"
                style={{ "--rb-width-fraction": `${fraction * 100}%` } as CSSProperties}
              />
            </div>
          </div>
          <div className="settings-width-scale" data-hidden={showDetails ? undefined : "hidden"}>
            <span>560 px</span>
            <span>1,200 px</span>
          </div>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// The theme library (appearance.rs:1760-1971)
// ---------------------------------------------------------------------------

function LibraryEntryRow(props: {
  readonly entry: ThemeLibraryEntry;
  readonly onReview: () => void;
  readonly onDuplicate: () => void;
  readonly onRemove: () => void;
}) {
  const entry = props.entry;
  const variantCount = entry.family.variants.length;
  const status = `Imported · ${variantCount} variant${variantCount === 1 ? "" : "s"} · ${entry.importedFrom ?? "Self-contained snapshot"}`;
  return (
    <div className="settings-row">
      <RowTile icon="document" />
      <div className="settings-row-main">
        <span className="settings-row-title">{entry.name}</span>
        <span className="library-entry-status">{status}</span>
      </div>
      <div className="settings-row-actions library-entry-actions">
        <CompactAction onClick={props.onReview}>Review</CompactAction>
        <CompactAction onClick={props.onDuplicate}>Duplicate</CompactAction>
        <CompactActionDanger onClick={props.onRemove}>Remove</CompactActionDanger>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// The import dialog (appearance.rs:1213-1694)
// ---------------------------------------------------------------------------

interface ImportDialogState {
  readonly fileName: string | null;
  readonly compilation: ThemeCompilation | null;
  readonly selected: ReadonlySet<string>;
  readonly detailsVariant: string | null;
  readonly error: string | null;
}

function ThemeImportDialog(props: {
  readonly state: ImportDialogState;
  readonly setState: (next: ImportDialogState) => void;
  readonly onClose: () => void;
}) {
  const state = props.state;
  const inputRef = useRef<HTMLInputElement>(null);
  const compilation = state.compilation;
  const ready = compilation !== null && state.selected.size > 0;

  const onPickFile = (file: File): void => {
    void file.text().then((text) => {
      const familyName = sourceName(file.name);
      const familyId = `custom-${slug(familyName)}`;
      try {
        const parsed = parseThemeSource(text, familyId, familyName, file.name);
        props.setState({
          fileName: file.name,
          compilation: parsed,
          selected: new Set(parsed.family.variants.map((variant) => variant.id)),
          detailsVariant: null,
          error: null,
        });
      } catch (error) {
        props.setState({
          fileName: file.name,
          compilation: null,
          selected: new Set(),
          detailsVariant: null,
          error: error instanceof Error ? error.message : String(error),
        });
      }
    });
  };

  const onPrimary = (): void => {
    if (compilation === null) {
      // `compile_import`'s empty-source rejection, web copy (§2.11).
      props.setState({ ...state, error: "Choose a theme file." });
      return;
    }
    if (state.selected.size === 0) {
      props.setState({ ...state, error: "Select at least one variant to import." });
      return;
    }
    try {
      themeLibrary.install(compilation, [...state.selected]);
      props.onClose();
    } catch (error) {
      props.setState({ ...state, error: error instanceof Error ? error.message : String(error) });
    }
  };

  return (
    <Dialog ariaLabel="Add a theme" onClose={props.onClose}>
      <div className="import-dialog-card">
        <header className="import-dialog-header">
          <div className="import-dialog-heading">
            <h2 className="import-dialog-title">Add a theme</h2>
            <p className="import-dialog-body">Import a local theme into your library.</p>
          </div>
          <button type="button" className="import-dialog-close" aria-label="Close" onClick={props.onClose}>
            <Icon name="close" size={12} className="import-dialog-close-icon" />
          </button>
        </header>
        <div className="import-dialog-main">
          <span className="import-section-label">Source</span>
          <div className="import-source-row">
            <div className="import-source-field">{state.fileName ?? ""}</div>
            <CompactAction className="import-browse" onClick={() => inputRef.current?.click()}>
              Browse…
            </CompactAction>
            <input
              ref={inputRef}
              type="file"
              accept="application/json,.json"
              hidden
              onChange={(event) => {
                const file = event.target.files?.[0];
                event.target.value = "";
                if (file !== undefined) {
                  onPickFile(file);
                }
              }}
            />
          </div>
          <div className="import-mode-block">
            <span className="import-mode-label">Import a copy</span>
            <p className="import-mode-description">Works independently from the original file.</p>
          </div>
          {compilation === null ? (
            <p className="import-info-line">
              <Icon name="infoCircle" size={13} className="import-info-icon" />
              Zeron finds light and dark variants automatically.
            </p>
          ) : (
            <>
              <div className="import-detected-header">
                <span className="import-section-label">Detected themes</span>
                <span className="import-detected-count">
                  {compilation.family.variants.length} variant
                  {compilation.family.variants.length === 1 ? "" : "s"}
                </span>
              </div>
              {compilation.family.variants.map((variant) => (
                <ImportVariantRow
                  key={variant.id}
                  variant={variant}
                  report={compilation.reports.get(variant.id)}
                  selected={state.selected.has(variant.id)}
                  detailsOpen={state.detailsVariant === variant.id}
                  onToggleSelected={() => {
                    const next = new Set(state.selected);
                    if (next.has(variant.id)) {
                      next.delete(variant.id);
                    } else {
                      next.add(variant.id);
                    }
                    props.setState({ ...state, selected: next });
                  }}
                  onToggleDetails={() => {
                    props.setState({
                      ...state,
                      detailsVariant: state.detailsVariant === variant.id ? null : variant.id,
                    });
                  }}
                />
              ))}
              {compilation.failures.map((failure) => (
                <p key={failure.name} className="import-failure-strip">
                  {failure.name} could not be compiled · {failure.message}
                </p>
              ))}
            </>
          )}
          {state.error !== null && (
            <p className="import-error-strip">
              <Icon name="dangerTriangle" size={13} className="import-error-icon" />
              {state.error}
            </p>
          )}
        </div>
        <footer className="import-dialog-footer">
          <CompactAction className="import-footer-cancel" onClick={props.onClose}>
            Cancel
          </CompactAction>
          <BtnPrimary
            className={`import-footer-primary ${compilation !== null && !ready ? "import-footer-disabled" : ""}`}
            onClick={compilation === null || ready ? onPrimary : undefined}
          >
            {compilation !== null ? "Import selected" : "Analyze theme"}
          </BtnPrimary>
        </footer>
      </div>
    </Dialog>
  );
}

function ImportVariantRow(props: {
  readonly variant: ThemeVariant;
  readonly report: ThemeImportReport | undefined;
  readonly selected: boolean;
  readonly detailsOpen: boolean;
  readonly onToggleSelected: () => void;
  readonly onToggleDetails: () => void;
}) {
  return (
    <div className={`import-variant-row ${props.selected ? "import-variant-row-selected" : ""}`}>
      <div className="import-variant-row-head">
        <button
          type="button"
          className={`import-variant-select ${props.selected ? "import-variant-select-on" : ""}`}
          role="checkbox"
          aria-checked={props.selected}
          aria-label={props.variant.name}
          onClick={props.onToggleSelected}
        >
          {props.selected && <Icon name="check" size={12} className="import-variant-check" />}
        </button>
        <PalettePreview variant={props.variant} />
        <div className="import-variant-main">
          <span className="import-variant-name">{props.variant.name}</span>
          <span className="import-variant-appearance">
            {props.variant.appearance === "dark" ? "Dark" : "Light"}
          </span>
        </div>
        <CompactAction onClick={props.onToggleDetails}>{props.detailsOpen ? "Hide details" : "Details"}</CompactAction>
      </div>
      {props.detailsOpen && (
        <div className="import-variant-details">
          <ImportScenePreview variant={props.variant} />
          {props.report !== undefined && <ReportPanel report={props.report} />}
        </div>
      )}
    </div>
  );
}

/**
 * `import_scene_preview` (appearance.rs:775-856): the 3-pane mock editor —
 * a 152px miniature, a fake code line + the ANSI strip, and the diff pane
 * with the only place a candidate's diff colors preview (0.35 alpha).
 */
function ImportScenePreview(props: { readonly variant: ThemeVariant }) {
  const { colors, syntax, terminal } = props.variant;
  return (
    <div className="import-scene-preview">
      <div className="scene-miniature">
        <ThemeMiniature variant={props.variant} corners="all" />
      </div>
      <div className="scene-code" style={{ borderColor: colors.border, background: colors.background }}>
        <code className="scene-code-line">
          <span style={{ color: syntax.keyword }}>fn </span>
          <span style={{ color: syntax.function }}>preview</span>
          <span style={{ color: syntax.punctuation }}>() {"{"}</span>
        </code>
        <code className="scene-code-line" style={{ color: syntax.string }}>
          {"  \"Theme mapping\""}
        </code>
        <div className="scene-ansi">
          {terminal.ansi.slice(0, 8).map((color, ix) => (
            <span key={ix} className="scene-ansi-slot" style={{ background: color }} />
          ))}
        </div>
      </div>
      <div className="scene-diff" style={{ borderColor: colors.border, background: colors.shell }}>
        <span className="scene-diff-bar" style={{ background: `color-mix(in srgb, ${colors.diffAdd} 35%, transparent)` }} />
        <span className="scene-diff-bar" style={{ background: `color-mix(in srgb, ${colors.diffDelete} 35%, transparent)` }} />
        <span className="scene-diff-bar" style={{ background: props.variant.accent.wash }} />
      </div>
    </div>
  );
}

/** `report_panel` (appearance.rs:858-929): the mapping/validation log. */
function ReportPanel(props: { readonly report: ThemeImportReport }) {
  const lines = useMemo(() => reportLines(props.report), [props.report]);
  return (
    <div className="report-panel">
      <span className="report-summary">{reportSummary(props.report)}</span>
      {lines.map((line, ix) => (
        <span key={ix} className="report-line">
          {line}
        </span>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// The review dialog (appearance.rs:1709-1758)
// ---------------------------------------------------------------------------

function ThemeReviewDialog(props: {
  readonly entry: ThemeLibraryEntry | null;
  readonly onClose: () => void;
}) {
  const entry = props.entry;
  if (entry === null) {
    return null;
  }
  return (
    <Dialog ariaLabel="Theme mapping" onClose={props.onClose}>
      <div className="review-dialog-card">
        <h2 className="import-dialog-title">Theme mapping</h2>
        <p className="import-dialog-body review-dialog-body">
          {entry.name} · Imported
        </p>
        {entry.family.variants.map((variant) => (
          <div key={variant.id} className="review-variant">
            <span className="review-variant-name">{variant.name}</span>
            <ImportScenePreview variant={variant} />
            {entry.reports.get(variant.id) !== undefined && (
              <ReportPanel report={entry.reports.get(variant.id)!} />
            )}
          </div>
        ))}
        <div className="review-dialog-footer">
          <BtnPrimary onClick={props.onClose}>Done</BtnPrimary>
        </div>
      </div>
    </Dialog>
  );
}
