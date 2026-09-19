# Rich composer PR plan

Status: rich-composer hardening validated on September 18. The current
settings live under **Agents**, with a logo and independent `$` completion and
slash-menu separation preferences for each of the nine production harnesses.
See the current [command mapping audit](../harness-command-audit.md) for command
semantics and provider limits. The sections below preserve earlier milestones;
their original settings locations and harness counts are historical.

## September 18 hardening

- Restored the original active/bare-bullet contract: bullets remain rendered
  while editing before, inside, or after a list. Added parser-aware task,
  heading, strikethrough, quoted-list, CRLF, and fenced-code handling while
  retaining canonical Markdown and editable active syntax.
- Synchronized projection with edits, selection, undo, and IME composition;
  preserved UTF-16 replacement offsets, soft-wrap caret affinity, and vertical
  navigation's preferred horizontal position. Native fixtures exercise actual
  arrow-key movement as well as preselected caret positions.
- Kept literal code/image examples literal when copied; external clipboard text
  uses readable links while internal paste restores canonical references.
  Repeated chips stay compact and distinct paths retain distinct labels.
- Completion respects closing formatting, link destinations, reference
  definitions, quote prefixes, and Unicode combining marks. Insertion preserves
  surrounding Markdown and punctuation. Malformed remote catalog entries are
  filtered before they can produce undecodable references.
- Hardened canonical labels and cross-harness delivery, including malformed
  skill discovery, native command whitespace, retries, steering, attachments,
  and queue edits. All nine production harnesses share the same rich editor.
- No ZUI changes or companion checkout are required. Native fixture evidence
  covers light/narrow and dark/wide composers; full live-provider and platform
  QA remain separate from these automated and isolated native checks.

Validation for this batch: 1,120 UI library tests passed serially; 37 protocol
tests and 319 harness tests passed, with 10 environment/live-provider tests
ignored. The subsequent catalog-validation changes passed 16 focused harness
tests and the 37 protocol tests. Three engine delivery matrices cover all nine
harnesses. `cargo check --locked -p zeron` passed. The native fixture generated
42 captures, including active/inactive bullets, quoted tasks, CRLF lists,
Unicode wrapping, and selections across chips. Caret blink can hide the caret
in a screenshot; native geometry assertions cover its exact placement.
An additional UI regression passed with real delayed RPC replies across all
nine harness selections, followed by checkout and preference changes. Eighteen
abandoned catalog replies delivered after the current result cannot replace
the visible choices or undo slash-menu separation.

A standalone optimized label probe with 5,000 repeated references dropped from
about 130 ms to 0.5 ms after grouping and reusing disambiguation results. This
measures that helper only, not end-to-end editor latency.

## Optional slash-menu skills

- Settings → Shortcuts now has **Show skills in / menu**, off by default.
  `$` always lists skills; opting in includes typed skills alongside `/`
  commands. Skill chips preserve their canonical path regardless of trigger.
- Removed Codex's legacy skills-to-commands conversion. Codex app-server does
  not advertise a separate command catalog; Zeron now maps `/compact` and
  `/review` to native operations. Providers' advertised catalogs remain intact.
  See [the harness command audit](../harness-command-audit.md) for mapping and review gaps.
- The preference persists locally and invalidates completion caches and
  in-flight requests immediately. Restore defaults turns it off.
- Passed 1,026 UI and 148 harness library tests, the Codex discovery integration
  regression, and `cargo build --locked -p zeron`. Native settings interaction
  could not be completed because UI automation returned `noWindowsAvailable`.

## Inline icon and model menu follow-up

- File/folder chips now use the existing Symbols identity icons; skill chips
  and skill completion rows use the shared Solar Widget icon. Commands retain
  their slash marker. Icon slots share the text projection's source mapping,
  including selection and IME, while clipboard/transcript text stays readable.
- Model menus now use the shared measured trigger geometry in both new and
  existing chats, with end alignment, above/below placement, a capped card,
  and separate model-list/settings-tray budgets.
