//! Composer pickers (feature-inventory §1.7): RepoPicker (recents + search +
//! in-app folder browser + clone/create), BranchPicker (search + isolated-
//! worktree toggle), HarnessModelPicker (harness rail + model list, harness
//! locked once the chat exists), TraitsPicker (reasoning ladder + advertised
//! model options; trigger shows the non-default summary "High · 1M · Fast").
//!
//! All selections accumulate into a [`DraftConfig`] the composer threads into
//! the Run command and the `Mutate createChat` call on first send.
//!
//! Pure logic (repo ordering, folder-browser navigation, traits summary) lives
//! in free functions with unit tests; RPC results land in [`Loadable`] slots
//! rendered as skeletons / inline errors with Retry.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable as _, KeyDownEvent, SharedString,
    Subscription, Task, Window, div, prelude::*, px,
};

use zeron_engine::registry::{HarnessDescriptor, TitleSettings};
use zeron_proto::{
    ChatConfig, FolderListing, HarnessId, Model, ReasoningLevel, RepoRef, SandboxLevel, Space,
};
use zeron_rpc::methods;

/// Display cap for the ref list (t3code shows pages of 100 with a status
/// footer; a flat cap + "Showing X of Y refs" reads the same without
/// pagination plumbing).
const MAX_REF_ROWS: usize = 300;

const FOOTER_CHIP_RADIUS: f32 = 6.0;

/// Both sides of the composer handoff share one leading-aligned workspace
/// cluster. Available width belongs after the pair, never between its labels.
fn workspace_footer_row() -> gpui::Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.0))
}

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::motion;
use crate::popover::{self, Loadable, MenuKey};
use crate::settings::composer::ComposerDefaults;
use crate::state::{AppState, EngineHandle};
use crate::theme::Theme;

/// Dev/testing knob: `ZERON_SLOW_CATALOG_MS=<ms>` delays every harness and
/// model catalog result app-side — the chip/tab/list loading states are
/// sub-second against a warm local daemon and unstageable otherwise
/// (headless-rig captures; same family as `ZERON_OPEN_PICKER`).
fn slow_catalog_delay() -> Option<std::time::Duration> {
    std::env::var("ZERON_SLOW_CATALOG_MS")
        .ok()
        .and_then(|ms| ms.parse::<u64>().ok())
        .map(std::time::Duration::from_millis)
}

// ---------------------------------------------------------------------------
// Catalog invalidation (Settings → Providers toggles)
// ---------------------------------------------------------------------------

/// Marker global: [`bump_harness_catalog`] pokes it whenever a Settings →
/// Agents toggle changes some device's enabled set, and every [`Pickers`]
/// observes it to force-refresh its cached harness catalog — without this the
/// composer served the boot-time list until restart (user report).
#[derive(Default)]
pub struct HarnessCatalogChanged;

impl gpui::Global for HarnessCatalogChanged {}

/// Notify all composers that some device's harness catalog changed. The
/// global carries no data — `default_global` pushes the observer effect, and
/// the observers re-fetch from the engine (the source of truth).
pub fn bump_harness_catalog(cx: &mut App) {
    cx.default_global::<HarnessCatalogChanged>();
}

// ---------------------------------------------------------------------------
// Draft config (what the pickers accumulate)
// ---------------------------------------------------------------------------

/// Everything a new chat is configured with before the first send. The folder
/// and device come from the selected SPACE — the draft only carries the git
/// extras (ref + checkout kind) and the run config.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DraftConfig {
    pub harness: Option<HarnessId>,
    pub model: Option<String>,
    pub reasoning: Option<ReasoningLevel>,
    /// The picked ref (base branch in NewWorktree mode; a worktree's branch
    /// when reusing one). `None` = the repo's current branch.
    pub branch: Option<String>,
    /// Where the new session runs (the t3code env-mode).
    pub checkout: CheckoutKind,
}

/// Where a new session runs (t3code's env-mode: `local | worktree`). "Current
/// worktree" is NOT a third mode — it's `Local` when the picked ref is already
/// materialized as a worktree (the session reuses that checkout's path).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CheckoutKind {
    /// The space's own folder — or the picked ref's existing worktree.
    #[default]
    Local,
    /// A fresh isolated worktree created off the picked base ref on send.
    NewWorktree,
}

/// The resolved on-send checkout action (composer consumes this — see
/// [`Pickers::checkout_plan`]).
#[derive(Debug, Clone, PartialEq)]
pub enum CheckoutPlan {
    /// Run in the space folder as-is. `branch` is the checkout's branch (the
    /// picked or current ref), carried onto `createChat` so the session names
    /// it from the first frame; `None` = refs never loaded.
    CurrentCheckout { branch: Option<String> },
    /// Reuse the picked ref's existing worktree (a cwd override; no git).
    ReuseWorktree { path: String, branch: String },
    /// `CreateWorktree` off `base` on send (zeron mints a `zeron/<name>`
    /// branch). `base: None` = refs never loaded — send falls back to the
    /// space folder rather than failing.
    NewWorktree { base: Option<String> },
}

/// The fully-resolved run configuration the composer sends: concrete harness,
/// model and reasoning (never a "default" passthrough once the catalog is
/// loaded), plus the explicit non-default option picks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedRunConfig {
    pub harness: Option<HarnessId>,
    pub model: Option<String>,
    pub reasoning: Option<ReasoningLevel>,
    pub model_options: serde_json::Map<String, serde_json::Value>,
}

impl ResolvedRunConfig {
    /// The `ChatConfig` recorded on `Mutate createChat` (needs a known harness).
    pub fn chat_config(&self) -> Option<ChatConfig> {
        Some(ChatConfig {
            harness: self.harness?,
            model: self.model.clone(),
            reasoning: self.reasoning,
            model_options: self.model_options.clone(),
            sandbox: SandboxLevel::WorkspaceWrite,
        })
    }
}

// ---------------------------------------------------------------------------
// Pure: default resolution (no "Default" placeholders — a concrete pick always)
// ---------------------------------------------------------------------------

/// The harness's default model: the first catalog row (both curated catalogs
/// lead with the flagship — zeron's `pickDefaultModel` Opus preference maps to
/// the same row here).
pub fn default_model(models: &[Model]) -> Option<&Model> {
    models.first()
}

/// An explicit selection never silently becomes a different model after refresh.
fn selected_catalog_model<'a>(models: &'a [Model], selected: Option<&str>) -> Option<&'a Model> {
    match selected {
        Some(id) => models.iter().find(|model| model.id == id),
        None => default_model(models),
    }
}

/// A model's default reasoning: X-High when the ladder offers it (zeron
/// `DEFAULT_REASONING = "xhigh"`), else High, else the ladder's first entry.
/// `None` only for ladder-less models (e.g. Haiku's thinking toggle instead).
pub fn default_reasoning(ladder: &[ReasoningLevel]) -> Option<ReasoningLevel> {
    // The recommended default is High (user-corrected — not X-High globally);
    // fall to Medium then the ladder's first entry for shorter ladders.
    if ladder.contains(&ReasoningLevel::High) {
        return Some(ReasoningLevel::High);
    }
    if ladder.contains(&ReasoningLevel::Medium) {
        return Some(ReasoningLevel::Medium);
    }
    ladder.first().copied()
}

/// Clamp a picked/remembered level to what the model actually offers: keep it
/// when the ladder lists it, else fall to the model's default (never a stale
/// or foreign level — zeron use-run-config.ts's derived-model discipline).
pub fn clamp_reasoning(
    level: Option<ReasoningLevel>,
    ladder: &[ReasoningLevel],
) -> Option<ReasoningLevel> {
    match level {
        Some(level) if ladder.contains(&level) => Some(level),
        _ => default_reasoning(ladder),
    }
}

// ---------------------------------------------------------------------------
// Pure: labels + traits summary
// ---------------------------------------------------------------------------

pub fn reasoning_label(level: ReasoningLevel) -> &'static str {
    match level {
        ReasoningLevel::Minimal => "Minimal",
        ReasoningLevel::Low => "Low",
        ReasoningLevel::Medium => "Medium",
        ReasoningLevel::High => "High",
        ReasoningLevel::XHigh => "X-High",
        ReasoningLevel::Max => "Max",
        ReasoningLevel::Ultra => "Ultra",
        ReasoningLevel::Ultracode => "Ultracode",
        ReasoningLevel::Ultrathink => "Ultrathink",
    }
}

/// Keep only the picks `model` still offers. Remembered picks outlive the
/// model they were made on, and harnesses apply some options blindly (Claude
/// appends `[1m]` to any model id when `contextWindow` is "1m").
pub fn offered_options(
    model: &Model,
    mut selections: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    selections.retain(|id, choice| {
        model.options.iter().any(|option| {
            option.id == *id
                && choice
                    .as_str()
                    .is_some_and(|choice| option.choices.iter().any(|c| c.id == choice))
        })
    });
    selections
}

// ---------------------------------------------------------------------------
// Pure: folder-browser navigation (used by the shell's add-space flow)
// ---------------------------------------------------------------------------

/// Parent of an absolute path; `None` at the filesystem root.
pub fn parent_path(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None; // was "/" (or empty)
    }
    match trimmed.rfind('/') {
        Some(0) => Some("/".to_string()),
        Some(at) => Some(trimmed[..at].to_string()),
        None => None,
    }
}

/// Join a listing path and an entry name.
pub fn child_path(base: &str, name: &str) -> String {
    if base.ends_with('/') {
        format!("{base}{name}")
    } else {
        format!("{base}/{name}")
    }
}

/// Byte length of `name`'s prefix matching `query`, compared char-for-char
/// case-insensitively; `None` when `query` isn't a prefix of `name`. The
/// length indexes into `name` (not `query`) so the completion suffix keeps
/// the folder's real casing: `("Documents", "doc") → Some(3)` → `"uments"`.
pub fn completion_prefix_len(name: &str, query: &str) -> Option<usize> {
    let mut len = 0;
    let mut name_chars = name.chars();
    for qc in query.chars() {
        let nc = name_chars.next()?;
        if !nc.to_lowercase().eq(qc.to_lowercase()) {
            return None;
        }
        len += nc.len_utf8();
    }
    Some(len)
}

/// Resolve a typed path segment against folder `names` (slash-descend):
/// exact match first — case-SENSITIVE before case-insensitive, so `GitHub/`
/// picks a `GitHub` sibling over `github` — then a unique case-insensitive
/// prefix. Ambiguity resolves to `None`: the slash stays in the query.
pub fn segment_target(names: &[&str], query: &str) -> Option<usize> {
    if let Some(ix) = names.iter().position(|n| *n == query) {
        return Some(ix);
    }
    if let Some(ix) = names
        .iter()
        .position(|n| completion_prefix_len(n, query) == Some(n.len()))
    {
        return Some(ix);
    }
    let mut hits = names
        .iter()
        .enumerate()
        .filter(|(_, n)| completion_prefix_len(n, query).is_some());
    let (ix, _) = hits.next()?;
    hits.next().is_none().then_some(ix)
}

/// Interpret a palette query as a typed path jump: absolute (`/disk2/projects`)
/// or home-relative (`~`, `~/github`). Returns the absolute path to browse,
/// trailing slash trimmed. `home` is the device's resolved home — `None`
/// until the first listing lands, when `~` can't expand yet. A query like
/// `~foo` is a folder name, not a path.
pub fn typed_path_target(query: &str, home: Option<&str>) -> Option<String> {
    let query = query.trim();
    if let Some(rest) = query.strip_prefix('~') {
        let home = home?.trim_end_matches('/');
        if rest.is_empty() {
            return Some(home.to_string());
        }
        let rest = rest.strip_prefix('/')?.trim_end_matches('/');
        return Some(if rest.is_empty() {
            home.to_string()
        } else {
            format!("{home}/{rest}")
        });
    }
    if query.starts_with('/') {
        let trimmed = query.trim_end_matches('/');
        return Some(if trimmed.is_empty() {
            "/".to_string()
        } else {
            trimmed.to_string()
        });
    }
    None
}

/// Breadcrumb segments for a path: `(label, full path)`, root first.
pub fn breadcrumbs(path: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![("/".to_string(), "/".to_string())];
    let mut acc = String::new();
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        acc.push('/');
        acc.push_str(segment);
        out.push((segment.to_string(), acc.clone()));
    }
    out
}

/// Directory rows of a listing (files never render in the browser).
pub fn browser_rows(listing: &FolderListing) -> Vec<&zeron_proto::FolderEntry> {
    listing.entries.iter().filter(|e| e.is_dir).collect()
}

// ---------------------------------------------------------------------------
// Entity
// ---------------------------------------------------------------------------

/// Sentinel for "no keyboard-highlighted row" (`active`): matches no index,
/// and `usize::MAX as isize == -1` — `menu_step` treats it like `None`, so
/// the first Down lands on row 0.
const NO_ACTIVE_ROW: usize = usize::MAX;

/// Which pane the harness/model picker's icon rail is showing (t3code
/// ModelPickerContent `selectedInstanceId | "favorites"`). `Harness` means
/// "the effective harness's list" — the rail has no browse-without-commit
/// state; clicking a brand icon picks that harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ModelRail {
    Favorites,
    #[default]
    Harness,
}

/// Cache key for the flattened model-row list: any input that changes the
/// list's CONTENT (not its highlight/selection, which render per-row).
#[derive(Clone, PartialEq, Eq)]
struct ModelRowsKey {
    query: String,
    rail: ModelRail,
    effective: Option<HarnessId>,
    locked: bool,
    catalog_rev: u64,
    selected: Option<String>,
}

/// One row of the model list: the model plus the harness it belongs to —
/// search results and the favorites view mix harnesses, and every row's
/// subline names its harness (t3code ModelListRow `showProvider`).
#[derive(Debug, Clone)]
struct ModelRowData {
    harness: HarnessId,
    harness_name: SharedString,
    model: Model,
    selected_only: bool,
    /// Another listed row of the same harness shares this label, so the
    /// row shows its description to tell them apart.
    ambiguous: bool,
}

/// Flag rows whose label another listed row of the same harness shares.
fn mark_ambiguous(rows: &mut [ModelRowData]) {
    let mut labels: HashMap<(HarnessId, String), usize> = HashMap::new();
    for row in rows.iter() {
        *labels
            .entry((row.harness, row.model.label.clone()))
            .or_default() += 1;
    }
    for row in rows.iter_mut() {
        row.ambiguous = labels[&(row.harness, row.model.label.clone())] > 1;
    }
}

/// Which picker popover is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PickerKind {
    Branch,
    /// The checkout-kind dropdown in the composer footer (Current
    /// checkout/worktree | New worktree).
    Checkout,
    /// The combined agent/model/traits popover: harness tabs across the top,
    /// the tab's model list beneath the search, and the pinned traits tray
    /// (reasoning ladder + model options) at the bottom — one trigger, one
    /// card (the separate Traits popover folded in here).
    HarnessModel,
    /// New-session canvas only: which project the session mints into. A pick
    /// re-keys everything project-derived (refs, harness/model catalogs) via
    /// the state observer.
    Space,
    /// New-session canvas only: the device project-less sessions run on (a
    /// project pick implies its own host and overrides this).
    Device,
}

pub(crate) struct ReturnComposerFocus;

impl gpui::EventEmitter<ReturnComposerFocus> for Pickers {}

/// A model picked while bound to thread naming (see [`Pickers::new_for_titles`]);
/// the owner persists it with `SetTitleSettings`.
pub struct TitleModelPicked(pub TitleSettings);

impl gpui::EventEmitter<TitleModelPicked> for Pickers {}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ModelSetting {
    Reasoning,
    Option(String),
}

/// Where a model setting row lives: the tray under the model list (the
/// selected model's settings) or the hover card of a model configured in
/// place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingScope {
    Tray,
    Card,
}

/// A model whose options pick what it runs (Devin Fusion: `lead` and
/// `sidekick`, plus effort and fast mode) is configured from a card that
/// opens when its row is hovered.
fn configured_in_place(model: &Model) -> bool {
    model.options.iter().any(|o| o.id == "lead")
}

/// The card reads like Devin's own Fusion panel: Lead, Effort, Sidekick,
/// then switches.
fn card_order(mut groups: Vec<SettingGroup>) -> Vec<SettingGroup> {
    if let Some(at) = groups
        .iter()
        .position(|g| g.id == ModelSetting::Option("lead".into()))
    {
        let lead = groups.remove(at);
        groups.insert(0, lead);
    }
    for group in &mut groups {
        if group.id == ModelSetting::Reasoning {
            group.label = "Effort".into();
        }
    }
    groups
}

const CONFIG_CARD_WIDTH: f32 = 252.0;

/// A two-choice option renders as a switch in the hover card (Fast Mode):
/// on = the non-default choice.
fn is_toggle(group: &SettingGroup) -> bool {
    matches!(group.id, ModelSetting::Option(_)) && group.choices.len() == 2
}

#[derive(Clone)]
struct SettingChoice {
    label: String,
    reasoning: Option<ReasoningLevel>,
    value: String,
    selected: bool,
    default: bool,
}

#[derive(Clone)]
struct SettingGroup {
    id: ModelSetting,
    label: String,
    choices: Vec<SettingChoice>,
}

pub struct Pickers {
    state: Entity<AppState>,
    config: DraftConfig,
    /// Thread-naming binding (Settings → General): the model picker reads and
    /// emits the local device's title settings instead of a composer draft.
    /// `config.harness` then only tracks the tab being browsed.
    title: Option<TitleSettings>,
    /// Sticky last-used picks (zeron `zeron.composer.defaults:v1`): seeds the
    /// new-chat chips and is rewritten on every new-chat pick.
    defaults: ComposerDefaults,
    defaults_base: ComposerDefaults,
    /// Where [`Self::defaults`] persists (`{data_dir}/composer-defaults.json`);
    /// `None` before bootstrap stamps the state (writes are skipped).
    data_dir: Option<PathBuf>,
    /// Selection the draft picks belong to — switching chats drops them so a
    /// pick made in one chat never leaks into another.
    draft_owner: Option<String>,
    /// Space the branch draft/cache belong to (see the state observer).
    space_owner: Option<String>,
    device_owner: Option<String>,
    target_generation: u64,
    menu_geometry: HashMap<PickerKind, popover::MenuGeometry>,
    model_trigger_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    // Keep the open menu anchored while its selected label and options change.
    open_model_width: Option<gpui::Pixels>,
    open_model_height: f32,
    open: popover::Popup<PickerKind>,
    /// The harness/model picker's rail selection (favorites vs the effective
    /// harness's list). Re-primed on every open.
    model_rail: ModelRail,
    setting_menu: Option<ModelSetting>,
    setting_active: usize,
    setting_on_left: bool,
    setting_hover: popover::HoverIntent<ModelSetting>,
    setting_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    setting_scroll: gpui::ScrollHandle,
    setting_scope: SettingScope,
    /// The model row whose settings card is open (see [`configured_in_place`]).
    config_row: Option<usize>,
    config_hover: popover::HoverIntent<usize>,
    config_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    config_on_left: bool,
    /// Where a card choice list just closed after a pick: the pointer is
    /// still over it, and that must not read as leaving the card.
    config_grace: Option<gpui::Bounds<gpui::Pixels>>,
    harnesses: Loadable<Vec<HarnessDescriptor>>,
    models: HashMap<HarnessId, Loadable<Vec<Model>>>,
    model_refresh_errors: HashMap<HarnessId, String>,
    refs: Loadable<Vec<RepoRef>>,
    /// Space id the `refs` slot belongs to (invalidated on space change).
    refs_space: Option<String>,
    /// Highlighted row in the open list (keyboard nav).
    active: usize,
    /// Models-list scroll — keyboard nav keeps the highlighted row in view.
    /// A `UniformListScrollHandle`: the model list virtualizes (7k-model
    /// catalogs must scroll smoothly), and this is its handle; the plain
    /// base handle inside serves the floating scrollbar's metrics.
    model_scroll: gpui::UniformListScrollHandle,
    /// Flattened rows the list/keyboard/⌘N all walk, cached per
    /// [`ModelRowsKey`]: a 7k-model catalog rebuilt+ranked on every
    /// keystroke, arrow press AND render was the picker's open/scroll lag.
    model_rows_cache: std::cell::RefCell<Option<(ModelRowsKey, std::sync::Arc<Vec<ModelRowData>>)>>,
    /// Bumped on every catalog/favorites mutation; invalidates the cache.
    catalog_rev: u64,
    /// Hover/drag state of the floating menu scrollbar. One instance serves
    /// every picker list like `menu_scroll` does — the popups are mutually
    /// exclusive, so only one list mounts at a time.
    menu_bar: popover::MenuScrollbarState,
    /// Scroll handle shared by the plain-div picker lists (branch, project,
    /// device) — the popups are mutually exclusive, so only one mounts at a
    /// time and a fresh open resets the offset.
    menu_scroll: gpui::ScrollHandle,
    /// Shared search / URL / name input, reused across popovers.
    search: Entity<ComposerInput>,
    /// One-shot mute for the next Edited event's highlight reset — armed by
    /// [`Self::toggle`]'s programmatic clear (see the subscription).
    search_reset_muted: bool,
    focus: FocusHandle,
    /// `ZERON_OPEN_PICKER` boot: keep claiming focus until it sticks, so
    /// keyboard nav drives the data-side-opened popover (headless rigs have
    /// no synthetic pointer, but synthetic keys do arrive).
    boot_focus_pending: bool,
    /// Reclaim focus while mounting: shell recovery can replace an immediate
    /// focus request while the menu is still absent from the dispatch tree.
    focus_on_mount: bool,
    load_task: Option<Task<()>>,
    /// Own slot: the refs load runs concurrently with the eager
    /// harness/model loads — sharing `load_task` would abort one mid-flight.
    refs_task: Option<Task<()>>,
    /// In-flight mid-session `SwitchRef` (the ref being switched to).
    switching: Option<String>,
    switch_task: Option<Task<()>>,
    /// Last mid-session switch failure (shown in the ref popover).
    switch_error: Option<String>,
    mutate_task: Option<Task<()>>,
    _search_events: Subscription,
    _state_observe: Subscription,
    _catalog_observe: Subscription,
}

