//! Transient sidebar presentation. None of this state is persisted.
use super::*;

pub(super) const EDGE: f32 = 14.0;
const LEAVE_SLOP: f32 = 12.0;
const OPEN_DELAY: Duration = Duration::from_millis(120);
const CLOSE_DELAY: Duration = Duration::from_millis(300);
const PEEK_MOTION: motion::MotionSpec = motion::MotionSpec::new(120, motion::EASE_OUT);

#[derive(Default)]
pub(super) struct SidebarPeek {
    open: bool,
    tween: Option<WidthTween>,
    pending: Option<bool>,
    task: Option<Task<()>>,
    /// Explicit dismissal must not immediately reopen under a stationary mouse.
    suppressed: bool,
    pointer_inside: bool,
    pressed: bool,
    /// Pointer clicks can leave focus behind; only keyboard use retains it.
    keyboard_focus: bool,
    /// Keep the floating presentation while the pinned layout catches up.
    handoff: bool,
}

#[derive(Clone, Copy, Default)]
struct PeekInput {
    enabled: bool,
    edge: bool,
    panel: bool,
    held: bool,
    busy: bool,
}

fn wants_open(open: bool, visible: bool, suppressed: bool, input: PeekInput) -> bool {
    input.enabled
        && !suppressed
        && if open {
            input.panel || input.held || input.busy
        } else {
            !input.busy && !input.held && (input.edge || (visible && input.panel))
        }
}

impl Shell {
    pub(super) fn reset_sidebar_peek(&mut self, cx: &mut Context<Self>) {
        self.sidebar_peek.pending = None;
        self.sidebar_peek.task = None;
        self.sidebar_peek.pressed = false;
        self.sidebar_peek.keyboard_focus = false;
        self.sidebar_peek.pointer_inside = false;
        self.set_sidebar_peek(false, cx);
    }

    fn sidebar_peek_progress(&self) -> f32 {
        self.eval_tween_with_spec(
            self.sidebar_peek.tween,
            if self.sidebar_peek.open { 1.0 } else { 0.0 },
            PEEK_MOTION,
        )
    }

    pub(super) fn sidebar_peek_mounted(&self) -> bool {
        self.sidebar_peek.handoff
            || (self.settings.sidebar_collapsed && self.sidebar_peek_progress() > 0.0)
    }

    fn sidebar_peek_menu_open(&self) -> bool {
        self.spaces_menu.get().is_some()
            || self.sidebar_view_menu.get().is_some()
            || self.user_menu.get().is_some()
            || self.chat_menu.get().is_some()
            || self.space_menu.get().is_some()
    }

    fn sidebar_peek_input(&self, window: &Window, cx: &App) -> PeekInput {
        let position = window.mouse_position();
        let x = f32::from(position.x);
        let y = f32::from(position.y);
        // MouseExit can be delivered when handing input to a native child,
        // while the window still reports hovered and its last position is stale.
        let hovered = self.sidebar_peek.pointer_inside;
        let in_height = y >= 0.0 && y < f32::from(window.viewport_size().height);
        let modal = self.rename_dialog.is_some()
            || self.rename_space_dialog.is_some()
            || self.delete_confirm.is_some()
            || self.delete_space_confirm.is_some()
            || self.add_space.is_some()
            || self.command_palette.is_some()
            || self.sync_flow.has_visible_overlay()
            || self.sync_flow == SyncFlow::SignedOutRestartRequired;
        PeekInput {
            enabled: settings::sidebar_hover_enabled(cx)
                && self.settings.sidebar_collapsed
                && !self.tween_active(self.sidebar_tween)
                && window.is_window_active()
                && !modal
                && matches!(self.state.read(cx).gate(), GatePhase::Ready)
                && self.splash == SplashPhase::Gone,
            edge: hovered && in_height && y >= Theme::TITLEBAR_HEIGHT && (0.0..=EDGE).contains(&x),
            // Measure the painted panel during reversals, not its final width.
            panel: hovered
                && in_height
                && x >= 0.0
                && x <= self.settings.sidebar_width * self.sidebar_peek_progress() + LEAVE_SLOP,
            held: self.sidebar_peek_menu_open()
                || (self.sidebar_peek_focus.contains_focused(window, cx)
                    && (self.sidebar_peek.keyboard_focus
                        || window
                            .context_stack()
                            .iter()
                            .any(|context| context.contains("Composer")))),
            busy: self.sidebar_peek.pressed
                || cx.has_active_drag()
                || self.pane_resize_dragging.is_some(),
        }
    }