- Passed all 1,023 UI library tests with `--test-threads=1`, including new icon
  slot/IME and menu budget regressions. `cargo build --locked -p zeron` passed.
- Native isolated preview verified Rust/CSS file icons, the skill icon and
  slash chip; the model menu stays aligned and shortens on window resize,
  with the settings tray visible and the last model reachable by scrolling.
  Broader theme/platform/animation checks remain pending.

## Rebase validation (2026-09-17)

- Rebased onto `origin/main` at `8ee7a622`, preserving upstream Enter/stop
  behavior, nested model menus, popup search highlighting, new-thread menu
  placement, and Windows animation support.
- Passed all 1,021 UI library tests with `--test-threads=1` (the CI setting),
  181 engine library tests, 148 harness library tests, and 27 protocol tests.
  The first concurrent UI run aborted with SIGABRT; the serial rerun passed.
- `git diff --check` passed. Full picker/animation visual QA and Linux/Xvfb
  performance checks remain pending as recorded below.

## Implementation and validation record

- Added word/line pointer selection, selection-unit dragging, typed command/skill
  chips, Unicode-safe trigger parsing, and readable clipboard text with lossless
  internal metadata.
- Added Markdown emphasis/code styling, inactive-line delimiter projection,
  typographic bullet/task markers, hanging list indents, list continuation/exit,
  and list indentation. Selection and IME use the same source/display mapping.
- Added project/device/worktree-scoped `ListSkills` discovery. Codex implements
  the skill catalog; other harnesses explicitly return unsupported. Projectless
  discovery uses the target host's home directory. Skills preserve selected
  paths and duplicate names; canonical references are stored in drafts/messages
  and translated to ordinary command text or skill Markdown links at the engine
  boundary, including steering.
- Added measured trigger/window menu budgets, device end alignment, Standard
  tier suppression, a bounded Fast bolt animation, and the existing model-list
  edge fade. Worktree interactions retain their existing behavior.
- Validation: `cargo check -p zeron-ui`, `cargo build -p zeron`, 985 UI tests,
  119 harness tests, 4 engine RPC tests, and the invocation protocol round-trip
  test passed. `git diff --check` passed. The UI tests include actual rendered
  pointer-event selection and existing layout-cache/resize regressions.
- Fixed one existing test fixture's macOS portability: its fake PATH now includes
  `/usr/bin`, allowing the shell environment probe to locate `env`.
- Follow-up from user screenshots: recognize bare `-`, `*`, and `+` list
  markers and render bullets on the active line too. Added a regression for the
  exact three-line empty-list case. Skill discovery errors now name skills.
- Native isolated preview verified three empty bullets, bold/italic text,
  numbered lists, and double-click selecting only a word. A matching engine
  returned 18 real Codex skills through `ListSkills`, preserving their paths.
- The user's debug UI was connected to the older installed application's engine
  on port 27654. The isolated matching build uses port 49783 and separate data.
  A UI-only rebuild cannot add the new skill RPC to an already running engine.
- Full picker/animation visual review and the Linux/Xvfb composer performance
  script remain unchecked. Do not treat passing unit tests as visual acceptance.

The remaining sections preserve the implementation plan and acceptance contract.

Scope: native desktop GPUI composer, the four surrounding environment selectors,
and the in-composer model picker. Proposed title: **Rich composer: Markdown,
inline commands and skills, and adaptive picker menus**.

## Observed starting point

- `crates/ui/src/composer.rs`: `press_intent` explicitly maps every click count
  of two or more to select-all. File mentions already use a canonical raw-text
  representation, display projection, atomic ranges, tooltips, and transcript
  projection. `/` completion only recognizes the first prompt token and inserts
  plain text. There is no equivalent `$` completion here.