impl Pickers {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self::build(state, None, cx)
    }

    /// The model picker bound to thread naming: only title-capable agents,
    /// no traits tray, and picks surface as [`TitleModelPicked`].
    pub fn new_for_titles(
        state: Entity<AppState>,
        settings: TitleSettings,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(state, Some(settings), cx)
    }

    pub fn set_title_settings(&mut self, settings: TitleSettings, cx: &mut Context<Self>) {
        self.config.harness = None;
        self.title = Some(settings);
        self.catalog_rev += 1;
        cx.notify();
    }

    fn build(
        state: Entity<AppState>,
        title: Option<TitleSettings>,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            ComposerInput::with_context("Search…", "PaletteSearch", cx)
                .with_accessibility_role(gpui::Role::SearchInput)
        });
        let search_events = cx.subscribe(&search, |this: &mut Self, _, event, cx| match event {
            ComposerInputEvent::Edited => {
                // Typing in a filter resets the highlight to the top of the
                // fresh results. `set_text` emits Edited on programmatic
                // clears too, and this subscription runs AFTER `toggle`
                // returns — an unmuted reset clobbers the just-anchored
                // selected row back to 0, leaving the top row wearing a
                // second highlight next to the selection (user report;
                // `toggle` arms the mute right before its clear).
                if !std::mem::take(&mut this.search_reset_muted) {
                    if matches!(
                        this.open_kind(),
                        Some(PickerKind::Branch | PickerKind::Space | PickerKind::Device)
                    ) {
                        this.active = 0;
                    }
                    if this.open_kind() == Some(PickerKind::HarnessModel) {
                        this.setting_menu = None;
                        this.setting_bounds = None;
                        this.active = 0;
                        this.model_scroll_base().set_offset(gpui::Point::default());
                    }
                }
                cx.notify();
            }
            ComposerInputEvent::Submitted | ComposerInputEvent::ModifiedSubmitted => {
                this.on_search_submit(cx)
            }
            // Pasted images/files don't apply to a search box.
            ComposerInputEvent::PastedImages(_)
            | ComposerInputEvent::PastedPaths(_)
            | ComposerInputEvent::PastedText { .. }
            | ComposerInputEvent::CursorMoved
            | ComposerInputEvent::ViewportChanged
            | ComposerInputEvent::MentionNavigate(_)
            | ComposerInputEvent::MentionAccept
            | ComposerInputEvent::MentionDismiss => {}
        });
        // Chat selection / config changes must re-render the chips (child views
        // only re-render on their own notify). A selection change also drops
        // the draft picks — they belonged to the previous chat/new-chat canvas.
        let state_observe = cx.observe(&state, |this: &mut Self, state, cx| {
            let selected = state.read(cx).selected_chat.clone();
            if selected != this.draft_owner {
                this.draft_owner = selected;
                this.config.harness = None;
                this.config.model = None;
                this.config.reasoning = None;
                this.switch_error = None;
            }
            // A space switch invalidates the branch draft + cache — the folder
            // (and possibly the device) changed under them.
            let space = state.read(cx).selected_space.clone();
            let device = state.read(cx).effective_device_id();
            if space != this.space_owner || device != this.device_owner {
                this.space_owner = space;
                this.device_owner = device;
                this.target_generation = this.target_generation.wrapping_add(1);
                this.setting_menu = None;
                this.setting_bounds = None;
                this.refs_task = None;
                this.load_task = None;
                this.config.branch = None;
                this.config.checkout = CheckoutKind::default();
                this.refs = Loadable::Idle;
                this.refs_space = None;
                // Catalogs are per-DEVICE (fetched from the space's host):
                // a space switch may land on another device, so refetch.
                this.harnesses = Loadable::Idle;
                this.models.clear();
                this.model_refresh_errors.clear();
                this.catalog_rev += 1;
            }
            cx.notify();
        });
        // A Settings → Providers toggle changed some device's enabled set:
        // force-refresh the cached catalog so the rail/chips follow without a
        // restart (stale rows stay visible while the reload runs).
        let catalog_observe = cx.observe_global::<HarnessCatalogChanged>(|this: &mut Self, cx| {
            this.ensure_harnesses(true, cx);
            cx.notify();
        });
        // Dev/testing knob: `ZERON_OPEN_PICKER=model|traits|repo|branch` boots
        // with that popover open — synthetic input can't reach the app on
        // headless compositors, so captures need a data-side path.
        let boot_open = match std::env::var("ZERON_OPEN_PICKER").ok().as_deref() {
            _ if title.is_some() => None,
            Some("model") => Some(PickerKind::HarnessModel),
            Some("traits") => Some(PickerKind::HarnessModel),
            Some("branch") => Some(PickerKind::Branch),
            Some("checkout") => Some(PickerKind::Checkout),
            Some("project") => Some(PickerKind::Space),
            Some("device") => Some(PickerKind::Device),
            _ => None,
        };
        let mut open = popover::Popup::default();
        if let Some(kind) = boot_open {
            open.open(kind);
        }
        // Sticky last-used picks: loaded synchronously so the very first frame
        // shows the remembered harness/model/reasoning, never a placeholder.
        let data_dir = state.read(cx).data_dir.clone();
        let defaults = data_dir
            .as_deref()
            .map(ComposerDefaults::load)
            .unwrap_or_default();
        // Restore explicit opt-outs as well as project picks before the first frame.
        if title.is_none() {
            state.update(cx, |s, _| s.restore_composer_target(&defaults));
        }
        let draft_owner = state.read(cx).selected_chat.clone();
        let space_owner = state.read(cx).selected_space.clone();
        let device_owner = state.read(cx).effective_device_id();
        Self {
            state,
            space_owner,
            device_owner,
            target_generation: 0,
            menu_geometry: HashMap::new(),
            model_trigger_bounds: None,
            open_model_width: None,
            open_model_height: model_menu_height(0),
            config: DraftConfig::default(),
            defaults_base: defaults.clone(),
            title,
            defaults,
            data_dir,
            draft_owner,
            open,
            model_rail: ModelRail::default(),
            setting_menu: None,
            setting_active: 0,
            setting_on_left: false,
            setting_hover: popover::HoverIntent::default(),
            setting_bounds: None,
            setting_scroll: gpui::ScrollHandle::new(),
            setting_scope: SettingScope::Tray,
            config_row: None,
            config_hover: popover::HoverIntent::default(),
            config_bounds: None,
            config_on_left: false,
            config_grace: None,
            harnesses: Loadable::Idle,
            models: HashMap::new(),
            model_refresh_errors: HashMap::new(),
            refs: Loadable::Idle,
            refs_space: None,
            active: 0,
            model_scroll: gpui::UniformListScrollHandle::new(),
            model_rows_cache: std::cell::RefCell::new(None),
            catalog_rev: 0,
            menu_bar: popover::MenuScrollbarState::default(),
            menu_scroll: gpui::ScrollHandle::new(),
            search,
            search_reset_muted: false,
            focus: cx.focus_handle(),
            boot_focus_pending: boot_open.is_some(),
            focus_on_mount: false,
            load_task: None,
            refs_task: None,
            switching: None,
            switch_task: None,
            switch_error: None,
            mutate_task: None,
            _search_events: search_events,
            _state_observe: state_observe,
            _catalog_observe: catalog_observe,
        }
    }

    /// Persist the sticky defaults (best-effort; picks are rare and tiny).
    fn save_defaults(&mut self) {
        // The title picker never writes run picks into the composer's memory.
        if self.title.is_some() {
            return;
        }
        self.persist_defaults();
    }

    /// Re-read the shared memory: the composer and the thread-naming picker
    /// each hold a copy, and both write favorites.
    fn reload_defaults(&mut self) {
        if let Some(dir) = self.data_dir.as_deref() {
            self.defaults = ComposerDefaults::load(dir);
            self.defaults_base = self.defaults.clone();
            self.catalog_rev += 1;
        }
    }

    fn persist_defaults(&mut self) {
        if let Some(dir) = self.data_dir.as_deref() {
            let mut latest =
                serde_json::to_value(ComposerDefaults::load(dir)).expect("defaults serialize");
            let previous = serde_json::to_value(&self.defaults_base).expect("defaults serialize");
            let next = serde_json::to_value(&self.defaults).expect("defaults serialize");
            crate::settings::windows::merge_delta(&mut latest, &previous, &next);
            if let Ok(merged) = serde_json::from_value::<ComposerDefaults>(latest) {
                if let Err(err) = merged.save(dir) {
                    tracing::warn!(error = %err, "composer-defaults save failed");
                } else {
                    self.defaults = merged;
                    self.defaults_base = self.defaults.clone();
                }
            }
        }
    }

    pub fn draft(&self) -> &DraftConfig {
        &self.config
    }

    /// Harness is locked once the chat exists (feature-inventory §1.7).
    fn harness_locked(&self, cx: &App) -> bool {
        self.title.is_none() && self.state.read(cx).selected_chat.is_some()
    }

    /// The harnesses this picker offers: runnable ones for the composer,
    /// narrowed to title-capable agents when bound to thread naming.
    fn offered(&self, list: &[HarnessDescriptor]) -> Vec<HarnessDescriptor> {
        let mut offered = offered_harnesses(list);
        if self.title.is_some() {
            offered.retain(|d| zeron_harness::supports_titles(d.id) && d.id != HarnessId::Mock);
        }
        offered
    }

    fn engine(&self, cx: &App) -> Option<EngineHandle> {
        self.state.read(cx).engine().cloned()
    }

    /// The selected target device when it differs from the connected
    /// engine's own — harness/model catalogs come from the device that RUNS
    /// the agents (the CLIs live there; the viewer may have neither claude
    /// nor codex installed — user report: "can't load codex models/traits
    /// anywhere" from a Mac without codex).
    fn space_target(&self, cx: &App) -> Option<String> {
        // Thread naming is this device's setting; so are its catalogs.
        if self.title.is_some() {
            return None;
        }
        let state = self.state.read(cx);
        let device = state.effective_device_id()?;
        (state.local_device_id.as_deref() != Some(device.as_str())).then_some(device)
    }

    /// Effective harness: picked, or the chat's config, or the first listed.
    fn effective_harness(&self, cx: &App) -> Option<HarnessId> {
        if let Some(harness) = self.config.harness {
            return Some(harness);
        }
        if let Some(title) = &self.title {
            // Browsing starts on the saved agent, else the first title-capable one.
            return title.harness.or_else(|| {
                self.harnesses
                    .ready()
                    .and_then(|list| self.offered(list).first().map(|d| d.id))
            });
        }
        if let Some(config) = self
            .state
            .read(cx)
            .selected_chat_row()
            .and_then(|c| c.config.as_ref())
        {
            return Some(config.harness);
        }
        // New-chat canvas: the remembered last-used harness (sticky defaults),
        // when the loaded catalog still offers it (the device may have
        // disabled it in Settings → Providers since).
        if let Some(harness) = self.defaults.harness {
            let offered = match self.harnesses.ready() {
                Some(list) => offered_harnesses(list).iter().any(|d| d.id == harness),
                None => true, // catalog not loaded yet — trust the memory
            };
            if offered {
                return Some(harness);
            }
        }
        // Fall back to the first OFFERED harness: the registry lists the mock
        // harness first, and resolving chips against it would boot the
        // new-chat canvas onto "Mock" instead of Claude Code + its default
        // model (it stays available under `ZERON_HARNESS=mock`).
        self.harnesses
            .ready()
            .and_then(|list| offered_harnesses(list).first().map(|d| d.id))
    }

    /// Effective model id: the draft pick, the selected chat's config, or (on
    /// the new-chat canvas) the remembered last-used model for the harness.
    fn effective_model_id<'a>(&'a self, cx: &'a App) -> Option<&'a str> {
        if let Some(title) = &self.title {
            // Only the saved agent's tab shows a selected row.
            return title.model.as_deref().filter(|_| {
                title.harness.is_some() && title.harness == self.effective_harness(cx)
            });
        }
        if let Some(id) = self.config.model.as_deref() {
            return Some(id);
        }
        if let Some(chat) = self.state.read(cx).selected_chat_row() {
            return chat.config.as_ref().and_then(|c| c.model.as_deref());
        }
        let harness = self.effective_harness(cx)?;
        self.defaults.model_for(harness).map(|m| m.id.as_str())
    }

    /// Effective reasoning — always concrete once the model is known: the
    /// draft pick / chat config / remembered default, clamped to the selected
    /// model's ladder, falling back to the model's default level.
    fn effective_reasoning(&self, cx: &App) -> Option<ReasoningLevel> {
        let explicit = self.config.reasoning.or_else(|| {
            match self.state.read(cx).selected_chat_row() {
                Some(chat) => chat.config.as_ref().and_then(|c| c.reasoning),
                // New chat: the remembered last-used level.
                None => self.defaults.reasoning,
            }
        });
        if self.selected_model(cx).is_none() {
            // Catalog not loaded yet: show the explicit value as-is (nothing
            // to clamp against); it resolves to a concrete level on load.
            return explicit;
        }
        clamp_reasoning(explicit, &self.trait_ladder(cx))
    }

    /// Only an implicit selection follows the harness default. An explicit ID
    /// absent from the live catalog keeps its identity and remembered chip label.
    fn selected_model<'a>(&'a self, cx: &'a App) -> Option<&'a Model> {
        let harness = self.effective_harness(cx)?;
        let models = self.models.get(&harness)?.ready()?;
        let selected = self.effective_model_id(cx);
        // An unset title model means "cheapest", not the catalog default.
        if self.title.is_some() && selected.is_none() {
            return None;
        }
        selected_catalog_model(models, selected)
    }

    fn selected_model_label(&self, cx: &App) -> Option<String> {
        self.selected_model(cx)
            .map(|model| model.label.clone())
            .or_else(|| {
                let remembered = self
                    .effective_harness(cx)
                    .and_then(|h| self.defaults.model_for(h));
                match self.effective_model_id(cx) {
                    Some(id) => Some(
                        remembered
                            .filter(|m| m.id == id)
                            .map(|m| m.label.clone())
                            .or_else(|| self.defaults.label_for(id).map(str::to_owned))
                            .unwrap_or_else(|| id.to_owned()),
                    ),
                    None => remembered.map(|m| m.label.clone()),
                }
            })
    }

    /// The explicit (non-default) option picks: the chat's persisted
    /// selections for existing chats, the remembered picks for the model the
    /// new-chat canvas resolves to (same id [`Self::resolved`] sends).
    fn explicit_options(&self, cx: &App) -> serde_json::Map<String, serde_json::Value> {
        if let Some(chat) = self.state.read(cx).selected_chat_row() {
            return chat
                .config
                .as_ref()
                .map(|c| c.model_options.clone())
                .unwrap_or_default();
        }
        let Some(harness) = self.effective_harness(cx) else {
            return Default::default();
        };
        match self.selected_model(cx) {
            Some(model) => offered_options(
                model,
                self.defaults
                    .model_options_for(harness, &model.id)
                    .cloned()
                    .unwrap_or_default(),
            ),
            // Catalog not loaded (or failed): the picks were validated for
            // this exact model when made, so they are safe to send as-is.
            None => self
                .effective_model_id(cx)
                .and_then(|id| self.defaults.model_options_for(harness, id))
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// The catalog is loaded and offers nothing runnable — the no-agents
    /// state (every enabled harness is missing its CLI, or nothing is
    /// enabled). False while the catalog is still loading or failed
    /// (nothing to conclude yet; offline sends must not be blocked on it).
    pub fn no_agents_available(&self) -> bool {
        self.harnesses
            .ready()
            .is_some_and(|list| self.offered(list).is_empty())
    }

    pub(crate) fn steers_mid_turn(&self, cx: &App) -> bool {
        self.harnesses
            .ready()
            .and_then(|list| {
                let selected = self.effective_harness(cx)?;
                list.iter().find(|h| h.id == selected)
            })
            .is_some_and(|h| {
                h.supports_steering && h.steering_mode == zeron_proto::SteeringMode::StepBoundary
            })
    }

    /// The fully-resolved config the composer threads into the Run request and
    /// `Mutate createChat`: concrete model + reasoning whenever the catalog is
    /// loaded (no "engine picks a default" passthrough).
    pub fn resolved(&self, cx: &App) -> ResolvedRunConfig {
        ResolvedRunConfig {
            harness: self.effective_harness(cx),
            model: self
                .selected_model(cx)
                .map(|m| m.id.clone())
                // Catalog not loaded (offline): still send the id we know.
                .or_else(|| self.effective_model_id(cx).map(str::to_string)),
            reasoning: self.effective_reasoning(cx),
            model_options: self.explicit_options(cx),
        }
    }

    // ---- open/close ----

    /// The picker that's open AND interactive — `None` while one animates out.
    fn open_kind(&self) -> Option<PickerKind> {
        self.open.as_open().copied()
    }

    /// Whether any picker popover is open (shell-side: session-nav shortcuts
    /// go quiet underneath an open popover instead of yanking the session out
    /// from under it).
    pub fn is_open(&self) -> bool {
        self.open.as_open().is_some()
    }

    /// The picker to render: open or mid-exit.
    fn mounted_kind(&self) -> Option<PickerKind> {
        self.open.get().copied()
    }

    /// Begin the exit animation (shared by every close path).
    fn animate_close(&mut self, cx: &mut Context<Self>) {
        if self.is_open() {
            cx.emit(ReturnComposerFocus);
        }
        self.dismiss(cx);
    }

    /// Outside clicks and navigation keep focus at the clicked destination.
    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.focus_on_mount = false;
        self.cancel_setting_hover();
        self.setting_menu = None;
        self.setting_bounds = None;
        self.close_config();
        self.menu_bar = popover::MenuScrollbarState::default();
        if self.open.begin_close() {
            popover::reap_popup(cx, |pickers: &mut Self| &mut pickers.open);
        }
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.animate_close(cx);
        cx.notify();
    }

    /// Capture knob (`ZERON_OPEN_DIALOG=model`): open the combined
    /// harness/model menu programmatically.
    /// A jump-slot press while the model menu is open. The shell's session
    /// bindings (Mod+1…9) win the dispatch race — gpui runs a matched
    /// binding before any key handler — so the shell forwards the slot here
    /// instead of going quiet and eating the very chips the rows advertise
    /// (macOS field report: "cmd shortcuts do nothing in the model
    /// selector"). Returns whether the menu was open and the slot consumed.
    pub fn jump_model_slot(&mut self, slot: usize, cx: &mut Context<Self>) -> bool {
        if self.open_kind() != Some(PickerKind::HarnessModel) {
            return false;
        }
        self.activate_model_index(slot, cx);
        cx.notify();
        true
    }

    pub fn open_model_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_kind() != Some(PickerKind::HarnessModel) {
            self.toggle(PickerKind::HarnessModel, window, cx);
        }
    }

    fn toggle(&mut self, kind: PickerKind, window: &mut Window, cx: &mut Context<Self>) {
        // A press that found this picker open closes it — the card's
        // `on_mouse_down_out` already began the close on that same press,
        // so by click time the popup reads as closed and a plain toggle
        // would reopen it. A press while a DIFFERENT picker is open doesn't
        // count (see note_trigger_press_matching): that click switches.
        let pressed_open = self.open.take_press_was_open();
        if self.open_kind() == Some(kind) || pressed_open {
            self.animate_close(cx);
            if pressed_open {
                cx.emit(ReturnComposerFocus);
            }
            cx.notify();
            return;
        }
        if kind == PickerKind::HarnessModel {
            self.open_model_width = self.model_trigger_bounds.map(|bounds| bounds.size.width);
            self.open_model_height = model_menu_height(self.setting_groups(cx).len());
        }
        self.open.open(kind);
        self.focus_on_mount = true;
        // The plain-div menus (branch / project / device) share one scroll
        // handle; a fresh open starts at the top. The model list resets its
        // own virtualized handle below. Sync the rail baselines so the jump
        // back to the top isn't read as scrolling.
        if kind != PickerKind::HarnessModel {
            popover::reset_menu_scroll(&self.menu_scroll, &mut self.menu_bar);
        }
        // Clearing stale text emits Edited AFTER this function returns —
        // mute that one event so its reset can't clobber the highlight
        // anchored below (the no-op clear is also skipped for the same
        // reason).
        self.search_reset_muted = !self.search.read(cx).text().is_empty();
        self.search.update(cx, |input, cx| {
            input.set_placeholder("Search…", cx);
            if !input.text().is_empty() {
                input.set_text("", cx);
            }
        });
        // Prime the model picker's rail BEFORE anchoring the highlight (the
        // visible rows depend on it): the favorites view when stars exist —
        // t3 ModelPickerContent's initial selection — else the effective
        // harness. Locked chats stay on their own harness.
        if kind == PickerKind::HarnessModel {
            // Favorites may have been starred from the other picker.
            self.reload_defaults();
        }
        if kind == PickerKind::HarnessModel && self.title.is_some() {
            // Each open browses from the saved agent's tab.
            self.config.harness = None;
            self.model_rail = ModelRail::Harness;
        } else if kind == PickerKind::HarnessModel {
            self.model_rail = if !self.harness_locked(cx) && !self.defaults.favorites.is_empty() {
                ModelRail::Favorites
            } else {
                ModelRail::Harness
            };
        }
        // The keyboard-nav highlight starts ON the selected row — row 0
        // otherwise reads as a second active row (user report).
        self.active = match kind {
            PickerKind::Checkout => match self.config.checkout {
                CheckoutKind::Local => 0,
                CheckoutKind::NewWorktree => 1,
            },
            PickerKind::Branch => self.selected_ref_index(cx),
            PickerKind::HarnessModel => self.selected_model_index(cx),
            PickerKind::Space => self.selected_space_index(cx),
            PickerKind::Device => self.selected_device_index(cx),
        };
        if kind == PickerKind::HarnessModel {
            // scroll_to_item below may land anywhere; the first note of the
            // settled position is the fresh baseline, not scroll motion.
            popover::reset_menu_scroll(&self.model_scroll_base(), &mut self.menu_bar);
            self.model_scroll
                .scroll_to_item(self.active, gpui::ScrollStrategy::Nearest);
        }
        // Searchable pickers focus the filter input (it sits inside the frame,
        // so the frame's key handler still sees arrows/Enter); the rest focus
        // the frame itself for pure keyboard nav.
        match kind {
            PickerKind::Branch => {
                self.switch_error = None; // stale mid-session failures don't linger
                let handle = self.search.read(cx).focus_handle(cx);
                self.search.update(cx, |input, cx| {
                    input.set_placeholder("Search refs…", cx);
                });
                window.focus(&handle, cx);
            }
            PickerKind::Space => {
                let handle = self.search.read(cx).focus_handle(cx);
                self.search.update(cx, |input, cx| {
                    input.set_placeholder("Search projects…", cx);
                });
                window.focus(&handle, cx);
            }
            PickerKind::Device => {
                let handle = self.search.read(cx).focus_handle(cx);
                self.search.update(cx, |input, cx| {
                    input.set_placeholder("Search devices…", cx);
                });
                window.focus(&handle, cx);
            }
            PickerKind::HarnessModel => {
                let handle = self.search.read(cx).focus_handle(cx);
                self.search.update(cx, |input, cx| {
                    input.set_placeholder("Search models…", cx);
                });
                window.focus(&handle, cx);
            }
            _ => window.focus(&self.focus, cx),
        }
        match kind {
            // Force: the checkout state moves under us (a send mints a
            // worktree+branch, terminals switch refs) — every open
            // revalidates, keeping stale rows visible until fresh ones land.
            PickerKind::Branch | PickerKind::Checkout => self.ensure_refs(true, cx),
            PickerKind::HarnessModel => {
                // Force: the enabled set moves under us (Settings → Providers,
                // possibly from another viewer) — every open revalidates,
                // keeping current rows visible until the fresh catalog lands.
                self.ensure_harnesses(true, cx);
                // Model discovery can recover after a slow/plugin-heavy ACP
                // cold start. Revalidate on every open instead of pinning a
                // timeout/fallback result until the application restarts.
                self.prefetch_models(true, cx);
            }
            // Projects and devices are already synced state — nothing to load.
            PickerKind::Space | PickerKind::Device => {}
        }
        cx.notify();
    }

    // ---- loads ----

    fn ensure_harnesses(&mut self, force: bool, cx: &mut Context<Self>) {
        // Non-forced (the render loop's eager kick) only loads from Idle: an
        // Error that could re-trigger a load would flip back to Loading
        // before the retry row ever painted (and spam the engine); Retry
        // resets to Idle. FORCED refreshes (a Settings → Providers toggle, a
        // picker open) reload through Ready/Error too — the enabled set just
        // changed under the cache, which otherwise served the boot-time
        // catalog until restart (user report). Stale-while-revalidate: loaded
        // rows stay on screen while the fresh catalog lands.
        let reload = match self.harnesses {
            Loadable::Idle => true,
            Loadable::Loading => false,
            Loadable::Ready(_) | Loadable::Error(_) => force,
        };
        if !reload {
            return;
        }
        let Some(engine) = self.engine(cx) else {
            return;
        };
        let target = self.space_target(cx);
        let generation = self.target_generation;
        if !matches!(self.harnesses, Loadable::Ready(_)) {
            self.harnesses = Loadable::Loading;
            self.catalog_rev += 1;
        }
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let mut params = serde_json::Map::new();
            if let Some(target) = &target {
                params.insert(
                    "targetDeviceId".into(),
                    serde_json::Value::String(target.clone()),
                );
            }
            let result = engine
                .client()
                .call(methods::LIST_HARNESSES, serde_json::Value::Object(params))
                .await;
            if let Some(delay) = slow_catalog_delay() {
                cx.background_executor().timer(delay).await;
            }
            this.update(cx, |pickers, cx| {
                if pickers.target_generation != generation {
                    return;
                }
                pickers.catalog_rev += 1;
                pickers.harnesses = match result {
                    Ok(value) => match serde_json::from_value::<Vec<HarnessDescriptor>>(value) {
                        Ok(list) => Loadable::Ready(list),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                pickers.prefetch_models(false, cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Kick a model load for the effective harness AND every offered one, in
    /// parallel — by the time the user opens the picker (or switches rail
    /// tabs) the lists are already there, instead of a per-selection
    /// "Loading models…" round-trip. Each `ensure_models` call is guarded by
    /// its slot state, so re-running this every catalog load/render is free.
    fn prefetch_models(&mut self, force: bool, cx: &mut Context<Self>) {
        let mut targets: Vec<HarnessId> = match self.harnesses.ready() {
            Some(list) => self.offered(list).iter().map(|d| d.id).collect(),
            None => Vec::new(),
        };
        // The committed chat's harness may be outside the offered set (e.g.
        // disabled after the chat was created) — its models still matter.
        if let Some(effective) = self.effective_harness(cx)
            && !targets.contains(&effective)
        {
            targets.push(effective);
        }
        for harness in targets {
            self.ensure_models(harness, force, cx);
        }
    }

    fn ensure_models(&mut self, harness: HarnessId, force: bool, cx: &mut Context<Self>) {
        // Normal prefetches load absent/Idle slots once. Picker-open refreshes
        // also retry Ready/Error slots, while an in-flight load is always
        // reused. Ready rows stay visible until the replacement lands.
        let reload = match self.models.get(&harness) {
            None | Some(Loadable::Idle) => true,
            Some(Loadable::Loading) => false,
            Some(Loadable::Ready(_)) | Some(Loadable::Error(_)) => force,
        };
        if !reload {
            return;
        }
        let Some(engine) = self.engine(cx) else {
            return;
        };
        let target = self.space_target(cx);
        let generation = self.target_generation;
        if !matches!(self.models.get(&harness), Some(Loadable::Ready(_))) {
            self.models.insert(harness, Loadable::Loading);
            self.catalog_rev += 1;
        }
        cx.spawn(async move |this, cx| {
            let mut params = serde_json::json!({ "harness": harness, "force": force });
            if let (Some(target), Some(object)) = (&target, params.as_object_mut()) {
                object.insert(
                    "targetDeviceId".into(),
                    serde_json::Value::String(target.clone()),
                );
            }
            // A plugin-heavy OpenCode cold start can fail once while caches,
            // MCP servers, or plugin runtimes are still warming. Keep this
            // single Loading slot alive for two retries so recovery requires
            // no picker close/reopen and cannot launch duplicate probes.
            let mut attempt = 1_u64;
            let result = loop {
                let result = engine
                    .client()
                    .call(methods::LIST_MODELS, params.clone())
                    .await;
                if result.is_ok() || harness != HarnessId::Opencode || attempt >= 3 {
                    break result;
                }
                if let Err(error) = &result {
                    tracing::warn!(
                        %error,
                        attempt,
                        "OpenCode model discovery failed; retrying automatically"
                    );
                }
                if this.update(cx, |_, _| {}).is_err() {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(attempt * 2))
                    .await;
                attempt += 1;
            };
            if let Some(delay) = slow_catalog_delay() {
                cx.background_executor().timer(delay).await;
            }
            this.update(cx, |pickers, cx| {
                if pickers.target_generation != generation {
                    return;
                }
                let loaded = match result {
                    Ok(value) => match serde_json::from_value::<Vec<Model>>(value) {
                        // Display hygiene for catalogs from older engines
                        // (`default` alias rows, orphan `[1m]` variants,
                        // version-less alias labels).
                        Ok(models) => Loadable::Ready(normalize_model_rows(harness, models)),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                pickers.apply_model_catalog(harness, loaded, cx);
            })
            .ok();
        })
        .detach();
    }

    fn apply_model_catalog(
        &mut self,
        harness: HarnessId,
        loaded: Loadable<Vec<Model>>,
        cx: &mut Context<Self>,
    ) {
        if let Loadable::Error(error) = &loaded {
            self.model_refresh_errors.insert(harness, error.clone());
        } else {
            self.model_refresh_errors.remove(&harness);
        }
        if let Loadable::Error(error) = &loaded
            && matches!(self.models.get(&harness), Some(Loadable::Ready(_)))
        {
            tracing::warn!(%error, ?harness, "Model refresh failed; retaining visible rows");
            cx.notify();
            return;
        }
        if let Loadable::Ready(models) = &loaded {
            let fresh = self
                .defaults
                .remember_labels(models.iter().map(|m| (m.id.as_str(), m.label.as_str())));
            if fresh {
                self.save_defaults();
            }
        }
        self.models.insert(harness, loaded);
        self.catalog_rev += 1;
        // A list that landed while its popover is open re-anchors the
        // keyboard highlight onto the selected row (it sat at 0 while
        // loading).
        if self.open_kind() == Some(PickerKind::HarnessModel)
            && self.effective_harness(cx) == Some(harness)
        {
            self.active = self.selected_model_index(cx);
        }
        cx.notify();
    }

    /// ListRefs for the selected SPACE's folder — targeted at the space's
    /// device (relay-forwarded when remote), keyed/invalidated by space id.
    /// Rows carry checkout state (`current`, `worktreePath`) so the picker can
    /// tag refs and the checkout-kind selector can offer worktree reuse.
    fn ensure_refs(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(space) = self.state.read(cx).selected_space_row().cloned() else {
            return;
        };
        if !space.git_detected {
            return;
        }
        let fresh = self.refs_space.as_deref() == Some(space.id.as_str());
        if fresh && matches!(self.refs, Loadable::Loading) {
            return; // a load is already in flight
        }
        // Non-forced (the footer's eager kick, re-run every render) only loads
        // from Idle: an Error must WAIT for an explicit retry/reopen (force),
        // or re-render would flip Error back to Loading before the retry row
        // ever paints — an eternal skeleton plus an RPC storm (user report:
        // "the ref dropdown never loads anything").
        if !force && fresh && !matches!(self.refs, Loadable::Idle) {
            return;
        }
        let Some(engine) = self.engine(cx) else {
            return;
        };
        let local = self.state.read(cx).local_device_id.clone();
        // Stale-while-revalidate: a forced refresh of an already-loaded space
        // keeps the current rows on screen while the reload runs — a send that
        // just minted a worktree (or a terminal-side branch) appears on the
        // popover's next open without the list ever flashing to a skeleton.
        if !(force && fresh && matches!(self.refs, Loadable::Ready(_))) {
            self.refs = Loadable::Loading;
        }
        self.refs_space = Some(space.id.clone());
        self.refs_task = Some(cx.spawn(async move |this, cx| {
            let mut params = serde_json::Map::new();
            params.insert(
                "repoPath".into(),
                serde_json::Value::String(space.path.clone()),
            );
            if local.as_deref() != Some(space.device_id.as_str()) {
                params.insert(
                    "targetDeviceId".into(),
                    serde_json::Value::String(space.device_id.clone()),
                );
            }
            let result = engine
                .client()
                .call(methods::LIST_REFS, serde_json::Value::Object(params))
                .await;
            this.update(cx, |pickers, cx| {
                pickers.refs = match result {
                    Ok(value) => match serde_json::from_value::<Vec<RepoRef>>(value) {
                        Ok(refs) => Loadable::Ready(refs),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                // Rows landed under an open, un-searched popover: re-home the
                // nav highlight to the selected row.
                if pickers.open_kind() == Some(PickerKind::Branch)
                    && pickers.search.read(cx).text().is_empty()
                {
                    pickers.active = pickers.selected_ref_index(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    // ---- selections ----

    fn pick_ref(&mut self, row: RepoRef, cx: &mut Context<Self>) {
        // Refs are fixed at creation: an existing session can never move
        // (wing's rule — the footer renders read-only labels there, so this
        // is a belt-and-braces guard).
        if self.state.read(cx).selected_chat_row().is_some() {
            return;
        }
        if row.worktree_path.is_some() {
            // Reuse the ref's existing worktree ("Current worktree") — the
            // t3code `reuseExistingWorktree` path.
            self.config.branch = Some(row.name.clone());
            self.config.checkout = CheckoutKind::Local;
        } else if self.config.checkout == CheckoutKind::NewWorktree || row.current {
            // Base pick for a new worktree, or the already-current ref.
            self.config.branch = Some(row.name.clone());
        } else {
            // Local mode + a plain non-current ref: CHECK OUT the space
            // folder (full t3code `switchRef` — picking `main` means "put my
            // local checkout on main", it must never flip the mode).
            self.switch_draft_ref(row, cx);
            return;
        }
        self.animate_close(cx);
        cx.notify();
    }

    /// Draft-mode checkout switch: `git checkout` in the SPACE's folder
    /// (relay-forwarded for remote spaces). Success records the pick and
    /// refreshes tags; failure keeps the popover open with git's message.
    fn switch_draft_ref(&mut self, row: RepoRef, cx: &mut Context<Self>) {
        if self.switching.is_some() {
            return; // one switch at a time
        }
        let Some(space) = self.state.read(cx).selected_space_row().cloned() else {
            return;
        };
        let Some(engine) = self.engine(cx) else {
            return;
        };
        let local = self.state.read(cx).local_device_id.clone();
        self.switch_error = None;
        self.switching = Some(row.name.clone());
        let ref_name = row.name.clone();
        self.switch_task = Some(cx.spawn(async move |this, cx| {
            let mut params = serde_json::Map::new();
            params.insert(
                "repoPath".into(),
                serde_json::Value::String(space.path.clone()),
            );
            params.insert(
                "refName".into(),
                serde_json::Value::String(ref_name.clone()),
            );
            if local.as_deref() != Some(space.device_id.as_str()) {
                params.insert(
                    "targetDeviceId".into(),
                    serde_json::Value::String(space.device_id.clone()),
                );
            }
            let result = engine
                .client()
                .call(methods::SWITCH_REF, serde_json::Value::Object(params))
                .await;
            this.update(cx, |pickers, cx| {
                pickers.switching = None;
                match result {
                    Ok(_) => {
                        pickers.config.branch = Some(ref_name);
                        pickers.animate_close(cx);
                        pickers.ensure_refs(true, cx);
                    }
                    Err(err) => pickers.switch_error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn pick_checkout(&mut self, kind: CheckoutKind, cx: &mut Context<Self>) {
        if kind == CheckoutKind::Local
            && self.config.checkout == CheckoutKind::NewWorktree
            && self.selected_ref_worktree().is_none()
            && self.selected_ref().is_some_and(|r| !r.current)
        {
            // Back to "Current checkout" with a non-current plain ref picked:
            // drop the pick (we don't checkout the main folder) — the current
            // branch takes over.
            self.config.branch = None;
        }
        self.config.checkout = kind;
        self.animate_close(cx);
        cx.notify();
    }

    fn pick_harness(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        if self.harness_locked(cx) {
            return;
        }
        if self.config.harness != Some(harness) {
            // The remembered model for this harness takes over via the
            // defaults fallback; a foreign pick must not linger.
            self.config.model = None;
            self.config.reasoning = None;
        }
        self.config.harness = Some(harness);
        if self.title.is_none() {
            self.defaults.harness = Some(harness);
            self.save_defaults();
        }
        self.model_scroll_base().set_offset(gpui::Point::default());
        self.ensure_models(harness, false, cx);
        // Re-anchor the keyboard highlight onto the new harness's selected row.
        self.active = self.selected_model_index(cx);
        cx.notify();
    }

    fn pick_model(&mut self, model_id: String, cx: &mut Context<Self>) {
        self.setting_menu = None;
        if self.title.is_some() {
            let settings = TitleSettings {
                harness: self.effective_harness(cx),
                model: Some(model_id),
            };
            self.title = Some(settings.clone());
            self.catalog_rev += 1;
            cx.emit(TitleModelPicked(settings));
            self.close(cx);
            return;
        }
        // The card stays open on a pick (user request): model and traits
        // share one popover now, and adjusting the tray right after choosing
        // a model is the expected flow. Esc, click-out, or the chip close it.
        if self.state.read(cx).selected_chat.is_some() {
            // Existing chat: persist to the chat row (Mutate setChatConfig) —
            // survives restarts and syncs; next runs in this chat use it.
            self.update_chat_config(cx, move |config| config.model = Some(model_id));
        } else {
            // New chat: draft pick + sticky last-used memory for this harness.
            self.config.model = Some(model_id.clone());
            if let Some(harness) = self.effective_harness(cx) {
                let label = self
                    .models
                    .get(&harness)
                    .and_then(|l| l.ready())
                    .and_then(|models| models.iter().find(|m| m.id == model_id))
                    .map(|m| m.label.clone())
                    .unwrap_or_else(|| model_id.clone());
                self.defaults.remember_model(harness, model_id, label);
                self.save_defaults();
            }
        }
        cx.notify();
    }

    fn pick_reasoning(&mut self, level: ReasoningLevel, cx: &mut Context<Self>) {
        // Always a concrete selection (no toggle-back-to-default).
        if self.state.read(cx).selected_chat.is_some() {
            self.update_chat_config(cx, move |config| config.reasoning = Some(level));
        } else {
            self.config.reasoning = Some(level);
            self.defaults.reasoning = Some(level);
            self.save_defaults();
        }
        cx.notify();
    }

    fn pick_option(
        &mut self,
        option_id: String,
        choice_id: String,
        default: bool,
        cx: &mut Context<Self>,
    ) {
        if self.state.read(cx).selected_chat.is_some() {
            self.update_chat_config(cx, move |config| {
                if default {
                    config.model_options.remove(&option_id);
                } else {
                    config
                        .model_options
                        .insert(option_id, serde_json::Value::String(choice_id));
                }
            });
        } else if let Some(harness) = self.effective_harness(cx)
            && let Some(model) = self
                .selected_model(cx)
                .filter(|m| m.options.iter().any(|o| o.id == option_id))
                .map(|m| m.id.clone())
        {
            // New chat: the pick is the sticky memory for the catalog model
            // that offered it — never stored unvalidated.
            let options = self.defaults.model_options_mut(harness, &model);
            if default {
                options.remove(&option_id);
            } else {
                options.insert(option_id, serde_json::Value::String(choice_id));
            }
            self.save_defaults();
        }
        cx.notify();
    }

    /// Apply `change` to the selected chat's effective config and persist it:
    /// optimistic row stamp (chips update on click) + `Mutate setChatConfig`
    /// (LWW workspace write — restarts and other devices see it). The written
    /// row always carries the CONCRETE resolved model/reasoning, with the
    /// reasoning re-clamped to the (possibly just-changed) model's ladder.
    fn update_chat_config(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut ChatConfig)) {
        let Some(chat_id) = self.state.read(cx).selected_chat.clone() else {
            return;
        };
        let resolved = self.resolved(cx);
        let Some(mut config) = resolved.chat_config() else {
            return; // harness unknown (catalog + chat row both missing) — nothing safe to write
        };
        // Preserve fields the pickers don't own.
        if let Some(existing) = self
            .state
            .read(cx)
            .selected_chat_row()
            .and_then(|c| c.config.as_ref())
        {
            config.sandbox = existing.sandbox;
        }
        change(&mut config);
        // Reasoning must stay concrete for whatever model the row now names —
        // same ladder resolution as [`Self::trait_ladder`] (model levels, else
        // the harness's advertised ladder).
        if let Some(models) = self.models.get(&config.harness).and_then(|l| l.ready()) {
            let mut ladder = config
                .model
                .as_deref()
                .and_then(|id| models.iter().find(|m| m.id == id))
                .map(|m| m.reasoning_levels.clone())
                .unwrap_or_default();
            if ladder.is_empty()
                && let Some(descriptor) = self
                    .harnesses
                    .ready()
                    .and_then(|list| list.iter().find(|d| d.id == config.harness))
            {
                ladder = descriptor.reasoning_levels.clone();
            }
            if !ladder.is_empty() {
                config.reasoning = clamp_reasoning(config.reasoning, &ladder);
            }
            // Options likewise: a model switch must not carry picks the new
            // model doesn't offer (e.g. a 1M context onto Haiku).
            if let Some(model) = config
                .model
                .as_deref()
                .and_then(|id| models.iter().find(|m| m.id == id))
            {
                config.model_options =
                    offered_options(model, std::mem::take(&mut config.model_options));
            }
        }
        self.state.update(cx, |state, cx| {
            state.apply_chat_config(&chat_id, config.clone());
            cx.notify();
        });
        let Some(engine) = self.engine(cx) else {
            return;
        };
        self.mutate_task = Some(cx.spawn(async move |_, _| {
            let params = serde_json::json!({
                "op": "setChatConfig",
                "chatId": chat_id,
                "config": config,
            });
            if let Err(err) = engine.client().call(methods::MUTATE, params).await {
                tracing::warn!(error = %err, "setChatConfig mutate failed");
            }
        }));
    }

    // ---- keyboard ----

    /// The traits popover's reasoning ladder (model levels, falling back to
    /// the harness's advertised ladder) — shared by render and keyboard nav.
    fn trait_ladder(&self, cx: &App) -> Vec<ReasoningLevel> {
        let Some(model) = self.selected_model(cx) else {
            return Vec::new();
        };
        if !model.reasoning_levels.is_empty() {
            return model.reasoning_levels.clone();
        }
        self.effective_harness(cx)
            .and_then(|h| {
                self.harnesses
                    .ready()
                    .and_then(|list| list.iter().find(|d| d.id == h))
                    .map(|d| d.reasoning_levels.clone())
            })
            .unwrap_or_default()
    }

    /// The harness descriptors the picker rail offers, with the committed
    /// harness force-included even when it's outside the offered set (a
    /// dev session's mock harness, or one disabled after the chat existed).
    /// Existing chats only offer their own harness.
    fn rail_descriptors(&self, cx: &App) -> Vec<HarnessDescriptor> {
        let Some(list) = self.harnesses.ready() else {
            return Vec::new();
        };
        let mut descriptors = self.offered(list);
        if let Some(effective) = self.effective_harness(cx)
            && !descriptors.iter().any(|d| d.id == effective)
            && let Some(descriptor) = list.iter().find(|d| d.id == effective && d.installed)
        {
            descriptors.insert(0, descriptor.clone());
        }
        if self.harness_locked(cx) {
            let effective = self.effective_harness(cx);
            descriptors.retain(|d| Some(d.id) == effective);
        }
        descriptors
    }

    /// The model rows the picker currently shows, flat and in render order —
    /// keyboard nav, ⌘N jumps, Enter and the render walk THE SAME list.
    ///
    /// A live search spans every ready harness (t3: the sidebar hides and
    /// the query ignores it); otherwise the rail selection decides —
    /// favorites across harnesses, or the effective harness's list with its
    /// starred rows floated to the top (t3 `groupFavorites`). A locked chat
    /// restricts every view to its own harness.
    /// Cached [`Self::visible_model_rows`]: selection/highlight changes and
    /// re-renders share one flattened list until an input actually changes.
    fn model_rows(&self, cx: &App) -> std::sync::Arc<Vec<ModelRowData>> {
        let key = ModelRowsKey {
            query: self.search.read(cx).text().trim().to_string(),
            rail: self.model_rail,
            effective: self.effective_harness(cx),
            locked: self.harness_locked(cx),
            catalog_rev: self.catalog_rev,
            selected: self.effective_model_id(cx).map(str::to_owned),
        };
        if let Some((cached_key, rows)) = self.model_rows_cache.borrow().as_ref()
            && *cached_key == key
        {
            return rows.clone();
        }
        let rows = std::sync::Arc::new(self.visible_model_rows(cx));
        *self.model_rows_cache.borrow_mut() = Some((key, rows.clone()));
        rows
    }

    fn visible_model_rows(&self, cx: &App) -> Vec<ModelRowData> {
        let effective = self.effective_harness(cx);
        let descriptors = self.rail_descriptors(cx);
        // Favorite lookups are per-row; the Vec scan made the flatten
        // O(models × favorites).
        let favorites: std::collections::HashSet<(HarnessId, &str)> = self
            .defaults
            .favorites
            .iter()
            .map(|f| (f.harness, f.model.as_str()))
            .collect();
        let query = self.search.read(cx).text().trim().to_string();
        let mut rows = scoped_model_rows(
            &query,
            self.model_rail,
            effective,
            &descriptors,
            |harness| {
                self.models
                    .get(&harness)
                    .and_then(|l| l.ready())
                    .map(|models| models.as_slice())
            },
            |harness, model| favorites.contains(&(harness, model)),
        );
        // This row belongs only to the current selection. It is never merged
        // into the fresh catalog or made available as a new choice elsewhere.
        if self.model_rail == ModelRail::Harness
            && let Some(harness) = effective
            && let Some(id) = self.effective_model_id(cx)
            && self
                .models
                .get(&harness)
                .and_then(Loadable::ready)
                .is_some_and(|models| !models.iter().any(|m| m.id == id))
            && let Some(descriptor) = descriptors.iter().find(|d| d.id == harness)
        {
            let label = self
                .selected_model_label(cx)
                .unwrap_or_else(|| id.to_owned());
            let query = query.to_lowercase();
            if query.is_empty()
                || id.to_lowercase().contains(&query)
                || label.to_lowercase().contains(&query)
            {
                rows.insert(
                    0,
                    ModelRowData {
                        harness,
                        harness_name: descriptor.name.clone().into(),
                        selected_only: true,
                        ambiguous: false,
                        model: Model {
                            id: id.into(),
                            label,
                            description: Some(
                                "Selected in this chat; absent from the current model list".into(),
                            ),
                            reasoning_levels: vec![],
                            options: vec![],
                        },
                    },
                );
            }
        }
        mark_ambiguous(&mut rows);
        rows
    }

    /// The row the keyboard-nav highlight starts on: the resolved selected
    /// model's index in the VISIBLE rows (the favorites/search views may not
    /// contain it — then 0), 0 while the list is loading.
    fn selected_model_index(&self, cx: &App) -> usize {
        let selected = self
            .effective_model_id(cx)
            .or_else(|| self.selected_model(cx).map(|m| m.id.as_str()));
        let effective = self.effective_harness(cx);
        self.model_rows(cx)
            .iter()
            .position(|row| {
                Some(row.harness) == effective && selected == Some(row.model.id.as_str())
            })
            .unwrap_or(0)
    }

    /// The picker's visible row count (keyboard nav bounds).
    fn model_rows_len(&self, cx: &App) -> usize {
        self.model_rows(cx).len()
    }

    /// Enter on the harness/model popover: pick the highlighted model.
    fn activate_model_row(&mut self, cx: &mut Context<Self>) {
        if self.setting_menu.is_some() {
            self.activate_setting_choice(cx);
        } else if let Some(index) = self.active.checked_sub(self.model_rows_len(cx)) {
            if let Some(group) = self.setting_groups(cx).get(index) {
                self.open_setting(SettingScope::Tray, group.id.clone(), cx);
            }
        } else {
            self.activate_model_index(self.active, cx);
        }
    }

    /// Pick the visible row at `ix` — a foreign-harness row (favorites /
    /// search) switches the harness first, exactly like clicking its rail
    /// icon and then the model.
    fn activate_model_index(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.model_rows(cx).get(ix).cloned() else {
            return;
        };
        if row.selected_only {
            return;
        }
        if self.effective_harness(cx) != Some(row.harness) {
            if self.harness_locked(cx) {
                return;
            }
            self.pick_harness(row.harness, cx);
        }
        self.pick_model(row.model.id, cx);
    }

    /// Star/unstar a model and persist it with the sticky defaults.
    fn toggle_model_favorite(&mut self, harness: HarnessId, model: &str, cx: &mut Context<Self>) {
        self.reload_defaults();
        self.defaults.toggle_favorite(harness, model);
        self.persist_defaults();
        self.catalog_rev += 1;
        // Starring REORDERS the list (stars float to the top / leave the
        // favorites view) — re-home the keyboard highlight onto the SELECTED
        // row so exactly one row reads highlighted afterwards. Following the
        // starred row instead left its cursor wash next to the selected
        // row's ring: "two highlighted rows" (user report, twice).
        self.active = self.selected_model_index(cx);
        cx.notify();
    }

    fn filtered_ref_rows(&self, cx: &App) -> Vec<RepoRef> {
        let Some(refs) = self.refs.ready() else {
            return Vec::new();
        };
        let names: Vec<String> = refs.iter().map(|r| r.name.clone()).collect();
        let query = self.search.read(cx).text().to_string();
        popover::filter_indices(&query, &names)
            .into_iter()
            .map(|ix| refs[ix].clone())
            .collect()
    }

    // ---- checkout resolution (the t3code env-mode semantics) ----

    /// Index of the highlighted-by-default row in the (filtered) ref list:
    /// the session's branch on an existing chat, the draft pick on a new one,
    /// else the current branch. Capped to the displayed window.
    fn selected_ref_index(&self, cx: &App) -> usize {
        let rows = self.filtered_ref_rows(cx);
        let selected = self
            .state
            .read(cx)
            .selected_chat_row()
            .and_then(|c| c.branch.clone())
            .or_else(|| self.config.branch.clone());
        let index = match selected {
            Some(name) => rows.iter().position(|r| r.name == name).unwrap_or(0),
            None => rows.iter().position(|r| r.current).unwrap_or(0),
        };
        index.min(MAX_REF_ROWS.saturating_sub(1))
    }

    /// The picked ref's row, else the repo's current branch's row.
    fn selected_ref(&self) -> Option<&RepoRef> {
        let refs = self.refs.ready()?;
        match self.config.branch.as_deref() {
            Some(name) => refs.iter().find(|r| r.name == name),
            None => refs.iter().find(|r| r.current),
        }
    }

    /// The picked (or current) ref's name.
    fn effective_ref_name(&self) -> Option<String> {
        self.config
            .branch
            .clone()
            .or_else(|| self.selected_ref().map(|r| r.name.clone()))
    }

    /// The existing worktree the picked ref is materialized in, if any.
    fn selected_ref_worktree(&self) -> Option<String> {
        self.selected_ref().and_then(|r| r.worktree_path.clone())
    }

    /// The resolved on-send checkout action for a new session.
    pub fn checkout_plan(&self) -> CheckoutPlan {
        match self.config.checkout {
            CheckoutKind::NewWorktree => CheckoutPlan::NewWorktree {
                base: self.effective_ref_name(),
            },
            CheckoutKind::Local => match self.selected_ref_worktree() {
                Some(path) => CheckoutPlan::ReuseWorktree {
                    path,
                    branch: self.effective_ref_name().unwrap_or_default(),
                },
                None => CheckoutPlan::CurrentCheckout {
                    branch: self.effective_ref_name(),
                },
            },
        }
    }

    /// Label of the checkout-kind trigger (t3code `resolveEnvModeLabel` /
    /// `resolveCurrentWorkspaceLabel`).
    fn checkout_label(&self) -> &'static str {
        match self.config.checkout {
            CheckoutKind::NewWorktree => "New worktree",
            CheckoutKind::Local => {
                if self.selected_ref_worktree().is_some() {
                    "Current worktree"
                } else {
                    "Current checkout"
                }
            }
        }
    }

    /// Label of the ref trigger: `From <ref>` only when a NEW worktree will be
    /// created off it (t3code `getBranchTriggerLabel`); the bare name otherwise.
    fn ref_label(&self) -> SharedString {
        match (self.config.checkout, self.effective_ref_name()) {
            (_, None) => SharedString::from("Select ref"),
            (CheckoutKind::NewWorktree, Some(name)) => SharedString::from(format!("From {name}")),
            (CheckoutKind::Local, Some(name)) => SharedString::from(name),
        }
    }

    // ---- the space picker (new-session canvas) ----

    /// The picker's project rows: scoped to the canvas's device — the device
    /// switcher narrows the list, projects on other devices don't show
    /// (pick the device first, then its project). Unscoped only while the
    /// device is still unknown (pre-probe boot).
    fn scoped_space_rows(&self, cx: &App) -> Vec<Space> {
        let state = self.state.read(cx);
        let device = state.effective_device_id();
        state
            .spaces_sorted()
            .into_iter()
            .filter(|s| match device.as_deref() {
                Some(d) => s.device_id == d,
                None => true,
            })
            .cloned()
            .collect()
    }

    /// [`Self::scoped_space_rows`] matching the search query, ranked
    /// (`popover::filter_indices`).
    fn filtered_space_rows(&self, cx: &App) -> Vec<Space> {
        let query = self.search.read(cx).text().to_string();
        let spaces = self.scoped_space_rows(cx);
        let names: Vec<String> = spaces
            .iter()
            .map(|s| s.display_name().to_string())
            .collect();
        popover::filter_indices(&query, &names)
            .into_iter()
            .map(|ix| spaces[ix].clone())
            .collect()
    }

    /// Current project row on an unsearched open, or the final opt-out row.
    /// An implicit empty selection has no highlight until the user navigates.
    fn selected_space_index(&self, cx: &App) -> usize {
        if self.state.read(cx).no_project {
            return self.scoped_space_rows(cx).len();
        }
        let selected = self
            .state
            .read(cx)
            .selected_space_row()
            .map(|s| s.id.clone());
        selected
            .as_deref()
            .and_then(|id| self.scoped_space_rows(cx).iter().position(|s| s.id == id))
            .unwrap_or(NO_ACTIVE_ROW)
    }

    /// Re-home the canvas onto another project. The state observer does the
    /// heavy lifting: branch draft, ref cache, and the per-device
    /// harness/model catalogs all invalidate on the project change.
    fn pick_space(&mut self, space_id: String, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.auto_selected = true;
            s.select_space(Some(space_id), cx);
        });
        self.remember_target(cx);
        self.close(cx);
    }

    fn pick_no_project(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            // A late opening chats frame must not auto-open an old session
            // after the user has explicitly chosen the new-session target.
            s.auto_selected = true;
            s.select_space(None, cx);
        });
        self.remember_target(cx);
        self.close(cx);
    }

    fn pick_device(&mut self, device_id: String, cx: &mut Context<Self>) {
        self.state
            .update(cx, |s, cx| s.select_device(device_id, cx));
        self.remember_target(cx);
        self.close(cx);
    }

    /// Persist the device/project picks — the "last selected" defaults the
    /// next boot's canvas restores.
    fn remember_target(&mut self, cx: &App) {
        {
            let state = self.state.read(cx);
            self.defaults.device = state
                .selected_device
                .clone()
                .or_else(|| state.local_device_id.clone());
            self.defaults.project = state.selected_space.clone();
            self.defaults.no_project = state.no_project;
        }
        self.save_defaults();
    }

    /// Devices in picker order: this device first, then by name.
    fn device_rows(&self, cx: &App) -> Vec<zeron_proto::Device> {
        let state = self.state.read(cx);
        let local = state.local_device_id.clone();
        let mut devices: Vec<zeron_proto::Device> = state.devices.clone();
        devices.sort_by_key(|d| {
            (
                local.as_deref() != Some(d.id.as_str()),
                d.name.to_lowercase(),
                d.id.clone(),
            )
        });
        devices
    }

    /// [`Self::device_rows`] filtered by the search box (same ranked
    /// substring match as the project rows).
    fn filtered_device_rows(&self, cx: &App) -> Vec<zeron_proto::Device> {
        let query = self.search.read(cx).text().to_string();
        let rows = self.device_rows(cx);
        let names: Vec<String> = rows.iter().map(|d| d.name.clone()).collect();
        popover::filter_indices(&query, &names)
            .into_iter()
            .map(|ix| rows[ix].clone())
            .collect()
    }

    fn selected_device_index(&self, cx: &App) -> usize {
        let effective = self.state.read(cx).effective_device_id();
        self.device_rows(cx)
            .iter()
            .position(|d| Some(d.id.as_str()) == effective.as_deref())
            .unwrap_or(0)
    }

    /// The device popover: search + one row per device (name, muted "offline"
    /// tag, check on the canvas's effective device).
    fn render_device_popover(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let now = chrono::Utc::now();
        let rows = self.filtered_device_rows(cx);
        let (effective, local, online): (Option<String>, Option<String>, Vec<bool>) = {
            let state = self.state.read(cx);
            (
                state.effective_device_id(),
                state.local_device_id.clone(),
                rows.iter()
                    .map(|d| state.device_online(&d.id, now))
                    .collect(),
            )
        };
        let active = self.active;
        let scrollbar = popover::rail(self, "device-scrollbar", &theme, cx);
        let body: AnyElement = if rows.is_empty() {
            div()
                .p(px(Theme::SPACE_SM))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_faint)
                .child(SharedString::from("No devices match."))
                .into_any_element()
        } else {
            popover::menu_scroll_host("device-list-host")
                .on_hover(cx.listener(Self::on_menu_list_hover))
                .child(popover::faded_menu_list(
                    &self.menu_scroll,
                    popover::menu_scroll_list("device-list", &self.menu_scroll)
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .max_h(px(self.list_budget(64.0)))
                        .children(rows.into_iter().zip(online).enumerate().map(
                            |(ix, (device, online))| {
                                let is_local = local.as_deref() == Some(device.id.as_str());
                                let label: SharedString = device.name.clone().into();
                                let is_selected = effective.as_deref() == Some(device.id.as_str());
                                let pick_id = device.id.clone();
                                popover::menu_row_nav(
                                    &theme,
                                    is_selected,
                                    ix == active,
                                    format!("device-row-{ix}"),
                                )
                                .id(("device-row", ix))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.pick_device(pick_id.clone(), cx);
                                }))
                                .child(div().flex_1().min_w_0().truncate().child(label))
                                // The local device wears a muted right-aligned "You"
                                // instead of a "(this device)" suffix in the name.
                                .when(is_local, |el| {
                                    el.child(
                                        div()
                                            .flex_none()
                                            .text_size(crate::typography::ui_rems(10.0))
                                            .text_color(theme.text_muted)
                                            .child(SharedString::from("You")),
                                    )
                                })
                                // Disconnected glyph, not the word (user request).
                                .when(!online, |el| {
                                    el.child(
                                        crate::icons::icon(crate::icons::WIFI_OFF)
                                            .size(px(12.0))
                                            .flex_none()
                                            .text_color(theme.warning.opacity(0.8)),
                                    )
                                })
                            },
                        )),
                ))
                .children(scrollbar)
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .child(self.search_box(&theme))
            .child(body)
            .into_any_element()
    }

    /// The project popover: search + one row per project on the picked device
    /// (check on the current pick), then "New project…" and the opt-out rows. Rows
    /// are device-scoped, so no per-row `@ device` tag — the device chip next
    /// door names the host.
    fn render_space_popover(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let rows = self.filtered_space_rows(cx);
        let selected = self
            .state
            .read(cx)
            .selected_space_row()
            .map(|s| s.id.clone());
        let active = self.active;
        let no_project_index = rows.len();
        let scrollbar = popover::rail(self, "space-scrollbar", &theme, cx);
        let body: AnyElement = if rows.is_empty() {
            // Distinguish "the filter ate everything" from "this device has
            // no projects yet" — the scoped list makes the latter common.
            let empty: &str = if self.search.read(cx).text().is_empty() {
                "No projects on this device."
            } else {
                "No projects match."
            };
            div()
                .p(px(Theme::SPACE_SM))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_faint)
                .child(SharedString::from(empty.to_string()))
                .into_any_element()
        } else {
            popover::menu_scroll_host("space-list-host")
                .on_hover(cx.listener(Self::on_menu_list_hover))
                .child(popover::faded_menu_list(
                    &self.menu_scroll,
                    popover::menu_scroll_list("space-list", &self.menu_scroll)
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .max_h(px(self.list_budget(152.0)))
                        .children(rows.into_iter().enumerate().map(|(ix, space)| {
                            let label: SharedString = space.display_name().to_string().into();
                            let is_selected = selected.as_deref() == Some(space.id.as_str());
                            let pick_id = space.id.clone();
                            popover::menu_row_nav(
                                &theme,
                                is_selected,
                                ix == active,
                                format!("space-row-{ix}"),
                            )
                            .id(("space-row", ix))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pick_space(pick_id.clone(), cx);
                            }))
                            .child(div().flex_1().min_w_0().truncate().child(label))
                        })),
                ))
                .children(scrollbar)
                .into_any_element()
        };
        let no_project = popover::menu_row_nav(
            &theme,
            self.state.read(cx).no_project,
            active == no_project_index,
            "project-none".to_string(),
        )
        .id("project-none")
        .on_click(cx.listener(|this, _, _, cx| this.pick_no_project(cx)))
        .child(
            crate::icons::icon(crate::icons::CLOSE)
                .size(px(12.0))
                .flex_none()
                .text_color(theme.text_muted),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child("Don't work in a project"),
        );
        // Action row under a hairline: mint a project.
        let new_project = popover::menu_row_nav(&theme, false, false, "project-new".to_string())
            .id("project-new")
            .on_click(cx.listener(|this, _, window, cx| {
                this.dismiss(cx);
                window.dispatch_action(Box::new(crate::shell::AddSpacePalette), cx);
            }))
            .child(
                crate::icons::icon(crate::icons::PLUS)
                    .size(px(12.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from("New project…")),
            );
        div()
            .flex()
            .flex_col()
            // Same 2px rhythm as the list's own row gap — the action rows
            // sat flush while list rows breathed (user report).
            .gap(px(2.0))
            .child(self.search_box(&theme))
            .child(body)
            .child(
                // Full-bleed through the card's shared inset — a divider
                // stopping short of the edges read as a mistake.
                div()
                    .my(px(2.0))
                    .mx(px(-popover::CARD_INSET))
                    .h(px(1.0))
                    .flex_none()
                    .bg(theme.border.opacity(0.6)),
            )
            .child(new_project)
            .child(no_project)
            .into_any_element()
    }

    fn on_search_submit(&mut self, cx: &mut Context<Self>) {
        if self.open_kind() == Some(PickerKind::Branch)
            && let Some(row) = self.filtered_ref_rows(cx).into_iter().nth(self.active)
        {
            self.pick_ref(row, cx);
        }
        if self.open_kind() == Some(PickerKind::Space) {
            let rows = self.filtered_space_rows(cx);
            if let Some(space) = rows.get(self.active) {
                self.pick_space(space.id.clone(), cx);
            } else if self.active == rows.len() {
                self.pick_no_project(cx);
            }
        }
        if self.open_kind() == Some(PickerKind::Device)
            && let Some(device) = self.filtered_device_rows(cx).into_iter().nth(self.active)
        {
            self.pick_device(device.id, cx);
        }
        // Palette-search Enter submits the highlighted model or setting.
        if self.open_kind() == Some(PickerKind::HarnessModel) {
            self.activate_model_row(cx);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &Window, cx: &mut Context<Self>) {
        self.cancel_setting_hover();
        // The frame stays mounted (and possibly focused) through the exit
        // animation — keys must not drive a dying popover.
        if !self.open.is_open() {
            return;
        }
        if self.setting_menu.is_some() {
            match event.keystroke.key.as_str() {
                "escape" | "left" => {
                    self.setting_menu = None;
                    self.setting_bounds = None;
                }
                "up" | "down" => {
                    let count = self
                        .scope_groups(self.setting_scope, cx)
                        .into_iter()
                        .find(|g| Some(&g.id) == self.setting_menu.as_ref())
                        .map(|g| g.choices.len())
                        .unwrap_or(0);
                    self.setting_active = popover::menu_step(
                        Some(self.setting_active),
                        count,
                        if event.keystroke.key == "up" { -1 } else { 1 },
                    )
                    .unwrap_or(0);
                    self.setting_scroll.scroll_to_item(self.setting_active);
                }
                "enter" => self.activate_setting_choice(cx),
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if event.keystroke.key == "right"
            && self.open_kind() == Some(PickerKind::HarnessModel)
            && self.active >= self.model_rows_len(cx)
        {
            self.activate_model_row(cx);
            cx.stop_propagation();
            return;
        }
        // ⌘1…⌘9 jump-picks the Nth visible model row (t3 modelPickerKeys;
        // the chips on the rows advertise these).
        if self.open_kind() == Some(PickerKind::HarnessModel)
            && event.keystroke.modifiers.platform
            && let Ok(n) = event.keystroke.key.parse::<usize>()
            && (1..=9).contains(&n)
        {
            self.activate_model_index(n - 1, cx);
            cx.notify();
            return;
        }
        let key = popover::classify_key(
            event.keystroke.key.as_str(),
            event.keystroke.modifiers.platform,
            event.keystroke.modifiers.control,
        );

        match key {
            MenuKey::Escape => {
                self.animate_close(cx);
                cx.notify();
                cx.stop_propagation();
            }
            MenuKey::Up | MenuKey::Down => {
                let delta = if key == MenuKey::Up { -1 } else { 1 };
                let count = match self.open_kind() {
                    Some(PickerKind::Branch) => self.filtered_ref_rows(cx).len().min(MAX_REF_ROWS),
                    Some(PickerKind::Checkout) => 2,
                    // Continue from model rows into the pinned settings triggers.
                    Some(PickerKind::HarnessModel) => {
                        self.model_rows_len(cx) + self.setting_groups(cx).len()
                    }
                    Some(PickerKind::Space) => self.filtered_space_rows(cx).len() + 1,
                    Some(PickerKind::Device) => self.filtered_device_rows(cx).len(),
                    None => 0,
                };
                let current = (self.active != NO_ACTIVE_ROW).then_some(self.active);
                self.active = popover::menu_step(current, count, delta).unwrap_or(0);
                // Keep the highlighted MODEL row in view (the rows are the
                // scroll container's direct children, so indices map 1:1);
                // the traits chips below live in the pinned tray and never
                // need scrolling into view.
                if self.open_kind() == Some(PickerKind::HarnessModel)
                    && self.active < self.model_rows_len(cx)
                {
                    self.model_scroll
                        .scroll_to_item(self.active, gpui::ScrollStrategy::Nearest);
                }
                cx.notify();
                cx.stop_propagation();
            }
            MenuKey::Enter | MenuKey::ModEnter => {
                if self.open_kind() == Some(PickerKind::HarnessModel) {
                    self.activate_model_row(cx);
                } else if self.open_kind() == Some(PickerKind::Checkout) {
                    let kind = if self.active == 0 {
                        CheckoutKind::Local
                    } else {
                        CheckoutKind::NewWorktree
                    };
                    self.pick_checkout(kind, cx);
                } else {
                    self.on_search_submit(cx);
                }
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn menu_geometry(&self) -> popover::MenuGeometry {
        self.mounted_kind()
            .and_then(|kind| self.menu_geometry.get(&kind).copied())
            .unwrap_or(popover::MenuGeometry {
                height: 320.0,
                below: false,
            })
    }

    fn list_budget(&self, chrome: f32) -> f32 {
        (self.menu_geometry().height - chrome).clamp(0.0, 224.0)
    }

    fn measure_trigger(&self, kind: PickerKind, cx: &Context<Self>) -> impl IntoElement {
        let entity = cx.entity().downgrade();
        // New-thread model and workspace menus open down, matching the
        // centered composer's layout. In-thread menus retain adaptive placement.
        let below = (self.title.is_some() || self.state.read(cx).selected_chat.is_none())
            && matches!(
                kind,
                PickerKind::HarnessModel | PickerKind::Branch | PickerKind::Checkout
            );
        gpui::canvas(
            move |bounds, window, cx| {
                let mut geometry = popover::menu_geometry(
                    f32::from(bounds.top()),
                    f32::from(bounds.bottom()),
                    f32::from(window.viewport_size().height),
                );
                let space_below = (f32::from(window.viewport_size().height - bounds.bottom())
                    - 14.0)
                    .clamp(0.0, 640.0);
                // Prefer below on the new-chat canvas only when it can fit
                // useful content. Tall drafts/attachments can put the trigger
                // near the window bottom; retain adaptive placement there.
                if below && space_below >= 180.0 {
                    geometry = popover::MenuGeometry {
                        height: space_below,
                        below: true,
                    };
                }
                entity
                    .update(cx, |this, cx| {
                        if kind == PickerKind::HarnessModel {
                            this.model_trigger_bounds = Some(bounds);
                            if this.mounted_kind() == Some(kind) && this.open_model_width.is_none()
                            {
                                this.open_model_width = Some(bounds.size.width);
                                cx.notify();
                            }
                        }
                        if this.menu_geometry.get(&kind) != Some(&geometry) {
                            this.menu_geometry.insert(kind, geometry);
                            cx.notify();
                        }
                    })
                    .ok();
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    // ---- render ----

    #[allow(clippy::too_many_arguments)]
    fn trigger_chip(
        &self,
        kind: PickerKind,
        label: SharedString,
        set: bool,
        chip_icon: Option<(&'static str, Option<gpui::Hsla>)>,
        // The chip never collapses while identity resolves (user report):
        // `icon_loading` swaps the brand slot for the pixel-glyph loader
        // (harness unknown), `label_loading` swaps the text for a ghost bar
        // (model unknown).
        icon_loading: bool,
        label_loading: bool,
        suffix: Option<(SharedString, Option<gpui::Hsla>)>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let base = match kind {
            PickerKind::Branch => "picker-branch",
            PickerKind::Checkout => "picker-checkout",
            PickerKind::HarnessModel => "picker-model",
            PickerKind::Space => "picker-space",
            PickerKind::Device => "picker-device",
        };
        // Hover state is keyed globally: two composers on screen (main +
        // side chat) must not light each other's chips, so the key carries
        // this picker's entity.
        let id: SharedString = format!("{base}-{}", cx.entity_id()).into();
        let open = self.open_kind() == Some(kind);
        // Ghost pill (zeron composer/styles.tsx `pill`): `h-8 rounded-lg px-2.5
        // gap-1.5 text-[12px] font-medium text-muted-foreground`, icons size-4,
        // hover/open wash — no border, no caret; the actions row stays quiet.
        div()
            .relative()
            .child(self.measure_trigger(kind, cx))
            .id(id.clone())
            .h(px(32.0))
            .max_w(px(248.0))
            .when(
                kind == PickerKind::HarnessModel && self.mounted_kind() == Some(kind),
                |chip| chip.when_some(self.open_model_width, |chip, width| chip.w(width)),
            )
            // Shrinkable under row pressure — four footer chips share one
            // line; without min_w_0 they overflowed and painted overlapped.
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            // The model trigger sits immediately beside the attachment.
            // Its own padding participates in that visible gap; footer and
            // destination triggers keep their wider independent hit areas.
            .px(px(if kind == PickerKind::HarnessModel {
                6.0
            } else {
                10.0
            }))
            .rounded(px(8.0))
            .text_size(crate::typography::ui_rems(12.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            // zeron composer/styles.tsx `pill`: `transition-colors` — the wash
            // and text brighten fade over 150ms.
            .text_color(motion::hover_blend(
                &id,
                if set {
                    theme.text.opacity(0.9)
                } else {
                    theme.text_muted
                },
                theme.text,
            ))
            .bg(if open {
                theme.element_hover
            } else {
                motion::hover_blend(&id, gpui::transparent_black(), theme.element_hover)
            })
            .on_hover(motion::hover_listener(id.clone()))
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, _| {
                    this.open.note_trigger_press_matching(|open| *open == kind)
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.toggle(kind, window, cx)))
            .when(icon_loading, |el| {
                el.child(div().flex_none().child(crate::loaders::mini_glyph_spinner(
                    "picker-chip-loader",
                    2.0,
                    theme.glyph,
                    cx.entity_id(),
                    cx,
                )))
            })
            .when_some(
                (!icon_loading).then_some(chip_icon).flatten(),
                |el, (path, tint)| {
                    el.child(
                        crate::icons::icon(path)
                            .size(px(16.0))
                            .text_color(tint.unwrap_or(theme.text_muted)),
                    )
                },
            )
            .when(label_loading, |el| {
                el.child(popover::skeleton_bar(56.0, cx.entity_id(), cx))
            })
            .when(!label_loading, |el| {
                el.child(div().min_w_0().truncate().child(label))
            })
            // The effort half of the combined model+effort chip (and the space
            // chip's "@ device" tag): muted, no icon — one button, two tones.
            // `tint` overrides the muted tone (the offline warning). Under row
            // pressure the suffix yields FIRST (large shrink factor) so the
            // model name — the run's identity — truncates last.
            .when_some(suffix, |el, (suffix, tint)| {
                el.child(
                    div()
                        .flex_shrink(1000.0)
                        .min_w_0()
                        .truncate()
                        .text_color(tint.unwrap_or(theme.text_muted.opacity(0.7)))
                        .child(suffix),
                )
            })
    }

    /// A footer-row trigger (t3code ghost `Button size="xs"`): leading icon,
    /// truncating label, trailing chevron — smaller and quieter than the
    /// in-pill chips.
    fn footer_chip(
        &self,
        kind: PickerKind,
        id: &'static str,
        icon_path: &'static str,
        label: SharedString,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let open = self.open_kind() == Some(kind);
        // Per-picker hover key, like the trigger chips.
        let id: SharedString = format!("{id}-{}", cx.entity_id()).into();
        div()
            .relative()
            .child(self.measure_trigger(kind, cx))
            .id(id.clone())
            .h(px(20.0))
            .max_w(px(280.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(8.0))
            .rounded(px(FOOTER_CHIP_RADIUS))
            .text_size(crate::typography::ui_rems(12.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(motion::hover_blend(
                &id,
                theme.text_muted.opacity(0.7),
                theme.text.opacity(0.8),
            ))
            .bg(if open {
                theme.element_hover
            } else {
                motion::hover_blend(&id, gpui::transparent_black(), theme.element_hover)
            })
            .on_hover(motion::hover_listener(id.clone()))
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, _| {
                    this.open.note_trigger_press_matching(|open| *open == kind)
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.toggle(kind, window, cx)))
            .child(
                crate::icons::icon(icon_path)
                    .size(px(12.0))
                    .flex_none()
                    .text_color(theme.text_muted.opacity(0.7)),
            )
            .child(div().min_w_0().truncate().child(label))
            .child(
                crate::icons::icon(crate::icons::ALT_ARROW_DOWN)
                    .size(px(12.0))
                    .flex_none()
                    .text_color(theme.text_muted.opacity(0.5)),
            )
    }

    /// A read-only footer label (locked sessions — t3code's
    /// `resolveLockedWorkspaceLabel` span).
    fn footer_label(icon_path: &'static str, label: SharedString, theme: &Theme) -> gpui::Div {
        div()
            .h(px(20.0))
            // Four of these share one row now (device, project, checkout,
            // ref): cap each early and let them SHRINK (`min_w_0`) — without
            // it the clusters overflowed into each other and the labels
            // painted overlapped (user report).
            .max_w(px(160.0))
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(8.0))
            .text_size(crate::typography::ui_rems(12.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme.text_muted.opacity(0.6))
            .child(
                crate::icons::icon(icon_path)
                    .size(px(12.0))
                    .text_color(theme.text_muted.opacity(0.6)),
            )
            .child(div().min_w_0().truncate().child(label))
    }

    /// New-session destination controls. Machine and project form the
    /// original chip-only cluster floating above the composer's trailing edge.
    pub fn render_new_thread_target_selectors(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let closing = (self.open.closing_since(), self.menu_geometry().below);
        let mut overlay: Option<(PickerKind, AnyElement)> = match self.mounted_kind() {
            Some(PickerKind::Space) => {
                let content = self.render_space_popover(cx);
                Some((PickerKind::Space, self.popover_frame(280.0, content, cx)))
            }
            Some(PickerKind::Device) => {
                let content = self.render_device_popover(cx);
                Some((PickerKind::Device, self.popover_frame(224.0, content, cx)))
            }
            _ => None,
        };
        let (device_label, project_label, offline) = {
            let state = self.state.read(cx);
            let device_id = state.effective_device_id();
            let device_label: SharedString = device_id
                .as_deref()
                .and_then(|id| state.device_name(id))
                .map(str::to_string)
                .unwrap_or_else(|| "This device".to_string())
                .into();
            let offline = device_id
                .as_deref()
                .is_some_and(|id| !state.device_online(id, chrono::Utc::now()));
            let project_label: SharedString = state
                .selected_space_row()
                .map(|s| s.display_name().to_string())
                .unwrap_or_else(|| "No project".to_string())
                .into();
            (device_label, project_label, offline)
        };
        let device_chip = self
            .footer_chip(
                PickerKind::Device,
                "picker-device",
                crate::icons::MONITOR,
                device_label,
                &theme,
                cx,
            )
            .when(offline, |el| el.text_color(theme.warning.opacity(0.8)));
        let project_chip = self.footer_chip(
            PickerKind::Space,
            "picker-project",
            crate::icons::FOLDER,
            project_label,
            &theme,
            cx,
        );
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .child(attach_overlay_end(
                device_chip,
                &mut overlay,
                PickerKind::Device,
                "device-popover",
                closing,
            ))
            .child(attach_overlay_end(
                project_chip,
                &mut overlay,
                PickerKind::Space,
                "project-popover",
                closing,
            ))
            .into_any_element()
    }

    /// New-session Git controls. Checkout mode and branch form the original
    /// chip-only cluster floating below the composer's leading edge.
    pub fn render_new_thread_git_selectors(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let git = self
            .state
            .read(cx)
            .selected_space_row()
            .is_some_and(|space| space.git_detected);
        if !git {
            return None;
        }
        self.ensure_refs(false, cx);
        let theme = Theme::of(cx).clone();
        let closing = (self.open.closing_since(), self.menu_geometry().below);
        let mut overlay: Option<(PickerKind, AnyElement)> = match self.mounted_kind() {
            Some(PickerKind::Branch) => {
                let content = self.render_branch_popover(cx);
                Some((PickerKind::Branch, self.popover_frame(320.0, content, cx)))
            }
            Some(PickerKind::Checkout) => {
                let content = self.render_checkout_popover(cx);
                Some((PickerKind::Checkout, self.popover_frame(224.0, content, cx)))
            }
            _ => None,
        };
        let kind_icon = match (self.config.checkout, self.selected_ref_worktree().is_some()) {
            (CheckoutKind::Local, false) => crate::icons::FOLDER,
            _ => crate::icons::FOLDER_WITH_FILES,
        };
        let checkout_chip = self.footer_chip(
            PickerKind::Checkout,
            "picker-checkout",
            kind_icon,
            SharedString::from(self.checkout_label()),
            &theme,
            cx,
        );
        let branch_chip = self.footer_chip(
            PickerKind::Branch,
            "picker-branch",
            crate::icons::GIT_BRANCH,
            self.ref_label(),
            &theme,
            cx,
        );
        Some(
            workspace_footer_row()
                .child(attach_overlay_below(
                    checkout_chip,
                    &mut overlay,
                    PickerKind::Checkout,
                    "checkout-popover",
                    closing,
                ))
                .child(attach_overlay_below(
                    branch_chip,
                    &mut overlay,
                    PickerKind::Branch,
                    "branch-popover",
                    closing,
                ))
                .into_any_element(),
        )
    }

    /// The composer footer row: checkout-kind + ref, LEFT-aligned, only when
    /// the picked (or session's) project has git. New sessions use the floating
    /// chip clusters; sessions name their target in the titlebar.
    pub fn render_footer(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = Theme::of(cx).clone();
        // A selected chat whose workspace row hasn't synced yet (the moment
        // right after send mints it) still renders the DRAFT footer — the
        // values are identical, so the toolbar never blinks through a
        // half-empty locked state.
        let (space, session, change_request) = {
            let state = self.state.read(cx);
            let space = state.selected_space_row().cloned();
            let session = state
                .selected_chat
                .as_ref()
                .and_then(|_| state.selected_chat_row().cloned());
            let change_request = session
                .as_ref()
                .and_then(|chat| state.change_request_for_chat(chat).cloned());
            (space, session, change_request)
        };
        let row = || {
            // The composer owns the row's animated reveal and negative bottom
            // margin. Keeping that geometry outside this reusable content
            // lets the new-thread route handoff collapse the footer without
            // clipping its controls or changing its steady-state spacing.
            // `w_full` is load-bearing: without it the canvas layout sizes
            // the row to CONTENT, and the left cluster's flex_1 (basis 0)
            // collapsed to zero width — both clusters painted from the same
            // origin, chips overlapping (user report).
            workspace_footer_row().px(px(10.0))
        };

        if let Some(chat) = &session {
            // Sessions never move: read-only checkout-kind + ref labels,
            // LEFT-aligned, only when the session's project has git. The
            // target (project @ device) lives in the titlebar now.
            let Some(space) = space.as_ref().filter(|s| s.git_detected) else {
                return None;
            };
            let is_worktree = chat.cwd.as_deref().is_some_and(|cwd| cwd != space.path);
            let (icon_path, label) = if is_worktree {
                (crate::icons::FOLDER_WITH_FILES, "Worktree")
            } else {
                (crate::icons::FOLDER, "Local checkout")
            };
            // Keep the same reading order and leading edge as the draft.
            let left = div()
                .flex()
                .flex_row()
                .items_center()
                .min_w_0()
                .child(Self::footer_label(
                    icon_path,
                    SharedString::from(label),
                    &theme,
                ));
            let right = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .min_w_0()
                .child(Self::footer_label(
                    crate::icons::GIT_BRANCH,
                    chat.branch
                        .clone()
                        .map(SharedString::from)
                        .unwrap_or_else(|| SharedString::from("No ref")),
                    &theme,
                ));
            // Checkout + branch stay together. PR and usage form the trailing
            // status group, independently of the branch label's length.
            return Some(
                row()
                    .pr_0()
                    .child(left)
                    .child(right)
                    .child(div().flex_1().min_w_0())
                    .when_some(change_request, |el, summary| {
                        el.child(div().flex_none().child(
                            crate::change_requests::pull_request_badge(
                                "composer-pull-request".into(),
                                summary,
                                crate::change_requests::ChangeRequestBadgeSurface::Composer,
                                &theme,
                            ),
                        ))
                    })
                    .into_any_element(),
            );
        }

        // New-session draft: checkout + ref only, LEFT-aligned (device +
        // project live in the row above the pill now).
        let git = space.as_ref().is_some_and(|s| s.git_detected);
        if !git {
            return None;
        }
        // Refs feed the draft labels — eager + idempotent.
        self.ensure_refs(false, cx);
        let closing = (self.open.closing_since(), self.menu_geometry().below);
        let mut overlay: Option<(PickerKind, AnyElement)> = match self.mounted_kind() {
            Some(PickerKind::Branch) => {
                let content = self.render_branch_popover(cx);
                Some((PickerKind::Branch, self.popover_frame(320.0, content, cx)))
            }
            Some(PickerKind::Checkout) => {
                let content = self.render_checkout_popover(cx);
                Some((PickerKind::Checkout, self.popover_frame(224.0, content, cx)))
            }
            // Space/Device popovers mount in the floating row above the pill.
            _ => None,
        };

        let ref_label = self.ref_label();
        let ref_chip = self.footer_chip(
            PickerKind::Branch,
            "picker-branch",
            crate::icons::GIT_BRANCH,
            ref_label,
            &theme,
            cx,
        );
        let kind_icon = match (self.config.checkout, self.selected_ref_worktree().is_some()) {
            (CheckoutKind::Local, false) => crate::icons::FOLDER,
            _ => crate::icons::FOLDER_WITH_FILES,
        };
        let kind_chip = self.footer_chip(
            PickerKind::Checkout,
            "picker-checkout",
            kind_icon,
            SharedString::from(self.checkout_label()),
            &theme,
            cx,
        );
        // Match the floating draft's adjacent checkout/ref pair, including
        // while the newly created session is waiting for its workspace row.
        let left = div()
            .flex()
            .flex_row()
            .items_center()
            .min_w_0()
            .child(attach_overlay(
                kind_chip,
                &mut overlay,
                PickerKind::Checkout,
                "checkout-popover",
                closing,
            ));
        let right = div()
            .flex()
            .flex_row()
            .items_center()
            .min_w_0()
            .child(attach_overlay(
                ref_chip,
                &mut overlay,
                PickerKind::Branch,
                "branch-popover",
                closing,
            ));
        Some(row().child(left).child(right).into_any_element())
    }

    fn popover_frame(&self, width: f32, content: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        popover::popover_card(&theme)
            .w(px(width))
            // zeron caps its tallest picker at min(640px, 75vh).
            .max_h(px(self.menu_geometry().height))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_key_down(event, window, cx)
            }))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.is_open() && !this.focus.contains_focused(window, cx) {
                        window.focus(&this.focus, cx);
                    }
                }),
            )
            .on_mouse_down_out(
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    if this.setting_menu.is_some()
                        && this
                            .setting_bounds
                            .is_some_and(|bounds| bounds.contains(&event.position))
                    {
                        return;
                    }
                    this.dismiss(cx);
                    if this.focus.contains_focused(window, cx) {
                        window.blur();
                    }
                }),
            )
            .flex()
            .flex_col()
            .child(content)
            .into_any_element()
    }

    /// [`Self::popover_frame`] without the p-1 inset — the harness/model
    /// picker's rail + list panes bleed to the card edge (zeron
    /// harness-model-picker.tsx `className="w-80 p-0"`).
    fn popover_frame_flush(
        &self,
        width: f32,
        content: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        popover::popover_card_flush(&theme)
            .w(px(width))
            .h(px(self.menu_geometry().height.min(self.open_model_height)))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_key_down(event, window, cx)
            }))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.is_open() && !this.focus.contains_focused(window, cx) {
                        window.focus(&this.focus, cx);
                    }
                }),
            )
            .on_mouse_down_out(
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    if this.setting_menu.is_some()
                        && this
                            .setting_bounds
                            .is_some_and(|bounds| bounds.contains(&event.position))
                    {
                        return;
                    }
                    this.dismiss(cx);
                    if this.focus.contains_focused(window, cx) {
                        window.blur();
                    }
                }),
            )
            .flex()
            .flex_col()
            .child(content)
            .into_any_element()
    }

    fn search_box(&self, theme: &Theme) -> AnyElement {
        popover::search_input_frame(theme, self.search.clone().into_any_element())
            .into_any_element()
    }

    fn retry_row(
        &self,
        id: &'static str,
        message: &str,
        kind: PickerKind,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        popover::error_row(theme, message)
            .child(
                div()
                    .id(id)
                    .px(px(Theme::SPACE_SM))
                    .py(px(3.0))
                    .rounded(px(Theme::CONTROL_RADIUS))
                    .border_1()
                    .border_color(theme.border)
                    .text_color(theme.text)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.element_hover))
                    .on_click(cx.listener(move |this, _, _, cx| match kind {
                        PickerKind::Branch | PickerKind::Checkout => this.ensure_refs(true, cx),
                        PickerKind::HarnessModel => {
                            this.harnesses = Loadable::Idle;
                            this.models.clear();
                            this.model_refresh_errors.clear();
                            this.catalog_rev += 1;
                            this.ensure_harnesses(false, cx);
                        }
                        // Projects/devices load nothing; no retry surface exists.
                        PickerKind::Space | PickerKind::Device => {}
                    }))
                    .child(SharedString::from("Retry")),
            )
            .into_any_element()
    }

    /// The virtualized list's plain scroll handle (bounds/offset for the
    /// floating scrollbar; `UniformList` tracks it internally).
    fn model_scroll_base(&self) -> gpui::ScrollHandle {
        self.model_scroll.0.borrow().base_handle.clone()
    }

    /// The scroll handle of whichever picker menu is mounted. The popups are
    /// mutually exclusive: the model list owns its virtualized handle, the
    /// plain-div menus (branch / project / device) share `menu_scroll`.
    /// Keys on the MOUNTED menu, not `open_kind` — popovers keep rendering
    /// through the exit animation, and the rail must keep measuring the
    /// closing menu's own handle, not `menu_scroll`'s idle geometry.
    fn active_menu_scroll(&self) -> gpui::ScrollHandle {
        if self.mounted_kind() == Some(PickerKind::HarnessModel) {
            self.model_scroll_base()
        } else {
            self.menu_scroll.clone()
        }
    }

    /// The list-hover half of the rail treatment; the strip's own hover,
    /// press, drag, and mouse-up listeners come from [`popover::rail`].
    fn on_menu_list_hover(
        &mut self,
        hovered: &bool,
        _window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.menu_bar.set_list_hovered(*hovered) {
            cx.notify();
        }
    }

    /// The ref picker (t3code BranchToolbarBranchSelector): search on top,
    /// rows with right-aligned muted `current`/`worktree` tags, and a
    /// "Showing X of Y refs" footer when the list is capped.
    fn render_branch_popover(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        if self.state.read(cx).selected_space_row().is_none() {
            return div()
                .p(px(Theme::SPACE_SM))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_faint)
                .child(SharedString::from("No project selected"))
                .into_any_element();
        }
        let rows = self.filtered_ref_rows(cx);
        let total = rows.len();
        let shown = total.min(MAX_REF_ROWS);
        // Existing session: the highlighted row is the SESSION's branch and a
        // pick switches the checkout (see `pick_ref`); a new chat highlights
        // the draft pick.
        let session_branch = self
            .state
            .read(cx)
            .selected_chat_row()
            .and_then(|c| c.branch.clone());
        let switching = self.switching.clone();
        let scrollbar = popover::rail(self, "branch-scrollbar", &theme, cx);
        let body: AnyElement = match &self.refs {
            Loadable::Loading | Loadable::Idle => {
                popover::skeleton_rows("branch-skeleton", &theme, 4, cx.entity_id(), cx)
            }
            Loadable::Error(message) => {
                let message = message.clone();
                self.retry_row("branch-retry", &message, PickerKind::Branch, &theme, cx)
            }
            Loadable::Ready(_) if rows.is_empty() => div()
                .p(px(Theme::SPACE_SM))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_faint)
                .child(SharedString::from("No refs found."))
                .into_any_element(),
            Loadable::Ready(_) => {
                let active = self.active;
                let selected = session_branch.or_else(|| self.config.branch.clone());
                popover::menu_scroll_host("branch-list-host")
                    .on_hover(cx.listener(Self::on_menu_list_hover))
                    .child(popover::faded_menu_list(
                        &self.menu_scroll,
                        popover::menu_scroll_list("branch-list", &self.menu_scroll)
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .max_h(px(self.list_budget(144.0)))
                            .children(rows.into_iter().take(MAX_REF_ROWS).enumerate().map(
                                |(ix, row)| {
                                    let label: SharedString = row.name.clone().into();
                                    let is_selected =
                                        selected.as_deref() == Some(row.name.as_str());
                                    // Right-aligned muted tag (t3code `text-[10px]
                                    // text-muted-foreground/45`): current beats worktree.
                                    let tag: Option<&'static str> = if row.current {
                                        Some("current")
                                    } else if row.worktree_path.is_some() {
                                        Some("worktree")
                                    } else {
                                        None
                                    };
                                    let is_switching =
                                        switching.as_deref() == Some(row.name.as_str());
                                    popover::menu_row_nav(
                                        &theme,
                                        is_selected,
                                        ix == active,
                                        format!("branch-row-{ix}"),
                                    )
                                    .id(("branch-row", ix))
                                    .when(switching.is_some(), |el| el.opacity(0.55))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.pick_ref(row.clone(), cx);
                                    }))
                                    .child(div().flex_1().min_w_0().truncate().child(label))
                                    .when(is_switching, |el| {
                                        el.child(
                                            div()
                                                .flex_none()
                                                .text_size(crate::typography::ui_rems(10.0))
                                                .text_color(theme.text_muted)
                                                .child(SharedString::from("switching…")),
                                        )
                                    })
                                    .when_some(
                                        tag,
                                        |el, tag| {
                                            el.child(
                                                div()
                                                    .flex_none()
                                                    .text_size(crate::typography::ui_rems(10.0))
                                                    .text_color(theme.text_muted)
                                                    .child(SharedString::from(tag)),
                                            )
                                        },
                                    )
                                },
                            )),
                    ))
                    .children(scrollbar)
                    .into_any_element()
            }
        };
        let mut popover = div()
            .flex()
            .flex_col()
            .child(self.search_box(&theme))
            .child(body);
        // Mid-session switch failure (dirty tree, ref checked out elsewhere):
        // git's own message, under a hairline.
        if let Some(error) = &self.switch_error {
            popover = popover.child(
                popover::menu_section().child(
                    div()
                        .px(px(Theme::SPACE_SM))
                        .py(px(4.0))
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.danger.opacity(0.9))
                        .child(SharedString::from(error.clone())),
                ),
            );
        }
        if total > shown {
            popover = popover.child(
                popover::menu_section().child(
                    div()
                        .px(px(Theme::SPACE_SM))
                        .py(px(4.0))
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_faint)
                        .child(SharedString::from(format!(
                            "Showing {shown} of {total} refs"
                        ))),
                ),
            );
        }
        popover.into_any_element()
    }

    /// The checkout-kind dropdown (t3code BranchToolbarEnvModeSelector): two
    /// rows — "Current checkout"/"Current worktree" (local) and "New worktree".
    fn render_checkout_popover(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let has_worktree = self.selected_ref_worktree().is_some();
        let local_label: &'static str = if has_worktree {
            "Current worktree"
        } else {
            "Current checkout"
        };
        let local_icon = if has_worktree {
            crate::icons::FOLDER_WITH_FILES
        } else {
            crate::icons::FOLDER
        };
        let options: [(CheckoutKind, &'static str, &'static str); 2] = [
            (CheckoutKind::Local, local_label, local_icon),
            (
                CheckoutKind::NewWorktree,
                "New worktree",
                crate::icons::FOLDER_WITH_FILES,
            ),
        ];
        let active = self.active;
        let current = self.config.checkout;
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .children(
                options
                    .into_iter()
                    .enumerate()
                    .map(|(ix, (kind, label, icon_path))| {
                        let is_selected = current == kind;
                        popover::menu_row_nav(
                            &theme,
                            is_selected,
                            ix == active,
                            format!("checkout-row-{ix}"),
                        )
                        .id(("checkout-row", ix))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pick_checkout(kind, cx);
                        }))
                        .child(
                            crate::icons::icon(icon_path)
                                .size(px(14.0))
                                .text_color(theme.text_muted),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(SharedString::from(label)),
                        )
                    }),
            )
            .into_any_element()
    }

    /// Model picker with favorites and harness tabs above a scoped search.
    /// Existing chats show only their own harness tab and models.
    fn render_harness_model_popover(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let height = self.menu_geometry().height.min(self.open_model_height);
        let (list_height, tray_height) = model_menu_budgets(height, self.setting_groups(cx).len());

        let theme = Theme::of(cx).for_popup();

        // Catalog-level loading/error take over the whole card — the tabs ARE
        // the catalog, so there is nothing stable to draw above the skeleton.
        match &self.harnesses {
            Loadable::Loading | Loadable::Idle => {
                return div()
                    .h(px(list_height))
                    .p(px(8.0))
                    .child(popover::skeleton_menu_rows(
                        "harness-skeleton",
                        &theme,
                        5,
                        cx.entity_id(),
                        cx,
                    ))
                    .into_any_element();
            }
            Loadable::Error(message) => {
                let message = message.clone();
                return div()
                    .h(px(list_height))
                    .p(px(8.0))
                    .child(self.retry_row(
                        "harness-retry",
                        &message,
                        PickerKind::HarnessModel,
                        &theme,
                        cx,
                    ))
                    .into_any_element();
            }
            Loadable::Ready(_) => {}
        }

        let effective = self.effective_harness(cx);
        let model_scroll = self.model_scroll.clone();
        let query = self.search.read(cx).text().trim().to_string();
        let searching = !query.is_empty();
        let favorites_view = self.model_rail == ModelRail::Favorites;
        let descriptors = self.rail_descriptors(cx);
        // No-agents empty state: the catalog loaded but offers nothing
        // runnable (every enabled harness is missing its CLI, or nothing is
        // enabled) and there's no committed chat harness to force-include —
        // guidance instead of an empty tab row.
        if descriptors.is_empty() {
            return div()
                .p(px(16.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.0))
                .child(
                    crate::icons::icon(crate::icons::TERMINAL)
                        .size(px(20.0))
                        .text_color(theme.text_muted),
                )
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(13.0))
                        .text_color(theme.text)
                        .child(SharedString::from("No agents available")),
                )
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted)
                        .text_center()
                        .child(SharedString::from(
                            "Enable an installed agent in Settings → Providers, \
                             or install an agent CLI.",
                        )),
                )
                .into_any_element();
        }
        let rows = self.model_rows(cx);

        // ── tabs: the favorites star, then one brand icon per harness —
        //    ACROSS THE TOP (user request; was a left rail). The
        //    viewed tab wears a 2px accent bar sitting on the row's bottom
        //    hairline. Tabs never hide: a live search only filters the
        //    viewed tab's list, so switching tabs re-scopes the same query.
        let mut tabs = div()
            .flex_none()
            .h(px(40.0))
            .px(px(popover::CARD_INSET))
            .border_b_1()
            .border_color(crate::theme::hairline(0.08))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0));
        tabs = tabs.child(
            div()
                .id("model-tab-favorites")
                .relative()
                .w(px(32.0))
                .h(px(32.0))
                .rounded(px(popover::MENU_ITEM_RADIUS))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(!favorites_view, |el| {
                    el.hover(|s| s.bg(crate::theme::ink(0.06)))
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.setting_menu = None;
                    this.model_rail = ModelRail::Favorites;
                    // Anchor on the selected row when it's starred, else
                    // the top — never a stray second highlight.
                    this.active = this.selected_model_index(cx);
                    this.model_scroll_base().set_offset(gpui::Point::default());
                    this.model_scroll
                        .scroll_to_item(this.active, gpui::ScrollStrategy::Nearest);
                    cx.notify();
                }))
                .child(
                    crate::icons::icon(crate::icons::STAR_BOLD)
                        .size(px(15.0))
                        .text_color(if favorites_view {
                            theme.text
                        } else {
                            theme.text_muted
                        }),
                )
                .when(favorites_view, |el| el.child(tab_indicator(theme.accent))),
        );
        for (ix, descriptor) in descriptors.iter().enumerate() {
            let harness = descriptor.id;
            let is_viewed = !favorites_view && effective == Some(harness);
            let (icon_path, tint) = harness_brand_icon(harness);
            tabs =
                tabs.child(
                    div()
                        .id(("harness-tab", ix))
                        .relative()
                        .w(px(32.0))
                        .h(px(32.0))
                        .rounded(px(popover::MENU_ITEM_RADIUS))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .when(!is_viewed, |el| el.hover(|s| s.bg(crate::theme::ink(0.06))))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.setting_menu = None;
                            this.model_rail = ModelRail::Harness;
                            this.pick_harness(harness, cx);
                            cx.notify();
                        }))
                        .child(crate::icons::icon(icon_path).size(px(16.0)).text_color(
                            tint.unwrap_or(if is_viewed {
                                theme.text
                            } else {
                                theme.text_muted
                            }),
                        ))
                        .when(is_viewed, |el| el.child(tab_indicator(theme.accent))),
                );
        }

        // ── search row: icon + borderless input over a full-bleed hairline.
        //    The placeholder names the scope — the query never leaves the
        //    viewed tab (user request; the old global search hid the rail).
        let search_row = div()
            .flex_none()
            .h(px(40.0))
            .px(px(10.0))
            .border_b_1()
            .border_color(crate::theme::hairline(0.08))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                crate::icons::icon(crate::icons::MAGNIFER)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(crate::typography::ui_rems(13.0))
                    .child(self.search.clone()),
            );

        // ── model rows: a VIRTUALIZED uniform list — only the visible slice
        //    renders, so a 7k-model catalog scrolls as smoothly as seven
        //    (field report: the un-virtualized stack was the picker's lag).
        //    Keyboard nav scrolls via the UniformListScrollHandle.
        let effective_models = effective.and_then(|h| self.models.get(&h));
        let model_list: Option<AnyElement> = if !rows.is_empty() {
            let entity = cx.entity();
            let row_data = rows.clone();
            Some(
                gpui::uniform_list(
                    "model-menu-scroll",
                    rows.len(),
                    move |range, _window, app| {
                        entity.update(app, |this, cx| {
                            range
                                .filter_map(|ix| {
                                    row_data
                                        .get(ix)
                                        .map(|row| this.render_model_row(ix, row, cx))
                                })
                                .collect::<Vec<AnyElement>>()
                        })
                    },
                )
                .size_full()
                .px(px(popover::CARD_INSET))
                .track_scroll(&model_scroll)
                .into_any_element(),
            )
        } else {
            None
        };
        let list_children: Vec<AnyElement> = if !rows.is_empty() {
            Vec::new()
        } else if searching {
            vec![empty_list_note(&theme, "No models found")]
        } else if favorites_view {
            vec![empty_list_note(
                &theme,
                "No starred models yet — hit a row's star",
            )]
        } else {
            match effective_models {
                Some(Loadable::Error(message)) => {
                    let message = message.clone();
                    vec![self.retry_row(
                        "model-retry",
                        &message,
                        PickerKind::HarnessModel,
                        &theme,
                        cx,
                    )]
                }
                _ => vec![popover::skeleton_menu_rows(
                    "model-skeleton",
                    &theme,
                    5,
                    cx.entity_id(),
                    cx,
                )],
            }
        };

        let refresh_error = effective
            .and_then(|harness| self.model_refresh_errors.get(&harness))
            .filter(|_| !rows.is_empty())
            .cloned()
            .map(|message| {
                self.retry_row(
                    "model-refresh-retry",
                    &message,
                    PickerKind::HarnessModel,
                    &theme,
                    cx,
                )
            });

        let model_scrollbar = popover::rail(self, "model-scrollbar", &theme, cx);
        let list_host = div()
            .id("model-list-scroll-host")
            .relative()
            .flex_none()
            .h(px(list_height))
            .py(px(popover::CARD_INSET))
            // A whisper of wash keeps the scrolling band readable between
            // the pinned chrome above and the traits tray below.
            .bg(crate::theme::ink(0.02))
            .on_hover(cx.listener(Self::on_menu_list_hover))
            .child(match model_list {
                Some(list) => {
                    popover::faded_menu_list(&self.model_scroll_base(), list).into_any_element()
                }
                // Empty/loading/error notes: a plain static stack.
                None => div()
                    .id("model-menu-scroll")
                    .size_full()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .px(px(popover::CARD_INSET))
                    .children(list_children)
                    .into_any_element(),
            })
            // Absolute child: the hit rail and thumb float above the
            // scroll content without consuming any list width.
            .children(model_scrollbar);

        // ── traits tray: the reasoning ladder + model options PINNED under
        //    the list (the separate Traits popover folded in here — user
        //    request). Hidden entirely when the selected model has neither.
        let has_tray = self.title.is_none()
            && (!self.trait_ladder(cx).is_empty()
                || self
                    .selected_model(cx)
                    .is_some_and(|m| !m.options.is_empty()));
        let tray: Option<AnyElement> = has_tray.then(|| {
            let sections = self.render_traits_sections(cx);
            div()
                .id("model-traits-tray")
                .flex_none()
                .border_t_1()
                .border_color(crate::theme::hairline(0.08))
                // Long option stacks scroll inside the tray rather than
                // growing the card past the viewport.
                .h(px(tray_height))
                .overflow_y_scroll()
                .px(px(popover::CARD_INSET))
                .child(sections)
                .into_any_element()
        });

        div()
            .flex()
            .flex_col()
            .child(tabs)
            .child(search_row)
            .children(refresh_error)
            .child(list_host)
            .children(tray)
            .into_any_element()
    }

    /// One model row for the virtualized list. `ix` is the row's GLOBAL index
    /// (⌘N chips, hover-cursor, and activation all key on it). The 2px
    /// inter-row gap is baked into each item's bottom padding so every item
    /// is the same height (uniform_list measures the first).
    fn render_model_row(
        &mut self,
        ix: usize,
        row: &ModelRowData,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let effective = self.effective_harness(cx);
        let is_selected = Some(row.harness) == effective
            && self
                .effective_model_id(cx)
                .or_else(|| self.selected_model(cx).map(|m| m.id.as_str()))
                == Some(row.model.id.as_str());
        let is_active = ix == self.active;
        let is_fav = self.defaults.is_favorite(row.harness, &row.model.id);
        let (icon_path, tint) = harness_brand_icon(row.harness);
        let label: SharedString = row.model.label.clone().into();
        let harness_name = row.harness_name.clone();
        let harness = row.harness;
        let star_model = row.model.id.clone();
        // Rows show only the model name. The description appears only to
        // tell identically-named rows apart (field report: "GLM-5.2" exists
        // under 64 connected opencode providers, whose driver ships the
        // provider name in `description`).
        let attribution: Option<SharedString> = row
            .model
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| {
                row.ambiguous && !d.is_empty() && !d.eq_ignore_ascii_case(harness_name.as_ref())
            })
            .map(|d| SharedString::from(d.to_owned()));
        let compact = self.model_rail == ModelRail::Harness;
        let mut el = div()
            .id(("model-row", ix))
            .px(px(8.0))
            .py(px(if compact { 5.0 } else { 6.0 }))
            .rounded(px(popover::MENU_ITEM_RADIUS))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .cursor_pointer();
        // ONE moving highlight (t3/Base-UI combobox): hovering moves the
        // keyboard cursor instead of painting its own wash, so hover + arrow
        // cursor can never wear two washes at once. Selection is the
        // distinct stronger treatment (wash + ring).
        if is_selected {
            el = el
                .bg(crate::theme::card_selected_bg())
                .shadow(crate::theme::card_selected_shadows());
        } else if is_active {
            el = el.bg(crate::theme::ink(0.05));
        }
        el = el.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            if *hovered && this.active != ix {
                this.active = ix;
                cx.notify();
            }
        }));
        // Compact single-line rows on a harness tab (user request): every
        // row there shares the tab's harness, so the identity subline is
        // dead weight — attribution rides inline instead (opencode ships
        // identically-named models under 64 providers; it must stay
        // visible). The favorites tab mixes harnesses and keeps the
        // two-line layout with the brand subline.
        let body: AnyElement = if compact {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .flex_none()
                        .max_w_full()
                        .truncate()
                        .text_size(crate::typography::ui_rems(12.5))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(label),
                )
                .when_some(attribution, |el, attribution| {
                    el.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_muted)
                            .child(attribution),
                    )
                })
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .w_full()
                        .truncate()
                        .text_size(crate::typography::ui_rems(12.5))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(label),
                )
                .child(
                    // Harness identity subline (t3 `showProvider`), plus
                    // the model's own attribution when it carries one.
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            crate::icons::icon(icon_path)
                                .size(px(11.0))
                                .flex_none()
                                .text_color(tint.unwrap_or(theme.text_muted)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_muted)
                                .child(harness_name),
                        )
                        .when_some(attribution, |el, attribution| {
                            el.child(
                                div()
                                    .flex_none()
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from("·")),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .text_color(theme.text_muted)
                                    .child(attribution),
                            )
                        }),
                )
                .into_any_element()
        };
        el = el
            .on_click(cx.listener(move |this, _, _, cx| {
                this.activate_model_index(ix, cx);
            }))
            .child(body);
        if ix < 9 {
            el = el.child(popover::kbd_hint(&theme, &format!("⌘{}", ix + 1)));
        }
        // A model configured in place opens its settings card on hover.
        // The title picker has no settings, so no card either.
        let configurable =
            self.title.is_none() && configured_in_place(&row.model) && !row.selected_only;
        let card_open = configurable && self.config_row == Some(ix);
        if configurable {
            let entity = cx.entity().downgrade();
            let exit_entity = cx.entity().downgrade();
            el = el.relative().child(
                gpui::canvas(
                    move |bounds, window, cx| {
                        let left = bounds.right() + px(CONFIG_CARD_WIDTH + 12.0)
                            > window.viewport_size().width;
                        let _ = entity.update(cx, |this, cx| {
                            if this.config_on_left != left {
                                this.config_on_left = left;
                                cx.notify();
                            }
                        });
                    },
                    move |trigger, _, window, _| {
                        if !card_open {
                            return;
                        }
                        window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                            if phase != gpui::DispatchPhase::Bubble {
                                return;
                            }
                            let _ = exit_entity.update(cx, |this, cx| {
                                if this.config_row != Some(ix) {
                                    return;
                                }
                                let pointer = event.position;
                                // An open card choice list — including the cone
                                // from its trigger row toward it — is inside the card.
                                let in_choices = this.setting_scope == SettingScope::Card
                                    && this.setting_menu.is_some()
                                    && this.setting_hover.contains_pointer(
                                        gpui::Bounds::default(),
                                        this.setting_bounds,
                                        pointer,
                                        this.setting_on_left,
                                    );
                                if this.config_bounds.is_some_and(|b| b.contains(&pointer)) {
                                    this.config_grace = None;
                                }
                                let in_grace =
                                    this.config_grace.is_some_and(|b| b.contains(&pointer));
                                if in_choices
                                    || in_grace
                                    || this.config_hover.contains_pointer(
                                        trigger,
                                        this.config_bounds,
                                        pointer,
                                        this.config_on_left,
                                    )
                                {
                                    return;
                                }
                                this.close_config();
                                cx.notify();
                            });
                        });
                    },
                )
                .absolute()
                .inset_0(),
            );
            if card_open {
                let card = self.render_config_card(cx);
                el = el.child(popover::nested_menu(
                    format!("model-config-{ix}"),
                    card,
                    self.config_on_left,
                ));
            }
        }
        el = el.child(
            div()
                .id(("model-star", ix))
                .flex_none()
                .w(px(22.0))
                .h(px(22.0))
                .rounded(px(popover::MENU_ITEM_RADIUS))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(crate::theme::ink(0.08)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_model_favorite(harness, &star_model, cx);
                }))
                .child(
                    crate::icons::icon(if is_fav {
                        crate::icons::STAR_BOLD
                    } else {
                        crate::icons::STAR
                    })
                    .size(px(13.0))
                    .text_color(if is_fav {
                        theme.warning
                    } else {
                        theme.text_muted
                    }),
                ),
        );
        if !configurable {
            return div().pb(px(2.0)).child(el).into_any_element();
        }
        // Hover intent on a stable wrapper, as for the setting triggers.
        div()
            .id(("model-config-hover", ix))
            .pb(px(2.0))
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                if *hovered {
                    let pointer = window.mouse_position();
                    let action = this.config_hover.enter(
                        this.config_row.as_ref(),
                        &ix,
                        pointer,
                        this.config_bounds,
                        this.config_on_left,
                    );
                    this.apply_config_hover(action, ix, pointer, cx);
                } else {
                    this.config_hover.leave(&ix);
                }
            }))
            .on_mouse_move(
                cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                    let action = this.config_hover.moved(
                        this.config_row.as_ref(),
                        &ix,
                        event.position,
                        this.config_bounds,
                        this.config_on_left,
                    );
                    this.apply_config_hover(action, ix, event.position, cx);
                }),
            )
            .child(el)
            .into_any_element()
    }

    /// The hover card of a model configured in place: its setting triggers
    /// (each with nested choices) and its switches.
    fn render_config_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let Some(row) = self.card_row(cx) else {
            return div().into_any_element();
        };
        let settings = self.render_setting_rows(SettingScope::Card, cx);
        let toggles: Vec<AnyElement> = self
            .card_groups(cx)
            .into_iter()
            .filter(is_toggle)
            .enumerate()
            .map(|(i, group)| {
                let on = group.choices.iter().any(|c| c.selected && !c.default);
                let label = SharedString::from(group.label.clone());
                popover::menu_row_nav(&theme, false, false, format!("card-toggle-{i}"))
                    .id(("card-toggle", i))
                    .h(px(30.0))
                    .py(px(0.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_card_option(group.clone(), cx);
                        cx.stop_propagation();
                    }))
                    .child(div().flex_1().child(label))
                    .child(crate::settings::widgets::toggle_switch(
                        &theme,
                        on,
                        format!("model-card-{i}"),
                    ))
                    .into_any_element()
            })
            .collect();
        let entity = cx.entity().downgrade();
        popover::popover_card(&theme)
            .w(px(CONFIG_CARD_WIDTH))
            .relative()
            .child(
                div()
                    .px(px(8.0))
                    .pt(px(6.0))
                    .text_size(crate::typography::ui_rems(12.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(SharedString::from(row.model.label.clone())),
            )
            .child(settings)
            .when(!toggles.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .pb(px(popover::CARD_INSET))
                        .children(toggles),
                )
            })
            .child(
                gpui::canvas(
                    move |bounds, _, cx| {
                        let _ = entity.update(cx, |this, _| this.config_bounds = Some(bounds));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .into_any_element()
    }

    /// The tray's settings. A model configured in place keeps them in its
    /// hover card only.
    fn setting_groups(&self, cx: &App) -> Vec<SettingGroup> {
        // Titles always run at minimal reasoning with no model options.
        if self.title.is_some() || self.selected_model(cx).is_some_and(configured_in_place) {
            return Vec::new();
        }
        self.live_groups(cx)
    }

    /// The selected model's settings with the current picks.
    fn live_groups(&self, cx: &App) -> Vec<SettingGroup> {
        build_setting_groups(
            self.trait_ladder(cx),
            self.effective_reasoning(cx),
            self.selected_model(cx),
            &self.explicit_options(cx),
        )
    }

    fn scope_groups(&self, scope: SettingScope, cx: &App) -> Vec<SettingGroup> {
        match scope {
            SettingScope::Tray => self.setting_groups(cx),
            SettingScope::Card => self.card_groups(cx),
        }
    }

    /// The row whose settings card is open, while it still lists a model
    /// configured in place.
    fn card_row(&self, cx: &App) -> Option<ModelRowData> {
        self.config_row
            .and_then(|ix| self.model_rows(cx).get(ix).cloned())
            .filter(|row| {
                self.title.is_none() && !row.selected_only && configured_in_place(&row.model)
            })
    }

    fn row_is_selected(&self, row: &ModelRowData, cx: &App) -> bool {
        Some(row.harness) == self.effective_harness(cx)
            && self
                .effective_model_id(cx)
                .or_else(|| self.selected_model(cx).map(|m| m.id.as_str()))
                == Some(row.model.id.as_str())
    }

    /// The card's settings: the live ones when its model is the selected
    /// one, otherwise what picking it would start from (the remembered
    /// options, the current effort clamped to its ladder).
    fn card_groups(&self, cx: &App) -> Vec<SettingGroup> {
        let Some(row) = self.card_row(cx) else {
            return Vec::new();
        };
        if self.row_is_selected(&row, cx) {
            return card_order(self.live_groups(cx));
        }
        let levels = if row.model.reasoning_levels.is_empty() {
            self.harnesses
                .ready()
                .and_then(|list| list.iter().find(|d| d.id == row.harness))
                .map(|d| d.reasoning_levels.clone())
                .unwrap_or_default()
        } else {
            row.model.reasoning_levels.clone()
        };
        let reasoning = clamp_reasoning(self.effective_reasoning(cx), &levels);
        let selections = offered_options(
            &row.model,
            self.defaults
                .model_options_for(row.harness, &row.model.id)
                .cloned()
                .unwrap_or_default(),
        );
        card_order(build_setting_groups(
            levels,
            reasoning,
            Some(&row.model),
            &selections,
        ))
    }

    /// A choice made in the card selects its model first.
    fn select_card_model(&mut self, cx: &mut Context<Self>) {
        if let Some(ix) = self.config_row
            && let Some(row) = self.card_row(cx)
            && !self.row_is_selected(&row, cx)
        {
            self.activate_model_index(ix, cx);
        }
    }

    fn open_config(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.config_hover.cancel();
        self.config_hover.reset();
        if self.setting_scope == SettingScope::Card {
            self.setting_menu = None;
            self.setting_bounds = None;
        }
        self.config_row = Some(ix);
        self.config_bounds = None;
        cx.notify();
    }

    fn close_config(&mut self) {
        self.config_hover.cancel();
        self.config_hover.reset();
        self.config_row = None;
        self.config_bounds = None;
        self.config_grace = None;
        if self.setting_scope == SettingScope::Card {
            self.setting_menu = None;
            self.setting_bounds = None;
        }
    }

    fn apply_config_hover(
        &mut self,
        action: popover::HoverAction,
        ix: usize,
        pointer: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        match action {
            popover::HoverAction::None => {}
            popover::HoverAction::Open => {
                self.open_config(ix, cx);
                self.config_hover.record_origin(pointer);
            }
            popover::HoverAction::Defer => {
                let source = self.config_row;
                self.config_hover.defer(cx, move |this, cx| {
                    if this.is_open()
                        && this.config_row == source
                        && this.config_hover.pending() == Some(&ix)
                    {
                        this.open_config(ix, cx);
                        this.config_hover.record_origin(pointer);
                    }
                });
            }
        }
    }

    fn cancel_setting_hover(&mut self) {
        self.setting_hover.cancel();
    }

    fn hover_setting(
        &mut self,
        scope: SettingScope,
        id: ModelSetting,
        index: usize,
        pointer: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .setting_menu
            .as_ref()
            .filter(|_| self.setting_scope == scope);
        let action = self.setting_hover.enter(
            current,
            &id,
            pointer,
            self.setting_bounds,
            self.setting_on_left,
        );
        self.apply_setting_hover(scope, action, id, index, pointer, cx);
    }

    fn apply_setting_hover(
        &mut self,
        scope: SettingScope,
        action: popover::HoverAction,
        id: ModelSetting,
        index: usize,
        pointer: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        match action {
            popover::HoverAction::None => {}
            popover::HoverAction::Open => {
                if scope == SettingScope::Tray {
                    self.active = index;
                }
                self.open_setting(scope, id, cx);
                self.setting_hover.record_origin(pointer);
            }
            popover::HoverAction::Defer => {
                let source = self.setting_menu.clone();
                self.setting_hover.defer(cx, move |this, cx| {
                    if this.is_open()
                        && this.setting_menu == source
                        && this.setting_hover.pending() == Some(&id)
                    {
                        if scope == SettingScope::Tray {
                            this.active = index;
                        }
                        this.open_setting(scope, id, cx);
                        this.setting_hover.record_origin(pointer);
                    }
                });
            }
        }
    }

    fn open_setting(&mut self, scope: SettingScope, id: ModelSetting, cx: &mut Context<Self>) {
        self.cancel_setting_hover();
        self.setting_hover.reset();
        self.setting_active = self
            .scope_groups(scope, cx)
            .iter()
            .find(|g| g.id == id)
            .and_then(|g| g.choices.iter().position(|c| c.selected))
            .unwrap_or(0);
        self.setting_scope = scope;
        self.setting_menu = Some(id);
        self.setting_bounds = None;
        self.setting_scroll = gpui::ScrollHandle::new();
        self.setting_scroll.scroll_to_item(self.setting_active);
        cx.notify();
    }

    fn activate_setting_choice(&mut self, cx: &mut Context<Self>) {
        let scope = self.setting_scope;
        let Some(group) = self
            .scope_groups(scope, cx)
            .into_iter()
            .find(|g| Some(&g.id) == self.setting_menu.as_ref())
        else {
            return;
        };
        let Some(choice) = group.choices.get(self.setting_active) else {
            return;
        };
        if scope == SettingScope::Card {
            // Keep the card: the pointer rests where this list was.
            self.config_grace = self.setting_bounds;
            self.select_card_model(cx);
        }
        match group.id {
            ModelSetting::Reasoning => {
                if let Some(level) = choice.reasoning {
                    self.pick_reasoning(level, cx);
                }
            }
            ModelSetting::Option(id) => {
                self.pick_option(id, choice.value.clone(), choice.default, cx)
            }
        }
        self.setting_menu = None;
        self.setting_bounds = None;
        cx.notify();
    }

    /// Flip a card switch (Fast Mode): selects the card's model first.
    fn toggle_card_option(&mut self, group: SettingGroup, cx: &mut Context<Self>) {
        let ModelSetting::Option(id) = group.id else {
            return;
        };
        let Some(next) = group.choices.into_iter().find(|c| !c.selected) else {
            return;
        };
        self.select_card_model(cx);
        self.pick_option(id, next.value, next.default, cx);
    }

    /// Each model setting gets a compact trigger and its own nested choices.
    fn render_traits_sections(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.render_setting_rows(SettingScope::Tray, cx)
    }

    /// Setting triggers for the tray or a hover card. Card rows stay out of
    /// the keyboard cursor, and its switches render separately.
    fn render_setting_rows(&mut self, scope: SettingScope, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let tray = scope == SettingScope::Tray;
        let base_index = if tray {
            self.model_rows_len(cx)
        } else {
            NO_ACTIVE_ROW / 2
        };
        let (row_id, hover_id, choice_id, key) = if tray {
            (
                "model-setting",
                "model-setting-hover",
                "setting-choice",
                "model",
            )
        } else {
            (
                "card-setting",
                "card-setting-hover",
                "card-setting-choice",
                "card",
            )
        };
        let mut rows = Vec::new();
        let groups = self
            .scope_groups(scope, cx)
            .into_iter()
            .filter(|g| tray || !is_toggle(g));
        for (ix, group) in groups.enumerate() {
            let open = self.setting_scope == scope && self.setting_menu.as_ref() == Some(&group.id);
            let value = group
                .choices
                .iter()
                .find(|c| c.selected)
                .map(|c| c.label.clone())
                .unwrap_or_default();
            let id = group.id.clone();
            let outside_id = id.clone();
            let move_id = id.clone();
            let exit_id = id.clone();
            let exit_entity = cx.entity().downgrade();
            let entity = cx.entity().downgrade();
            let mut row = popover::menu_row(
                &theme,
                open || (tray && self.active == base_index + ix),
                format!("{key}-setting-{ix}"),
            )
            .id((row_id, ix))
            .relative()
            .h(px(30.0))
            .py(px(0.0))
            .on_click(cx.listener(move |this, _, window, cx| {
                if tray {
                    this.active = base_index + ix;
                }
                this.cancel_setting_hover();
                this.setting_menu = None;
                this.setting_bounds = None;
                window.focus(&this.focus, cx);
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_down_out(
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    // The trigger dismisses its own child. Elsewhere in the parent,
                    // close this child during capture and let that control receive
                    // the same click. Clicks inside the floating child stay local.
                    if this.setting_scope == scope
                        && this.setting_menu.as_ref() == Some(&outside_id)
                        && !this
                            .setting_bounds
                            .is_some_and(|bounds| bounds.contains(&event.position))
                    {
                        this.cancel_setting_hover();
                        this.setting_menu = None;
                        this.setting_bounds = None;
                        cx.notify();
                    }
                }),
            )
            .child(
                gpui::canvas(
                    move |bounds, window, cx| {
                        let left = bounds.right() + px(244.0) > window.viewport_size().width;
                        let _ = entity.update(cx, |this, cx| {
                            if this.setting_on_left != left {
                                this.setting_on_left = left;
                                cx.notify();
                            }
                        });
                    },
                    move |trigger, _, window, _| {
                        if !open {
                            return;
                        }
                        window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                            if phase != gpui::DispatchPhase::Bubble {
                                return;
                            }
                            let _ = exit_entity.update(cx, |this, cx| {
                                if this.setting_scope != scope
                                    || this.setting_menu.as_ref() != Some(&exit_id)
                                {
                                    return;
                                }
                                if this.setting_hover.contains_pointer(
                                    trigger,
                                    this.setting_bounds,
                                    event.position,
                                    this.setting_on_left,
                                ) {
                                    return;
                                }
                                this.cancel_setting_hover();
                                this.setting_menu = None;
                                this.setting_bounds = None;
                                this.setting_hover.reset();
                                // Do not leave a keyboard-style selection on the
                                // trigger after pointer navigation dismisses it.
                                if tray {
                                    this.active = 0;
                                }
                                cx.notify();
                            });
                        });
                    },
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(group.label.clone())),
            )
            .child(
                div()
                    .max_w(px(100.0))
                    .truncate()
                    .text_color(theme.text_muted)
                    .child(SharedString::from(value)),
            )
            .child(
                crate::icons::icon(crate::icons::ALT_ARROW_RIGHT)
                    .size(px(12.0))
                    .text_color(theme.text_muted),
            );
            if open {
                let entity = cx.entity().downgrade();
                let menu = popover::popover_card(&theme)
                    .w(px(232.0))
                    .relative()
                    .child(
                        div()
                            .id("model-setting-choices")
                            .max_h(px(240.0))
                            .overflow_y_scroll()
                            .track_scroll(&self.setting_scroll)
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .children(group.choices.into_iter().enumerate().map(
                                |(choice_ix, choice)| {
                                    popover::menu_row(
                                        &theme,
                                        choice_ix == self.setting_active,
                                        format!("{key}-setting-choice-{ix}-{choice_ix}"),
                                    )
                                    .id((choice_id, choice_ix))
                                    .h(px(30.0))
                                    .py(px(0.0))
                                    .flex_none()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.setting_active = choice_ix;
                                        this.activate_setting_choice(cx);
                                        cx.stop_propagation();
                                    }))
                                    .child(SharedString::from(choice.label))
                                    .child(div().flex_1())
                                    .when(choice.default, |el| el.child(default_badge(&theme)))
                                    .when(
                                        choice.selected,
                                        |el| {
                                            el.child(
                                                crate::icons::icon(crate::icons::CHECK)
                                                    .size(px(14.0))
                                                    .text_color(theme.text),
                                            )
                                        },
                                    )
                                },
                            )),
                    )
                    .child(
                        gpui::canvas(
                            move |bounds, _, cx| {
                                let _ =
                                    entity.update(cx, |this, _| this.setting_bounds = Some(bounds));
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    );
                row = row.child(popover::nested_menu(
                    format!("{key}-setting-menu-{ix}"),
                    menu.into_any_element(),
                    self.setting_on_left,
                ));
            }
            // Keep hover ownership on a stable wrapper: menu_row already
            // owns its hover animation, and changing the open row's styling
            // must not reopen a child just dismissed by clicking its trigger.
            rows.push(
                div()
                    .id((hover_id, ix))
                    .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                        if *hovered {
                            this.hover_setting(
                                scope,
                                id.clone(),
                                base_index + ix,
                                window.mouse_position(),
                                cx,
                            );
                        } else {
                            this.setting_hover.leave(&id);
                        }
                    }))
                    .on_mouse_move(
                        cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                            let current = this
                                .setting_menu
                                .as_ref()
                                .filter(|_| this.setting_scope == scope);
                            let action = this.setting_hover.moved(
                                current,
                                &move_id,
                                event.position,
                                this.setting_bounds,
                                this.setting_on_left,
                            );
                            this.apply_setting_hover(
                                scope,
                                action,
                                move_id.clone(),
                                base_index + ix,
                                event.position,
                                cx,
                            );
                        }),
                    )
                    .child(row),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .py(px(popover::CARD_INSET))
            .children(rows)
            .into_any_element()
    }
}

