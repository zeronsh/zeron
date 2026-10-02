//! Saved workflows in the shell: the launcher dialog (a generated args form
//! that starts a run in the current chat or a new one), "Save as workflow…",
//! and "Run again".
//!
//! A person starting a saved workflow is the approval: no agent turn exists to
//! carry the engine's approval question, so the launcher shows what the
//! approval would (the graph summary, the commands, every argument value) and
//! its Run button is the answer. The engine only honours that for saved
//! workflows (`byUser` + `saved`), never for a script.
//!
//! Child module of `shell` so it can reach the shell's private state.

use super::*;
use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::popover::Loadable;
use crate::settings::widgets as sw;
use crate::workflow::saved::{ArgsForm, FormErrors, SaveErrors, SaveForm, infer_args};
use serde_json::{Value, json};
use zeron_proto::{SavedArg, SavedScope, SavedWorkflowDetail, SavedWorkflowSummary};

// ── where a run can start ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LaunchTarget {
    /// A chat that already exists.
    Chat {
        chat_id: String,
        title: String,
        device_id: String,
    },
    /// A fresh chat in a project of this device.
    NewChat { space_id: String, label: String },
}

impl LaunchTarget {
    fn label(&self) -> String {
        match self {
            LaunchTarget::Chat { title, .. } => format!("This chat · {title}"),
            LaunchTarget::NewChat { label, .. } => format!("New chat in {label}"),
        }
    }
}

fn chat_in_project(chat: &zeron_proto::Chat, wf: &SavedWorkflowSummary) -> bool {
    match (&wf.space_id, &wf.project_root) {
        (None, None) => false,
        (space, root) => {
            space.is_some() && chat.space_id == *space
                || root
                    .as_deref()
                    .is_some_and(|r| chat.cwd.as_deref().is_some_and(|c| c == r))
        }
    }
}

/// The places a run of `wf` may start: the open chat (when the workflow is
/// visible there) and a new chat in each project of this device that can see
/// it. A project workflow only runs in its own project.
pub(super) fn launch_targets(
    state: &AppState,
    wf: &SavedWorkflowSummary,
    origin_chat: Option<&str>,
) -> Vec<LaunchTarget> {
    let mut targets = Vec::new();
    let chat_id = origin_chat
        .map(str::to_owned)
        .or_else(|| state.selected_chat.clone());
    if let Some(chat) = chat_id
        .as_deref()
        .and_then(|id| state.chats.iter().find(|c| c.id == id))
        .filter(|c| !c.archived)
        .filter(|c| wf.scope != SavedScope::Project || chat_in_project(c, wf))
    {
        targets.push(LaunchTarget::Chat {
            chat_id: chat.id.clone(),
            title: transcript::single_line(chat.title.as_deref().unwrap_or("New session")),
            device_id: chat.device_id.clone(),
        });
    }
    let local = state.local_device_id.as_deref();
    let selected = state.selected_space_row().map(|s| s.id.clone());
    let mut spaces: Vec<&zeron_proto::Space> = state
        .spaces_sorted()
        .into_iter()
        .filter(|s| local.is_none_or(|l| s.device_id == l))
        .filter(|s| {
            wf.scope != SavedScope::Project
                || wf.space_id.as_deref() == Some(s.id.as_str())
                || wf.project_root.as_deref() == Some(s.path.as_str())
        })
        .collect();
    // The project the composer points at first.
    spaces.sort_by_key(|s| Some(&s.id) != selected.as_ref());
    for space in spaces {
        targets.push(LaunchTarget::NewChat {
            space_id: space.id.clone(),
            label: space.display_name().to_owned(),
        });
    }
    targets
}

/// A project workflow starts in its own project; anything else in the chat
/// the person is looking at, else the first project.
pub(super) fn default_target(
    targets: &[LaunchTarget],
    wf: &SavedWorkflowSummary,
    origin_chat: Option<&str>,
) -> usize {
    let first_chat = targets
        .iter()
        .position(|t| matches!(t, LaunchTarget::Chat { .. }));
    let first_new = targets
        .iter()
        .position(|t| matches!(t, LaunchTarget::NewChat { .. }));
    if origin_chat.is_some() {
        return first_chat.or(first_new).unwrap_or(0);
    }
    if wf.scope == SavedScope::Project {
        return first_new.or(first_chat).unwrap_or(0);
    }
    first_chat.or(first_new).unwrap_or(0)
}

// ── state ─────────────────────────────────────────────────────────────────

pub(super) struct Launcher {
    wf: SavedWorkflowSummary,
    preview: Loadable<SavedWorkflowDetail>,
    form: ArgsForm,
    inputs: Vec<Option<Entity<ComposerInput>>>,
    errors: FormErrors,
    targets: Vec<LaunchTarget>,
    target: usize,
    pending: bool,
    error: Option<SharedString>,
    focus_pending: bool,
    _subs: Vec<Subscription>,
}

pub(super) struct SaveDialog {
    chat_id: String,
    run_id: String,
    form: SaveForm,
    name: Entity<ComposerInput>,
    description: Entity<ComposerInput>,
    when_to_use: Entity<ComposerInput>,
    errors: SaveErrors,
    /// Declarations inferred from the arguments the run was started with.
    args: Vec<SavedArg>,
    has_project: bool,
    /// The name is taken in that scope: the next press replaces it.
    replace: bool,
    pending: bool,
    error: Option<SharedString>,
    focus_pending: bool,
    _subs: Vec<Subscription>,
}

#[derive(Default)]
pub(super) struct SavedUi {
    launcher: Option<Launcher>,
    save: Option<SaveDialog>,
    task: Option<Task<()>>,
    preview_task: Option<Task<()>>,
    /// Capture knob `ZERON_OPEN_SAVED` (screenshots), consumed once:
    /// `launch:<name>[:submit]` opens that workflow's launcher (`:submit`
    /// presses Run, which shows the validation errors); `save[:<description>]`
    /// opens "Save as workflow…" for the newest run, description typed in;
    /// `slash:<text>` types into the composer to open its completion.
    capture: Option<String>,
    prefill_description: Option<String>,
}

impl SavedUi {
    pub(super) fn from_env() -> Self {
        Self {
            capture: std::env::var("ZERON_OPEN_SAVED").ok(),
            ..Self::default()
        }
    }

    pub(super) fn is_open(&self) -> bool {
        self.launcher.is_some() || self.save.is_some()
    }

    /// Close the dialogs (Escape); the capture knob stays spent.
    pub(super) fn close_all(&mut self) {
        self.launcher = None;
        self.save = None;
        self.preview_task = None;
    }
}