- `crates/ui/src/pickers.rs`: the new-thread device trigger uses start alignment,
  while its neighboring project trigger uses end alignment. Project and branch
  lists have fixed 224px caps; the shared card has a fixed 640px cap, without an
  actual available-height calculation. Model rows use a virtualized list.
- `traits_summary` deliberately includes defaults today. Service-tier suppression
  must be specific to that option, preserving reasoning and other summaries.
- `crates/harness/src/codex/mod.rs` currently flattens `skills/list` results into
  `SlashCommand`, deduplicating by name and discarding skill identity/path.
  Its discovery cache is not a sufficient project-scoped skill catalog.

## Proposed interaction contract

### Markdown editing

Use lightweight rich editing backed by the existing Markdown string. The first
version supports bullets, numbered lists, task lists, nesting, bold, italic,
inline code, and fenced code. Reuse transcript typography, spacing, and colors;
unsupported Markdown remains editable literal text.

- Render bullet/number/task markers and hanging indents so wrapped list items
  align with their content. Formatting delimiters remain editable; expose hidden
  delimiters on the active editing span, with stable source/display mapping.
- A newline action continues the current list; on an empty item it exits the
  list. Tab/Shift-Tab indent/outdent a list item when completion is closed.
  Preserve the configured send/newline shortcut contract: a send action must
  still send, and accepting completion takes precedence over list indentation.
- Style inline emphasis and code. Code regions suppress completion triggers and
  automatic list transformations. Pasting preserves source text and indentation.
- Keep canonical Markdown for draft storage, undo, copy/paste, queued prompts,
  edits, and submission. Reuse parser concepts from `markdown/parser.rs`, but
  keep editable hit testing and IME inside `ComposerInput`.

### Selection and inline chips

- Single click places the caret; drag selects the traversed range; double click
  selects a word; triple click selects a logical line. Cmd/Ctrl+A selects all.
  Double-click drag extends by word; triple-click drag extends by line.
- Treat a resolved chip as an atomic editing unit. Selection, arrow movement,
  deletion, undo, and clipboard operations must agree on its boundaries.
- `/`, `$`, and `@` completion works at valid token boundaries anywhere in prose,
  including within list items and between existing chips. Reuse the existing
  completion card, filtering, keyboard controls, focus retention, dismissal,
  empty/loading/error states, and scroll rail.
- A chosen result becomes a chip at the caret. Plain punctuation, unknown tokens,
  email addresses, paths, currency amounts, and escaped triggers remain text.
  Do not silently convert arbitrary pasted text into references.
- Use one shared chip treatment with a clear kind marker: command `/`, skill `$`,
  file/folder `@`. Keep descriptions and full paths available through the existing
  tooltip treatment. Reuse disambiguation for duplicate names.
- Chip placement is independent of execution semantics. An inline `/command`
  must not be moved to the beginning or executed as a separate action. Preserve
  each provider's supported command transport and prefix behavior; document
  where a non-leading command is simply prompt text.

### Environment selectors

- End-align the device menu to its trigger, matching the adjacent project menu
  in the trailing selector cluster. Check both new-thread and existing-chat
  layouts, since attachment paths differ.
- Give project and branch menus a shared measured height budget: space from the
  trigger to the usable window edge, minus menu gap and outer margin. Prefer
  opening above; use below when above cannot fit the menu's minimum usable
  chrome and the other side offers more room.
- Subtract search/header/footer heights from that budget and constrain the list
  itself. Keep chrome visible and the trigger anchored while results scroll.
  Recompute after window resizing and composer growth; window-edge clamping
  alone does not solve an oversized menu.
- Retain the current worktree interaction and verify it with the shared geometry.

### Model selector

- Hide the effective Standard service tier from the composer summary, including
  an explicitly selected Standard. Keep it visible and selectable in the menu.
  Identify the tier by option/choice IDs, with provider normalization, rather
  than filtering every choice whose display label happens to be “Standard”.