/// The floating-scrollbar treatment for every picker list
/// ([`popover::rail`] folds the note/hide/metrics/render + pointer listeners
/// into one call): one shared rail state, fed by whichever handle
/// [`Pickers::active_menu_scroll`] resolves for the mounted menu.
impl popover::ScrollRailHost for Pickers {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        &mut self.menu_bar
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        Some(self.active_menu_scroll())
    }
}

/// The "Default" marker beside a section's default choice: a ghost badge —
/// bare muted text, no border or fill (user request; t3code draws an outline
/// pill here).
/// Reasoning plus each offered option, with the current picks marked.
fn build_setting_groups(
    levels: Vec<ReasoningLevel>,
    reasoning: Option<ReasoningLevel>,
    model: Option<&Model>,
    selections: &serde_json::Map<String, serde_json::Value>,
) -> Vec<SettingGroup> {
    let mut groups = Vec::new();
    if !levels.is_empty() {
        let default = default_reasoning(&levels);
        groups.push(SettingGroup {
            id: ModelSetting::Reasoning,
            label: "Reasoning".into(),
            choices: levels
                .into_iter()
                .map(|level| SettingChoice {
                    label: reasoning_label(level).into(),
                    value: String::new(),
                    reasoning: Some(level),
                    selected: reasoning == Some(level),
                    default: default == Some(level),
                })
                .collect(),
        });
    }
    for option in model.map(|m| m.options.as_slice()).unwrap_or_default() {
        if option.choices.is_empty() {
            continue;
        }
        let selected = selections
            .get(&option.id)
            .and_then(|v| v.as_str())
            .unwrap_or(&option.default_choice);
        groups.push(SettingGroup {
            id: ModelSetting::Option(option.id.clone()),
            label: option.label.clone(),
            choices: option
                .choices
                .iter()
                .map(|choice| SettingChoice {
                    label: choice.label.clone(),
                    value: choice.id.clone(),
                    reasoning: None,
                    selected: selected == choice.id,
                    default: option.default_choice == choice.id,
                })
                .collect(),
        });
    }
    groups
}

