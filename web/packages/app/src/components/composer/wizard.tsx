import type { Ref, ReactNode } from "react";
import type { Wizard } from "../../lib/wizard";

/**
 * The question wizard — `render_wizard` (composer.rs:6903-7104): replaces
 * the composer pill IN PLACE (same 26px radius, same width) when a run asks
 * the user a question, over a 150ms `fade_quick`. Not a modal — it is the
 * composer column's own surface, and the shared composer input is re-parented
 * into its free-text slot (`inputSlot`; the host renders the same textarea
 * subtree there, never a second input).
 *
 * The panel wrapper carries `on_wizard_key` (composer.rs:6863-6894): bare
 * digits 1-9 select only while the input is unfocused or empty (modified
 * digits are the sidebar's), Enter advances when the input is unfocused,
 * Escape pages back — swallowed either way. Keys bubbling out of the input
 * must not double-handle: the input's own Enter policy (bare Enter submits
 * the page) runs first in the textarea and stops propagation.
 */

export interface ComposerWizardProps {
  /** The wizard state (mutated by the host's callbacks). */
  readonly wizard: Wizard;
  /** Whether the shared input's text is empty — a typed override visually
   * deselects every option (zeron question-panel.tsx `isSel`). */
  readonly typedEmpty: boolean;
  /** The shared composer input, re-parented into the free-text slot. */
  readonly inputSlot: ReactNode;
  readonly onSelect: (ix: number) => void;
  readonly onAdvance: () => void;
  readonly onBack: () => void;
  /** `on_wizard_key` — the panel wrapper's key handler. */
  readonly onKeyDown: (event: React.KeyboardEvent<HTMLDivElement>) => void;
  /** The host's ref to the focus-tracked panel (`wizard_focus`). */
  readonly panelRef?: Ref<HTMLDivElement>;
}

export function ComposerWizard(props: ComposerWizardProps) {
  const { wizard, typedEmpty } = props;
  const question = wizard.current();
  if (question === null) {
    return null;
  }
  const last = wizard.page + 1 >= wizard.questions.length;
  const canAdvance = wizard.pageHasPick() || !typedEmpty;
  return (
    <div
      className="wizard-panel"
      id="question-panel"
      tabIndex={-1}
      ref={props.panelRef}
      onKeyDown={props.onKeyDown}
    >
      <div className="wizard-upper">
        <div className="wizard-header">
          <div className="wizard-header-label">{question.header}</div>
          {wizard.questions.length > 1 && <div className="wizard-counter">{wizard.counter()}</div>}
        </div>
        <div className="wizard-question">{question.question}</div>
        {question.multiSelect && (
          <div className="wizard-multiselect-hint">Select one or more options.</div>
        )}
        <div className="wizard-options">
          {question.options.map((label, ix) => (
            <WizardOptionRow
              key={`${ix}:${label}`}
              label={label}
              ix={ix}
              picked={wizard.isPicked(ix) && typedEmpty}
              onSelect={props.onSelect}
            />
          ))}
        </div>
        {/* The free-text override over a hairline (composer.rs:7065-7076):
            the shared composer input, re-parented — never a second one. */}
        <div className="wizard-input-slot">{props.inputSlot}</div>
      </div>
      <div className="wizard-footer">
        {wizard.page > 0 ? (
          <button
            type="button"
            className="wizard-btn-ghost"
            id="wizard-back"
            onClick={props.onBack}
          >
            Back
          </button>
        ) : (
          <span />
        )}
        {/*
          `!can_advance` is opacity only — the click handler still fires
          (composer.rs:7099's deliberate non-disable).
        */}
        <button
          type="button"
          className={`wizard-btn-primary${canAdvance ? "" : " wizard-btn-primary-dim"}`}
          id="wizard-submit"
          onClick={props.onAdvance}
        >
          {last ? "Submit" : "Next"}
        </button>
      </div>
    </div>
  );
}

/** One option row (composer.rs:6917-6986): the label (13.5/500) and, for
 * indices 0-8, the 22px number kbd chip. Picked reads `ink(0.16)` border +
 * `ink(0.09)` fill; unpicked hovers `ink(0.025)`→`ink(0.06)` over 150ms. */
function WizardOptionRow({
  label,
  ix,
  picked,
  onSelect,
}: {
  readonly label: string;
  readonly ix: number;
  readonly picked: boolean;
  readonly onSelect: (ix: number) => void;
}) {
  return (
    <button
      type="button"
      className={`wizard-option${picked ? " wizard-option-picked" : ""}`}
      data-option-ix={ix}
      onClick={() => onSelect(ix)}
    >
      <span className="wizard-option-label">{label}</span>
      {ix < 9 && <span className="wizard-option-number">{ix + 1}</span>}
    </button>
  );
}