    fn set_sidebar_peek(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.sidebar_peek.open != open {
            let from = self.sidebar_peek_progress();
            self.sidebar_peek.open = open;
            if !open {
                self.sidebar_peek.keyboard_focus = false;
            }
            self.sidebar_peek.tween = Some(WidthTween::new(from, if open { 1.0 } else { 0.0 }));
            cx.notify();
        }
    }

    pub(super) fn sidebar_peek_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        // Capture before child controls consume navigation. Tab may enter the
        // sidebar later in this dispatch; the hold still requires focus inside.
        if self.sidebar_peek.open
            && (event.keystroke.key == "tab"
                || self.sidebar_peek_focus.contains_focused(window, cx))
        {
            self.sidebar_peek.keyboard_focus = true;
            cx.notify();
        }
    }

    pub(super) fn reconcile_sidebar_peek(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.sidebar_peek.handoff {
            if self.tween_active(self.sidebar_tween) {
                return;
            }
            self.sidebar_peek = SidebarPeek::default();
        }
        let input = self.sidebar_peek_input(window, cx);
        if !input.edge && !input.panel {
            self.sidebar_peek.suppressed = false;
        }
        let wanted = wants_open(
            self.sidebar_peek.open,
            self.sidebar_peek_progress() > 0.0,
            self.sidebar_peek.suppressed,
            input,
        );
        if !input.enabled {
            self.sidebar_peek.pending = None;
            self.sidebar_peek.task = None;
            self.set_sidebar_peek(false, cx);
            return;
        }
        if wanted == self.sidebar_peek.open {
            self.sidebar_peek.pending = None;
            self.sidebar_peek.task = None;
            return;
        }
        // Reentering a departing panel reverses immediately, from the painted
        // position. Only a fresh edge approach waits for the intent delay.
        if wanted && self.sidebar_peek_progress() > 0.0 {
            self.sidebar_peek.pending = None;
            self.sidebar_peek.task = None;
            self.set_sidebar_peek(true, cx);
            return;
        }
        if self.sidebar_peek.pending == Some(wanted) {
            return;
        }
        self.sidebar_peek.pending = Some(wanted);
        self.sidebar_peek.task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(if wanted { OPEN_DELAY } else { CLOSE_DELAY })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.sidebar_peek.pending != Some(wanted) {
                    return;
                }
                this.sidebar_peek.pending = None;
                let input = this.sidebar_peek_input(window, cx);
                let still_wanted = wants_open(
                    this.sidebar_peek.open,
                    this.sidebar_peek_progress() > 0.0,
                    this.sidebar_peek.suppressed,
                    input,
                );
                if still_wanted == wanted {
                    this.set_sidebar_peek(wanted, cx);
                }
            })
            .ok();
        }));
    }

    /// Called by the explicit toggle; peeking never writes UiSettings.
    pub(super) fn prepare_sidebar_peek_toggle(&mut self, cx: &mut Context<Self>) {
        let pinning = self.settings.sidebar_collapsed && self.sidebar_peek_mounted();
        self.sidebar_peek.pending = None;
        self.sidebar_peek.task = None;
        self.sidebar_peek.suppressed = true;
        self.sidebar_peek.handoff = pinning;
        self.set_sidebar_peek(pinning, cx);
    }

    pub(super) fn dismiss_sidebar_peek(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.sidebar_peek.open || self.sidebar_peek.handoff || self.sidebar_peek_menu_open() {
            return false;
        }
        let pointer_inside = self.sidebar_peek.pointer_inside;
        self.reset_sidebar_peek(cx);
        self.sidebar_peek.pointer_inside = pointer_inside;
        self.sidebar_peek.suppressed = true;
        if self.sidebar_peek_focus.contains_focused(window, cx) {
            window.focus(&self.composer.focus_handle(cx), cx);
        }
        true
    }

    pub(super) fn sidebar_content(&self) -> AnyElement {
        div()
            .id("sidebar-content")
            .track_focus(&self.sidebar_peek_focus)
            .size_full()
            .child(
                self.sidebar_pane.clone().cached(
                    gpui::StyleRefinement::default()
                        .w(px(self.settings.sidebar_width))
                        .h_full()
                        .flex_none(),
                ),
            )
            .into_any_element()
    }

    pub(super) fn render_sidebar_peek(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let progress = self.sidebar_peek_progress();
        let theme = Theme::of(cx);
        let width = self.settings.sidebar_width;
        let shadow_alpha = progress
            * if theme.appearance.is_dark() {
                0.18
            } else {
                0.10
            };
        // The same tint/blur as contextual menus, extending behind the native
        // traffic lights. Only the content is inset below the titlebar; the
        // surface itself is flush with all three window edges.
        let panel = div()
            .id("sidebar-peek-panel")
            .debug_selector(|| "sidebar-peek-panel".into())
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(-width * (1.0 - progress)))
            .w(px(width))
            .occlude()
            .border_r_1()
            .border_color(theme.border)
            .bg(popover::surface_bg(theme))
            .child(
                div()
                    .size_full()
                    .pt(px(Theme::TITLEBAR_HEIGHT))
                    .child(self.sidebar_content()),
            )
            .child(
                self.titlebar_drag_region(
                    "sidebar-peek-titlebar",
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(Theme::TITLEBAR_HEIGHT)),
                    cx,
                ),
            );
        // Priority zero is above native browser content and below menus (1)
        // and dialogs (2). Mount this native input overlay only during peek;
        // idle controls must stay in the base scene so the page keeps input.
        gpui::deferred(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(width))
                .child(crate::frost::frosted(0.0, crate::frost::MENU_BLUR, panel))
                // Keep the shadow outside the glass: a filled drop shadow
                // behind the panel would darken its translucent surface.
                // Follow the painted edge and fade with the reveal tween.
                .child(
                    div()
                        .absolute()
                        .left(px(width * progress))
                        .top_0()
                        .bottom_0()
                        .w(px(12.0))
                        .bg(gpui::linear_gradient(
                            90.0,
                            gpui::linear_color_stop(gpui::black().opacity(shadow_alpha), 0.0),
                            gpui::linear_color_stop(gpui::black().opacity(0.0), 1.0),
                        )),
                ),
        )
        .into_any_element()
    }

    pub(super) fn sidebar_peek_pointer_observer(&self, cx: &Context<Self>) -> AnyElement {
        let shell = cx.weak_entity();
        gpui::canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moving = shell.clone();
                window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        moving
                            .update(cx, |this, cx| {
                                this.sidebar_peek.pointer_inside = true;
                                this.sidebar_peek.pressed = event.pressed_button.is_some();
                                this.reconcile_sidebar_peek(window, cx);
                            })
                            .ok();
                    }
                });
                let exiting = shell.clone();
                window.on_mouse_event(move |_: &gpui::MouseExitEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        exiting
                            .update(cx, |this, cx| {
                                this.sidebar_peek.pressed = false;
                                this.sidebar_peek.pointer_inside = false;
                                this.reconcile_sidebar_peek(window, cx);
                            })
                            .ok();
                    }
                });
                let pressing = shell.clone();
                window.on_mouse_event(move |_: &MouseDownEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        pressing
                            .update(cx, |this, cx| {
                                this.sidebar_peek.pressed = true;
                                this.sidebar_peek.keyboard_focus = false;
                                this.sidebar_peek.pointer_inside = true;
                                this.reconcile_sidebar_peek(window, cx);
                            })
                            .ok();
                    }
                });
                let releasing = shell.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, window, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        releasing
                            .update(cx, |this, cx| {
                                this.sidebar_peek.pressed = false;
                                this.reconcile_sidebar_peek(window, cx);
                                cx.notify(); // recheck after drop handlers release their holds
                            })
                            .ok();
                    }
                });
            },
        )
        .absolute()
        .inset_0()
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext, VisualTestContext};

    // Exercise the production pointer observer and timers without starting an
    // engine. The tiny host avoids unrelated transcript and platform services.
    struct PeekHost {
        shell: Entity<Shell>,
        _dir: tempfile::TempDir,
    }

    impl Render for PeekHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.shell.update(cx, |shell, cx| {
                shell.reconcile_sidebar_peek(window, cx);
                div()
                    .size_full()
                    .track_focus(&shell.sidebar_peek_focus)
                    .capture_key_down(cx.listener(|shell, event, window, cx| {
                        shell.sidebar_peek_key_down(event, window, cx);
                    }))
                    .child(shell.sidebar_peek_pointer_observer(cx))
            })
        }
    }

    fn setup(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let (host, cx) = cx.add_window_view(|_, cx| {
            let shell = cx.new(|cx| {
                let state = cx.new(|_| {
                    let mut state = AppState::new();
                    state.connection = ConnectionStatus::Ready;
                    state.workspace_scope = Some(WorkspaceScope::Local);
                    state
                });
                let mut shell = Shell::new(
                    state,
                    EngineBootConfig {
                        data_dir: dir.path().into(),
                        ipc_port: 0,
                        edge_url: "http://127.0.0.1:1".into(),
                        edge_token: None,
                        org_id: None,
                        workos_client_id: None,
                        default_harness: zeron_proto::HarnessId::Mock,
                    },
                    cx,
                );
                shell.settings.sidebar_collapsed = true;
                shell.splash = SplashPhase::Gone;
                shell.reduced_motion = true;
                shell
            });
            PeekHost { shell, _dir: dir }
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        cx.update(|window, cx| {
            window.activate_window();
            window.draw(cx).clear();
        });
        cx.run_until_parked();
        (shell, cx)
    }

    fn advance(cx: &mut VisualTestContext, duration: Duration) {
        cx.run_until_parked();
        cx.executor().advance_clock(duration);
        cx.run_until_parked();
    }

    #[gpui::test]
    fn sidebar_peek_pointer_intent_cancels_and_menus_hold_until_dismissed(cx: &mut TestAppContext) {
        let (shell, cx) = setup(cx);
        let edge = gpui::point(px(3.0), px(150.0));
        let chat = gpui::point(px(650.0), px(150.0));
        cx.simulate_mouse_move(edge, None, gpui::Modifiers::default());
        advance(cx, OPEN_DELAY / 2);
        shell.read_with(cx, |shell, _| assert!(!shell.sidebar_peek.open));
        cx.simulate_mouse_move(chat, None, gpui::Modifiers::default());
        advance(cx, OPEN_DELAY);
        shell.read_with(cx, |shell, _| assert!(!shell.sidebar_peek.open));

        cx.simulate_mouse_move(edge, None, gpui::Modifiers::default());
        advance(cx, OPEN_DELAY);
        shell.read_with(cx, |shell, _| assert!(shell.sidebar_peek.open));
        // The window controls belong to the peek, too. Crossing into their
        // titlebar must not start the close timer.
        cx.simulate_mouse_move(
            gpui::point(px(40.0), px(18.0)),
            None,
            gpui::Modifiers::default(),
        );
        advance(cx, CLOSE_DELAY * 2);
        shell.read_with(cx, |shell, _| assert!(shell.sidebar_peek.open));
        shell.update(cx, |shell, _| shell.user_menu.open(()));
        cx.simulate_mouse_move(chat, None, gpui::Modifiers::default());
        advance(cx, CLOSE_DELAY * 2);
        shell.read_with(cx, |shell, _| assert!(shell.sidebar_peek.open));
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.user_menu = Default::default();
                shell.reconcile_sidebar_peek(window, cx);
            })
        });
        advance(cx, CLOSE_DELAY / 2);
        shell.read_with(cx, |shell, _| assert!(shell.sidebar_peek.open));
        cx.simulate_mouse_move(edge, None, gpui::Modifiers::default());
        advance(cx, CLOSE_DELAY);
        shell.read_with(cx, |shell, _| assert!(shell.sidebar_peek.open));
        cx.simulate_mouse_move(chat, None, gpui::Modifiers::default());
        advance(cx, CLOSE_DELAY);
        shell.read_with(cx, |shell, _| assert!(!shell.sidebar_peek.open));
    }

    #[gpui::test]
    fn sidebar_hover_opt_out_cancels_pending_and_open_peeks_and_persists(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| settings::init(UiSettings::default(), dir.path(), cx));
        let (shell, cx) = setup(cx);
        let edge = gpui::point(px(3.0), px(150.0));
        cx.simulate_mouse_move(edge, None, Default::default());
        advance(cx, OPEN_DELAY / 2);
        cx.update(|_, cx| settings::set_sidebar_hover_enabled(false, cx));
        advance(cx, OPEN_DELAY * 2);
        shell.read_with(cx, |shell, _| assert!(!shell.sidebar_peek.open));

        cx.update(|_, cx| settings::set_sidebar_hover_enabled(true, cx));
        advance(cx, OPEN_DELAY);
        shell.read_with(cx, |shell, _| assert!(shell.sidebar_peek.open));
        cx.update(|_, cx| settings::set_sidebar_hover_enabled(false, cx));
        advance(cx, CLOSE_DELAY);
        shell.read_with(cx, |shell, _| {
            assert!(!shell.sidebar_peek.open);
            assert!(shell.settings.sidebar_collapsed);
        });
        cx.update(|_, cx| settings::flush(cx));
        assert!(!UiSettings::load(dir.path()).sidebar_hover_enabled);
    }

    #[gpui::test]
    fn sidebar_peek_click_focus_does_not_hold_but_keyboard_focus_does(cx: &mut TestAppContext) {
        let (shell, cx) = setup(cx);
        let edge = gpui::point(px(3.0), px(150.0));
        let chat = gpui::point(px(650.0), px(150.0));
        cx.simulate_mouse_move(edge, None, Default::default());
        advance(cx, OPEN_DELAY);
        cx.simulate_click(edge, Default::default());
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| window.focus(&shell.sidebar_peek_focus, cx));
        });
        cx.simulate_mouse_move(chat, None, Default::default());
        advance(cx, CLOSE_DELAY);
        shell.read_with(cx, |shell, _| assert!(!shell.sidebar_peek.open));

        cx.simulate_mouse_move(edge, None, Default::default());
        advance(cx, OPEN_DELAY);
        cx.simulate_keystrokes("down");
        cx.simulate_mouse_move(chat, None, Default::default());
        advance(cx, CLOSE_DELAY * 2);
        shell.read_with(cx, |shell, _| assert!(shell.sidebar_peek.open));

        // Returning to pointer interaction releases the keyboard hold even
        // when the clicked control leaves the same focus handle in place.
        cx.simulate_click(edge, Default::default());
        cx.simulate_mouse_move(chat, None, Default::default());
        advance(cx, CLOSE_DELAY);
        shell.read_with(cx, |shell, _| assert!(!shell.sidebar_peek.open));
    }

    #[gpui::test]
    fn sidebar_peek_drag_across_edge_does_not_open_and_exit_cancels(cx: &mut TestAppContext) {
        let (shell, cx) = setup(cx);
        let edge = gpui::point(px(3.0), px(150.0));
        cx.simulate_mouse_move(edge, Some(MouseButton::Left), gpui::Modifiers::default());
        advance(cx, OPEN_DELAY * 2);
        shell.read_with(cx, |shell, _| assert!(!shell.sidebar_peek.open));
        cx.simulate_mouse_move(edge, None, gpui::Modifiers::default());
        advance(cx, OPEN_DELAY);
        shell.read_with(cx, |shell, _| assert!(shell.sidebar_peek.open));
        cx.simulate_event(gpui::MouseExitEvent {
            position: edge,
            pressed_button: None,
            modifiers: Default::default(),
        });
        advance(cx, CLOSE_DELAY);
        shell.read_with(cx, |shell, _| assert!(!shell.sidebar_peek.open));
    }

    #[gpui::test]
    fn sidebar_peek_escape_requires_leaving_the_edge_before_reopening(cx: &mut TestAppContext) {
        let (shell, cx) = setup(cx);
        let edge = gpui::point(px(3.0), px(150.0));
        cx.simulate_mouse_move(edge, None, gpui::Modifiers::default());
        advance(cx, OPEN_DELAY);
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                assert!(shell.dismiss_sidebar_peek(window, cx));
                shell.reconcile_sidebar_peek(window, cx);
            })
        });
        cx.simulate_mouse_move(edge, None, gpui::Modifiers::default());
        advance(cx, OPEN_DELAY * 2);
        shell.read_with(cx, |shell, _| assert!(!shell.sidebar_peek.open));
        cx.simulate_mouse_move(
            gpui::point(px(600.0), px(150.0)),
            None,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_move(edge, None, gpui::Modifiers::default());
        advance(cx, OPEN_DELAY);
        shell.read_with(cx, |shell, _| assert!(shell.sidebar_peek.open));
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.delete_confirm = Some("fixture".into());
                shell.reconcile_sidebar_peek(window, cx);
                assert!(
                    !shell.sidebar_peek.open,
                    "a modal takes precedence over edge hover"
                );
            })
        });
    }

    #[gpui::test]
    fn sidebar_peek_reverses_without_reflow_or_persisting_and_pins_in_place(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            settings::init(UiSettings::default(), dir.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zeron_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        window
            .update(cx, |shell, _, cx| {
                shell.settings.sidebar_collapsed = true;
                shell.settings.sidebar_width = 310.0;
                shell.reduced_motion = false;
                let settings = shell.settings.clone();
                shell.set_sidebar_peek(true, cx);
                let opening = shell.sidebar_peek.tween.unwrap();
                shell.render_time = Some(
                    opening.started + PEEK_MOTION.total().mul_f32(motion::speed_scale() * 0.5),
                );
                let visible = shell.sidebar_peek_progress();
                assert!(visible > 0.0 && visible < 1.0);
                shell.set_sidebar_peek(false, cx);
                assert_eq!(shell.sidebar_peek.tween.unwrap().from, visible);
                assert_eq!(shell.sidebar_target(), 0.0);
                assert_eq!(shell.sidebar_now(), 0.0, "hover must not move the chat");
                assert_eq!(
                    shell.settings, settings,
                    "hover must not change preferences"
                );

                shell.render_time = None;
                shell.reduced_motion = true;
                assert_eq!(shell.sidebar_peek_progress(), 0.0);
                shell.set_sidebar_peek(true, cx);
                assert_eq!(shell.sidebar_peek_progress(), 1.0);
                shell.reduced_motion = false;
                shell.sidebar_peek.tween = None;
                shell.toggle_sidebar(cx);
                assert!(!shell.settings.sidebar_collapsed);
                assert!(shell.sidebar_peek.handoff);
                assert!(shell.sidebar_peek_mounted());
                assert_eq!(
                    shell.sidebar_peek_progress(),
                    1.0,
                    "pinning keeps the panel in place"
                );
                assert_eq!(shell.sidebar_tween.unwrap().from, 0.0);
                assert_eq!(shell.sidebar_tween.unwrap().to, 310.0);
            })
            .unwrap();
    }

    #[test]
    fn edge_intent_never_opens_during_drag_or_after_explicit_dismissal() {
        let input = PeekInput {
            enabled: true,
            edge: true,
            ..Default::default()
        };
        assert!(wants_open(false, false, false, input));
        assert!(!wants_open(false, false, true, input));
        assert!(!wants_open(
            false,
            false,
            false,
            PeekInput {
                busy: true,
                ..input
            }
        ));
        assert!(!wants_open(
            false,
            false,
            false,
            PeekInput {
                enabled: false,
                ..input
            }
        ));
    }

    #[test]
    fn menus_focus_and_drags_hold_an_open_panel_outside_its_rectangle() {
        let input = PeekInput {
            enabled: true,
            ..Default::default()
        };
        assert!(!wants_open(true, true, false, input));
        assert!(wants_open(
            true,
            true,
            false,
            PeekInput {
                held: true,
                ..input
            }
        ));
        assert!(wants_open(
            true,
            true,
            false,
            PeekInput {
                busy: true,
                ..input
            }
        ));
        assert!(!wants_open(
            true,
            true,
            false,
            PeekInput {
                enabled: false,
                held: true,
                ..input
            }
        ));
    }

    #[test]
    fn reentering_a_departing_panel_can_reverse_but_empty_chat_cannot_open_it() {
        let input = PeekInput {
            enabled: true,
            panel: true,
            ..Default::default()
        };
        assert!(wants_open(false, true, false, input));
        assert!(!wants_open(false, false, false, input));
    }
}