fn default_badge(theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .text_size(crate::typography::ui_rems(10.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.for_popup().text_muted)
        .child(SharedString::from("Default"))
}

/// Brand mark + optional tint for a harness (the Claude mark keeps its brand
/// orange even on the monochrome surface; the mock harness scripts
/// Claude-flavoured runs, so it wears the Claude mark).
/// The 2px underline marking the viewed top tab: sits on the tab row's
/// bottom hairline (the tab is 32px tall inside a 40px row, so -4px lands
/// exactly on the border), rounded like a capsule.
fn tab_indicator(tint: gpui::Hsla) -> gpui::Div {
    div()
        .absolute()
        .bottom(px(-4.0))
        .left(px(6.0))
        .right(px(6.0))
        .h(px(2.0))
        .rounded(px(1.0))
        .bg(tint)
}

/// Flatten the picker's visible rows for one tab. The QUERY NEVER LEAVES THE
/// VIEWED TAB (user request; the old global search spanned every harness and
/// hid the rail): on a harness tab it ranks that harness's models only, on
/// the favorites tab it ranks the starred set. Without a query, a harness
/// tab lists its catalog stars-first and the favorites tab lists every star.
fn scoped_model_rows<'a>(
    query: &str,
    rail: ModelRail,
    effective: Option<HarnessId>,
    descriptors: &[HarnessDescriptor],
    models_for: impl Fn(HarnessId) -> Option<&'a [Model]>,
    is_favorite: impl Fn(HarnessId, &str) -> bool,
) -> Vec<ModelRowData> {
    let row = |descriptor: &HarnessDescriptor, model: &Model| ModelRowData {
        harness: descriptor.id,
        harness_name: SharedString::from(descriptor.name.clone()),
        model: model.clone(),
        selected_only: false,
        ambiguous: false,
    };
    let in_scope = |descriptor: &HarnessDescriptor, model: &Model| match rail {
        ModelRail::Favorites => is_favorite(descriptor.id, &model.id),
        ModelRail::Harness => Some(descriptor.id) == effective,
    };
    if !query.is_empty() {
        // Rank: label prefix < label substring < description hit; stars,
        // then input order, break ties (t3 modelPickerSearch's field ladder
        // + favorite boost, collapsed to our ranks). The description stays
        // in the haystack — opencode's provider attribution ("anthropic")
        // must find its models even inside one tab.
        let mut ranked: Vec<(usize, usize, usize, ModelRowData)> = Vec::new();
        let mut input_ix = 0usize;
        for descriptor in descriptors {
            let Some(models) = models_for(descriptor.id) else {
                continue;
            };
            for model in models {
                if !in_scope(descriptor, model) {
                    continue;
                }
                let by_label = popover::match_rank(query, &model.label);
                let by_description = popover::match_rank(
                    query,
                    &format!(
                        "{} {}",
                        model.description.as_deref().unwrap_or(""),
                        model.label
                    ),
                )
                .map(|rank| rank + 2);
                if let Some(rank) = by_label.into_iter().chain(by_description).min() {
                    let starred = !is_favorite(descriptor.id, &model.id);
                    ranked.push((rank, starred as usize, input_ix, row(descriptor, model)));
                }
                input_ix += 1;
            }
        }
        ranked.sort_by_key(|(rank, unstarred, ix, _)| (*rank, *unstarred, *ix));
        return ranked.into_iter().map(|(_, _, _, row)| row).collect();
    }
    match rail {
        ModelRail::Favorites => {
            let mut rows = Vec::new();
            for descriptor in descriptors {
                let Some(models) = models_for(descriptor.id) else {
                    continue;
                };
                for model in models {
                    if is_favorite(descriptor.id, &model.id) {
                        rows.push(row(descriptor, model));
                    }
                }
            }
            rows
        }
        ModelRail::Harness => {
            let Some(descriptor) = descriptors.iter().find(|d| Some(d.id) == effective) else {
                return Vec::new();
            };
            let Some(models) = models_for(descriptor.id) else {
                return Vec::new();
            };
            let (starred, rest): (Vec<&Model>, Vec<&Model>) = models
                .iter()
                .partition(|m| is_favorite(descriptor.id, &m.id));
            starred
                .into_iter()
                .chain(rest)
                .map(|model| row(descriptor, model))
                .collect()
        }
    }
}

