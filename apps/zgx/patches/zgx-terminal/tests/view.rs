//! The terminal zgx draws, headless: a real program's output in its rows,
//! typing reaching the program, its reports in the controller's state and
//! events, and the session outliving the view that showed it.
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};
use zgx::{
    compose::{TaskRunner, prelude::*},
    scene::NodeKind,
    task::LocalExecutor,
    terminal::{Terminal, TerminalOptions},
    widgets::Ui,
};
#[cfg(unix)]
use zgx::{
    input::{InputEvent, Key, Modifiers},
    terminal::TerminalEvent,
};

struct Harness {
    ui: Ui,
    executor: Rc<RefCell<LocalExecutor>>,
}
impl Harness {
    fn new() -> Self {
        Self {
            ui: Ui::new(640., 360.),
            executor: Rc::new(RefCell::new(LocalExecutor::new())),
        }
    }
    fn mount(&mut self, view: View) {
        self.ui.mount(provide(
            TaskRunner::from_executor(self.executor.clone()),
            view,
        ));
        self.ui.prepare_frame();
    }
    /// Run what is due (the session's wakes, posted deliveries, a frame).
    fn pump(&mut self) {
        while self.executor.borrow_mut().tick() > 0 {}
        zgx::native_view::deliver_posted();
        self.ui.prepare_frame();
    }
    fn until(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.pump();
            if done(self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: {:?}",
                self.texts()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    /// The text the rows draw, run by run, in order.
    fn texts(&self) -> Vec<String> {
        let scene = self.ui.scene.borrow();
        scene
            .paint_items()
            .filter_map(|item| match scene.kind(item.id) {
                NodeKind::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect()
    }
    fn shows(&self, text: &str) -> bool {
        self.texts().iter().any(|shown| shown == text)
    }
    /// The colour `text` is drawn in, where it shows.
    #[cfg(unix)]
    fn color_of(&self, text: &str) -> Option<zgx::scene::Color> {
        let scene = self.ui.scene.borrow();
        scene.paint_items().find_map(|item| match item.kind {
            NodeKind::Text {
                text: shown, color, ..
            } if &**shown == text => Some(*color),
            _ => None,
        })
    }
    /// The family `text` is drawn in, where it shows.
    #[cfg(unix)]
    fn family_of(&self, text: &str) -> Option<zgx::text_layout::FontFamily> {
        let scene = self.ui.scene.borrow();
        scene.paint_items().find_map(|item| match item.kind {
            NodeKind::Text { text: shown, .. } if &**shown == text => {
                Some(item.font.family.clone())
            }
            _ => None,
        })
    }
}

fn session(command: &str) -> TerminalOptions {
    TerminalOptions {
        command: Some(command.into()),
        ..Default::default()
    }
}

#[cfg(unix)]
#[test]
fn output_shows_in_rows_and_reports_reach_the_controller() {
    let mut app = Harness::new();
    let term = Terminal::new(
        &app.ui.runtime,
        session(r"printf '\033]2;Building\007hello world\r\n\033[31mred\033[0m\r\n'; sleep 30"),
    );
    // As a desktop host with the `terminal` feature provides it.
    let renderer = zgx::terminal::TerminalRenderer::new(zgx_terminal::terminal_view);
    app.mount(provide(
        renderer,
        zgx::terminal::terminal(&term).w(600.).h(300.),
    ));
    app.until("the output", |app| {
        app.shows("hello world") && app.shows("red")
    });
    // Its reports come with the read after: the view can draw what the
    // screen model holds (a resize's paint) before that read.
    app.until("the title", |_| term.title() == "Building");
    let state = term.state().get();
    assert!(state.columns > 10 && state.rows > 5, "{state:?}");
    assert!(zgx_terminal::is_engine_drawn(&term));
    // The shell running it is in front (`sh -c` doesn't take its
    // commands to the front).
    app.until("the foreground process", |_| {
        term.process() == "sh" && term.state().get().pid > 0
    });
}

/// What the terminal's live region (its `Status` node) says now.
fn news(app: &Harness) -> String {
    app.ui
        .semantics
        .borrow()
        .iter()
        .find(|(_, node)| node.role == zgx::semantics::Role::Status)
        .map(|(_, node)| node.label.clone())
        .unwrap_or_default()
}

/// Every platform's reader (Orca, VoiceOver, Narrator) hears the live
/// region through the same frame's accessibility update. Each line waits
/// for the test's Enter and the test delivers each frame, so reads and
/// frames come in the order it sets (output the view's first paint drew,
/// before it could be news, once lost a line here).
#[test]
fn a_reader_hears_every_read_of_output_since_the_last_frame() {
    let mut app = Harness::new();
    app.ui.set_assistive_technology(true);
    #[cfg(unix)]
    let command = "stty -echo; echo ready; read a; echo alpha; read b; echo beta; \
                   read c; echo gamma; sleep 30";
    #[cfg(windows)]
    let command = "cmd.exe /d /c echo ready& set /p a=& echo alpha& set /p b=& echo beta& \
                   set /p c=& echo gamma& ping -n 30 127.0.0.1 >nul";
    let term = Terminal::new(&app.ui.runtime, session(command));
    let frames = zgx::frame::FrameClock::new();
    let renderer = zgx::terminal::TerminalRenderer::new(zgx_terminal::terminal_view);
    app.mount(provide(
        frames.clone(),
        provide(renderer, zgx::terminal::terminal(&term).w(600.).h(300.)),
    ));
    let mut index = 0;
    let mut frame = |app: &mut Harness| {
        index += 1;
        frames.deliver(zgx::frame::Frame {
            index,
            time: Instant::now(),
            interval: Duration::from_millis(16),
        });
        app.pump();
    };
    app.until("ready", |app| app.shows("ready"));
    // What showed as the view mounted is no news; anything waiting goes.
    frame(&mut app);
    let before = news(&app);
    let mut heard = Vec::new();
    // Two reads before a frame: both said with it, in order.
    term.input("\r");
    app.until("alpha", |app| app.shows("alpha"));
    term.input("\r");
    app.until("beta", |app| app.shows("beta"));
    assert_eq!(news(&app), before, "news went out before its frame");
    frame(&mut app);
    heard.push(news(&app));
    // One read, the next frame: said alone.
    term.input("\r");
    app.until("gamma", |app| app.shows("gamma"));
    frame(&mut app);
    heard.push(news(&app));
    assert_eq!(heard, ["alpha\nbeta", "gamma"]);
}

#[cfg(unix)]
#[test]
fn the_process_in_front_is_named_again_after_it_execs_another() {
    let mut app = Harness::new();
    // The shell, then `cat` in its place: the same pid, another program.
    let term = Terminal::new(
        &app.ui.runtime,
        session("stty -echo; printf ready; read x; exec cat"),
    );
    let renderer = zgx::terminal::TerminalRenderer::new(zgx_terminal::terminal_view);
    app.mount(provide(
        renderer,
        zgx::terminal::terminal(&term).w(600.).h(300.),
    ));
    app.until("the shell in front", |_| term.process() == "sh");
    let pid = term.state().get().pid;
    term.input("\r");
    // cat's output is the next check's cue.
    term.input("hello\r");
    app.until("cat in front", |_| term.process() == "cat");
    assert_eq!(term.state().get().pid, pid, "exec kept the pid");
}

#[cfg(unix)]
#[test]
fn the_process_in_front_is_named_after_its_session_moves_to_another_window() {
    let mut app = Harness::new();
    let term = Terminal::new(
        &app.ui.runtime,
        // `ready` once echo is off: what is typed from then isn't echoed.
        session("stty -echo; echo ready; read x; exec cat"),
    );
    let renderer = zgx::terminal::TerminalRenderer::new(zgx_terminal::terminal_view);
    app.mount(provide(
        renderer.clone(),
        zgx::terminal::terminal(&term).w(600.).h(300.),
    ));
    app.until("the shell ready", |app| {
        app.shows("ready") && term.process() == "sh"
    });
    term.input("\r");
    term.input("hello\r");
    // cat's output read: a check is due, on this window's tasks.
    app.until("cat's output", |app| app.shows("hello"));
    // The view shows in another window, and the first window's tasks
    // never run again, as when it closes.
    let first = std::mem::replace(
        &mut app.executor,
        Rc::new(RefCell::new(LocalExecutor::new())),
    );
    app.mount(provide(
        renderer,
        zgx::terminal::terminal(&term).w(600.).h(300.),
    ));
    drop(first);
    app.until("cat in front", |_| term.process() == "cat");
}

#[cfg(unix)]
#[test]
fn a_quiet_command_typed_at_a_shell_is_named_in_front() {
    let mut app = Harness::new();
    // An interactive shell (job control: a command takes the front), and
    // a command that prints nothing after the shell's echo of it.
    let term = Terminal::new(&app.ui.runtime, session("PS1='$ ' exec sh -i"));
    let renderer = zgx::terminal::TerminalRenderer::new(zgx_terminal::terminal_view);
    app.mount(provide(
        renderer,
        zgx::terminal::terminal(&term).w(600.).h(300.),
    ));
    app.until("the prompt", |app| {
        app.texts().iter().any(|t| t.starts_with('$'))
    });
    app.until("the shell in front", |_| term.process() == "sh");
    term.input("sleep 30\r");
    app.until("sleep in front", |_| term.process() == "sleep");
}

#[cfg(unix)]
#[test]
fn streaming_output_is_read_at_most_once_a_refresh() {
    let mut app = Harness::new();
    // Three hundred lines, each its own chunk a millisecond or two apart:
    // far more often than a refresh.
    let term = Terminal::new(
        &app.ui.runtime,
        session(
            "i=1; while [ $i -le 300 ]; do echo \"line $i\"; i=$((i+1)); sleep 0.001; done; sleep 30",
        ),
    );
    let renderer = zgx::terminal::TerminalRenderer::new(zgx_terminal::terminal_view);
    app.mount(provide(
        renderer,
        zgx::terminal::terminal(&term).w(600.).h(300.),
    ));
    app.until("line 1", |app| !lines_shown(app).is_empty());
    let (start, before) = (Instant::now(), zgx_terminal::reads(&term));
    app.until("line 300", |app| {
        lines_shown(app).last().is_some_and(|last| last.0 == 300)
    });
    let reads = zgx_terminal::reads(&term) - before;
    let refreshes = start.elapsed().as_secs_f64() / app.ui.refresh_interval().as_secs_f64();
    // The harness looks every 5 ms: read at each look, that would be about
    // three times as many.
    assert!(
        (reads as f64) <= refreshes + 2.,
        "{reads} reads in {refreshes:.1} refreshes"
    );
}

/// Every `line N` shown, with the top of its box on screen, in the order
/// the scene paints them (which assistive technology reads).
#[cfg(unix)]
fn lines_shown(app: &Harness) -> Vec<(u32, f32)> {
    let scene = app.ui.scene.borrow();
    scene
        .paint_items()
        .filter_map(|item| match scene.kind(item.id) {
            NodeKind::Text { text, .. } => text
                .strip_prefix("line ")
                .and_then(|n| n.trim().parse().ok())
                .map(|n| (n, scene.transformed_bounds(item.id).y)),
            _ => None,
        })
        .collect()
}

#[cfg(unix)]
#[test]
fn scrolled_output_shows_each_line_at_its_row_in_order() {
    let mut app = Harness::new();
    // Forty lines at once, then twenty one by one (a scroll of a line or
    // two between frames), past the bottom of about twenty rows.
    let term = Terminal::new(
        &app.ui.runtime,
        session(
            "i=1; while [ $i -le 40 ]; do echo \"line $i\"; i=$((i+1)); done; \
             while [ $i -le 60 ]; do echo \"line $i\"; i=$((i+1)); sleep 0.02; done; sleep 30",
        ),
    );
    let renderer = zgx::terminal::TerminalRenderer::new(zgx_terminal::terminal_view);
    app.mount(provide(
        renderer,
        zgx::terminal::terminal(&term).w(600.).h(300.),
    ));
    let mut checked = 0;
    app.until("line 60", |app| {
        // Each frame on the way: the lines shown are consecutive, each a row
        // below the one before, in reading order, and on screen.
        let shown = lines_shown(app);
        if let Some((first, top)) = shown.first().copied() {
            let rows = shown.len() as f32;
            let height = shown.get(1).map_or(0., |second| second.1 - top);
            for (i, (line, y)) in shown.iter().enumerate() {
                assert_eq!(*line, first + i as u32, "lines out of order: {shown:?}");
                assert!(
                    (y - (top + i as f32 * height)).abs() < 0.01,
                    "line {line} not at its row: {shown:?}"
                );
            }
            assert!(
                height > 0. || shown.len() == 1,
                "lines on one row: {shown:?}"
            );
            assert!(
                top >= 0. && top + rows * height <= 300.,
                "lines off screen: {shown:?}"
            );
            checked += 1;
        }
        shown.last().is_some_and(|last| last.0 == 60)
    });
    assert!(checked > 1, "only {checked} frames checked");
    let shown = lines_shown(&app);
    assert!(
        shown.len() > 10 && shown[0].0 > 1,
        "the screen didn't scroll: {shown:?}"
    );
}

#[cfg(unix)]
#[test]
fn typing_reaches_the_program() {
    let mut app = Harness::new();
    let term = Terminal::new(&app.ui.runtime, session("cat"));
    app.mount(zgx_terminal::terminal_view(&term).w(600.).h(300.));
    term.focus();
    app.until("focus", |_| term.focused());
    app.ui.dispatch(InputEvent::Text("abc".into()));
    app.ui.dispatch(InputEvent::KeyDown {
        key: Key::Enter,
        modifiers: Modifiers::default(),
        repeat: false,
    });
    // The tty's echo, then cat's copy.
    app.until("the echo and the copy", |app| {
        app.texts().iter().filter(|t| *t == "abc").count() == 2
    });
    // Control chords reach it as control characters: ^D ends cat.
    let events = Rc::new(RefCell::new(Vec::new()));
    let heard = events.clone();
    let _subscription = term.on_event(move |event| heard.borrow_mut().push(event.clone()));
    app.ui.dispatch(InputEvent::KeyDown {
        key: Key::Character("d".into()),
        modifiers: Modifiers {
            control: true,
            ..Modifiers::default()
        },
        repeat: false,
    });
    app.until("the exit", |_| term.exited().is_some());
    assert_eq!(term.exited(), Some(0));
    assert!(
        events.borrow().iter().any(|event| matches!(
            event,
            TerminalEvent::CloseRequested {
                process_alive: false
            }
        )),
        "{:?}",
        events.borrow()
    );
}

#[cfg(unix)]
#[test]
fn the_session_outlives_its_view() {
    let mut app = Harness::new();
    let term = Terminal::new(&app.ui.runtime, session(r"printf 'kept\r\n'; sleep 30"));
    let shown = app.ui.runtime.signal(true);
    let (view_term, view_shown) = (term.clone(), shown.clone());
    app.mount(switch(
        move || view_shown.get(),
        move |showing, _| {
            if showing {
                zgx_terminal::terminal_view(&view_term).w(600.).h(300.)
            } else {
                text("elsewhere")
            }
        },
    ));
    app.until("the output", |app| app.shows("kept"));
    let backend = term.backend().expect("a backend");
    shown.set(false);
    app.pump();
    assert!(!app.shows("kept"), "{:?}", app.texts());
    assert!(term.exited().is_none(), "unmounting keeps the session");
    shown.set(true);
    app.until("the output again", |app| app.shows("kept"));
    assert!(
        Rc::ptr_eq(&backend, &term.backend().unwrap()),
        "the same session"
    );
    term.close();
    assert!(term.backend().is_none());
}

#[cfg(unix)]
#[test]
fn the_grid_is_in_the_themes_monospaced_font_and_follows_it() {
    use zgx::{
        text_layout::FontFamily,
        theme::{ThemeOverride, tok},
    };
    let mut app = Harness::new();
    let term = Terminal::new(&app.ui.runtime, session(r"printf 'fonts\r\n'; sleep 30"));
    // An app's own code font, over its theme's (none at first).
    let chosen = app.ui.runtime.signal(None::<&'static str>);
    let over = chosen.clone();
    let renderer = zgx::terminal::TerminalRenderer::new(zgx_terminal::terminal_view);
    app.mount(provide(
        renderer,
        theme_scope_with(
            move || match over.get() {
                Some(name) => ThemeOverride::new().with(tok::FONT_MONO, FontFamily::from(name)),
                None => ThemeOverride::new(),
            },
            zgx::terminal::terminal(&term).w(600.).h(300.),
        ),
    ));
    app.until("the output", |app| app.shows("fonts"));
    // zgx's own themes name the platform's monospaced font.
    assert_eq!(app.family_of("fonts"), Some(FontFamily::Monospace));
    chosen.set(Some("Test Mono"));
    app.until("the rows in the chosen font", |app| {
        app.family_of("fonts") == Some(FontFamily::from("Test Mono"))
    });
    chosen.set(None);
    app.until("the rows in the theme's font again", |app| {
        app.family_of("fonts") == Some(FontFamily::Monospace)
    });
    term.close();
}

#[cfg(unix)]
#[test]
fn the_rows_are_in_the_themes_terminal_colours_and_follow_each_switch() {
    use zgx::{
        scene::Color,
        theme::{Theme, ThemeOverride, tok},
    };
    let mut app = Harness::new();
    app.ui.theme().register(
        Theme::from_json(
            r##"{ "name": "test-paper", "type": "light", "tokens": {
                "terminal.foreground": "#202124", "terminal.ansi.1": "#b00020" } }"##,
        )
        .unwrap(),
    );
    let term = Terminal::new(
        &app.ui.runtime,
        session(
            r"printf 'plain [31mred[0m
'; sleep 30",
        ),
    );
    // A scope's own text colour over the theme's (none at first).
    let scoped = app.ui.runtime.signal(false);
    let over = scoped.clone();
    let renderer = zgx::terminal::TerminalRenderer::new(zgx_terminal::terminal_view);
    app.mount(provide(
        renderer,
        theme_scope_with(
            move || match over.get() {
                true => ThemeOverride::new().with(tok::TERMINAL_FOREGROUND, Color(0, 0, 255, 255)),
                false => ThemeOverride::new(),
            },
            zgx::terminal::terminal(&term).w(600.).h(300.),
        ),
    ));
    app.until("the output", |app| app.shows("red"));
    // zgx's own themes set no terminal colours: the palette's show.
    let palette = zgx_terminal::Palette::default();
    let own =
        |rgb: zgx_terminal::alacritty_terminal::vte::ansi::Rgb| Color(rgb.r, rgb.g, rgb.b, 255);
    let (text, red) = (own(palette.foreground), own(palette.ansi[1]));
    let colours = |app: &Harness| (app.color_of("plain"), app.color_of("red"));
    assert_eq!(colours(&app), (Some(text), Some(red)));
    let paper = (
        Some(Color(0x20, 0x21, 0x24, 255)),
        Some(Color(0xb0, 0x00, 0x20, 255)),
    );
    for _ in 0..3 {
        app.ui.theme().set("test-paper").unwrap();
        app.until("the paper theme's colours", |app| colours(app) == paper);
        // A scope's token over it; the unset red stays the theme's.
        scoped.set(true);
        app.until("the scope's text colour", |app| {
            colours(app) == (Some(Color(0, 0, 255, 255)), paper.1)
        });
        scoped.set(false);
        app.until("the theme's again", |app| colours(app) == paper);
        app.ui.theme().set(zgx::theme::DARK).unwrap();
        app.until("the palette's own again", |app| {
            colours(app) == (Some(text), Some(red))
        });
    }
    term.close();
}

#[cfg(unix)]
#[test]
fn an_apps_resolver_names_what_is_in_front() {
    // A session server's client in front: the app says what is behind it.
    zgx_terminal::front::resolve_with(|pid| {
        (pid != 0).then(|| zgx_terminal::Foreground {
            name: "claude".into(),
            pid: 4242,
            idle: false,
            status: Some(zgx::terminal::ProgramStatus {
                state: zgx::terminal::ProgramState::Blocked,
                kind: Some("permission".into()),
                ..Default::default()
            }),
        })
    });
    zgx_terminal::front::set_pushed(true);
    let mut app = Harness::new();
    let term = Terminal::new(&app.ui.runtime, session("sleep 30"));
    app.mount(zgx_terminal::terminal_view(&term).w(600.).h(300.));
    app.until("the resolved process", |_| term.process() == "claude");
    let state = term.state().get();
    assert_eq!(state.pid, 4242);
    assert_eq!(
        state.status.map(|status| status.state),
        Some(zgx::terminal::ProgramState::Blocked)
    );
    assert!(term.needs_confirm_close(), "it isn't idle");
    // The app says it changed: each terminal asks again.
    zgx_terminal::front::resolve_with(|pid| {
        (pid != 0).then(|| zgx_terminal::Foreground {
            name: "zsh".into(),
            pid: 4243,
            idle: true,
            status: None,
        })
    });
    zgx_terminal::front::changed();
    app.until("the new process", |_| term.process() == "zsh");
    assert!(!term.needs_confirm_close());
}

#[cfg(windows)]
#[test]
fn a_windows_programs_output_shows_in_rows() {
    let mut app = Harness::new();
    let term = Terminal::new(
        &app.ui.runtime,
        session("cmd.exe /d /c echo hello world& ping -n 30 127.0.0.1 >nul"),
    );
    app.mount(zgx_terminal::terminal_view(&term).w(600.).h(300.));
    app.until("the output", |app| app.shows("hello world"));
    let state = term.state().get();
    assert!(state.columns > 10 && state.rows > 5, "{state:?}");
    // cmd starts ping after its echo shows: closing asks first once ping
    // is in front (cmd's newest child), not as soon as the echo is read.
    app.until("ping in front", |_| term.needs_confirm_close());
}

#[cfg(unix)]
#[test]
fn a_program_asking_for_the_kitty_keyboard_protocol_gets_it() {
    let mut app = Harness::new();
    // Push "disambiguate" (CSI > 1 u), ask what is on (CSI ? u), read the
    // answer, then read one key.
    let term = Terminal::new(
        &app.ui.runtime,
        session(
            r#"exec sh -c 'stty raw -echo; printf "\033[>1u\033[?u"; dd bs=1 count=5 2>/dev/null | od -An -c | tr -d " \n"; printf "\r\n"; dd bs=1 count=7 2>/dev/null | od -An -c | tr -d " \n"; printf "\r\n"; exec sleep 30'"#,
        ),
    );
    app.mount(zgx_terminal::terminal_view(&term).w(600.).h(300.));
    app.until("the flags' answer", |app| app.shows(r"033[?1u"));
    term.focus();
    app.until("focus", |_| term.focused());
    app.ui.dispatch(InputEvent::KeyDown {
        key: Key::Character("c".into()),
        modifiers: Modifiers {
            control: true,
            ..Modifiers::default()
        },
        repeat: false,
    });
    // Ctrl+C as CSI 99;5 u, not ^C.
    app.until("the key", |app| app.shows(r"033[99;5u"));
}

/// A platform keeping a clipboard and a primary selection in memory.
#[cfg(unix)]
#[derive(Default)]
struct Selections {
    clipboard: RefCell<Option<String>>,
    primary: RefCell<Option<String>>,
}
#[cfg(unix)]
impl zgx::platform::Platform for Selections {
    fn clipboard_text(&self) -> Option<String> {
        self.clipboard.borrow().clone()
    }
    fn set_clipboard_text(&self, text: String) -> bool {
        *self.clipboard.borrow_mut() = Some(text);
        true
    }
    fn primary_text(&self) -> Option<String> {
        self.primary.borrow().clone()
    }
    fn set_primary_text(&self, text: String) -> bool {
        *self.primary.borrow_mut() = Some(text);
        true
    }
    fn open_files(
        &self,
        _: zgx::platform::FilePicker,
    ) -> zgx::platform::Pending<Result<Vec<std::path::PathBuf>, String>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn save_file(
        &self,
        _: zgx::platform::FilePicker,
    ) -> zgx::platform::Pending<Result<Option<std::path::PathBuf>, String>> {
        Box::pin(async { Ok(None) })
    }
}

#[cfg(unix)]
#[test]
fn a_selection_is_the_primary_and_a_middle_click_pastes_it() {
    use zgx::input::PointerButton;
    let mut app = Harness::new();
    let selections = Rc::new(Selections::default());
    let platform = zgx::platform::PlatformService(selections.clone());
    let term = Terminal::new(&app.ui.runtime, session(r"printf 'pick me\r\n'; exec cat"));
    app.mount(provide(
        platform,
        zgx_terminal::terminal_view(&term).w(600.).h(300.),
    ));
    app.until("the output", |app| app.shows("pick me"));
    // Drag across the first row's text (the grid starts 10 points in, 6
    // down), as far as its end.
    let (y, right) = (12., 400.);
    let press = |app: &mut Harness, event| {
        app.ui.dispatch(event);
        app.pump();
    };
    press(
        &mut app,
        InputEvent::PointerDown {
            x: 11.,
            y,
            button: PointerButton::Primary,
        },
    );
    press(&mut app, InputEvent::PointerMove { x: right, y });
    press(
        &mut app,
        InputEvent::PointerUp {
            x: right,
            y,
            button: PointerButton::Primary,
        },
    );
    assert_eq!(selections.primary.borrow().as_deref(), Some("pick me"));
    // A middle click pastes it to cat, which echoes it back.
    *selections.primary.borrow_mut() = Some("from elsewhere".into());
    press(
        &mut app,
        InputEvent::PointerDown {
            x: 100.,
            y: 100.,
            button: PointerButton::Middle,
        },
    );
    press(
        &mut app,
        InputEvent::PointerUp {
            x: 100.,
            y: 100.,
            button: PointerButton::Middle,
        },
    );
    app.until("the paste", |app| {
        app.texts().iter().any(|t| t.starts_with("from elsewhere"))
    });
}

/// A program's first output reaches the app's clipboard: the platform is
/// the session's before it is first read, which can be as it starts (on a
/// busy machine, the program had written before the view gave the
/// terminal its platform, and its copy or its question went unanswered).
#[cfg(unix)]
#[test]
fn a_programs_first_output_copies_to_the_clipboard() {
    let mut app = Harness::new();
    let selections = Rc::new(Selections::default());
    let platform = zgx::platform::PlatformService(selections.clone());
    // OSC 52: copy "secret" (c2VjcmV0), first thing.
    let term = Terminal::new(
        &app.ui.runtime,
        session(r#"printf '\033]52;c;c2VjcmV0\007'; exec sleep 30"#),
    );
    app.mount(provide(
        platform,
        zgx_terminal::terminal_view(&term).w(600.).h(300.),
    ));
    app.until("the copy", |_| {
        selections.clipboard.borrow().as_deref() == Some("secret")
    });
}

#[cfg(unix)]
#[test]
fn a_program_reads_the_clipboard_when_the_app_allows_it() {
    for allow in [true, false] {
        let mut app = Harness::new();
        let selections = Rc::new(Selections::default());
        *selections.clipboard.borrow_mut() = Some("secret".into());
        let platform = zgx::platform::PlatformService(selections.clone());
        // Ask for the clipboard (OSC 52) and show the answer's bytes.
        let term = Terminal::new(
            &app.ui.runtime,
            session(
                r#"exec sh -c 'stty raw -echo; printf "\033]52;c;?\007"; head -c 8 | od -An -c | tr -d " \n"; printf "\r\n"; exec sleep 30'"#,
            ),
        );
        let asked = Rc::new(std::cell::Cell::new(0));
        let counted = asked.clone();
        term.on_clipboard_read(Some(move |request: zgx::terminal::ClipboardRead| {
            counted.set(counted.get() + 1);
            if allow {
                request.allow();
            } else {
                request.deny();
            }
        }));
        app.mount(provide(
            platform,
            zgx_terminal::terminal_view(&term).w(600.).h(300.),
        ));
        // The answer's first eight bytes: the clipboard's text in base64
        // starting ("secret" is c2VjcmV0), or the answer ending empty.
        let expected = if allow { r"033]52;c;c" } else { r"033]52;c;\a" };
        app.until("the answer", |app| {
            app.texts().iter().any(|t| t.starts_with(expected))
        });
        assert_eq!(asked.get(), 1, "the app's reader was asked once");
    }
}