- Show Fast with a small bolt and a brief sheen/pulse on activation. Use the
  app's motion helpers and theme colors, reserve stable width, and settle to a
  static indicator. Reduced motion shows the static indicator immediately.
  This avoids introducing permanent idle redraws for a configuration setting.
- Wrap only the model scroll region in `edge_fade::edge_faded`, gated at paint
  time using `model_scroll_base()`. Preserve virtualization, pinned search/tabs,
  traits sections, keyboard visibility, and the existing scrollbar rail.

## Implementation sequence within one PR

1. **Selection foundation.** Replace the multi-click select-all policy with
   word/line selection and range-aware dragging. Extend the existing source to
   display projection into typed spans shared by Markdown styling and chips.
   Keep this composer-specific where generic search inputs share `ComposerInput`.
2. **Typed discovery and chips.** Introduce backward-compatible skill discovery
   data with stable identity, path, description, availability, and context.
   Separate skills from commands at the harness/RPC boundary; retain existing
   command compatibility. Scope caching and stale-response rejection to device,
   harness, and effective working directory, including worktrees. Unsupported
   providers get an honest unavailable state. Generalize completion and chip
   projection, then wire `$` and non-leading `/` into it.
3. **Round-trip transport.** Specify command and skill serialization at the
   provider boundary before enabling submission. Internal chip encoding must
   never leak as an unrecognized invocation. Preserve current file mention
   transport, old drafts, clipboard readability, draft restoration, queue/edit
   paths, and sent-message rendering; retain metadata where identity needs it.
4. **Markdown layout.** Add source-aware styled runs, list markers/indents,
   newline continuation, and list indentation on top of the selection mapping.
   Integrate with shaping-cache invalidation and the existing animated viewport.
5. **Picker geometry and polish.** Add reusable sizing/alignment support in
   `popover.rs`, consume it in `pickers.rs`, and add service-tier summary,
   bounded Fast animation, and model edge fades.
6. **Validation and review captures.** Complete the checks below and attach
   before/after captures to the PR.

The main architectural risk is the combination of hidden formatting syntax,
atomic chips, UTF-8/UTF-16 offsets, wrapping, and IME. Resolve that mapping early;
avoid separate overlapping parsers with different coordinate systems.

## Acceptance and validation

- Editor tests: words/lines, reverse and shifted selection, multi-click drag,
  wrapped lines, emoji/combining characters/CJK, chip boundaries, IME marked
  ranges, undo/redo, copy/cut/paste, list continuation/exit, nesting, and the
  configured send/newline shortcuts. Update tests encoding the old select-all
  policy and test actual input events as well as pure helpers.
- Completion/transport tests: all three triggers at start/middle/end and on
  multiple lines; multiple adjacent chips; false positives; code/escaping;
  duplicate skill names; stale responses after context changes; absent skill
  support; and draft → send → transcript/edit round trips.
- Geometry tests and captures: short and narrow windows, expanded composer,
  many projects/branches, search reducing results, resizing an open menu, and
  new/existing sessions. Menu content stays inside the usable viewport, the
  trigger remains correctly aligned, and every result is reachable.
- Model checks: default and explicit Standard, Fast, no tier, other non-default
  tiers, reduced motion, light/dark and glass/opaque themes. Fades appear only
  over actual overflow and clear after filtering/clamping the scroll offset.
- Run focused tests, `cargo test -p zeron-ui --lib`, `cargo check -p zeron-ui`,
  relevant protocol/harness tests for discovery/serialization changes, and
  `git diff --check`. Inspect the built native app for caret and menu behavior.
- Reuse `scripts/profile-composer.py` and `docs/performance-composer.md` to check
  long drafts, typing, scrolling, resize, and unfocused idle. Selection and
  scrolling should reuse shaping; a settled Fast indicator schedules no frames.

Assumptions for review: Markdown is an editable rich surface without a separate
preview mode; Fast animates briefly on activation; this PR targets the desktop
composer. These are proposed product choices, not claims about existing behavior.