/// Centered muted note filling an empty model list ("No models found").
fn empty_list_note(theme: &Theme, copy: &str) -> AnyElement {
    div()
        .px(px(8.0))
        .py(px(24.0))
        .text_size(crate::typography::ui_rems(12.0))
        .text_color(theme.for_popup().text_muted)
        .text_center()
        .child(SharedString::from(copy.to_string()))
        .into_any_element()
}

/// Display-side model-list hygiene, mirroring the engine's discovery-side
/// fold (`models_from_session`) for catalogs served by OLDER engines (the
/// space's device may run any version): the `default` alias row drops when a
/// real row exists, an orphan `<model>[1m]` variant presents as its base id
/// with the Context Window trait pinned to 1M, and Claude rows adopt the
/// curated catalog's labels so the version number always shows ("Opus 5.5",
/// not the wire's terse "Opus" alias — user request). Idempotent over
/// already-clean lists. The send path recomposes the advertised id from the
/// base + trait (`pick_model_value`), so a folded pick still runs.
pub(crate) fn normalize_model_rows(harness: HarnessId, models: Vec<Model>) -> Vec<Model> {
    fn strip_1m(id: &str) -> Option<&str> {
        id.strip_suffix("[1m]").or_else(|| id.strip_suffix("-1m"))
    }
    fn norm(id: &str) -> String {
        id.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase()
    }
    let catalog = match harness {
        HarnessId::ClaudeCode => zeron_harness::claude::catalog::static_models(),
        _ => Vec::new(),
    };
    // Curated label for an id: exact normalized match, else — for bare
    // alphabetic aliases like `opus` — the first (flagship-ordered) family
    // row. Versioned foreign ids never fuzzy-match.
    let curated_label = |id: &str| -> Option<String> {
        let id_norm = norm(id);
        if let Some(row) = catalog.iter().find(|m| norm(&m.id) == id_norm) {
            return Some(row.label.clone());
        }
        (!id_norm.is_empty() && id_norm.chars().all(|c| c.is_ascii_alphabetic()))
            .then(|| catalog.iter().find(|m| norm(&m.id).contains(&id_norm)))
            .flatten()
            .map(|m| m.label.clone())
    };
    let ids: Vec<String> = models.iter().map(|m| m.id.clone()).collect();
    let has_real = ids.iter().any(|id| !id.eq_ignore_ascii_case("default"));
    models
        .into_iter()
        .filter_map(|mut model| {
            if has_real && model.id.eq_ignore_ascii_case("default") {
                return None;
            }
            if let Some(base) = strip_1m(&model.id.clone()) {
                if ids.iter().any(|other| other == base) {
                    // The bare base is listed too — the engine already gave
                    // it the Context Window trait; the variant row is noise.
                    return None;
                }
                model.id = base.to_string();
                // "Opus (1M context)" → "Opus".
                if let Some(at) = model.label.rfind(" (")
                    && model.label.ends_with(')')
                {
                    model.label.truncate(at);
                    while model.label.ends_with(' ') {
                        model.label.pop();
                    }
                }
                if !model.options.iter().any(|o| o.id == "contextWindow") {
                    model.options.push(zeron_proto::ModelOption {
                        id: "contextWindow".into(),
                        label: "Context Window".into(),
                        choices: vec![
                            zeron_proto::ModelOptionChoice {
                                id: "200k".into(),
                                label: "200K".into(),
                            },
                            zeron_proto::ModelOptionChoice {
                                id: "1m".into(),
                                label: "1M".into(),
                            },
                        ],
                        default_choice: "1m".into(),
                    });
                }
            }
            if let Some(label) = curated_label(&model.id) {
                model.label = label;
            }
            Some(model)
        })
        .collect()
}