/// `targetDeviceId` for a chat hosted elsewhere.
fn target_device(state: &AppState, chat_id: &str) -> Option<String> {
    let chat = state.chats.iter().find(|c| c.id == chat_id)?;
    (state.local_device_id.as_deref() != Some(chat.device_id.as_str()))
        .then(|| chat.device_id.clone())
}

fn with_target(mut params: Value, target: Option<String>) -> Value {
    if let Some(t) = target {
        params["targetDeviceId"] = t.into();
    }
    params
}

fn rpc_error_text(e: &zeron_rpc::RpcError) -> String {
    match e {
        zeron_rpc::RpcError::UnknownMethod(_) => {
            "Update Zeron on that device to run saved workflows.".into()
        }
        other => other.to_string(),
    }
}

impl Shell {
    /// Screenshot knob `ZERON_OPEN_SAVED`, polled from the shell's state
    /// observer until its preconditions (a selected chat, a run) hold.
    pub(super) fn capture_saved_workflows(&mut self, cx: &mut Context<Self>) {
        let Some(spec) = self.saved_ui.capture.clone() else {
            return;
        };
        let (chat, run) = {
            let state = self.state.read(cx);
            (
                state.selected_chat.clone(),
                state.workflows.runs.last().map(|r| r.header.run_id.clone()),
            )
        };
        let Some(chat) = chat else { return };
        if let Some(rest) = spec.strip_prefix("launch:") {
            let Some(engine) = self.state.read(cx).engine().cloned() else {
                return;
            };
            self.saved_ui.capture = None;
            let (name, submit) = match rest.split_once(':') {
                Some((name, "submit")) => (name.to_owned(), true),
                _ => (rest.to_owned(), false),
            };
            self.saved_ui.task = Some(cx.spawn(async move |this, cx| {
                let list = engine
                    .client()
                    .call(methods::WORKFLOW_SAVED_LIST, json!({ "all": true }))
                    .await
                    .ok()
                    .and_then(|v| serde_json::from_value::<zeron_proto::SavedWorkflowList>(v).ok());
                this.update(cx, |shell, cx| {
                    let Some(wf) = list.and_then(|l| {
                        l.workflows
                            .into_iter()
                            .find(|w| w.name == name && w.shadowed_by.is_none())
                    }) else {
                        return;
                    };
                    shell.open_saved_launcher_with(wf, Value::Null, Some(chat), None, cx);
                    if submit {
                        shell.submit_saved_launch(cx);
                    }
                })
                .ok();
            }));
        } else if let Some(rest) = spec.strip_prefix("save") {
            let Some(run) = run else { return };
            self.saved_ui.capture = None;
            self.saved_ui.prefill_description = rest
                .strip_prefix(':')
                .filter(|d| !d.is_empty())
                .map(str::to_owned);
            self.open_save_workflow(chat, run, cx);
        } else if let Some(text) = spec.strip_prefix("slash:") {
            self.saved_ui.capture = None;
            // Typing right at boot loses the draft to the composer's own
            // chat switch: wait for the chat to settle, like a person would.
            let text = text.to_owned();
            self.saved_ui.task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(2500))
                    .await;
                this.update(cx, |shell, cx| {
                    shell
                        .composer
                        .update(cx, |composer, cx| composer.capture_draft(&text, cx))
                })
                .ok();
            }));
        } else {
            self.saved_ui.capture = None;
        }
    }

    // ── launcher ──────────────────────────────────────────────────────────

    /// Open the launcher from Settings.
    pub(super) fn open_saved_launcher(&mut self, wf: SavedWorkflowSummary, cx: &mut Context<Self>) {
        self.open_saved_launcher_with(wf, Value::Null, None, None, cx);
    }

    /// Open the launcher with some values already typed (from `/workflow`) and
    /// optionally a reason ("ticket is required").
    pub(super) fn open_saved_launcher_with(
        &mut self,
        wf: SavedWorkflowSummary,
        given: Value,
        origin_chat: Option<String>,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let targets = launch_targets(self.state.read(cx), &wf, origin_chat.as_deref());
        let target = default_target(&targets, &wf, origin_chat.as_deref());
        let form = ArgsForm::new(&wf.args, &given);
        let mut subs = Vec::new();
        let inputs = form
            .fields
            .iter()
            .enumerate()
            .map(|(ix, f)| {
                if f.is_toggle() {
                    return None;
                }
                let placeholder = if f.is_multiline() {
                    format!("{} (Shift+Enter for a new line)", f.placeholder())
                } else {
                    f.placeholder()
                };
                let multiline = f.is_multiline();
                let input = cx.new(|cx| {
                    let input = ComposerInput::new(placeholder, cx)
                        .with_accessibility_role(gpui::Role::TextInput);
                    if multiline {
                        input
                    } else {
                        input.with_single_line()
                    }
                });
                input.update(cx, |input, cx| input.set_text(f.text.clone(), cx));
                subs.push(
                    cx.subscribe(&input, move |this: &mut Shell, input, event, cx| {
                        match event {
                            ComposerInputEvent::Edited => {
                                let text = input.read(cx).text().to_owned();
                                if let Some(l) = this.saved_ui.launcher.as_mut() {
                                    l.form.set_text(ix, text);
                                    l.errors.fields.remove(&ix);
                                    l.error = None;
                                }
                                cx.notify();
                            }
                            // Enter submits a one-line field; a JSON box keeps
                            // Enter for itself (Shift+Enter breaks the line).
                            ComposerInputEvent::Submitted if !multiline => {
                                this.submit_saved_launch(cx)
                            }
                            _ => {}
                        }
                    }),
                );
                Some(input)
            })
            .collect();
        self.saved_ui.launcher = Some(Launcher {
            wf: wf.clone(),
            preview: Loadable::Loading,
            form,
            inputs,
            errors: FormErrors::default(),
            targets,
            target,
            pending: false,
            error: error.map(Into::into),
            focus_pending: true,
            _subs: subs,
        });
        self.load_launcher_preview(wf, cx);
        cx.notify();
    }

    /// What the script would do (the approval's graph), for the dialog.
    fn load_launcher_preview(&mut self, wf: SavedWorkflowSummary, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let mut params = json!({ "name": wf.name, "scope": wf.scope });
        if let Some(space) = &wf.space_id {
            params["spaceId"] = space.clone().into();
        }
        let key = (wf.name.clone(), wf.scope);
        self.saved_ui.preview_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::WORKFLOW_SAVED_GET, params)
                .await
                .map_err(|e| e.to_string())
                .and_then(|v| {
                    serde_json::from_value::<SavedWorkflowDetail>(v).map_err(|e| e.to_string())
                });
            this.update(cx, |shell, cx| {
                if let Some(l) = shell
                    .saved_ui
                    .launcher
                    .as_mut()
                    .filter(|l| (l.wf.name.clone(), l.wf.scope) == key)
                {
                    l.preview = match result {
                        Ok(d) => Loadable::Ready(d),
                        Err(e) => Loadable::Error(e),
                    };
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    pub(super) fn close_saved_launcher(&mut self, cx: &mut Context<Self>) {
        if self.saved_ui.launcher.take().is_some() {
            self.saved_ui.preview_task = None;
            cx.notify();
        }
    }

    fn submit_saved_launch(&mut self, cx: &mut Context<Self>) {
        let Some(l) = self.saved_ui.launcher.as_mut() else {
            return;
        };
        if l.pending {
            return;
        }
        let args = match l.form.collect() {
            Ok(args) => args,
            Err(errors) => {
                l.errors = errors;
                cx.notify();
                return;
            }
        };
        let Some(target) = l.targets.get(l.target).cloned() else {
            l.error =
                Some("There is nowhere to run it: open a chat or add a project first.".into());
            cx.notify();
            return;
        };
        l.errors = FormErrors::default();
        l.error = None;
        l.pending = true;
        let wf = l.wf.clone();
        cx.notify();
        self.start_saved(wf, args, target, true, cx);
    }

    /// Start `wf` in `target`. From the launcher (`from_dialog`) a failure is
    /// shown in the dialog; otherwise (re-run, `/workflow`) as a notice.
    pub(super) fn start_saved(
        &mut self,
        wf: SavedWorkflowSummary,
        args: Value,
        target: LaunchTarget,
        from_dialog: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.finish_saved_start(Err("Engine not connected".into()), None, from_dialog, cx);
            return;
        };
        let (chat_id, create, remote) = match &target {
            LaunchTarget::Chat {
                chat_id, device_id, ..
            } => {
                let local = self.state.read(cx).local_device_id.clone();
                (
                    chat_id.clone(),
                    None,
                    (local.as_deref() != Some(device_id.as_str())).then(|| device_id.clone()),
                )
            }
            LaunchTarget::NewChat { space_id, .. } => {
                let id = uuid::Uuid::new_v4().to_string();
                let mut create = json!({ "op": "createChat", "chatId": id, "spaceId": space_id });
                if let Some(config) = self.composer.read(cx).resolved_chat_config(cx)
                    && let Ok(config) = serde_json::to_value(&config)
                {
                    create["config"] = config;
                }
                (id, Some(create), None)
            }
        };
        let is_new = create.is_some();
        let start = with_target(
            json!({
                "chatId": chat_id,
                "saved": { "name": wf.name, "scope": wf.scope, "args": args },
                "byUser": true,
            }),
            remote,
        );
        let title = format!("Workflow: {}", wf.name);
        let open = chat_id.clone();
        self.saved_ui.task = Some(cx.spawn(async move |this, cx| {
            let client = engine.client();
            let result: Result<(), String> = async {
                if let Some(create) = create {
                    client
                        .call(methods::MUTATE, create)
                        .await
                        .map_err(|e| rpc_error_text(&e))?;
                    // A name in the sidebar; best effort.
                    let _ = client
                        .call(
                            methods::MUTATE,
                            json!({ "op": "renameChat", "chatId": open, "title": title }),
                        )
                        .await;
                }
                client
                    .call(methods::WORKFLOW_START, start)
                    .await
                    .map(|_| ())
                    .map_err(|e| rpc_error_text(&e))
            }
            .await;
            this.update(cx, |shell, cx| {
                shell.finish_saved_start(result, Some((open, is_new)), from_dialog, cx)
            })
            .ok();
        }));
    }

    fn finish_saved_start(
        &mut self,
        result: Result<(), String>,
        chat: Option<(String, bool)>,
        from_dialog: bool,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(()) => {
                self.saved_ui.launcher = None;
                if let Some((chat_id, _)) = chat {
                    // The run's card appears in the chat as the engine journals it.
                    self.open_chat(chat_id, cx);
                }
            }
            Err(message) => {
                if from_dialog && let Some(l) = self.saved_ui.launcher.as_mut() {
                    l.pending = false;
                    l.error = Some(message.into());
                } else {
                    self.sidebar_notice = Some(message.into());
                }
            }
        }
        cx.notify();
    }

    // ── run again / save as ───────────────────────────────────────────────

    /// `/workflow …` in a composer resolved to a saved workflow: start it in
    /// that chat — or, with a required argument missing, ask for it.
    pub(super) fn saved_workflow_from_composer(
        &mut self,
        chat_id: String,
        wf: SavedWorkflowSummary,
        args: Value,
        missing: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if missing.is_empty() {
            let chat = self
                .state
                .read(cx)
                .chats
                .iter()
                .find(|c| c.id == chat_id)
                .cloned();
            let Some(chat) = chat else { return };
            let target = LaunchTarget::Chat {
                chat_id,
                title: transcript::single_line(chat.title.as_deref().unwrap_or("New session")),
                device_id: chat.device_id,
            };
            self.start_saved(wf, args, target, false, cx);
        } else {
            let reason = format!(
                "{} {} required.",
                missing.join(", "),
                if missing.len() == 1 { "is" } else { "are" }
            );
            self.open_saved_launcher_with(wf, args, Some(chat_id), Some(reason), cx);
        }
    }

    /// "Run again": the same saved workflow with the same arguments, in the
    /// same chat. The engine re-reads the file, validates the arguments and a
    /// person's click approves.
    pub(super) fn rerun_saved(&mut self, chat_id: String, run_id: String, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let target = target_device(self.state.read(cx), &chat_id);
        let get = with_target(json!({ "runId": run_id, "include": [] }), target.clone());
        let title = self
            .state
            .read(cx)
            .chats
            .iter()
            .find(|c| c.id == chat_id)
            .and_then(|c| c.title.clone())
            .unwrap_or_default();
        self.saved_ui.task = Some(cx.spawn(async move |this, cx| {
            let client = engine.client();
            let result: Result<(), String> = async {
                let got = client
                    .call(methods::WORKFLOW_GET, get)
                    .await
                    .map_err(|e| rpc_error_text(&e))?;
                let saved = got
                    .get("saved")
                    .cloned()
                    .ok_or("This run did not come from a saved workflow.")?;
                let start = with_target(
                    json!({
                        "chatId": chat_id,
                        "saved": {
                            "name": saved["name"],
                            "scope": saved["scope"],
                            "args": got.get("args").cloned().unwrap_or(json!({})),
                        },
                        "byUser": true,
                    }),
                    target,
                );
                client
                    .call(methods::WORKFLOW_START, start)
                    .await
                    .map(|_| ())
                    .map_err(|e| rpc_error_text(&e))
            }
            .await;
            if let Err(message) = result {
                this.update(cx, |shell, cx| {
                    shell.sidebar_notice = Some(format!("Run again failed: {message}").into());
                    cx.notify();
                })
                .ok();
            }
            let _ = title;
        }));
    }

    /// "Save as workflow…": ask for a name and description, then save the
    /// run's script. The dialog is the approval (the chat has no live turn to
    /// ask on); the engine writes atomically inside the workflows folder.
    pub(super) fn open_save_workflow(
        &mut self,
        chat_id: String,
        run_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let target = target_device(self.state.read(cx), &chat_id);
        let get = with_target(json!({ "runId": run_id, "include": [] }), target);
        let has_project = self
            .state
            .read(cx)
            .chats
            .iter()
            .find(|c| c.id == chat_id)
            .is_some_and(|c| c.space_id.is_some());
        self.saved_ui.task = Some(cx.spawn(async move |this, cx| {
            let got = engine.client().call(methods::WORKFLOW_GET, get).await;
            this.update(cx, |shell, cx| match got {
                Ok(got) => {
                    let name = got["run"]["name"].as_str().unwrap_or("workflow").to_owned();
                    let args = infer_args(got.get("args").unwrap_or(&Value::Null));
                    shell.build_save_dialog(chat_id, run_id, name, args, has_project, cx);
                }
                Err(e) => {
                    shell.sidebar_notice =
                        Some(format!("Could not read the run: {}", rpc_error_text(&e)).into());
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn build_save_dialog(
        &mut self,
        chat_id: String,
        run_id: String,
        run_name: String,
        args: Vec<SavedArg>,
        has_project: bool,
        cx: &mut Context<Self>,
    ) {
        let form = SaveForm::for_run(&run_name, has_project);
        let mut subs = Vec::new();
        let mut field =
            |placeholder: &'static str, text: String, which: u8, cx: &mut Context<Self>| {
                let input = cx.new(|cx| {
                    ComposerInput::new(placeholder, cx)
                        .with_single_line()
                        .with_accessibility_role(gpui::Role::TextInput)
                });
                input.update(cx, |i, cx| i.set_text(text, cx));
                subs.push(cx.subscribe(
                    &input,
                    move |this: &mut Shell, input, event, cx| match event {
                        ComposerInputEvent::Edited => {
                            let text = input.read(cx).text().to_owned();
                            if let Some(d) = this.saved_ui.save.as_mut() {
                                match which {
                                    0 => {
                                        d.form.name = text;
                                        d.errors.name = None;
                                    }
                                    1 => {
                                        d.form.description = text;
                                        d.errors.description = None;
                                    }
                                    _ => {
                                        d.form.when_to_use = text;
                                        d.errors.when_to_use = None;
                                    }
                                }
                                d.replace = false;
                                d.error = None;
                            }
                            cx.notify();
                        }
                        ComposerInputEvent::Submitted => this.submit_save_workflow(cx),
                        _ => {}
                    },
                ));
                input
            };
        let name = field("pr-review", form.name.clone(), 0, cx);
        let description = field(
            "What it does, in one line",
            self.saved_ui.prefill_description.take().unwrap_or_default(),
            1,
            cx,
        );
        let when_to_use = field("When to use it (optional)", String::new(), 2, cx);
        let mut form = form;
        form.description = description.read(cx).text().to_owned();
        self.saved_ui.save = Some(SaveDialog {
            chat_id,
            run_id,
            form,
            name,
            description,
            when_to_use,
            errors: SaveErrors::default(),
            args,
            has_project,
            replace: false,
            pending: false,
            error: None,
            focus_pending: true,
            _subs: subs,
        });
        cx.notify();
    }

    fn submit_save_workflow(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.saved_ui.save.as_mut() else {
            return;
        };
        if d.pending {
            return;
        }
        if let Err(errors) = d.form.validate(d.has_project) {
            d.errors = errors;
            cx.notify();
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            d.error = Some("Engine not connected".into());
            cx.notify();
            return;
        };
        d.pending = true;
        d.error = None;
        let target = target_device(self.state.read(cx), &d.chat_id);
        let name = d.form.name.trim().to_owned();
        let scope = d.form.scope;
        let args: Vec<Value> = d
            .args
            .iter()
            .filter_map(|a| serde_json::to_value(a).ok())
            .collect();
        let params = with_target(
            json!({
                "chatId": d.chat_id,
                "name": name,
                "description": d.form.description.trim(),
                "whenToUse": Some(d.form.when_to_use.trim()).filter(|w| !w.is_empty()),
                "args": args,
                "scope": scope,
                "fromRun": d.run_id,
                "byUser": true,
                "overwrite": d.replace,
            }),
            target,
        );
        cx.notify();
        self.saved_ui.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::WORKFLOW_SAVED_SAVE, params)
                .await;
            this.update(cx, |shell, cx| {
                let Some(d) = shell.saved_ui.save.as_mut() else {
                    return;
                };
                d.pending = false;
                match result {
                    Ok(_) => {
                        shell.saved_ui.save = None;
                        shell.sidebar_notice = Some(
                            format!(
                                "Saved {name} as a {} workflow. Find it in Settings → Workflows.",
                                scope.label().to_lowercase()
                            )
                            .into(),
                        );
                    }
                    Err(e) => {
                        let text = rpc_error_text(&e);
                        if text.contains("already exists") {
                            d.replace = true;
                            d.error = Some(
                                format!(
                                    "A {} workflow named {name} already exists. Press Replace to overwrite it.",
                                    scope.label().to_lowercase()
                                )
                                .into(),
                            );
                        } else {
                            d.error = Some(text.into());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    // ── overlays ──────────────────────────────────────────────────────────

    pub(super) fn render_saved_workflow_overlays(
        &mut self,
        viewport: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = Theme::of(cx).clone();
        let mut out = Vec::new();
        if let Some(l) = self.saved_ui.launcher.as_mut()
            && std::mem::take(&mut l.focus_pending)
            && let Some(first) = l.inputs.iter().flatten().next()
        {
            window.focus(&first.focus_handle(cx), cx);
        }
        if let Some(card) = self.render_launcher_card(&theme, cx) {
            out.push(popover::modal("saved-workflow-launcher", viewport, card));
        }
        if let Some(d) = self.saved_ui.save.as_mut()
            && std::mem::take(&mut d.focus_pending)
        {
            let focus = if d.form.name.is_empty() {
                &d.name
            } else {
                &d.description
            };
            window.focus(&focus.focus_handle(cx), cx);
        }
        if let Some(card) = self.render_save_card(&theme, cx) {
            out.push(popover::modal("saved-workflow-save", viewport, card));
        }
        out
    }

    fn render_launcher_card(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let l = self.saved_ui.launcher.as_ref()?;
        let wf = l.wf.clone();
        let pending = l.pending;
        let single_target = l.targets.len() <= 1;

        // Where it runs.
        let targets = (!l.targets.is_empty()).then(|| {
            let shown = l.targets.iter().enumerate().take(8).map(|(ix, t)| {
                let selected = ix == l.target;
                div()
                    .id(SharedString::from(format!("launch-target-{ix}")))
                    .role(gpui::Role::RadioButton)
                    .aria_label(SharedString::from(t.label()))
                    .tab_index(0)
                    .cursor_pointer()
                    .h(px(30.0))
                    .px(px(10.0))
                    .rounded(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(crate::typography::ui_rems(12.5))
                    .text_color(if selected {
                        theme.text
                    } else {
                        theme.text_muted
                    })
                    .when(selected, |el| el.bg(crate::theme::ink(0.07)))
                    .hover(|s| s.bg(crate::theme::ink(0.05)))
                    .when(!single_target, |el| {
                        el.on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(l) = this.saved_ui.launcher.as_mut() {
                                l.target = ix;
                                l.error = None;
                            }
                            cx.notify();
                        }))
                    })
                    .child(
                        div()
                            .flex_none()
                            .size(px(12.0))
                            .rounded_full()
                            .border_1()
                            .border_color(if selected {
                                theme.accent
                            } else {
                                theme.text_faint
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(selected, |el| {
                                el.child(div().size(px(6.0)).rounded_full().bg(theme.accent))
                            }),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(SharedString::from(t.label())),
                    )
            });
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(field_caption(theme, "Run in"))
                .children(shown)
        });
        let no_target = l.targets.is_empty().then(|| {
            div()
                .text_size(crate::typography::ui_rems(12.5))
                .text_color(theme.warning_muted)
                .child("There is nowhere to run it: open a chat or add a project first.")
        });

        // The args form.
        let mut form = div().flex().flex_col().gap(px(12.0));
        for (ix, f) in l.form.fields.iter().enumerate() {
            let error = l.errors.fields.get(&ix).cloned();
            let required = f.spec.required && f.spec.default.is_none();
            let mut head = div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .font_family(theme.font_mono.clone())
                        .text_size(px(theme.code_font_size))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(SharedString::from(f.spec.name.clone())),
                )
                .child(
                    div()
                        .px(px(5.0))
                        .rounded(px(4.0))
                        .bg(theme.wash(0.08))
                        .text_size(crate::typography::ui_rems(10.5))
                        .text_color(theme.text_muted)
                        .child(f.type_label()),
                );
            if required {
                head = head.child(
                    div()
                        .text_size(crate::typography::ui_rems(10.5))
                        .text_color(theme.warning_muted)
                        .child("required"),
                );
            }
            let control: AnyElement = if f.is_toggle() {
                let on = f.on;
                div()
                    .id(SharedString::from(format!("launch-arg-toggle-{ix}")))
                    .role(gpui::Role::Switch)
                    .aria_label(SharedString::from(f.spec.name.clone()))
                    .tab_index(0)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(l) = this.saved_ui.launcher.as_mut() {
                            l.form.toggle(ix);
                            l.error = None;
                        }
                        cx.notify();
                    }))
                    .child(sw::toggle_switch(theme, on, format!("launch-arg-{ix}")))
                    .into_any_element()
            } else if let Some(Some(input)) = l.inputs.get(ix) {
                popover::dialog_field(input.clone().into_any_element())
                    .when(error.is_some(), |el| {
                        el.border_color(theme.danger.opacity(0.6))
                    })
                    .when(f.is_multiline(), |el| {
                        el.max_h(px(120.0))
                            .overflow_hidden()
                            .font_family(theme.font_mono.clone())
                            .text_size(px(theme.code_font_size))
                    })
                    .when(
                        !f.is_multiline() && f.spec.ty != zeron_proto::SavedArgType::String,
                        |el| {
                            el.font_family(theme.font_mono.clone())
                                .text_size(px(theme.code_font_size))
                        },
                    )
                    .into_any_element()
            } else {
                div().into_any_element()
            };
            let (toggle_control, field_control) = if f.is_toggle() {
                (Some(control), None)
            } else {
                (None, Some(control))
            };
            let row = div()
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(match toggle_control {
                    Some(toggle) => div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(head)
                        .child(toggle)
                        .into_any_element(),
                    None => head.into_any_element(),
                })
                .children(f.spec.description.clone().map(|d| {
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .line_height(px(16.0))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(d))
                }))
                .children(field_control)
                .children(error.map(|e| {
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.danger_muted)
                        .child(SharedString::from(e))
                }));
            form = form.child(row);
        }
        let has_args = !l.form.fields.is_empty();
        let general: Vec<SharedString> = l
            .errors
            .general
            .iter()
            .cloned()
            .map(SharedString::from)
            .collect();

        // What the approval would show.
        let preview = match &l.preview {
            Loadable::Ready(d) => d.graph.as_ref().map(preview_line),
            _ => None,
        };
        let diagnostics = match &l.preview {
            Loadable::Ready(d) if !d.diagnostics.is_empty() => Some(d.diagnostics.join(" · ")),
            _ => None,
        };

        let run_label = if pending {
            "Starting…"
        } else {
            "Run workflow"
        };
        let can_run = !pending && !l.targets.is_empty();
        let card = popover::dialog_card(theme)
            .w(px(500.0))
            .max_h(px(880.0))
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    this.close_saved_launcher(cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        icon(icons::WORKFLOW)
                            .size(px(16.0))
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::ui_rems(15.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child(SharedString::from(wf.name.clone())),
                    )
                    .child(sw::badge(theme, wf.scope.label())),
            )
            .child(
                div()
                    .mt(px(6.0))
                    .child(popover::dialog_body(theme, wf.description.clone())),
            )
            .child(
                div()
                    .id("saved-launcher-body")
                    .mt(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    .overflow_y_scroll()
                    .max_h(px(660.0))
                    .children(targets)
                    .children(no_target)
                    .when(has_args, |el| {
                        el.child(field_caption(theme, "Arguments")).child(form)
                    })
                    .when(!has_args, |el| {
                        el.child(
                            div()
                                .text_size(crate::typography::ui_rems(12.5))
                                .text_color(theme.text_muted)
                                .child("This workflow takes no arguments."),
                        )
                    })
                    .children(general.into_iter().map(|g| {
                        div()
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(theme.danger_muted)
                            .child(g)
                    }))
                    .children(preview.map(|p| {
                        div()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(8.0))
                            .bg(crate::theme::ink(0.04))
                            .text_size(crate::typography::ui_rems(12.0))
                            .line_height(px(16.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(format!(
                                "Starting it is your approval. It will run {p}."
                            )))
                    }))
                    .children(diagnostics.map(|d| {
                        div()
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(theme.warning_muted)
                            .child(SharedString::from(format!(
                                "This script would not start: {d}"
                            )))
                    })),
            )
            .children(l.error.clone().map(|e| {
                div()
                    .mt(px(12.0))
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme.danger.opacity(0.25))
                    .bg(theme.danger.opacity(0.06))
                    .text_size(crate::typography::ui_rems(12.0))
                    .line_height(px(16.0))
                    .text_color(theme.danger_muted)
                    .child(e)
            }))
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        popover::btn_ghost(theme, "Cancel", "saved-launch-cancel")
                            .id("saved-launch-cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.close_saved_launcher(cx))),
                    )
                    .child(
                        popover::btn_primary(theme, run_label)
                            .id("saved-launch-run")
                            .role(gpui::Role::Button)
                            .when(!can_run, |el| el.opacity(0.5).cursor_default())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if can_run {
                                    this.submit_saved_launch(cx)
                                }
                            })),
                    ),
            )
            .into_any_element();
        Some(card)
    }

    fn render_save_card(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.saved_ui.save.as_ref()?;
        let scope = d.form.scope;
        let has_project = d.has_project;
        let pending = d.pending;
        let label = if pending {
            "Saving…"
        } else if d.replace {
            "Replace"
        } else {
            "Save"
        };
        let field =
            |caption: &'static str, input: &Entity<ComposerInput>, error: Option<String>| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .child(field_caption(theme, caption))
                    .child(
                        popover::dialog_field(input.clone().into_any_element())
                            .when(error.is_some(), |el| {
                                el.border_color(theme.danger.opacity(0.6))
                            }),
                    )
                    .children(error.map(|e| {
                        div()
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(theme.danger_muted)
                            .child(SharedString::from(e))
                    }))
            };
        let scope_choice = |which: SavedScope, text: &'static str, enabled: bool| {
            let selected = scope == which;
            div()
                .id(SharedString::from(format!("save-scope-{}", which.as_str())))
                .role(gpui::Role::RadioButton)
                .aria_label(text)
                .tab_index(0)
                .when(enabled, |el| el.cursor_pointer())
                .when(!enabled, |el| el.opacity(0.4))
                .h(px(28.0))
                .px(px(10.0))
                .rounded(px(8.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(crate::typography::ui_rems(12.5))
                .text_color(if selected {
                    theme.text
                } else {
                    theme.text_muted
                })
                .when(selected, |el| el.bg(crate::theme::ink(0.07)))
                .when(enabled, |el| {
                    el.hover(|s| s.bg(crate::theme::ink(0.05)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(d) = this.saved_ui.save.as_mut() {
                                d.form.scope = which;
                                d.errors.scope = None;
                                d.replace = false;
                                d.error = None;
                            }
                            cx.notify();
                        }))
                })
                .child(
                    div()
                        .flex_none()
                        .size(px(12.0))
                        .rounded_full()
                        .border_1()
                        .border_color(if selected {
                            theme.accent
                        } else {
                            theme.text_faint
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(selected, |el| {
                            el.child(div().size(px(6.0)).rounded_full().bg(theme.accent))
                        }),
                )
                .child(text)
        };
        let args_note = (!d.args.is_empty()).then(|| {
            format!(
                "Keeps the run's arguments as defaults: {}.",
                d.args
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
        let card = popover::dialog_card(theme)
            .w(px(460.0))
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    this.saved_ui.save = None;
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .child(popover::dialog_title(theme, "Save as workflow"))
            .child(div().mt(px(6.0)).child(popover::dialog_body(
                theme,
                "Keep this run’s script to run again with arguments, from Settings or with /workflow.",
            )))
            .child(
                div()
                    .mt(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(field("Name", &d.name, d.errors.name.clone()))
                    .child(field("Description", &d.description, d.errors.description.clone()))
                    .child(field("When to use it", &d.when_to_use, d.errors.when_to_use.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(field_caption(theme, "Save to"))
                            .child(scope_choice(
                                SavedScope::Project,
                                "This project (.zeron/workflows, shared with the repository)",
                                has_project,
                            ))
                            .child(scope_choice(
                                SavedScope::Global,
                                "Global (your ~/.zeron/workflows, every project)",
                                true,
                            ))
                            .children(d.errors.scope.clone().map(|e| {
                                div()
                                    .text_size(crate::typography::ui_rems(12.0))
                                    .text_color(theme.danger_muted)
                                    .child(SharedString::from(e))
                            })),
                    )
                    .children(args_note.map(|n| {
                        div()
                            .text_size(crate::typography::ui_rems(12.0))
                            .line_height(px(16.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(n))
                    })),
            )
            .children(d.error.clone().map(|e| {
                div()
                    .mt(px(12.0))
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme.warning.opacity(0.3))
                    .bg(theme.warning.opacity(0.06))
                    .text_size(crate::typography::ui_rems(12.0))
                    .line_height(px(16.0))
                    .text_color(theme.warning_muted)
                    .child(e)
            }))
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        popover::btn_ghost(theme, "Cancel", "saved-save-cancel")
                            .id("saved-save-cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.saved_ui.save = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        popover::btn_primary(theme, label)
                            .id("saved-save-confirm")
                            .role(gpui::Role::Button)
                            .when(pending, |el| el.opacity(0.5).cursor_default())
                            .on_click(cx.listener(|this, _, _, cx| this.submit_save_workflow(cx))),
                    ),
            )
            .into_any_element();
        Some(card)
    }
}

fn field_caption(theme: &Theme, text: &'static str) -> gpui::Div {
    div()
        .text_size(crate::typography::ui_rems(11.5))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.text_faint)
        .child(text)
}

/// `4 phases, 14 agents, commands: cargo test`.
fn preview_line(graph: &zeron_proto::WorkflowGraph) -> String {
    let n = |c: usize, one: &str, many: &str| format!("{c} {}", if c == 1 { one } else { many });
    let mut parts = vec![n(graph.phases.len(), "phase", "phases")];
    if !graph.actors.is_empty() {
        parts.push(n(graph.actors.len(), "agent", "agents"));
    }
    let mut line = parts.join(", ");
    let mut cmds: Vec<String> = graph
        .commands
        .iter()
        .map(|c| match &c.args {
            Some(a) if !a.is_empty() => format!("{} {}", c.command, a.join(" ")),
            _ => c.command.clone(),
        })
        .collect();
    cmds.dedup();
    if !cmds.is_empty() {
        let more = cmds.len().saturating_sub(3);
        cmds.truncate(3);
        line.push_str(&format!(" and the commands {}", cmds.join(", ")));
        if more > 0 {
            line.push_str(&format!(" (+{more} more)"));
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn wf(scope: SavedScope, space: Option<&str>, root: Option<&str>) -> SavedWorkflowSummary {
        SavedWorkflowSummary {
            name: "x".into(),
            scope,
            description: "d".into(),
            when_to_use: None,
            args: vec![],
            path: None,
            project_root: root.map(str::to_owned),
            space_id: space.map(str::to_owned),
            modified_at: None,
            shadowed_by: None,
            shadows: vec![],
        }
    }

    fn chat(id: &str, space: Option<&str>, cwd: Option<&str>, archived: bool) -> zeron_proto::Chat {
        serde_json::from_value(json!({
            "id": id, "title": format!("Chat {id}"), "deviceId": "local", "archived": archived,
            "spaceId": space, "cwd": cwd, "createdAt": Utc::now(),
        }))
        .unwrap()
    }

    fn space(id: &str, device: &str, path: &str, name: &str) -> zeron_proto::Space {
        serde_json::from_value(json!({
            "id": id, "deviceId": device, "path": path, "name": name,
            "gitDetected": true, "createdAt": Utc::now(),
        }))
        .unwrap()
    }

    fn state() -> AppState {
        let mut s = AppState::new();
        s.local_device_id = Some("local".into());
        s.spaces = vec![
            space("s-api", "local", "/p/api", "api"),
            space("s-web", "local", "/p/web", "web"),
            space("s-far", "other", "/p/far", "far"),
        ];
        s.chats = vec![
            chat("c-api", Some("s-api"), Some("/p/api"), false),
            chat("c-web", Some("s-web"), Some("/p/web"), false),
            chat("c-old", Some("s-web"), Some("/p/web"), true),
        ];
        s
    }

    fn labels(targets: &[LaunchTarget]) -> Vec<String> {
        targets.iter().map(LaunchTarget::label).collect()
    }

    #[test]
    fn a_global_workflow_can_run_in_the_open_chat_or_any_local_project() {
        let mut s = state();
        s.selected_chat = Some("c-api".into());
        let t = launch_targets(&s, &wf(SavedScope::Global, None, None), None);
        assert_eq!(
            labels(&t),
            [
                "This chat · Chat c-api",
                "New chat in api",
                "New chat in web"
            ],
            "another device's project is not a place a local dialog can create a chat in"
        );
        assert_eq!(
            default_target(&t, &wf(SavedScope::Global, None, None), None),
            0
        );
    }

    #[test]
    fn a_project_workflow_only_runs_in_its_own_project() {
        let mut s = state();
        s.selected_chat = Some("c-api".into());
        let w = wf(SavedScope::Project, Some("s-web"), Some("/p/web"));
        let t = launch_targets(&s, &w, None);
        assert_eq!(
            labels(&t),
            ["New chat in web"],
            "the open chat is in another project"
        );
        assert_eq!(default_target(&t, &w, None), 0);
        s.selected_chat = Some("c-web".into());
        let t = launch_targets(&s, &w, None);
        assert_eq!(labels(&t), ["This chat · Chat c-web", "New chat in web"]);
        // A project workflow launched from Settings starts a fresh chat in its project.
        assert_eq!(default_target(&t, &w, None), 1);
        // From a chat's own `/workflow` it stays in that chat.
        assert_eq!(default_target(&t, &w, Some("c-web")), 0);
    }

    #[test]
    fn archived_chats_and_unknown_origins_are_not_targets() {
        let mut s = state();
        s.selected_chat = Some("c-old".into());
        let t = launch_targets(&s, &wf(SavedScope::Global, None, None), None);
        assert!(!labels(&t).iter().any(|l| l.starts_with("This chat")));
        let t = launch_targets(&s, &wf(SavedScope::Global, None, None), Some("nope"));
        assert!(!labels(&t).iter().any(|l| l.starts_with("This chat")));
        // No projects and no chat: nowhere to run, said by the dialog.
        let empty = AppState::new();
        assert!(launch_targets(&empty, &wf(SavedScope::Global, None, None), None).is_empty());
        assert_eq!(
            default_target(&[], &wf(SavedScope::Global, None, None), None),
            0
        );
    }

    #[test]
    fn the_projects_the_composer_points_at_comes_first() {
        let mut s = state();
        s.selected_space = Some("s-web".into());
        let t = launch_targets(&s, &wf(SavedScope::Global, None, None), None);
        assert_eq!(labels(&t), ["New chat in web", "New chat in api"]);
    }

    #[test]
    fn the_approval_summary_names_phases_agents_and_commands() {
        use zeron_proto::{GraphActor, GraphCommand, GraphPhase, WorkflowGraph};
        let g = WorkflowGraph {
            phases: vec![GraphPhase::default(), GraphPhase::default()],
            actors: vec![GraphActor::default()],
            commands: vec![
                GraphCommand {
                    command: "cargo".into(),
                    args: Some(vec!["test".into()]),
                    ..Default::default()
                },
                GraphCommand {
                    command: "git".into(),
                    args: None,
                    ..Default::default()
                },
            ],
            unphased_asks: 0,
        };
        assert_eq!(
            preview_line(&g),
            "2 phases, 1 agent and the commands cargo test, git"
        );
        assert_eq!(preview_line(&WorkflowGraph::default()), "0 phases");
    }

    fn typed_wf() -> SavedWorkflowSummary {
        use zeron_proto::SavedArgType;
        let arg = |name: &str, ty, required, default: Option<Value>| SavedArg {
            name: name.into(),
            ty,
            required,
            default,
            description: Some(format!("the {name}")),
        };
        SavedWorkflowSummary {
            args: vec![
                arg("ticket", SavedArgType::Int, true, None),
                arg("base", SavedArgType::String, false, Some(json!("main"))),
                arg("strict", SavedArgType::Bool, false, Some(json!(false))),
                arg("exclude", SavedArgType::Json, false, Some(json!(["a"]))),
            ],
            ..wf(SavedScope::Global, None, None)
        }
    }

    fn shell_window(
        cx: &mut gpui::TestAppContext,
    ) -> (tempfile::TempDir, gpui::WindowHandle<Shell>) {
        let dir = tempfile::tempdir().unwrap();
        crate::shell::settings_modal_regressions::init_settings_test(
            Default::default(),
            dir.path(),
            cx,
        );
        let window = cx.add_window(|_, cx| {
            let mut shell = crate::shell::settings_modal_regressions::test_shell(dir.path(), cx);
            shell.state.update(cx, |state, _| {
                state.local_device_id = Some("local".into());
                state.chats = vec![chat("c1", None, None, false)];
                state.selected_chat = Some("c1".into());
            });
            shell
        });
        (dir, window)
    }

    #[gpui::test]
    fn the_launcher_draws_validates_and_names_every_field_problem(cx: &mut gpui::TestAppContext) {
        let (_dir, window) = shell_window(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.open_saved_launcher_with(
                    typed_wf(),
                    json!({"base": "dev"}),
                    Some("c1".into()),
                    Some("ticket is required.".into()),
                    cx,
                );
                let l = shell.saved_ui.launcher.as_ref().expect("open");
                assert_eq!(l.form.fields.len(), 4);
                assert_eq!(l.form.fields[1].text, "dev", "typed values beat defaults");
                assert_eq!(l.targets.len(), 1, "the open chat; no projects here");
                assert!(l.inputs[2].is_none(), "a bool is a switch, not a box");
                assert!(l.error.is_some());
                // Run with the required field empty: the form says so, nothing is sent.
                shell.submit_saved_launch(cx);
                let l = shell.saved_ui.launcher.as_ref().unwrap();
                assert_eq!(
                    l.errors.fields.get(&0).map(String::as_str),
                    Some("Required")
                );
                assert!(!l.pending, "a rejected form never starts anything");
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        // Typing into the box clears its error; a good value passes validation.
        window
            .update(cx, |shell, _, cx| {
                let input = shell.saved_ui.launcher.as_ref().unwrap().inputs[0]
                    .clone()
                    .unwrap();
                input.update(cx, |i, cx| i.set_text("4812", cx));
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                let l = shell.saved_ui.launcher.as_ref().unwrap();
                assert!(l.errors.fields.is_empty() || !l.errors.fields.contains_key(&0));
                assert_eq!(
                    l.form.collect().unwrap(),
                    json!({"ticket": 4812, "base": "dev", "strict": false, "exclude": ["a"]})
                );
                // Escape closes it, like every other dialog.
                assert!(shell.capture_escape_surface(cx));
                assert!(shell.saved_ui.launcher.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn without_an_engine_a_launch_fails_in_the_dialog_not_silently(cx: &mut gpui::TestAppContext) {
        let (_dir, window) = shell_window(cx);
        window
            .update(cx, |shell, _, cx| {
                let mut w = typed_wf();
                w.args.retain(|a| a.name == "base");
                shell.open_saved_launcher_with(w, Value::Null, Some("c1".into()), None, cx);
                shell.submit_saved_launch(cx);
                let l = shell.saved_ui.launcher.as_ref().expect("still open");
                assert!(!l.pending);
                assert_eq!(l.error.as_deref(), Some("Engine not connected"));
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
    }

    #[gpui::test]
    fn the_save_dialog_suggests_a_name_validates_and_closes_on_escape(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_dir, window) = shell_window(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.build_save_dialog(
                    "c1".into(),
                    "r1".into(),
                    "PR review!".into(),
                    infer_args(&json!({"base": "main", "n": 3})),
                    true,
                    cx,
                );
                let d = shell.saved_ui.save.as_ref().expect("open");
                assert_eq!(d.form.name, "pr-review");
                assert_eq!(d.form.scope, SavedScope::Project);
                assert_eq!(d.args.len(), 2);
                // No description yet: the dialog says so and sends nothing.
                shell.submit_save_workflow(cx);
                let d = shell.saved_ui.save.as_ref().unwrap();
                assert!(d.errors.description.is_some() && !d.pending);
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        window
            .update(cx, |shell, _, cx| {
                // The description box feeds the form.
                let input = shell.saved_ui.save.as_ref().unwrap().description.clone();
                input.update(cx, |i, cx| i.set_text("Reviews the diff", cx));
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                let d = shell.saved_ui.save.as_ref().unwrap();
                assert_eq!(d.form.description, "Reviews the diff");
                assert!(d.form.validate(d.has_project).is_ok());
                assert!(shell.capture_escape_surface(cx));
                assert!(shell.saved_ui.save.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_slash_start_with_everything_present_needs_no_dialog(cx: &mut gpui::TestAppContext) {
        let (_dir, window) = shell_window(cx);
        window
            .update(cx, |shell, _, cx| {
                // A missing required argument opens the launcher with the reason…
                shell.saved_workflow_from_composer(
                    "c1".into(),
                    typed_wf(),
                    json!({"base": "dev"}),
                    vec!["ticket".into()],
                    cx,
                );
                let l = shell.saved_ui.launcher.as_ref().expect("launcher");
                assert_eq!(l.error.as_deref(), Some("ticket is required."));
                assert_eq!(l.form.fields[1].text, "dev");
                shell.close_saved_launcher(cx);
                // …and nothing missing starts it (no engine here: a notice, no dialog).
                shell.saved_workflow_from_composer(
                    "c1".into(),
                    typed_wf(),
                    json!({"ticket": 1}),
                    vec![],
                    cx,
                );
                assert!(shell.saved_ui.launcher.is_none());
                assert_eq!(
                    shell.sidebar_notice.as_deref(),
                    Some("Engine not connected")
                );
            })
            .unwrap();
    }

    #[test]
    fn a_remote_chat_is_addressed_by_its_host() {
        let mut s = state();
        s.chats.push(
            serde_json::from_value(json!({
                "id": "c-far", "deviceId": "other", "archived": false, "createdAt": Utc::now(),
            }))
            .unwrap(),
        );
        assert_eq!(target_device(&s, "c-far").as_deref(), Some("other"));
        assert_eq!(target_device(&s, "c-api"), None);
        assert_eq!(target_device(&s, "missing"), None);
        assert_eq!(
            with_target(json!({"a": 1}), Some("d".into())),
            json!({"a": 1, "targetDeviceId": "d"})
        );
        assert_eq!(with_target(json!({"a": 1}), None), json!({"a": 1}));
    }
}
