# The component layers

Two layers under `src/components/`, one rule each:

| Layer | Owns | Never |
| --- | --- | --- |
| `base/` | Base UI wrappers — the parity contract encoded ONCE per primitive: `RbPopover` (no-flip positioning, `modal: false`, the escape/focus return, exit window, `overlaySource`), `RbDialog`/`RbDialogGlass`, `RbMenu`/`RbContextMenu`, `RbTooltip`, `RbSelect`, `RbSwitch`, plus `positioning.ts`/`overlay.ts` (pure). | Re-implementing a contract a wrapper already encodes, or composing surfaces. |
| `ui/` | Composites over `base/` — the shapes surfaces were re-inventing. Data + callbacks in, DOM out; no engine state. | Talking to `@base-ui` directly, or re-implementing a base/ contract. |

Everything else in `src/components/` is a surface (composer, sidebar,
transcript, …): it composes `ui/` + `base/`, owns its engine/session state
and its surface-specific CSS classes.

**New tickets build on `ui/` first.** Do not hand-roll card shells
(`PickerCard`), cursor lists (`useCursorList`), search frames
(`PickerSearchField`), menu rows/headings/separators (`MenuRows`), chips
(`Chip`), dialogs (`Dialog`), tooltips (`Tooltip`), key hints (`KeyHint`),
scrollbar rails (`Scrollbar`), or skeleton/error rows (`Skeleton`). If a
shape is missing, add it to `ui/` (pattern below) — don't fork it into the
surface.

## What lives where

- `base/popover.tsx` → **`RbPopover`**, `RbPopoverTrigger`,
  `createRbPopoverHandle`. A floating card's positioning, dismissal,
  focus, and keyboard-claim contract. `ui/PickerCard.tsx` wraps this
  whole family into one declarative unit (trigger element + card body +
  width/role/aria/overlay props).
- `ui/CursorList.tsx` → **`useCursorList`** (the cursor keyboard model:
  `menuStep` walk, Enter activation, scrollIntoView-on-cursor; `mode:
  "nullable"` for the desktop's `Option<usize>` menus), plus
  **`PickerSearchField`**/`SearchInputFrame` (the search input frame).
  The pure reducers (`menuStep`/`classifyKey`/`filterIndices`) stay in
  `lib/picker-search.ts` — they also serve store-level consumers
  (`state/add-space.ts`, `lib/`), so they are not component-owned.
- `ui/MenuRows.tsx` → `MenuRow`/`MenuRowNav`/`MenuHeading`/
  `MenuSeparator`/`MenuSection` — the row vocabulary of every card.
- `ui/Chip.tsx` → `FooterChip`/`FooterLabel` + `openChipClass` (the
  trigger open-snap convention).
- `ui/Dialog.tsx` → `Dialog` (mount-while-open shell over `RbDialog`) +
  the dialog card chrome.
- `ui/Tooltip.tsx` → `Tooltip` + the delay conventions (280 family
  default, 350 view-options, 500 context-meter).
- `ui/PopoverCard.tsx`, `ui/KeyHint.tsx`, `ui/Scrollbar.tsx`,
  `ui/Skeleton.tsx` → the remaining card chrome families (bare card
  frames, key caps, the floating-scrollbar rail, skeleton/error rows).
- Plain divs with surface classes (`.picker-empty-note` and friends)
  stay inline at the surface — `ui/` is not a class-rename machine.

Two standing split rules, inherited from the Base UI adoption
(`.scratch/web-parity/research-2026-09-17/baseui-step0-1-report.md`):

1. **Cursor menus vs focus menus.** Search-driven pickers and cursor menus
   ride `PickerCard` (→ `RbPopover`) + `useCursorList`; only a surface whose
   desktop self is genuinely a focus-walking, typeahead menu uses
   `base/menu.tsx`'s `RbMenu`.
2. **Escape is never the consumer's job.** It flows Base UI's dismiss
   pipeline; `escapeFocusTarget` is how a surface opts into the focus
   return.

## Adding a new `ui/` component

1. One self-contained file per component family, named after the
   component (`PickerCard.tsx`, not `pickers.tsx`).
2. Typed props; data + callbacks in, DOM out. No engine state, no
   session imports — the surface owns those.
3. Tokens only: classes from `styles/app.css` (already token-skinned in
   `styles/tokens.css`); no new hex/px literals where a `--rb-*` token
   exists.
4. The desktop citation goes in the file's header comment
   (`pickers.rs:2341-2422`, `popover.rs:713-765`, …) — the citation lives
   once, at the component, not copied into every consumer.
5. Compose `base/`; never import `@base-ui/*` outside `base/`.
6. Pure helpers that also serve `lib/`/`state/` consumers stay in `lib/`
   (`lib/picker-search.ts` is the precedent).