pub(crate) fn harness_brand_icon(harness: HarnessId) -> (&'static str, Option<gpui::Hsla>) {
    match harness {
        HarnessId::ClaudeCode | HarnessId::Mock => (
            crate::icons::CLAUDE_MARK,
            Some(crate::icons::claude_brand()),
        ),
        HarnessId::Codex => (crate::icons::OPENAI_MARK, None),
        HarnessId::Cursor => (crate::icons::CURSOR_MARK, None),
        // Cognition's mark (the Devin product icon), monochrome.
        HarnessId::Devin => (crate::icons::DEVIN_MARK, None),
        // Monochrome mark, tinted by the surface like OpenAI's.
        HarnessId::Grok => (crate::icons::GROK_MARK, None),
        // Nous Research's mark (the Hermes product icon), monochrome.
        HarnessId::Hermes => (crate::icons::HERMES_MARK, None),
        HarnessId::Pi => (crate::icons::PI_MARK, None),
        // The pixel-"o" from opencode's wordmark (their favicon), monochrome.
        HarnessId::Opencode => (crate::icons::OPENCODE_MARK, None),
        HarnessId::Antigravity => (crate::icons::ANTIGRAVITY_MARK, None),
    }
}

/// `ZERON_HARNESS=mock` (the e2e/dev rig) opts the mock harness into the UI;
/// production launches never set it, so the mock never surfaces there.
fn mock_harness_enabled() -> bool {
    std::env::var("ZERON_HARNESS")
        .ok()
        .as_deref()
        .map(str::trim)
        == Some("mock")
}

/// Production pickers AND chip resolution hide the mock harness — the
/// registry always lists it, but it must never surface in real UI (neither in
/// the picker rail nor as the eager default the chips resolve against).
/// `ZERON_HARNESS=mock` shows it; otherwise it only remains when it's
/// literally all there is (a dev build with no real harness registered).
pub fn visible_harnesses(list: &[HarnessDescriptor]) -> Vec<HarnessDescriptor> {
    visible_harnesses_impl(list, mock_harness_enabled())
}

fn visible_harnesses_impl(list: &[HarnessDescriptor], allow_mock: bool) -> Vec<HarnessDescriptor> {
    if allow_mock {
        return list.to_vec();
    }
    let real: Vec<HarnessDescriptor> = list
        .iter()
        .filter(|d| d.id != HarnessId::Mock)
        .cloned()
        .collect();
    if real.is_empty() { list.to_vec() } else { real }
}

/// What the composer actually offers: [`visible_harnesses`] narrowed to the
/// catalog device's enabled set AND installed CLIs (Settings → Providers is
/// per-device state, so a space on another device follows THAT device's
/// toggles; a default-enabled agent whose CLI is missing would only
/// manufacture NotInstalled errors at send). The dev-rig mock opt-in
/// survives the filter. There is NO fallback: a catalog where nothing is
/// both enabled and installed offers nothing, and the composer surfaces the
/// no-agents empty state + blocks new sends — resurrecting descriptors that
/// can only fail with NotInstalled is the #128 bug.
pub fn offered_harnesses(list: &[HarnessDescriptor]) -> Vec<HarnessDescriptor> {
    offered_harnesses_impl(list, mock_harness_enabled())
}

fn offered_harnesses_impl(list: &[HarnessDescriptor], allow_mock: bool) -> Vec<HarnessDescriptor> {
    visible_harnesses_impl(list, allow_mock)
        .into_iter()
        .filter(|d| {
            d.installed
                && (zeron_engine::registry::descriptor_enabled(d)
                    || (allow_mock && d.id == HarnessId::Mock))
        })
        .collect()
}

// Tabs and search consume 80px, with 2px reserved for the card border.
// Keep one model row where possible; long option stacks scroll in the tray.
fn model_menu_height(setting_count: usize) -> f32 {
    let tray = if setting_count == 0 {
        0.0
    } else {
        (setting_count as f32 * 32.0 + 7.0).min(236.0)
    };
    82.0 + 216.0 + tray
}

fn model_menu_budgets(height: f32, setting_count: usize) -> (f32, f32) {
    let body = (height - 82.0).max(0.0);
    let desired_tray = if setting_count == 0 {
        0.0
    } else {
        (setting_count as f32 * 32.0 + 7.0).min(236.0)
    };
    let tray = desired_tray.min((body - 30.0).max(0.0));
    // The list absorbs changes in tray height, keeping the card's top edge
    // and search field stationary while choosing models and options.
    (body - tray, tray)
}

/// Attach the (single) open popover above a selector trigger.
fn attach_overlay(
    chip: gpui::Stateful<gpui::Div>,
    overlay: &mut Option<(PickerKind, AnyElement)>,
    kind: PickerKind,
    id: &'static str,
    closing: (Option<std::time::Instant>, bool),
) -> gpui::Stateful<gpui::Div> {
    if overlay.as_ref().is_some_and(|(k, _)| *k == kind)
        && let Some((_, element)) = overlay.take()
    {
        return chip.child(if closing.1 {
            popover::anchored_menu_below(id, element, closing.0)
        } else {
            popover::anchored_menu_above(id, element, closing.0)
        });
    }
    chip
}

/// Attach the (single) open popover below a selector trigger.
fn attach_overlay_below(
    chip: gpui::Stateful<gpui::Div>,
    overlay: &mut Option<(PickerKind, AnyElement)>,
    kind: PickerKind,
    id: &'static str,
    closing: (Option<std::time::Instant>, bool),
) -> gpui::Stateful<gpui::Div> {
    if overlay.as_ref().is_some_and(|(k, _)| *k == kind)
        && let Some((_, element)) = overlay.take()
    {
        return chip.child(popover::anchored_menu_below(id, element, closing.0));
    }
    chip
}

/// Attach the menu ABOVE and RIGHT-ALIGNED to the trigger (t3code
/// `align="end"` — right-edge controls like the model picker open leftward).
fn attach_overlay_end(
    chip: gpui::Stateful<gpui::Div>,
    overlay: &mut Option<(PickerKind, AnyElement)>,
    kind: PickerKind,
    id: &'static str,
    closing: (Option<std::time::Instant>, bool),
) -> gpui::Stateful<gpui::Div> {
    if overlay.as_ref().is_some_and(|(k, _)| *k == kind)
        && let Some((_, element)) = overlay.take()
    {
        return chip.relative().child(if closing.1 {
            popover::anchored_menu_below_end(id, element, closing.0)
        } else {
            popover::anchored_menu_above_end(id, element, closing.0)
        });
    }
    chip
}

impl Render for Pickers {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        // A ZERON_OPEN_PICKER popover never went through `toggle`, so claim
        // its keyboard focus here (re-claim until it sticks — the shell's
        // first-paint fallback focuses the composer after our first render).
        if self.boot_focus_pending {
            match self.open_kind() {
                Some(PickerKind::Branch) => {
                    self.search.update(cx, |input, cx| {
                        input.set_placeholder("Search refs…", cx);
                    });
                    let handle = self.search.read(cx).focus_handle(cx);
                    if handle.is_focused(window) {
                        self.boot_focus_pending = false;
                    } else {
                        window.focus(&handle, cx);
                    }
                }
                Some(_) => {
                    if self.focus.is_focused(window) {
                        self.boot_focus_pending = false;
                    } else {
                        window.focus(&self.focus, cx);
                    }
                }
                None => self.boot_focus_pending = false,
            }
        }

        let focus_on_mount = std::mem::take(&mut self.focus_on_mount) && self.is_open();
        if focus_on_mount {
            // The frame exists even when loading/error/empty content omits
            // the input. Claim it during mount, after any pre-frame shell
            // recovery has handled the old dispatch tree.
            window.focus(&self.focus, cx);
        }
        if self.is_open() {
            let search = self.search.focus_handle(cx);
            let frame = self.focus.clone();
            window.defer(cx, move |window, cx| {
                let has_search = frame.contains(&search, window);
                if focus_on_mount && frame.is_focused(window) && has_search {
                    // Transfer to the filter only once it is actually mounted.
                    window.focus(&search, cx);
                } else if search.is_focused(window) && !has_search {
                    // Loading, empty, and error states can omit the search box.
                    // Keep Escape/arrow keys on the mounted menu in those states.
                    window.focus(&frame, cx);
                }
            });
        }

        // Eager-load the harness catalog + every offered harness's models so
        // the chip reads "Fable 5" (a concrete pick) before any popover
        // opens, and rail switches inside the picker are instant.
        self.ensure_harnesses(false, cx);
        self.prefetch_models(false, cx);
        // A popover opened data-side (ZERON_OPEN_PICKER) never went through
        // `toggle`, so kick its loads here (all ensure_* are idempotent).
        if matches!(
            self.open_kind(),
            Some(PickerKind::Branch) | Some(PickerKind::Checkout)
        ) && matches!(self.refs, Loadable::Idle)
        {
            self.ensure_refs(false, cx);
        }
        // Chip shows the model's display name alone (zeron `modelText`); the
        // harness reads from the brand mark beside it. Never "Default model":
        // before the catalog lands the remembered label (or the configured id)
        // names the pick; the loaded list then resolves it to a concrete row.
        // No-agents state: nothing runnable resolved (and the catalog is
        // loaded, so that's a conclusion, not a loading gap) — the chip says
        // so instead of wearing a brand mark for an agent that can't run.
        let no_agents = self.no_agents_available() && self.effective_harness(cx).is_none();
        let model_label: SharedString = if let Some(title) = &self.title {
            // The saved choice, not the tab being browsed.
            match (title.harness, title.model.as_deref()) {
                (None, _) => "Session agent".into(),
                (Some(_), None) => "Automatic".into(),
                (Some(harness), Some(id)) => self
                    .models
                    .get(&harness)
                    .and_then(Loadable::ready)
                    .and_then(|models| models.iter().find(|m| m.id == id))
                    .map(|m| m.label.clone())
                    .or_else(|| self.defaults.label_for(id).map(str::to_owned))
                    .unwrap_or_else(|| id.to_owned())
                    .into(),
            }
        } else if no_agents {
            SharedString::from("No agents available")
        } else {
            let label = self.selected_model_label(cx);
            label.map(SharedString::from).unwrap_or_default()
        };
        let catalog_loading = matches!(self.harnesses, Loadable::Idle | Loadable::Loading);
        let models_loading = self.effective_harness(cx).is_some_and(|harness| {
            !matches!(
                self.models.get(&harness),
                Some(Loadable::Ready(_)) | Some(Loadable::Error(_))
            )
        });
        // Harness unknown while the catalog resolves: the pixel-glyph loader
        // instead of guessing a brand mark.
        let chip_icon_loading = self.title.is_none()
            && self.effective_harness(cx).is_none()
            && !no_agents
            && catalog_loading;
        // Harness known but nothing names the model yet (fresh install, no
        // remembered pick): a ghost label instead of a bare icon.
        let chip_label_loading =
            !no_agents && model_label.is_empty() && (catalog_loading || models_loading);
        let chip_harness = match &self.title {
            Some(title) => title.harness,
            None => self.effective_harness(cx),
        };
        let harness_icon: (&'static str, Option<gpui::Hsla>) = match chip_harness {
            Some(harness) => harness_brand_icon(harness),
            None if self.title.is_some() => (crate::icons::CHAT_ROUND_LINE, Some(theme.text_muted)),
            None if no_agents => (crate::icons::TERMINAL, Some(theme.text_muted)),
            None => (
                crate::icons::CLAUDE_MARK,
                Some(crate::icons::claude_brand()),
            ),
        };
        // The chip names the model and its effort only; the other options
        // (context, fast mode, ...) live in the popover. The effort brightens
        // when it departs from the model's default.
        let effort = self.effective_reasoning(cx);
        let effort_customized = effort != default_reasoning(&self.trait_ladder(cx));
        // Render the open popover's body first (mutable borrow), then the
        // chips. Branch/Checkout render in the composer FOOTER row (see
        // `render_footer`), not here.
        let closing = (self.open.closing_since(), self.menu_geometry().below);
        let mut overlay: Option<(PickerKind, AnyElement)> = match self.mounted_kind() {
            // Footer-row pickers — their popovers mount down there.
            Some(PickerKind::Branch)
            | Some(PickerKind::Checkout)
            | Some(PickerKind::Space)
            | Some(PickerKind::Device) => None,
            Some(PickerKind::HarnessModel) => {
                let content = self.render_harness_model_popover(cx);
                Some((
                    PickerKind::HarnessModel,
                    // Compact single-harness pane (t3 ModelPickerContent
                    // shrunk to its tabbed layout).
                    self.popover_frame_flush(304.0, content, cx),
                ))
            }
            None => None,
        };

        // The composer places this model chip beside the attachment button:
        // brand icon + model name, then the effort as the chip's muted second
        // tone. No suffix when the model has no reasoning ladder, nor for the
        // title picker (titles always run at minimal reasoning).
        let chip_suffix = effort.filter(|_| self.title.is_none()).map(|level| {
            (
                SharedString::from(reasoning_label(level)),
                effort_customized.then(|| theme.text.opacity(0.85)),
            )
        });
        let model_chip = self.trigger_chip(
            PickerKind::HarnessModel,
            model_label,
            true,
            Some(harness_icon),
            chip_icon_loading,
            chip_label_loading,
            chip_suffix,
            &theme,
            cx,
        );
        let model_chip = attach_overlay_end(
            model_chip,
            &mut overlay,
            PickerKind::HarnessModel,
            "model-popover",
            closing,
        );
        div()
            .flex()
            .flex_row()
            .items_center()
            // Shrinkable under row pressure, like the footer chips: the chip's
            // own `min_w_0().truncate()` label/suffix only engage when this
            // cluster is allowed to give up width — `flex_none` here let the
            // labels paint over the attach/send buttons at narrow widths
            // instead of truncating (user report).
            .min_w_0()
            .gap(px(4.0))
            .child(model_chip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::{FolderEntry, Model, ModelOption, ModelOptionChoice};

    struct ModelShortcutHost {
        focus_sub: Option<gpui::Subscription>,
        root: FocusHandle,
        editor: FocusHandle,
        neutral: FocusHandle,
        pickers: Entity<Pickers>,
    }

    impl Render for ModelShortcutHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            if self.focus_sub.is_none() {
                self.focus_sub = Some(cx.on_focus_lost(window, |this: &mut Self, window, cx| {
                    let root = this.root.clone();
                    let editor = this.editor.clone();
                    let neutral = this.neutral.clone();
                    window.on_next_frame(move |window, cx| {
                        crate::shell::restore_mounted_focus(&root, &editor, &neutral, window, cx);
                    });
                    cx.notify();
                }));
            }
            let root = self.root.clone();
            let editor = self.editor.clone();
            let neutral = self.neutral.clone();
            window.defer(cx, move |window, cx| {
                crate::shell::restore_mounted_focus(&root, &editor, &neutral, window, cx);
            });
            div()
                .size_full()
                .track_focus(&self.root)
                .on_action(
                    cx.listener(|this, _: &crate::shell::OpenModelPicker, window, cx| {
                        this.pickers
                            .update(cx, |pickers, cx| pickers.open_model_menu(window, cx));
                        cx.notify();
                    }),
                )
                .child(div().track_focus(&self.editor))
                .child(div().track_focus(&self.neutral))
                .child(self.pickers.clone())
        }
    }

    #[gpui::test]
    fn saved_model_survives_a_fresh_catalog_without_becoming_a_new_choice(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| cx.set_global(Theme::dark()));
        for had_disk_catalog in [false, true] {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.chats.push(serde_json::from_value(serde_json::json!({
                    "id":"saved-chat", "deviceId":"device", "archived":false, "createdAt":"2026-09-01T00:00:00Z",
                    "config":{"harness":"codex", "model":"saved-model", "reasoning":"high", "sandbox":"workspace-write", "modelOptions":{"serviceTier":"fast"}}
                })).unwrap());
                state.selected_chat = Some("saved-chat".into()); state
            });
            let pickers = cx.new(|cx| Pickers::new(state, cx));
            pickers.update(cx, |pickers, cx| {
                pickers.defaults = ComposerDefaults::default();
                pickers.harnesses = Loadable::Ready(vec![descriptor(HarnessId::Codex, "Codex")]);
                if had_disk_catalog {
                    pickers.apply_model_catalog(
                        HarnessId::Codex,
                        Loadable::Ready(vec![bare_model("saved-model", "Remembered model")]),
                        cx,
                    );
                }
                pickers.apply_model_catalog(
                    HarnessId::Codex,
                    Loadable::Ready(vec![bare_model("fresh-default", "Fresh default")]),
                    cx,
                );
                assert_eq!(pickers.resolved(cx).model.as_deref(), Some("saved-model"));
                assert_eq!(pickers.resolved(cx).reasoning, Some(ReasoningLevel::High));
                assert_eq!(pickers.resolved(cx).model_options["serviceTier"], "fast");
                assert_eq!(
                    pickers.selected_model_label(cx).as_deref(),
                    Some(if had_disk_catalog {
                        "Remembered model"
                    } else {
                        "saved-model"
                    })
                );
                let rows = pickers.model_rows(cx);
                assert_eq!(rows[0].model.id, "saved-model");
                assert!(rows[0].selected_only);
                assert_eq!(pickers.selected_model_index(cx), 0);
                assert_eq!(pickers.models[&HarnessId::Codex].ready().unwrap().len(), 1);
                pickers.activate_model_index(0, cx);
                assert_eq!(pickers.resolved(cx).model.as_deref(), Some("saved-model"));
                // Changing selection also invalidates the row cache; the
                // unlisted ID does not remain available as a new choice.
                pickers.config.model = Some("fresh-default".into());
                let rows = pickers.model_rows(cx);
                assert_eq!(rows.len(), 1);
                assert!(!rows[0].selected_only);
                assert_eq!(
                    pickers.selected_model_label(cx).as_deref(),
                    Some("Fresh default")
                );
            });
        }
    }

    #[test]
    fn explicit_missing_model_does_not_resolve_to_the_catalog_default() {
        let rows = vec![bare_model("first", "First")];
        assert_eq!(selected_catalog_model(&rows, None).unwrap().id, "first");
        assert!(selected_catalog_model(&rows, Some("saved")).is_none());
    }

    #[gpui::test]
    fn missing_selected_harness_is_not_reintroduced_into_the_rail(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            let state = cx.new(|_| AppState::new());
            let pickers = cx.new(|cx| Pickers::new(state, cx));
            pickers.update(cx, |pickers, cx| {
                pickers.config.harness = Some(HarnessId::Codex);
                let mut missing = descriptor(HarnessId::Codex, "Codex");
                missing.installed = false;
                pickers.harnesses = Loadable::Ready(vec![missing]);
                assert!(pickers.rail_descriptors(cx).is_empty());
            });
        });
    }

    #[gpui::test]
    fn refresh_failure_keeps_rows_and_catalog_change_reanchors_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| cx.set_global(Theme::dark()));
        let handle = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            let mut pickers = Pickers::new(state, cx);
            pickers.config.harness = Some(HarnessId::ClaudeCode);
            pickers.config.model = Some("chosen".into());
            pickers.harnesses =
                Loadable::Ready(vec![descriptor(HarnessId::ClaudeCode, "Claude Code")]);
            pickers
        });
        handle
            .update(cx, |pickers, window, cx| {
                let rows = vec![bare_model("chosen", "Chosen"), bare_model("other", "Other")];
                pickers.apply_model_catalog(
                    HarnessId::ClaudeCode,
                    Loadable::Ready(rows.clone()),
                    cx,
                );
                pickers.open_model_menu(window, cx);
                assert_eq!(pickers.active, 0);
                pickers.apply_model_catalog(
                    HarnessId::ClaudeCode,
                    Loadable::Error("refresh failed".into()),
                    cx,
                );
                assert_eq!(
                    pickers.models[&HarnessId::ClaudeCode],
                    Loadable::Ready(rows.clone())
                );
                assert_eq!(
                    pickers
                        .model_refresh_errors
                        .get(&HarnessId::ClaudeCode)
                        .map(String::as_str),
                    Some("refresh failed")
                );
                pickers.apply_model_catalog(
                    HarnessId::ClaudeCode,
                    Loadable::Ready(rows.into_iter().rev().collect()),
                    cx,
                );
                assert_eq!(pickers.active, 1);
                assert!(
                    !pickers
                        .model_refresh_errors
                        .contains_key(&HarnessId::ClaudeCode)
                );
                pickers.apply_model_catalog(
                    HarnessId::Codex,
                    Loadable::Error("cold failure".into()),
                    cx,
                );
                assert!(matches!(
                    pickers.models[&HarnessId::Codex],
                    Loadable::Error(_)
                ));
            })
            .unwrap();
    }

    #[gpui::test]
    fn model_shortcut_focuses_mounted_picker_and_routes_navigation(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            crate::composer::init(cx, Default::default());
            cx.bind_keys([gpui::KeyBinding::new(
                "cmd-/",
                crate::shell::OpenModelPicker,
                None,
            )]);
        });
        let handle = cx.add_window(|_, cx| ModelShortcutHost {
            focus_sub: None,
            root: cx.focus_handle(),
            editor: cx.focus_handle(),
            neutral: cx.focus_handle(),
            pickers: cx.new(|cx| {
                let state = cx.new(|_| AppState::new());
                let mut pickers = Pickers::new(state, cx);
                pickers.config.harness = Some(HarnessId::ClaudeCode);
                pickers.config.model = Some("first".into());
                pickers.harnesses =
                    Loadable::Ready(vec![descriptor(HarnessId::ClaudeCode, "Claude Code")]);
                pickers.models.insert(
                    HarnessId::ClaudeCode,
                    Loadable::Ready(vec![
                        bare_model("first", "First"),
                        bare_model("second", "Second"),
                    ]),
                );
                pickers
            }),
        });
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        handle
            .update(cx, |host, window, cx| window.focus(&host.editor, cx))
            .unwrap();
        cx.simulate_keystrokes(handle.into(), "cmd-/");
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        handle
            .update(cx, |host, window, cx| {
                host.pickers.read_with(cx, |pickers, cx| {
                    assert!(pickers.is_open());
                    assert!(
                        pickers.focus.contains_focused(window, cx),
                        "shortcut must transfer focus into the mounted picker"
                    );
                });
            })
            .unwrap();
        cx.simulate_keystrokes(handle.into(), "down");
        handle
            .read_with(cx, |host, cx| assert_eq!(host.pickers.read(cx).active, 1))
            .unwrap();
        cx.simulate_keystrokes(handle.into(), "up");
        handle
            .read_with(cx, |host, cx| assert_eq!(host.pickers.read(cx).active, 0))
            .unwrap();
        cx.simulate_keystrokes(handle.into(), "escape");
        handle
            .read_with(cx, |host, cx| assert!(!host.pickers.read(cx).is_open()))
            .unwrap();

        // Catalog loading/error/empty cards omit the input but still need
        // immediate keyboard focus on their mounted frame for Escape.
        for catalog in [
            Loadable::Loading,
            Loadable::Error("offline".into()),
            Loadable::Ready(vec![]),
        ] {
            cx.executor().advance_clock(Duration::from_secs(1));
            cx.run_until_parked();
            handle
                .update(cx, |host, window, cx| {
                    host.pickers.update(cx, |pickers, cx| {
                        pickers.config.harness = None;
                        pickers.defaults.harness = None;
                        pickers.harnesses = catalog;
                        cx.notify();
                    });
                    window.focus(&host.editor, cx);
                })
                .unwrap();
            cx.simulate_keystrokes(handle.into(), "cmd-/");
            cx.run_until_parked();
            cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
                .unwrap();
            handle
                .update(cx, |host, window, cx| {
                    assert!(host.pickers.read(cx).focus.contains_focused(window, cx));
                })
                .unwrap();
            cx.simulate_keystrokes(handle.into(), "escape");
            handle
                .read_with(cx, |host, cx| assert!(!host.pickers.read(cx).is_open()))
                .unwrap();
        }
    }

    #[test]
    fn model_menu_height_tracks_both_sides_and_long_settings() {
        for (top, bottom, viewport, below) in [
            (80.0, 112.0, 500.0, true),
            (430.0, 462.0, 500.0, false),
            (190.0, 222.0, 360.0, false),
        ] {
            let geometry = popover::menu_geometry(top, bottom, viewport);
            assert_eq!(geometry.below, below);
            for settings in [0, 1, 3, 20] {
                let height = geometry.height.min(model_menu_height(settings));
                let (list, tray) = model_menu_budgets(height, settings);
                assert!(list + tray + 82.0 <= geometry.height);
                assert!((0.0..=216.0).contains(&list));
                assert!((0.0..=236.0).contains(&tray));
                if settings == 0 {
                    assert_eq!(tray, 0.0);
                }
            }
        }
        assert!(model_menu_budgets(180.0, 2).0 < model_menu_budgets(400.0, 2).0);
    }

    #[gpui::test]
    fn new_thread_model_menu_prefers_below_but_retains_room_for_choices(
        cx: &mut gpui::TestAppContext,
    ) {
        struct Fixture(Entity<Pickers>, bool);
        impl Render for Fixture {
            fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let top = if self.1 {
                    window.viewport_size().height - px(50.0)
                } else {
                    px(300.0)
                };
                div().size_full().relative().child(
                    div()
                        .absolute()
                        .top(top)
                        .left(px(200.0))
                        .child(self.0.clone()),
                )
            }
        }
        cx.update(|cx| cx.set_global(Theme::dark()));
        let handle = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Fixture(cx.new(|cx| Pickers::new(state, cx)), false)
        });
        for (new_thread, near_bottom) in
            [(true, false), (true, true), (false, false), (false, true)]
        {
            handle
                .update(cx, |fixture, _, cx| {
                    fixture.1 = near_bottom;
                    fixture.0.update(cx, |picker, cx| {
                        picker.state.update(cx, |state, cx| {
                            state.selected_chat = (!new_thread).then(|| "thread".into());
                            cx.notify();
                        });
                        cx.notify();
                    });
                    cx.notify();
                })
                .unwrap();
            cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
                .unwrap();
            handle
                .read_with(cx, |fixture, cx| {
                    let picker = fixture.0.read(cx);
                    let geometry = picker.menu_geometry[&PickerKind::HarnessModel];
                    assert_eq!(geometry.below, new_thread && !near_bottom);
                    assert!(geometry.height > 180.0);
                    assert!(
                        model_menu_budgets(geometry.height.min(model_menu_height(2)), 2).0 >= 30.0
                    );
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn open_model_trigger_keeps_its_width_across_model_and_option_changes(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| cx.set_global(Theme::dark()));
        let handle = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            let mut pickers = Pickers::new(state, cx);
            pickers.config.harness = Some(HarnessId::Codex);
            pickers.config.model = Some("short".into());
            pickers.harnesses = Loadable::Ready(vec![descriptor(HarnessId::Codex, "Codex")]);
            let mut long = bare_model("long", "A much longer model name");
            long.reasoning_levels = vec![ReasoningLevel::Low, ReasoningLevel::High];
            long.options.push(ModelOption {
                id: "serviceTier".into(),
                label: "Service tier".into(),
                default_choice: "default".into(),
                choices: vec![
                    ModelOptionChoice {
                        id: "default".into(),
                        label: "Standard".into(),
                    },
                    ModelOptionChoice {
                        id: "fast".into(),
                        label: "Fast".into(),
                    },
                ],
            });
            pickers.models.insert(
                HarnessId::Codex,
                Loadable::Ready(vec![bare_model("short", "Short"), long]),
            );
            pickers
        });
        let draw = |cx: &mut gpui::TestAppContext| {
            cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
                .unwrap();
            handle
                .read_with(cx, |pickers, _| pickers.model_trigger_bounds.unwrap())
                .unwrap()
        };
        let before = draw(cx);
        handle
            .update(cx, |pickers, window, cx| {
                pickers.open_model_menu(window, cx)
            })
            .unwrap();
        for (model, fast) in [("long", false), ("long", true), ("short", false)] {
            handle
                .update(cx, |pickers, _, cx| {
                    pickers.config.model = Some(model.into());
                    pickers
                        .defaults
                        .model_options_mut(HarnessId::Codex, model)
                        .insert(
                            "serviceTier".into(),
                            if fast { "fast" } else { "default" }.into(),
                        );
                    cx.notify();
                })
                .unwrap();
            assert_eq!(
                draw(cx),
                before,
                "open trigger moved for {model}, fast={fast}"
            );
        }
    }

    #[gpui::test]
    fn open_model_card_keeps_height_when_options_change(cx: &mut gpui::TestAppContext) {
        use std::{cell::Cell, rc::Rc};
        struct Fixture {
            pickers: Entity<Pickers>,
            bounds: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
        }
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let measured = self.bounds.clone();
                let menu = self.pickers.update(cx, |pickers, cx| {
                    let content = pickers.render_harness_model_popover(cx);
                    pickers.popover_frame_flush(304.0, content, cx)
                });
                div().size_full().relative().child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .w(px(304.0))
                        .child(menu)
                        .child(
                            gpui::canvas(move |bounds, _, _| measured.set(bounds), |_, _, _, _| {})
                                .absolute()
                                .inset_0(),
                        ),
                )
            }
        }
        cx.update(|cx| cx.set_global(Theme::dark()));
        let bounds = Rc::new(Cell::new(gpui::Bounds::default()));
        let handle = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            let pickers = cx.new(|cx| {
                let mut pickers = Pickers::new(state, cx);
                pickers.config.harness = Some(HarnessId::Codex);
                pickers.config.model = Some("model".into());
                pickers.harnesses = Loadable::Ready(vec![descriptor(HarnessId::Codex, "Codex")]);
                pickers.open_model_height = model_menu_height(2);
                pickers.menu_geometry.insert(
                    PickerKind::HarnessModel,
                    popover::MenuGeometry {
                        height: 500.0,
                        below: false,
                    },
                );
                pickers.open.open(PickerKind::HarnessModel);
                pickers
            });
            Fixture {
                pickers,
                bounds: bounds.clone(),
            }
        });
        for available in [500.0, 200.0, 500.0] {
            for settings in [0, 1, 3, 0] {
                handle
                    .update(cx, |fixture, _, cx| {
                        fixture.pickers.update(cx, |pickers, cx| {
                            let mut model = bare_model("model", "Model");
                            model.options = (0..settings)
                                .map(|i| ModelOption {
                                    id: format!("option-{i}"),
                                    label: format!("Option {i}"),
                                    default_choice: "on".into(),
                                    choices: vec![
                                        ModelOptionChoice {
                                            id: "on".into(),
                                            label: "On".into(),
                                        },
                                        ModelOptionChoice {
                                            id: "off".into(),
                                            label: "Off".into(),
                                        },
                                    ],
                                })
                                .collect();
                            pickers
                                .models
                                .insert(HarnessId::Codex, Loadable::Ready(vec![model]));
                            pickers
                                .menu_geometry
                                .get_mut(&PickerKind::HarnessModel)
                                .unwrap()
                                .height = available;
                            cx.notify();
                        });
                        cx.notify();
                    })
                    .unwrap();
                cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
                    .unwrap();
                assert!(
                    (f32::from(bounds.get().size.height) - available.min(model_menu_height(2)))
                        .abs()
                        < 0.1,
                    "settings={settings}, available={available}, bounds={:?}",
                    bounds.get()
                );
            }
        }
    }

    #[gpui::test]
    fn workspace_footer_pair_keeps_its_leading_edge_and_gap(cx: &mut gpui::TestAppContext) {
        struct Fixture {
            width: f32,
            bounds: std::rc::Rc<std::cell::RefCell<Vec<gpui::Bounds<gpui::Pixels>>>>,
        }
        impl gpui::Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let chip = |width| {
                    let measured = self.bounds.clone();
                    gpui::canvas(
                        move |bounds, _, _| measured.borrow_mut().push(bounds),
                        |_, _, _, _| {},
                    )
                    .w(px(width))
                    .h(px(20.0))
                    .flex_none()
                };
                div().w(px(self.width)).child(
                    workspace_footer_row()
                        .px(px(10.0))
                        .child(chip(120.0))
                        .child(chip(90.0))
                        .child(div().flex_1().min_w_0())
                        .child(chip(60.0)),
                )
            }
        }
        let bounds = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let handle = cx.add_window(|_, _| Fixture {
            width: 320.0,
            bounds: bounds.clone(),
        });
        let mut first_left = None;
        for width in [320.0, 680.0, 1000.0, 320.0] {
            handle
                .update(cx, |fixture, _, cx| {
                    fixture.width = width;
                    cx.notify();
                })
                .unwrap();
            bounds.borrow_mut().clear();
            cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
                .unwrap();
            let measured = bounds.borrow();
            let pair = &measured[measured.len() - 3..];
            assert_eq!(pair[0].left(), *first_left.get_or_insert(pair[0].left()));
            assert!((f32::from(pair[1].left() - pair[0].right()) - 4.0).abs() < 0.1);
            assert_eq!(pair[0].top(), pair[1].top());
            assert!((f32::from(pair[2].right() - pair[0].left()) - (width - 20.0)).abs() < 0.1);
        }
    }

    #[gpui::test]
    fn picker_completion_and_dismissal_have_distinct_focus_behavior(cx: &mut gpui::TestAppContext) {
        use std::cell::Cell;
        use std::rc::Rc;
        cx.update(|cx| cx.set_global(Theme::dark()));
        let handle = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Pickers::new(state, cx)
        });
        let returned = Rc::new(Cell::new(0));
        let observed = returned.clone();
        let _sub = cx.update(|cx| {
            cx.subscribe(
                &handle.entity(cx).unwrap(),
                move |_, _: &ReturnComposerFocus, _| observed.set(observed.get() + 1),
            )
        });
        handle
            .update(cx, |pickers, _, cx| {
                pickers.open.open(PickerKind::Checkout);
                pickers.pick_checkout(CheckoutKind::Local, cx);
            })
            .unwrap();
        assert_eq!(returned.get(), 1);
        handle
            .update(cx, |pickers, _, cx| {
                pickers.open.open(PickerKind::HarnessModel);
                pickers.pick_model("test-model".into(), cx);
                assert!(
                    pickers.is_open(),
                    "model options remain available after a selection"
                );
                pickers.dismiss(cx);
            })
            .unwrap();
        assert_eq!(
            returned.get(),
            1,
            "outside clicks must not request composer focus"
        );
        handle
            .update(cx, |pickers, window, cx| {
                pickers.open.open(PickerKind::Checkout);
                pickers.open.note_trigger_press();
                pickers.dismiss(cx); // Capture closes before the trigger's click.
                pickers.toggle(PickerKind::Checkout, window, cx);
            })
            .unwrap();
        assert_eq!(
            returned.get(),
            2,
            "closing via the trigger returns to the composer"
        );
    }

    #[gpui::test]
    fn projectless_picker_clears_checkout_and_supports_keyboard_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.local_device_id = Some("local".into());
            state.apply_spaces(vec![Space {
                id: "repo".into(),
                device_id: "local".into(),
                path: "/repo".into(),
                name: None,
                git_detected: true,
                git_checked_at: None,
                checkout_id: None,
                created_at: chrono::Utc::now(),
            }]);
            state
        });
        let pickers = cx.new(|cx| Pickers::new(state.clone(), cx));
        pickers.update(cx, |pickers, cx| {
            pickers.config.branch = Some("old-branch".into());
            pickers.config.checkout = CheckoutKind::NewWorktree;
            pickers.open.open(PickerKind::Space);
            pickers.active = 1; // project row followed by the projectless row
            pickers.on_search_submit(cx);
        });
        cx.run_until_parked();
        pickers.update(cx, |pickers, cx| {
            assert!(pickers.config.branch.is_none());
            assert_eq!(pickers.config.checkout, CheckoutKind::default());
            assert!(pickers.defaults.no_project);
            assert!(pickers.state.read(cx).auto_selected);
            assert!(pickers.defaults.project.is_none());
            assert_eq!(pickers.selected_space_index(cx), 1);
        });
        state.update(cx, |state, cx| state.select_device("remote".into(), cx));
        cx.run_until_parked();
        pickers.update(cx, |pickers, cx| {
            assert_eq!(pickers.space_target(cx).as_deref(), Some("remote"));
            assert_eq!(pickers.selected_space_index(cx), 0); // empty device
            assert!(pickers.target_generation >= 2);
        });
    }

    fn bare_model(id: &str, label: &str) -> Model {
        Model {
            id: id.into(),
            label: label.into(),
            description: None,
            reasoning_levels: Vec::new(),
            options: Vec::new(),
        }
    }

    #[gpui::test]
    fn nested_model_settings_navigate_and_preserve_independent_choices(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            crate::composer::init(cx, Default::default());
        });
        let handle = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Pickers::new(state, cx)
        });
        handle
            .update(cx, |pickers, window, cx| {
                let mut model = bare_model("opus", "Opus");
                model.reasoning_levels = vec![ReasoningLevel::Low, ReasoningLevel::High];
                model.options = ["contextWindow", "serviceTier"]
                    .map(|id| ModelOption {
                        id: id.into(),
                        label: id.into(),
                        default_choice: "standard".into(),
                        choices: ["standard", "extended"]
                            .map(|id| ModelOptionChoice {
                                id: id.into(),
                                label: id.into(),
                            })
                            .into(),
                    })
                    .into();
                pickers.config.harness = Some(HarnessId::ClaudeCode);
                pickers.harnesses =
                    Loadable::Ready(vec![descriptor(HarnessId::ClaudeCode, "Claude Code")]);
                pickers.models.insert(
                    HarnessId::ClaudeCode,
                    Loadable::Ready(vec![model, bare_model("haiku", "Haiku")]),
                );
                pickers.pick_model("opus".into(), cx);
                pickers.open.open(PickerKind::HarnessModel);
                window.focus(&pickers.focus, cx);
                assert_eq!(pickers.setting_groups(cx).len(), 3);
                let key = |key: &str| KeyDownEvent {
                    keystroke: gpui::Keystroke::parse(key).unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                };
                pickers.active = pickers.model_rows_len(cx);
                pickers.on_key_down(&key("right"), window, cx);
                assert_eq!(pickers.setting_menu, Some(ModelSetting::Reasoning));
                pickers.on_key_down(&key("up"), window, cx);
                pickers.on_key_down(&key("enter"), window, cx);
                assert_eq!(pickers.effective_reasoning(cx), Some(ReasoningLevel::Low));
                assert!(pickers.setting_menu.is_none());
                assert!(pickers.is_open());
                for id in ["contextWindow", "serviceTier"] {
                    pickers.open_setting(SettingScope::Tray, ModelSetting::Option(id.into()), cx);
                    assert_eq!(pickers.setting_active, 0);
                    pickers.on_key_down(&key("down"), window, cx);
                    pickers.on_key_down(&key("enter"), window, cx);
                    assert_eq!(pickers.resolved(cx).model_options[id], "extended");
                }
                // Returning to one setting keeps its choice, and restoring its
                // default does not reset a sibling option or close the picker.
                pickers.open_setting(
                    SettingScope::Tray,
                    ModelSetting::Option("contextWindow".into()),
                    cx,
                );
                assert_eq!(pickers.setting_active, 1);
                pickers.on_key_down(&key("up"), window, cx);
                pickers.on_key_down(&key("enter"), window, cx);
                assert!(
                    !pickers
                        .resolved(cx)
                        .model_options
                        .contains_key("contextWindow")
                );
                assert_eq!(
                    pickers.resolved(cx).model_options["serviceTier"],
                    "extended"
                );
                pickers.open_setting(SettingScope::Tray, ModelSetting::Reasoning, cx);
                pickers.on_key_down(&key("escape"), window, cx);
                assert!(pickers.is_open());
                assert!(pickers.setting_menu.is_none());
                pickers.open_setting(SettingScope::Tray, ModelSetting::Reasoning, cx);
                pickers.pick_model("haiku".into(), cx);
                assert!(pickers.setting_menu.is_none());
                assert!(pickers.setting_groups(cx).is_empty());
            })
            .unwrap();
        for setting in [
            ModelSetting::Reasoning,
            ModelSetting::Option("contextWindow".into()),
            ModelSetting::Option("serviceTier".into()),
        ] {
            handle
                .update(cx, |pickers, _, cx| {
                    pickers.pick_model("opus".into(), cx);
                    pickers.open_setting(SettingScope::Tray, setting, cx);
                })
                .unwrap();
            cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
                .unwrap();
        }
        handle
            .update(cx, |pickers, window, cx| {
                pickers.setting_menu = None;
                pickers.active = 0;
                window.focus(&pickers.search.read(cx).focus_handle(cx), cx);
            })
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        cx.simulate_keystrokes(handle.into(), "down down right");
        handle
            .read_with(cx, |pickers, _| {
                assert_eq!(pickers.setting_menu, Some(ModelSetting::Reasoning))
            })
            .unwrap();
        cx.simulate_keystrokes(handle.into(), "down enter");
        handle
            .read_with(cx, |pickers, cx| {
                assert_eq!(pickers.effective_reasoning(cx), Some(ReasoningLevel::High));
                assert!(pickers.setting_menu.is_none());
                assert!(pickers.is_open());
            })
            .unwrap();
    }

    /// Devin Fusion: one row configured in place. Its hover card lists Lead,
    /// Effort, Sidekick and a Fast Mode switch; any choice made there selects
    /// Fusion and applies to it.
    #[gpui::test]
    fn fusion_card_configures_and_selects_the_model(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            crate::composer::init(cx, Default::default());
        });
        let handle = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Pickers::new(state, cx)
        });
        let option = |id: &str, label: &str, choices: &[(&str, &str)]| ModelOption {
            id: id.into(),
            label: label.into(),
            default_choice: choices[0].0.into(),
            choices: choices
                .iter()
                .map(|(id, label)| ModelOptionChoice {
                    id: (*id).into(),
                    label: (*label).into(),
                })
                .collect(),
        };
        handle
            .update(cx, |pickers, window, cx| {
                let mut fusion = bare_model("fusion", "Fusion");
                fusion.reasoning_levels = vec![
                    ReasoningLevel::Low,
                    ReasoningLevel::Medium,
                    ReasoningLevel::High,
                    ReasoningLevel::XHigh,
                ];
                fusion.options = vec![
                    option(
                        "lead",
                        "Lead",
                        &[
                            ("claude-fable-5-1", "Claude Fable 5.1"),
                            ("gpt-6-sol", "GPT-6 Sol"),
                        ],
                    ),
                    option(
                        "sidekick",
                        "Sidekick",
                        &[
                            ("swe-2-medium", "SWE-2 Medium"),
                            ("swe-2-high", "SWE-2 High"),
                        ],
                    ),
                    option(
                        "speed",
                        "Fast Mode",
                        &[("standard", "Standard"), ("fast", "Fast")],
                    ),
                ];
                pickers.config.harness = Some(HarnessId::Devin);
                pickers.harnesses = Loadable::Ready(vec![descriptor(HarnessId::Devin, "Devin")]);
                pickers.models.insert(
                    HarnessId::Devin,
                    Loadable::Ready(vec![
                        bare_model("adaptive", "Adaptive"),
                        fusion,
                        bare_model("swe-2-high", "SWE-2"),
                    ]),
                );
                pickers.pick_model("adaptive".into(), cx);
                pickers.open.open(PickerKind::HarnessModel);
                window.focus(&pickers.focus, cx);
                // Only Fusion is configured in place.
                let rows = pickers.model_rows(cx);
                let configured: Vec<_> =
                    rows.iter().map(|r| configured_in_place(&r.model)).collect();
                assert_eq!(configured, [false, true, false]);

                pickers.open_config(1, cx);
                let labels: Vec<_> = pickers
                    .card_groups(cx)
                    .iter()
                    .map(|g| g.label.clone())
                    .collect();
                assert_eq!(labels, ["Lead", "Effort", "Sidekick", "Fast Mode"]);
                // Nothing is picked just by looking.
                assert_eq!(pickers.resolved(cx).model.as_deref(), Some("adaptive"));

                pickers.open_setting(SettingScope::Card, ModelSetting::Option("lead".into()), cx);
                pickers.setting_active = 1;
                pickers.activate_setting_choice(cx);
                assert_eq!(pickers.resolved(cx).model.as_deref(), Some("fusion"));
                assert_eq!(pickers.resolved(cx).model_options["lead"], "gpt-6-sol");

                pickers.open_setting(SettingScope::Card, ModelSetting::Reasoning, cx);
                pickers.setting_active = 3;
                pickers.activate_setting_choice(cx);
                assert_eq!(pickers.effective_reasoning(cx), Some(ReasoningLevel::XHigh));

                let fast = pickers
                    .card_groups(cx)
                    .into_iter()
                    .find(|g| g.label == "Fast Mode")
                    .unwrap();
                assert!(is_toggle(&fast));
                pickers.toggle_card_option(fast, cx);
                let resolved = pickers.resolved(cx);
                assert_eq!(resolved.model_options["speed"], "fast");
                assert_eq!(resolved.model_options["lead"], "gpt-6-sol");

                // A card flyout is not the tray's: the tray row with the same
                // setting stays closed while the card's is open.
                pickers.open_setting(
                    SettingScope::Card,
                    ModelSetting::Option("sidekick".into()),
                    cx,
                );
                assert_eq!(pickers.setting_scope, SettingScope::Card);
                pickers.close_config();
                assert!(pickers.setting_menu.is_none());
                assert!(pickers.config_row.is_none());
            })
            .unwrap();
        // Both the card and its choice list render.
        handle
            .update(cx, |pickers, _, cx| {
                pickers.open_config(1, cx);
            })
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        handle
            .update(cx, |pickers, _, cx| {
                pickers.open_setting(SettingScope::Card, ModelSetting::Option("lead".into()), cx);
            })
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        handle
            .read_with(cx, |pickers, _| {
                assert_eq!(pickers.config_row, Some(1));
                assert_eq!(
                    pickers.setting_menu,
                    Some(ModelSetting::Option("lead".into()))
                );
            })
            .unwrap();
    }

    /// Pointer paths through the Fusion card: the cone toward a card row's
    /// choice list keeps both open, a pick keeps the card, and the selected
    /// Fusion's settings live only in its card (not the tray).
    #[gpui::test]
    fn fusion_card_mouse_paths(cx: &mut gpui::TestAppContext) {
        use std::{cell::Cell, rc::Rc};
        struct CardFixture {
            pickers: Entity<Pickers>,
            bounds: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
        }
        impl Render for CardFixture {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let measured = self.bounds.clone();
                let menu = self.pickers.update(cx, |pickers, cx| {
                    let content = pickers.render_harness_model_popover(cx);
                    pickers.popover_frame_flush(304.0, content, cx)
                });
                div().size_full().relative().child(
                    div()
                        .absolute()
                        .top(px(60.0))
                        .left(px(32.0))
                        .w(px(304.0))
                        .child(menu)
                        .child(
                            gpui::canvas(move |bounds, _, _| measured.set(bounds), |_, _, _, _| {})
                                .absolute()
                                .inset_0(),
                        ),
                )
            }
        }
        cx.update(|cx| cx.set_global(Theme::dark()));
        let measured = Rc::new(Cell::new(gpui::Bounds::default()));
        let handle = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            let pickers = cx.new(|cx| Pickers::new(state, cx));
            pickers.update(cx, |pickers, cx| {
                let option = |id: &str, label: &str, choices: &[(&str, &str)]| ModelOption {
                    id: id.into(),
                    label: label.into(),
                    default_choice: choices[0].0.into(),
                    choices: choices
                        .iter()
                        .map(|(id, label)| ModelOptionChoice {
                            id: (*id).into(),
                            label: (*label).into(),
                        })
                        .collect(),
                };
                let mut fusion = bare_model("fusion", "Fusion");
                fusion.reasoning_levels = vec![ReasoningLevel::Medium, ReasoningLevel::High];
                fusion.options = vec![
                    option(
                        "lead",
                        "Lead",
                        &[("a", "Lead A"), ("b", "Lead B"), ("c", "Lead C")],
                    ),
                    option("sidekick", "Sidekick", &[("x", "Side X"), ("y", "Side Y")]),
                    option(
                        "speed",
                        "Fast Mode",
                        &[("standard", "Standard"), ("fast", "Fast")],
                    ),
                ];
                pickers.config.harness = Some(HarnessId::Devin);
                pickers.harnesses = Loadable::Ready(vec![descriptor(HarnessId::Devin, "Devin")]);
                pickers.models.insert(
                    HarnessId::Devin,
                    Loadable::Ready(vec![bare_model("adaptive", "Adaptive"), fusion]),
                );
                pickers.open.open(PickerKind::HarnessModel);
                pickers.pick_model("adaptive".into(), cx);
            });
            CardFixture {
                pickers,
                bounds: measured.clone(),
            }
        });
        let pickers = handle
            .read_with(cx, |fixture, _| fixture.pickers.clone())
            .unwrap();
        let hover = |window: &mut Window, cx: &mut App, position| {
            window.dispatch_event(
                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position,
                    ..Default::default()
                }),
                cx,
            );
            window.draw(cx).clear();
            window.draw(cx).clear();
        };
        let click = |window: &mut Window, cx: &mut App, position| {
            for input in [
                gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                    button: gpui::MouseButton::Left,
                    position,
                    click_count: 1,
                    ..Default::default()
                }),
                gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                    button: gpui::MouseButton::Left,
                    position,
                    click_count: 1,
                    ..Default::default()
                }),
            ] {
                window.dispatch_event(input, cx);
            }
            window.draw(cx).clear();
            window.draw(cx).clear();
        };
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        let parent = measured.get();
        // Find the Fusion row by hovering down the list.
        let mut row = None;
        for step in 0..60 {
            let at = gpui::point(
                parent.left() + px(60.0),
                parent.top() + px(4.0 * step as f32),
            );
            cx.update_window(handle.into(), |_, window, cx| hover(window, cx, at))
                .unwrap();
            if pickers.read_with(cx, |p, _| p.config_row) == Some(1) {
                row = Some(at);
                break;
            }
        }
        row.expect("hovering the Fusion row opens its card");
        let card = pickers
            .read_with(cx, |p, _| p.config_bounds)
            .expect("card measured");
        assert!(card.left() > parent.right(), "card opens beside the menu");
        // Find the Lead trigger inside the card.
        let mut lead = None;
        for step in 0..40 {
            let at = gpui::point(card.left() + px(40.0), card.top() + px(3.0 * step as f32));
            cx.update_window(handle.into(), |_, window, cx| hover(window, cx, at))
                .unwrap();
            if pickers.read_with(cx, |p, _| p.setting_menu.clone())
                == Some(ModelSetting::Option("lead".into()))
            {
                lead = Some(at);
                break;
            }
        }
        let lead = lead.expect("hovering Lead opens its choices");
        let list = pickers
            .read_with(cx, |p, _| p.setting_bounds)
            .expect("choices measured");
        assert!(list.left() > card.right());
        // Diagonal path from the Lead row to the bottom of its list: through
        // the gap and below the Lead row — inside the cone, not the card.
        let target = gpui::point(list.left() + px(12.0), list.bottom() - px(10.0));
        for t in 1..=10 {
            let f = t as f32 / 10.0;
            let at = gpui::point(
                lead.x + (target.x - lead.x) * f,
                lead.y + (target.y - lead.y) * f,
            );
            cx.update_window(handle.into(), |_, window, cx| hover(window, cx, at))
                .unwrap();
            pickers.read_with(cx, |p, _| {
                assert_eq!(p.config_row, Some(1), "card closed at step {t}");
                assert_eq!(
                    p.setting_menu,
                    Some(ModelSetting::Option("lead".into())),
                    "choices closed at step {t}"
                );
            });
        }
        // Pick the last choice: Fusion is selected, the list closes, the
        // card stays — also while the pointer rests where the list was.
        cx.update_window(handle.into(), |_, window, cx| {
            click(window, cx, target);
            hover(window, cx, target + gpui::point(px(3.0), px(-2.0)));
        })
        .unwrap();
        pickers.read_with(cx, |p, cx| {
            assert_eq!(p.resolved(cx).model.as_deref(), Some("fusion"));
            assert_eq!(p.resolved(cx).model_options["lead"], "c");
            assert!(p.setting_menu.is_none());
            assert_eq!(p.config_row, Some(1), "a pick keeps the card open");
            // Selected Fusion keeps its settings out of the tray.
            assert!(p.setting_groups(cx).is_empty());
            assert!(!p.card_groups(cx).is_empty());
        });
        // Back into the card, then far away: the card closes.
        cx.update_window(handle.into(), |_, window, cx| {
            hover(window, cx, card.center());
            hover(window, cx, gpui::point(px(4.0), px(4.0)));
        })
        .unwrap();
        pickers.read_with(cx, |p, _| assert!(p.config_row.is_none()));
    }

    #[gpui::test]
    fn nested_model_menu_mouse_paths_work_on_both_sides(cx: &mut gpui::TestAppContext) {
        use std::{cell::Cell, rc::Rc};
        struct MouseFixture {
            pickers: Entity<Pickers>,
            on_left: bool,
            bounds: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
        }
        impl Render for MouseFixture {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let measured = self.bounds.clone();
                let menu = self.pickers.update(cx, |pickers, cx| {
                    let content = pickers.render_harness_model_popover(cx);
                    pickers.popover_frame_flush(304.0, content, cx)
                });
                div().size_full().relative().child(
                    div()
                        .absolute()
                        .top(px(100.0))
                        .w(px(304.0))
                        .when(self.on_left, |el| el.right(px(32.0)))
                        .when(!self.on_left, |el| el.left(px(32.0)))
                        .child(menu)
                        .child(
                            gpui::canvas(move |bounds, _, _| measured.set(bounds), |_, _, _, _| {})
                                .absolute()
                                .inset_0(),
                        ),
                )
            }
        }
        cx.update(|cx| cx.set_global(Theme::dark()));
        for on_left in [false, true] {
            let measured = Rc::new(Cell::new(gpui::Bounds::default()));
            let handle = cx.add_window(|_, cx| {
                let state = cx.new(|_| AppState::new());
                let pickers = cx.new(|cx| Pickers::new(state, cx));
                pickers.update(cx, |pickers, cx| {
                    let mut model = bare_model("test", "Test model");
                    model.reasoning_levels = vec![ReasoningLevel::Low, ReasoningLevel::High];
                    model.options.push(ModelOption {
                        id: "contextWindow".into(),
                        label: "Context window".into(),
                        choices: ["200k", "1m"]
                            .map(|id| ModelOptionChoice {
                                id: id.into(),
                                label: id.into(),
                            })
                            .into(),
                        default_choice: "200k".into(),
                    });
                    pickers.config.harness = Some(HarnessId::ClaudeCode);
                    pickers.harnesses =
                        Loadable::Ready(vec![descriptor(HarnessId::ClaudeCode, "Claude Code")]);
                    pickers
                        .models
                        .insert(HarnessId::ClaudeCode, Loadable::Ready(vec![model]));
                    pickers.open.open(PickerKind::HarnessModel);
                    pickers.pick_model("test".into(), cx);
                });
                MouseFixture {
                    pickers,
                    on_left,
                    bounds: measured.clone(),
                }
            });
            let pickers = handle
                .read_with(cx, |fixture, _| fixture.pickers.clone())
                .unwrap();
            cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
                .unwrap();
            let parent = measured.get();
            let trigger = gpui::point(
                parent.center().x,
                parent.bottom() - px(popover::CARD_INSET + 30.0 + popover::MENU_GAP + 15.0),
            );
            let click = |window: &mut Window, cx: &mut App, position| {
                window.dispatch_event(
                    gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                        position,
                        ..Default::default()
                    }),
                    cx,
                );
                window.dispatch_event(
                    gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                        button: gpui::MouseButton::Left,
                        position,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
                window.dispatch_event(
                    gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                        button: gpui::MouseButton::Left,
                        position,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
            };
            let hover = |window: &mut Window, cx: &mut App, position| {
                window.dispatch_event(
                    gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                        position,
                        ..Default::default()
                    }),
                    cx,
                );
                window.draw(cx).clear();
                window.draw(cx).clear();
            };
            cx.update_window(handle.into(), |_, window, cx| {
                hover(window, cx, trigger);
                window.draw(cx).clear();
                window.draw(cx).clear();
            })
            .unwrap();
            let submenu = pickers.read_with(cx, |pickers, _| {
                assert_eq!(pickers.setting_menu, Some(ModelSetting::Reasoning));
                assert_eq!(pickers.setting_on_left, on_left);
                pickers
                    .setting_bounds
                    .expect("submenu measured after opening")
            });
            if on_left {
                assert!(submenu.right() < parent.left());
            } else {
                assert!(submenu.left() > parent.right());
            }
            // Clicking the hovered trigger dismisses its child and keeps it closed.
            cx.update_window(handle.into(), |_, window, cx| {
                click(window, cx, trigger);
                window.draw(cx).clear();
            })
            .unwrap();
            pickers.read_with(cx, |pickers, _| {
                assert!(
                    pickers.setting_menu.is_none(),
                    "trigger click dismisses the hovered submenu"
                );
                assert!(pickers.is_open());
            });
            cx.update_window(handle.into(), |_, window, cx| {
                // Moving inside the same hovered row must not reopen it.
                hover(window, cx, trigger + gpui::point(px(2.0), px(0.0)));
                assert!(pickers.read(cx).setting_menu.is_none());
                hover(window, cx, gpui::point(px(8.0), px(8.0)));
                hover(window, cx, trigger);
            })
            .unwrap();
            // Cross the neighboring trigger diagonally toward the open child.
            // It must not steal the menu while the pointer is inside the cone.
            let diagonal = gpui::point(
                if on_left {
                    parent.left() + px(8.0)
                } else {
                    parent.right() - px(8.0)
                },
                trigger.y + px(32.0),
            );
            cx.update_window(handle.into(), |_, window, cx| {
                hover(window, cx, diagonal);
                assert_eq!(pickers.read(cx).setting_menu, Some(ModelSetting::Reasoning));
                assert!(pickers.read(cx).setting_hover.pending().is_some());
            })
            .unwrap();
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(200));
            cx.run_until_parked();
            cx.update_window(handle.into(), |_, window, cx| {
                hover(
                    window,
                    cx,
                    diagonal + gpui::point(px(if on_left { -3.0 } else { 3.0 }), px(0.0)),
                );
            })
            .unwrap();
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(200));
            cx.run_until_parked();
            cx.update_window(handle.into(), |_, window, cx| {
                assert_eq!(
                    pickers.read(cx).setting_menu,
                    Some(ModelSetting::Reasoning),
                    "forward progress renews grace"
                );
                hover(window, cx, submenu.center());
            })
            .unwrap();
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(400));
            cx.run_until_parked();
            pickers.read_with(cx, |pickers, _| {
                assert_eq!(pickers.setting_menu, Some(ModelSetting::Reasoning));
                assert!(pickers.setting_hover.pending().is_none());
            });
            // Resting on the sibling expresses intent to switch after grace.
            cx.update_window(handle.into(), |_, window, cx| {
                hover(window, cx, trigger);
                hover(window, cx, diagonal);
            })
            .unwrap();
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(350));
            cx.run_until_parked();
            pickers.read_with(cx, |pickers, _| {
                assert_eq!(
                    pickers.setting_menu,
                    Some(ModelSetting::Option("contextWindow".into()))
                );
            });
            // Moving away from the cone switches immediately. A click during
            // the grace period dismisses instead, with no delayed reopening.
            cx.update_window(handle.into(), |_, window, cx| {
                hover(window, cx, trigger);
                hover(window, cx, diagonal);
                hover(window, cx, trigger + gpui::point(px(0.0), px(32.0)));
                assert_eq!(
                    pickers.read(cx).setting_menu,
                    Some(ModelSetting::Option("contextWindow".into()))
                );
                hover(window, cx, trigger);
                hover(window, cx, diagonal);
                click(window, cx, diagonal);
                window.draw(cx).clear();
            })
            .unwrap();
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(400));
            cx.run_until_parked();
            pickers.read_with(cx, |pickers, _| {
                assert!(pickers.setting_menu.is_none());
                assert!(pickers.setting_hover.pending().is_none());
                assert!(pickers.is_open());
            });
            cx.update_window(handle.into(), |_, window, cx| hover(window, cx, trigger))
                .unwrap();
            let gap_x = if on_left {
                (submenu.right() + parent.left()) / 2.0
            } else {
                (parent.right() + submenu.left()) / 2.0
            };
            // Leaving the safe corridor closes the child and clears its
            // trigger selection, while preserving the parent picker.
            for outside in [
                gpui::point(gap_x, submenu.bottom() + px(20.0)),
                gpui::point(parent.center().x, parent.top() + px(60.0)),
                gpui::point(px(8.0), px(8.0)),
            ] {
                cx.update_window(handle.into(), |_, window, cx| {
                    hover(window, cx, outside);
                    assert!(pickers.read(cx).setting_menu.is_none());
                    assert_eq!(pickers.read(cx).active, 0);
                    assert!(pickers.read(cx).is_open());
                    hover(window, cx, trigger);
                    assert_eq!(pickers.read(cx).setting_menu, Some(ModelSetting::Reasoning));
                })
                .unwrap();
            }
            // Moving through the gap into the child remains safe.
            let target = gpui::point(
                submenu.left() + px(30.0),
                submenu.bottom() - px(popover::CARD_INSET + 30.0 + popover::MENU_GAP + 15.0),
            );
            for position in [gpui::point(gap_x, trigger.y + px(16.0)), target] {
                cx.update_window(handle.into(), |_, window, cx| {
                    window.dispatch_event(
                        gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                            position,
                            ..Default::default()
                        }),
                        cx,
                    );
                    window.draw(cx).clear();
                })
                .unwrap();
                cx.executor().advance_clock(Duration::from_secs(2));
                cx.run_until_parked();
                pickers.read_with(cx, |pickers, _| {
                    assert!(pickers.is_open());
                    assert_eq!(pickers.setting_menu, Some(ModelSetting::Reasoning));
                });
            }
            cx.update_window(handle.into(), |_, window, cx| click(window, cx, target))
                .unwrap();
            pickers.read_with(cx, |pickers, cx| {
                assert_eq!(pickers.effective_reasoning(cx), Some(ReasoningLevel::Low));
                assert!(pickers.setting_menu.is_none());
                assert!(pickers.is_open());
            });
            // Hover switches directly between sibling triggers without clicking.
            cx.update_window(handle.into(), |_, window, cx| {
                window.draw(cx).clear();
                hover(window, cx, trigger);
                hover(window, cx, trigger + gpui::point(px(0.0), px(32.0)));
                window.draw(cx).clear();
                window.draw(cx).clear();
            })
            .unwrap();
            pickers.read_with(cx, |pickers, _| {
                assert_eq!(
                    pickers.setting_menu,
                    Some(ModelSetting::Option("contextWindow".into()))
                );
                assert!(pickers.is_open());
            });
            // Clicking the parent's search closes only the child and focuses
            // the input on that same click, rather than swallowing the press.
            cx.update_window(handle.into(), |_, window, cx| {
                click(
                    window,
                    cx,
                    gpui::point(parent.center().x, parent.top() + px(60.0)),
                );
                pickers.read_with(cx, |pickers, cx| {
                    assert!(pickers.setting_menu.is_none());
                    assert!(pickers.is_open());
                    assert!(pickers.search.read(cx).focus_handle(cx).is_focused(window));
                });
                window.draw(cx).clear();
                hover(window, cx, trigger);
                window.draw(cx).clear();
                window.draw(cx).clear();
            })
            .unwrap();
            pickers.read_with(cx, |pickers, _| assert!(pickers.setting_menu.is_some()));
            cx.update_window(handle.into(), |_, window, cx| {
                click(window, cx, gpui::point(px(8.0), px(8.0)))
            })
            .unwrap();
            pickers.read_with(cx, |pickers, _| assert!(!pickers.is_open()));
        }
    }

    #[gpui::test]
    fn remembered_options_stay_valid_for_the_sent_model_without_a_catalog(
        cx: &mut gpui::TestAppContext,
    ) {
        let one_m = serde_json::Value::String("1m".into());
        let mut opus = bare_model("opus", "Opus");
        opus.options.push(ModelOption {
            id: "contextWindow".into(),
            label: "Context window".into(),
            choices: ["200k", "1m"]
                .map(|id| ModelOptionChoice {
                    id: id.into(),
                    label: id.into(),
                })
                .into(),
            default_choice: "200k".into(),
        });
        let state = cx.new(|_| AppState::new());
        let pickers = cx.new(|cx| Pickers::new(state, cx));
        pickers.update(cx, |pickers, cx| {
            pickers.defaults.harness = Some(HarnessId::ClaudeCode);
            pickers.models.insert(
                HarnessId::ClaudeCode,
                Loadable::Ready(vec![opus.clone(), bare_model("haiku", "Haiku")]),
            );
            pickers.pick_model("opus".into(), cx);
            pickers.pick_option("contextWindow".into(), "1m".into(), false, cx);
            let resolved = pickers.resolved(cx);
            assert_eq!(resolved.model.as_deref(), Some("opus"));
            assert_eq!(resolved.model_options.get("contextWindow"), Some(&one_m));

            pickers.pick_model("haiku".into(), cx);
            assert!(pickers.resolved(cx).model_options.is_empty());

            // Restart (draft cleared) and send before the catalog is usable.
            for catalog in [
                Loadable::Idle,
                Loadable::Loading,
                Loadable::Error("down".into()),
            ] {
                pickers.config = DraftConfig::default();
                pickers.models.insert(HarnessId::ClaudeCode, catalog);
                let resolved = pickers.resolved(cx);
                assert_eq!(resolved.model.as_deref(), Some("haiku"));
                assert!(
                    resolved.model_options.is_empty(),
                    "Opus's 1M pick must not ride along with Haiku"
                );
            }

            // The Opus memory itself survives for when Opus is picked again.
            pickers.pick_model("opus".into(), cx);
            let resolved = pickers.resolved(cx);
            assert_eq!(resolved.model.as_deref(), Some("opus"));
            assert_eq!(resolved.model_options.get("contextWindow"), Some(&one_m));
        });
    }

    fn descriptor(id: HarnessId, name: &str) -> HarnessDescriptor {
        HarnessDescriptor {
            id,
            name: name.into(),
            installed: true,
            can_install: false,
            enabled: Some(true),
            reasoning_levels: Vec::new(),
            steering_mode: zeron_proto::SteeringMode::StepBoundary,
            supports_steering: false,
        }
    }

    #[test]
    fn tab_search_never_leaves_the_viewed_harness() {
        let descriptors = vec![
            descriptor(HarnessId::ClaudeCode, "Claude Code"),
            descriptor(HarnessId::Codex, "Codex"),
        ];
        let claude = vec![bare_model("fable-5", "Fable 5")];
        let codex = vec![bare_model("gpt-fable", "Fable (Codex)")];
        let models_for = |harness: HarnessId| -> Option<&[Model]> {
            match harness {
                HarnessId::ClaudeCode => Some(claude.as_slice()),
                HarnessId::Codex => Some(codex.as_slice()),
                _ => None,
            }
        };
        // Both catalogs match "fable", but the viewed tab is Claude — the
        // Codex hit must not appear.
        let rows = scoped_model_rows(
            "fable",
            ModelRail::Harness,
            Some(HarnessId::ClaudeCode),
            &descriptors,
            models_for,
            |_, _| false,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].harness, HarnessId::ClaudeCode);
        assert_eq!(rows[0].model.id, "fable-5");
    }

    #[test]
    fn favorites_tab_search_ranks_only_starred_rows() {
        let descriptors = vec![
            descriptor(HarnessId::ClaudeCode, "Claude Code"),
            descriptor(HarnessId::Codex, "Codex"),
        ];
        let claude = vec![bare_model("fable-5", "Fable 5")];
        let codex = vec![bare_model("gpt-fable", "Fable (Codex)")];
        let models_for = |harness: HarnessId| -> Option<&[Model]> {
            match harness {
                HarnessId::ClaudeCode => Some(claude.as_slice()),
                HarnessId::Codex => Some(codex.as_slice()),
                _ => None,
            }
        };
        let starred =
            |harness: HarnessId, model: &str| harness == HarnessId::Codex && model == "gpt-fable";
        let rows = scoped_model_rows(
            "fable",
            ModelRail::Favorites,
            Some(HarnessId::ClaudeCode),
            &descriptors,
            models_for,
            starred,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].harness, HarnessId::Codex);

        // Empty query on the favorites tab: the starred set, nothing else.
        let rows = scoped_model_rows(
            "",
            ModelRail::Favorites,
            Some(HarnessId::ClaudeCode),
            &descriptors,
            models_for,
            starred,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].model.id, "gpt-fable");
    }

    #[test]
    fn harness_tab_lists_stars_first_and_description_still_matches() {
        let descriptors = vec![descriptor(HarnessId::Opencode, "opencode")];
        let mut provider_a = bare_model("glm-5.2-a", "GLM-5.2");
        provider_a.description = Some("Anthropic".into());
        let mut provider_b = bare_model("glm-5.2-b", "GLM-5.2");
        provider_b.description = Some("Baseten".into());
        let models = vec![provider_a, provider_b];
        let models_for = |harness: HarnessId| -> Option<&[Model]> {
            (harness == HarnessId::Opencode).then_some(models.as_slice())
        };
        let starred = |harness: HarnessId, model: &str| {
            harness == HarnessId::Opencode && model == "glm-5.2-b"
        };
        // No query: catalog order with the star floated to the top.
        let rows = scoped_model_rows(
            "",
            ModelRail::Harness,
            Some(HarnessId::Opencode),
            &descriptors,
            models_for,
            starred,
        );
        assert_eq!(rows[0].model.id, "glm-5.2-b");
        assert_eq!(rows[1].model.id, "glm-5.2-a");
        // Provider attribution stays searchable inside the tab.
        let rows = scoped_model_rows(
            "baseten",
            ModelRail::Harness,
            Some(HarnessId::Opencode),
            &descriptors,
            models_for,
            starred,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].model.id, "glm-5.2-b");
    }

    /// Rows show descriptions only to tell same-named models apart.
    #[test]
    fn only_same_named_rows_are_ambiguous() {
        let descriptors = vec![
            descriptor(HarnessId::Opencode, "opencode"),
            descriptor(HarnessId::Devin, "Devin"),
        ];
        let mut glm_a = bare_model("glm-a", "GLM-5.2");
        glm_a.description = Some("Anthropic".into());
        let mut glm_b = bare_model("glm-b", "GLM-5.2");
        glm_b.description = Some("Baseten".into());
        let opencode = vec![glm_a, glm_b, bare_model("kimi", "Kimi")];
        let devin = vec![bare_model("glm", "GLM-5.2")];
        let mut rows = scoped_model_rows(
            "",
            ModelRail::Favorites,
            None,
            &descriptors,
            |harness| match harness {
                HarnessId::Opencode => Some(opencode.as_slice()),
                HarnessId::Devin => Some(devin.as_slice()),
                _ => None,
            },
            |_, _| true,
        );
        mark_ambiguous(&mut rows);
        let flags: Vec<_> = rows
            .iter()
            .map(|r| (r.model.id.as_str(), r.ambiguous))
            .collect();
        assert!(flags.contains(&("glm-a", true)));
        assert!(flags.contains(&("glm-b", true)));
        assert!(flags.contains(&("kimi", false)));
        assert!(
            flags.contains(&("glm", false)),
            "other harness: not a clash"
        );
    }

    #[test]
    fn normalize_drops_default_alias_and_folds_orphan_1m_rows() {
        // The shape an OLDER engine serves: a `default` alias row plus
        // 1M-pinned variants with no bare base. A non-claude harness keeps
        // wire labels (no curated catalog to borrow from).
        let models = normalize_model_rows(
            HarnessId::Codex,
            vec![
                bare_model("default", "Default (recommended)"),
                bare_model("titan[1m]", "Titan (1M context)"),
                bare_model("gpt-x-9[1m]", "GPT X-9"),
                bare_model("nano", "Nano"),
            ],
        );
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["titan", "gpt-x-9", "nano"]
        );
        assert_eq!(models[0].label, "Titan");
        assert_eq!(models[1].label, "GPT X-9");
        // Folded rows pin the Context Window trait to 1M.
        assert!(
            models[0]
                .options
                .iter()
                .any(|o| o.id == "contextWindow" && o.default_choice == "1m")
        );
        assert!(models[2].options.is_empty());

        // A `default`-only list survives (nothing real to prefer).
        let only_default =
            normalize_model_rows(HarnessId::Codex, vec![bare_model("default", "Default")]);
        assert_eq!(only_default.len(), 1);

        // A base-plus-variant pair (already folded by a NEWER engine — the
        // variant never reaches us; belt-and-braces if it does): variant
        // drops, base is untouched.
        let paired = normalize_model_rows(
            HarnessId::Codex,
            vec![
                bare_model("titan-5", "Titan 5"),
                bare_model("titan-5[1m]", "Titan 5 (1M)"),
            ],
        );
        assert_eq!(paired.len(), 1);
        assert_eq!(paired[0].id, "titan-5");

        // Idempotent over a clean list.
        let clean = vec![bare_model("titan-5", "Titan 5")];
        assert_eq!(normalize_model_rows(HarnessId::Codex, clean.clone()), clean);
    }

    #[test]
    fn normalize_gives_claude_rows_their_versioned_catalog_labels() {
        // The real prod shape: alias values with terse names. Claude rows
        // adopt the curated labels so the version number always shows
        // (user request), exact ids included; foreign ids pass through.
        let models = normalize_model_rows(
            HarnessId::ClaudeCode,
            vec![
                bare_model("default", "Default (recommended)"),
                bare_model("opus[1m]", "Opus (1M context)"),
                bare_model("claude-fable-5[1m]", "Fable"),
                bare_model("sonnet", "Sonnet"),
                bare_model("haiku", "Haiku"),
                bare_model("claude-nova-1", "Nova 1"),
            ],
        );
        assert_eq!(
            models.iter().map(|m| m.label.as_str()).collect::<Vec<_>>(),
            vec!["Opus 5.5", "Fable 5", "Sonnet 5", "Haiku 4.5", "Nova 1"]
        );
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["opus", "claude-fable-5", "sonnet", "haiku", "claude-nova-1"]
        );
    }

    #[test]
    fn remembered_picks_keep_only_offered_options() {
        let mut model = bare_model("opus", "Opus");
        model.options = vec![ModelOption {
            id: "context".into(),
            label: "Context window".into(),
            choices: vec![
                ModelOptionChoice {
                    id: "standard".into(),
                    label: "Standard".into(),
                },
                ModelOptionChoice {
                    id: "1m".into(),
                    label: "1M".into(),
                },
            ],
            default_choice: "standard".into(),
        }];
        let mut remembered = serde_json::Map::new();
        remembered.insert("context".into(), serde_json::Value::String("1m".into()));
        remembered.insert("fastMode".into(), serde_json::Value::String("on".into()));
        let mut stale = remembered.clone();
        stale.insert("context".into(), serde_json::Value::String("2m".into()));
        let mut want = serde_json::Map::new();
        want.insert("context".into(), serde_json::Value::String("1m".into()));
        assert_eq!(offered_options(&model, remembered), want);
        assert!(offered_options(&model, stale).is_empty());
    }

    #[test]
    fn folder_paths_and_breadcrumbs() {
        assert_eq!(parent_path("/home/w/dev"), Some("/home/w".to_string()));
        assert_eq!(parent_path("/home"), Some("/".to_string()));
        assert_eq!(parent_path("/home/"), Some("/".to_string()));
        assert_eq!(parent_path("/"), None);
        assert_eq!(parent_path(""), None);
        assert_eq!(child_path("/home", "w"), "/home/w");
        assert_eq!(child_path("/", "home"), "/home");
        let crumbs = breadcrumbs("/home/w/dev");
        let labels: Vec<&str> = crumbs.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["/", "home", "w", "dev"]);
        assert_eq!(crumbs[2].1, "/home/w");
        assert_eq!(breadcrumbs("/").len(), 1);
    }

    #[test]
    fn completion_prefix_lengths() {
        // Case-insensitive; the length indexes into the NAME's bytes.
        assert_eq!(completion_prefix_len("Documents", "doc"), Some(3));
        assert_eq!(&"Documents"[3..], "uments");
        assert_eq!(completion_prefix_len("zeron", "zeron"), Some(5));
        assert_eq!(completion_prefix_len("zeron", ""), Some(0));
        assert_eq!(completion_prefix_len("zeron", "dev"), None);
        // Longer than the name → not a prefix.
        assert_eq!(completion_prefix_len("dev", "devel"), None);
        // Multibyte names slice on a char boundary.
        assert_eq!(completion_prefix_len("héllo", "hé"), Some(3));
        assert_eq!(&"héllo"[3..], "llo");
    }

    #[test]
    fn segment_target_resolution() {
        let names = ["github", "GitHub", "worktree"];
        // Exact casing beats the earlier case-insensitive sibling…
        assert_eq!(segment_target(&names, "GitHub"), Some(1));
        assert_eq!(segment_target(&names, "github"), Some(0));
        // …but with no exact-cased hit, case-insensitive exact still lands.
        assert_eq!(segment_target(&names, "WORKTREE"), Some(2));
        // Unique prefix descends; an ambiguous one keeps the slash honest.
        assert_eq!(segment_target(&names, "work"), Some(2));
        assert_eq!(segment_target(&names, "g"), None);
        assert_eq!(segment_target(&names, "x"), None);
    }

    #[test]
    fn typed_path_target_expands_absolute_and_home_paths() {
        let home = Some("/home/wing");
        assert_eq!(typed_path_target("/disk2/", home), Some("/disk2".into()));
        assert_eq!(
            typed_path_target("/disk2/projects", home),
            Some("/disk2/projects".into())
        );
        assert_eq!(typed_path_target("/", home), Some("/".into()));
        assert_eq!(typed_path_target("~", home), Some("/home/wing".into()));
        assert_eq!(typed_path_target("~/", home), Some("/home/wing".into()));
        assert_eq!(
            typed_path_target("~/github/", home),
            Some("/home/wing/github".into())
        );
        // `~x` is a folder name; relative queries are searches, not paths.
        assert_eq!(typed_path_target("~x", home), None);
        assert_eq!(typed_path_target("src", home), None);
        // `~` can't expand before the device's home is known.
        assert_eq!(typed_path_target("~/github", None), None);
        assert_eq!(typed_path_target("/disk2", None), Some("/disk2".into()));
    }

    #[test]
    fn browser_navigation_reducer() {
        let listing = FolderListing {
            path: "/home/w".into(),
            entries: vec![
                FolderEntry {
                    name: "notes.txt".into(),
                    is_dir: false,
                    is_repo: false,
                },
                FolderEntry {
                    name: "dev".into(),
                    is_dir: true,
                    is_repo: false,
                },
                FolderEntry {
                    name: "zeron".into(),
                    is_dir: true,
                    is_repo: true,
                },
            ],
            truncated: false,
        };
        // Files never show as rows.
        assert_eq!(browser_rows(&listing).len(), 2);
        assert_eq!(browser_rows(&listing)[1].name, "zeron");
    }

    #[test]
    fn resolved_chat_config_requires_harness() {
        let mut resolved = ResolvedRunConfig::default();
        assert!(resolved.chat_config().is_none());
        resolved.harness = Some(HarnessId::ClaudeCode);
        resolved.model = Some("opus".into());
        resolved.reasoning = Some(ReasoningLevel::High);
        let config = resolved.chat_config().expect("harness set");
        assert_eq!(config.harness, HarnessId::ClaudeCode);
        assert_eq!(config.model.as_deref(), Some("opus"));
        assert_eq!(config.sandbox, SandboxLevel::WorkspaceWrite);
    }

    #[test]
    fn default_model_is_first_catalog_row() {
        let models = vec![
            Model {
                id: "flagship".into(),
                label: "Flagship".into(),
                description: None,
                reasoning_levels: vec![],
                options: vec![],
            },
            Model {
                id: "fast".into(),
                label: "Fast".into(),
                description: None,
                reasoning_levels: vec![],
                options: vec![],
            },
        ];
        assert_eq!(default_model(&models).map(|m| &*m.id), Some("flagship"));
        assert!(default_model(&[]).is_none());
    }

    #[test]
    fn default_reasoning_prefers_high_then_medium() {
        use ReasoningLevel::*;
        // Recommended default is High (user-corrected), even on full ladders.
        assert_eq!(
            default_reasoning(&[Low, Medium, High, XHigh, Max, Ultracode, Ultrathink]),
            Some(High)
        );
        assert_eq!(default_reasoning(&[Low, Medium, High, Max]), Some(High));
        // No High: Medium.
        assert_eq!(default_reasoning(&[Minimal, Low, Medium]), Some(Medium));
        // Neither offered: first entry.
        assert_eq!(default_reasoning(&[Minimal, Low]), Some(Minimal));
        // Ladder-less model (Haiku): no reasoning at all.
        assert_eq!(default_reasoning(&[]), None);
    }

    #[test]
    fn clamp_reasoning_keeps_offered_levels_and_heals_foreign_ones() {
        use ReasoningLevel::*;
        let ladder = [Low, Medium, High, Max];
        // A pick the ladder offers survives.
        assert_eq!(clamp_reasoning(Some(Max), &ladder), Some(Max));
        // A remembered level the new model doesn't offer heals to its default.
        assert_eq!(clamp_reasoning(Some(XHigh), &ladder), Some(High));
        // No pick at all resolves to the concrete default too.
        assert_eq!(clamp_reasoning(None, &ladder), Some(High));
        assert_eq!(clamp_reasoning(Some(High), &[]), None);
    }

    #[test]
    fn mock_harness_hidden_unless_alone() {
        let descriptor = |id: HarnessId, name: &str| HarnessDescriptor {
            id,
            name: name.into(),
            supports_steering: true,
            steering_mode: zeron_proto::SteeringMode::StepBoundary,
            reasoning_levels: vec![],
            installed: true,
            can_install: false,
            enabled: None,
        };
        let mixed = vec![
            descriptor(HarnessId::Mock, "Mock"),
            descriptor(HarnessId::ClaudeCode, "Claude Code"),
        ];
        // Env-independent core: mock hidden in production…
        let visible = visible_harnesses_impl(&mixed, false);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, HarnessId::ClaudeCode);
        let only_mock = vec![descriptor(HarnessId::Mock, "Mock")];
        assert_eq!(visible_harnesses_impl(&only_mock, false).len(), 1);
        // …and opted back in by ZERON_HARNESS=mock (the e2e rig).
        assert_eq!(visible_harnesses_impl(&mixed, true).len(), 2);
        assert_eq!(visible_harnesses_impl(&mixed, true)[0].id, HarnessId::Mock);
    }

    #[test]
    fn offered_harnesses_follow_the_catalog_enabled_flags() {
        let descriptor = |id: HarnessId, name: &str, enabled: Option<bool>| HarnessDescriptor {
            id,
            name: name.into(),
            supports_steering: true,
            steering_mode: zeron_proto::SteeringMode::StepBoundary,
            reasoning_levels: vec![],
            installed: true,
            can_install: false,
            enabled,
        };
        let catalog = |claude: Option<bool>, codex: Option<bool>, grok: Option<bool>| {
            vec![
                descriptor(HarnessId::Mock, "Mock", Some(false)),
                descriptor(HarnessId::ClaudeCode, "Claude Code", claude),
                descriptor(HarnessId::Codex, "Codex", codex),
                descriptor(HarnessId::Grok, "Grok", grok),
            ]
        };
        // A catalog from an engine predating the flag (all None) follows its
        // installed probes, so every detected real harness is offered.
        let offered = offered_harnesses_impl(&catalog(None, None, None), false);
        assert_eq!(
            offered.iter().map(|d| d.id).collect::<Vec<_>>(),
            vec![HarnessId::ClaudeCode, HarnessId::Codex, HarnessId::Grok]
        );
        // The device's flags win: Grok on, Codex off; catalog order holds.
        let offered = offered_harnesses_impl(&catalog(Some(true), Some(false), Some(true)), false);
        assert_eq!(
            offered.iter().map(|d| d.id).collect::<Vec<_>>(),
            vec![HarnessId::ClaudeCode, HarnessId::Grok]
        );
        // The dev-rig mock opt-in survives the enabled filter (and Grok's
        // unknown flag still resolves through its installed probe).
        let offered = offered_harnesses_impl(&catalog(Some(true), Some(false), None), true);
        assert_eq!(
            offered.iter().map(|d| d.id).collect::<Vec<_>>(),
            vec![HarnessId::Mock, HarnessId::ClaudeCode, HarnessId::Grok]
        );
        // Nothing enabled offers nothing — the composer renders the
        // no-agents empty state instead of resurrecting disabled agents.
        let offered =
            offered_harnesses_impl(&catalog(Some(false), Some(false), Some(false)), false);
        assert!(offered.is_empty());
        // So does a legacy catalog whose installed probes all failed: never
        // resurface unrunnable agents just to avoid an empty picker.
        let mut missing = catalog(None, None, None);
        missing.iter_mut().for_each(|d| d.installed = false);
        assert!(offered_harnesses_impl(&missing, false).is_empty());
    }

    #[test]
    fn offered_harnesses_require_an_installed_cli() {
        let descriptor =
            |id: HarnessId, name: &str, enabled: Option<bool>, installed: bool| HarnessDescriptor {
                id,
                name: name.into(),
                supports_steering: true,
                steering_mode: zeron_proto::SteeringMode::StepBoundary,
                reasoning_levels: vec![],
                installed,
                can_install: false,
                enabled,
            };
        // Enabled-but-missing-CLI agents stay out of the rail; an installed
        // enabled one rides along. A live engine no longer stamps that
        // combination (enablement follows detection), but a catalog from an
        // older engine still can — the filter is the cross-version defense.
        let catalog = vec![
            descriptor(HarnessId::ClaudeCode, "Claude Code", Some(true), false),
            descriptor(HarnessId::Codex, "Codex", Some(true), false),
            descriptor(HarnessId::Grok, "Grok", Some(true), true),
        ];
        let offered = offered_harnesses_impl(&catalog, false);
        assert_eq!(
            offered.iter().map(|d| d.id).collect::<Vec<_>>(),
            vec![HarnessId::Grok]
        );
        // Nothing enabled AND installed: an empty offered set — the fresh
        // machine where the default-enabled Claude/Codex have no CLIs (#128).
        // No fallback: offering them again would only manufacture
        // NotInstalled errors at send; the composer shows the no-agents
        // state and blocks new sends instead.
        let catalog = vec![
            descriptor(HarnessId::ClaudeCode, "Claude Code", Some(true), false),
            descriptor(HarnessId::Codex, "Codex", Some(false), false),
            descriptor(HarnessId::Grok, "Grok", Some(false), true),
        ];
        let offered = offered_harnesses_impl(&catalog, false);
        assert!(offered.is_empty());
    }
}

/// Catalog for the isolated native screenshot fixture; never used by the app.
#[cfg(feature = "project-palette-fixture")]
impl Pickers {
    pub(crate) fn fixture_model_catalog(&mut self, cx: &mut Context<Self>) {
        if matches!(self.models.get(&HarnessId::Codex), Some(Loadable::Ready(_))) {
            return;
        }
        self.config.harness = Some(HarnessId::Codex);
        self.config.model = Some("gpt-5.4".into());
        self.harnesses = Loadable::Ready(serde_json::from_value(serde_json::json!([
            {"id":"codex","name":"Codex","supportsSteering":true,"steeringMode":"step-boundary","reasoningLevels":[]}
        ])).unwrap());
        self.models.insert(HarnessId::Codex, Loadable::Ready(serde_json::from_value(serde_json::json!([
            {"id":"gpt-5.4","label":"GPT-5.4","description":"For complex coding and reasoning", "reasoningLevels":["low","medium","high","xhigh"], "options":[
                {"id":"context-window","label":"Context window","defaultChoice":"standard","choices":[{"id":"standard","label":"Standard"},{"id":"1m","label":"1M tokens"}]},
                {"id":"service-tier","label":"Service tier","defaultChoice":"auto","choices":[{"id":"auto","label":"Standard"},{"id":"fast","label":"Fast"}]}
            ]},
            {"id":"gpt-5.3-codex","label":"GPT-5.3 Codex","description":"Optimized for agentic coding"},
            {"id":"gpt-5.2","label":"GPT-5.2","description":"General purpose reasoning"},
            {"id":"gpt-5.1-codex-mini","label":"GPT-5.1 Codex Mini","description":"Fast, efficient coding"}
        ])).unwrap()));
        self.catalog_rev += 1;
        cx.notify();
    }
}
