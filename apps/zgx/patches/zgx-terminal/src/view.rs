//! The terminal zgx draws: a [`Session`] behind a `zgx::terminal::Terminal`
//! controller, shown by [`terminal_view`].
//!
//! Each row of the screen is a canvas drawing what [`crate::paint`] worked
//! out for it: when output arrives, only the rows the screen model marks
//! damaged are worked out again, and only those that differ draw again.
//! Canvases stand at their lines in a container that moves up as the screen
//! scrolls, so a scroll draws the new lines alone: the rest move with it.
//! Keys go to the program through a hidden editor at the cursor, so input
//! methods compose there and place their candidates at the cursor on every
//! platform; what they commit is sent, never kept.
use crate::{
    GridSize, Session, SessionEvent, SessionOptions, keys,
    paint::{self, CursorAt, Look, Metrics, RowPaint},
};
use alacritty_terminal::{
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionRange, SelectionType},
    term::TermMode,
};
use std::{
    any::Any,
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    sync::{Arc, Mutex},
    time::Duration,
};
use zgx::{
    compose::{NodeHandle, TaskRunner, TaskToken, prelude::*},
    input::{EventContext, EventPhase, InputEvent, Key, Modifiers, PointerButton},
    platform::PlatformService,
    reactive::{Runtime, Signal},
    semantics::{Live, Role},
    terminal::{
        Terminal, TerminalBackend, TerminalColors, TerminalCommand, TerminalEvent, WeakTerminal,
    },
};

/// Around the grid, in points (as the Ghostty backend's padding).
const PAD_X: f32 = 10.;
const PAD_Y: f32 = 6.;
/// The font size when the session's options give none.
const FONT_SIZE: f32 = 13.;
/// How long after output the foreground process is checked again: a
/// command starting or ending shows in its output, and a burst of output
/// costs one check.
const PROCESS_CHECK: Duration = Duration::from_millis(150);

/// A session drawn by zgx, behind a terminal controller (its backend).
pub struct EngineTerminal(Rc<Engine>);

struct Engine {
    term: WeakTerminal,
    runtime: Runtime,
    session: RefCell<Option<Session>>,
    /// The task reading the session as it wakes; cancelled with the engine.
    task: RefCell<Option<TaskToken>>,
    /// The window's that last showed it, and where the session's wakes go:
    /// a view mounting in another window takes them over, so they don't end
    /// with the first window's tasks.
    runner: RefCell<TaskRunner>,
    wake: Arc<Mutex<Option<zgx::task::LatestSender<()>>>>,
    platform: RefCell<Option<Rc<PlatformService>>>,
    /// Each row canvas's paint, as it draws it, and the line it shows (by
    /// slot), counted from `scrolled`'s base: lines stay where they are
    /// while `scrolled` moves the rows' container over them, so a scroll
    /// changes the canvases of new lines alone.
    rows: RefCell<Vec<Signal<Rc<RowPaint>>>>,
    lines: RefCell<Vec<Signal<usize>>>,
    /// The line at the top of the screen.
    scrolled: Signal<usize>,
    /// The slot showing each screen row, and that order for the view (and
    /// for readers, top to bottom).
    slots: RefCell<Vec<usize>>,
    order: Signal<Rc<[usize]>>,
    /// How many rows show, and the metrics they are laid out by (rows are
    /// made again when these change).
    layout: Signal<(u16, u32)>,
    metrics: Cell<Metrics>,
    generation: Cell<u32>,
    /// The grid's font: its family the theme's `font.mono` where the view
    /// is shown (`follow_theme_font`).
    font: RefCell<zgx::text_layout::FontStyle>,
    /// The colours it draws in: its palette, with the theme's `terminal.*`
    /// colours over it where the view is shown (`set_colors`).
    look: Signal<Look>,
    /// Where the hidden editor sits: the cursor's cell, in points.
    caret: Signal<(f32, f32)>,
    /// An input method's composition, drawn at the cursor.
    preedit: Signal<String>,
    /// What a screen reader is told of the latest output (a live region's
    /// label), and what was typed since: a shell's echo of it is not news.
    news: Signal<String>,
    typed: RefCell<String>,
    /// News since the live region last went out, published once a frame
    /// (`publish_news`): reads before a frame's accessibility update each
    /// set the region, and a reader heard only the last (a command's
    /// output lost to the prompt after it).
    pending_news: RefCell<String>,
    news_wake: RefCell<Option<zgx::task::LatestSender<()>>>,
    news_task: RefCell<Option<TaskToken>>,
    frames: RefCell<Option<zgx::frame::FrameClock>>,
    focused: Cell<bool>,
    /// The hidden editor, or the terminal's node (a press focuses it first),
    /// has keyboard focus; the two settle into `focused` once a dispatch is
    /// over, so focus passing between them reports nothing.
    input_has: Cell<bool>,
    root_has: Cell<bool>,
    settling: Cell<bool>,
    /// Views showing it now.
    views: Cell<u32>,
    /// The latest view's nodes (a view mounting in another split before
    /// the last unmounts takes them over), and which view that is.
    root: RefCell<Option<NodeHandle>>,
    input: RefCell<Option<NodeHandle>>,
    latest_view: Cell<u64>,
    /// Focus asked for before a view showed it.
    pending_focus: Cell<bool>,
    /// Every row must be worked out again (a view mounted, the font changed).
    full: Cell<bool>,
    last_cursor: Cell<Option<usize>>,
    last_selection: RefCell<Option<SelectionRange>>,
    last_offset: Cell<usize>,
    last_focus: Cell<bool>,
    /// The content box last laid out, in points.
    size: Cell<(f32, f32)>,
    /// A press being dragged: selecting, or reported to the program.
    pressed: Cell<Option<Press>>,
    /// Wheel travel short of a whole line.
    scroll_rest: Cell<f32>,
    pid: Cell<u32>,
    /// When the pending check of what is in front runs: `PROCESS_CHECK`
    /// after the latest output, pushed back by output meanwhile.
    process_check: Cell<Option<std::time::Instant>>,
    process_task: RefCell<Option<TaskToken>>,
    /// What the app's resolver last saw in front ([`crate::front`]), the
    /// app's count of changes last seen, and its polling.
    front: RefCell<Option<crate::Foreground>>,
    resolved: Cell<bool>,
    seen_changes: Cell<u64>,
    polling: Cell<bool>,
    poll_task: RefCell<Option<TaskToken>>,
    exited: Cell<bool>,
    /// How many times the session was read (`reads`).
    reads: Cell<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Press {
    Selecting,
    Reporting(PointerButton),
}

impl EngineTerminal {
    fn new(
        term: &Terminal,
        runtime: Runtime,
        runner: TaskRunner,
        platform: Option<Rc<PlatformService>>,
    ) -> std::io::Result<Self> {
        let options = term.options();
        let font = paint::font();
        let engine = Rc::new(Engine {
            term: term.downgrade(),
            runtime: runtime.clone(),
            session: RefCell::new(None),
            task: RefCell::new(None),
            runner: RefCell::new(runner),
            wake: Arc::new(Mutex::new(None)),
            // Before the session starts: what it reads first (a program asking
            // for the clipboard at once) is answered through it.
            platform: RefCell::new(platform),
            rows: RefCell::new(Vec::new()),
            lines: RefCell::new(Vec::new()),
            scrolled: runtime.signal(0),
            slots: RefCell::new(Vec::new()),
            order: runtime.signal(Rc::from([])),
            layout: runtime.signal((24, 0)),
            metrics: Cell::new(Metrics {
                font_size: options.font_size.unwrap_or(FONT_SIZE),
                cell_width: 0.,
                cell_height: 0.,
            }),
            generation: Cell::new(0),
            font: RefCell::new(font),
            look: runtime.signal(Look::default()),
            caret: runtime.signal((PAD_X, PAD_Y)),
            preedit: runtime.signal(String::new()),
            news: runtime.signal(String::new()),
            typed: RefCell::new(String::new()),
            pending_news: RefCell::new(String::new()),
            news_wake: RefCell::new(None),
            news_task: RefCell::new(None),
            frames: RefCell::new(None),
            focused: Cell::new(false),
            input_has: Cell::new(false),
            root_has: Cell::new(false),
            settling: Cell::new(false),
            views: Cell::new(0),
            root: RefCell::new(None),
            input: RefCell::new(None),
            latest_view: Cell::new(0),
            pending_focus: Cell::new(false),
            full: Cell::new(true),
            last_cursor: Cell::new(None),
            last_selection: RefCell::new(None),
            last_offset: Cell::new(0),
            last_focus: Cell::new(false),
            size: Cell::new((0., 0.)),
            pressed: Cell::new(None),
            scroll_rest: Cell::new(0.),
            pid: Cell::new(0),
            process_check: Cell::new(None),
            process_task: RefCell::new(None),
            front: RefCell::new(None),
            resolved: Cell::new(false),
            seen_changes: Cell::new(crate::front::changes()),
            polling: Cell::new(false),
            poll_task: RefCell::new(None),
            exited: Cell::new(false),
            reads: Cell::new(0),
        });
        crate::front::register(Arc::downgrade(&engine.wake));
        let wake = engine.wake.clone();
        let session = Session::spawn(
            &SessionOptions {
                command: options.command.clone(),
                working_directory: options.working_directory.clone().or_else(home_directory),
                env: options.env.clone(),
                ..SessionOptions::default()
            },
            GridSize::new(80, 24),
            move || {
                if let Some(wake) = &*wake.lock().unwrap() {
                    wake.send(());
                }
            },
        )?;
        session.set_palette(engine.look.with_untracked(|look| look.palette.clone()));
        *engine.session.borrow_mut() = Some(session);
        engine.listen();
        engine.check_process();
        Ok(Self(engine))
    }
}

fn home_directory() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    home.map(Into::into)
}

impl Engine {
    fn with_session<R>(&self, f: impl FnOnce(&Session) -> R) -> Option<R> {
        self.session.borrow().as_ref().map(f)
    }
    fn controller(&self) -> Option<Terminal> {
        self.term.upgrade()
    }

    /// Read the session as it wakes, on the runner's thread of tasks (again
    /// after a view mounts with another runner).
    fn listen(self: &Rc<Self>) {
        let (wake, mut woken) = zgx::task::latest::<()>();
        *self.wake.lock().unwrap() = Some(wake);
        let weak = Rc::downgrade(self);
        let task = self.runner.borrow().spawn(Box::pin(async move {
            // Read at most once a refresh: output arriving faster (a
            // program printing as fast as it can, in ConPTY's small chunks
            // a thousand times a second) waits for the next read, all of it
            // read and drawn at once. Output after a pause (a key's echo)
            // is read as it arrives.
            let mut read_at: Option<std::time::Instant> = None;
            while woken.recv().await.is_some() {
                let interval = {
                    let Some(engine) = weak.upgrade() else { return };
                    engine.refresh_interval()
                };
                if let Some(next) = read_at.map(|at| at + interval)
                    && std::time::Instant::now() < next
                {
                    zgx::timer::sleep_until(next).await;
                }
                let Some(engine) = weak.upgrade() else { return };
                read_at = Some(std::time::Instant::now());
                engine.refresh();
            }
        }));
        // The task before ends with its token.
        *self.task.borrow_mut() = Some(task);
        // News waits for the next frame, its accessibility update carrying
        // everything read since the last (`publish_news`).
        let (news_wake, mut news_woken) = zgx::task::latest::<()>();
        *self.news_wake.borrow_mut() = Some(news_wake);
        let weak = Rc::downgrade(self);
        let news_task = self.runner.borrow().spawn(Box::pin(async move {
            while news_woken.recv().await.is_some() {
                let frames = {
                    let Some(engine) = weak.upgrade() else { return };
                    engine.frames.borrow().clone()
                };
                if let Some(frames) = frames {
                    frames.next().await;
                }
                let Some(engine) = weak.upgrade() else { return };
                let news = std::mem::take(&mut *engine.pending_news.borrow_mut());
                if !news.is_empty() {
                    engine.news.set(news);
                }
            }
        }));
        *self.news_task.borrow_mut() = Some(news_task);
        // A wake the last task didn't read: the session won't wake again
        // until it is read.
        self.refresh();
    }

    /// The refresh period of the display the terminal shows on (60 Hz
    /// while it shows nowhere).
    fn refresh_interval(&self) -> std::time::Duration {
        self.root
            .borrow()
            .as_ref()
            .and_then(NodeHandle::refresh_interval)
            .unwrap_or(std::time::Duration::from_nanos(16_666_667))
    }

    /// Read what the session reported and draw what changed.
    fn refresh(self: &Rc<Self>) {
        let Some(events) = self.with_session(Session::take_events) else {
            return;
        };
        self.reads.set(self.reads.get() + 1);
        let term = self.controller();
        // The app said what is in front may have changed.
        let changes = crate::front::changes();
        let mut recheck = changes != self.seen_changes.replace(changes);
        for event in events {
            let Some(term) = &term else { break };
            match event {
                SessionEvent::Title(title) => {
                    term.update_state(|state| state.title = title.clone());
                    term.emit(TerminalEvent::TitleChanged { title });
                }
                SessionEvent::Pwd(pwd) => {
                    term.update_state(|state| state.pwd = pwd.clone());
                    term.emit(TerminalEvent::PwdChanged { pwd });
                    recheck = true;
                }
                SessionEvent::Bell => {
                    if !term.focused() {
                        term.update_state(|state| state.bell = true);
                    }
                    term.emit(TerminalEvent::Bell);
                }
                SessionEvent::Progress(progress) => term.update_state(|state| {
                    state.progress = progress.fraction();
                    state.progress_error = matches!(progress, crate::Progress::Error(_));
                }),
                SessionEvent::CommandFinished {
                    exit_code,
                    duration_ms,
                } => {
                    term.emit(TerminalEvent::CommandFinished {
                        exit_code,
                        duration_ms,
                    });
                    recheck = true;
                }
                SessionEvent::Notification { title, body } => {
                    term.emit(TerminalEvent::Notification { title, body })
                }
                SessionEvent::ClipboardStore(text) => {
                    if let Some(platform) = &*self.platform.borrow() {
                        platform.0.set_clipboard_text(text);
                    }
                }
                // The app answers, as its user says (its reader), or
                // nothing does: the program is told the clipboard is empty.
                SessionEvent::ClipboardLoad(load) => {
                    let weak = Rc::downgrade(self);
                    let request = zgx::terminal::ClipboardRead::new(move |allow| {
                        let Some(engine) = weak.upgrade() else { return };
                        let text = allow
                            .then(|| {
                                engine
                                    .platform
                                    .borrow()
                                    .as_ref()
                                    .and_then(|p| p.0.clipboard_text())
                            })
                            .flatten()
                            .unwrap_or_default();
                        engine
                            .with_session(|session| session.write(load.reply(&text).into_bytes()));
                    });
                    // Unanswered by the app: denied as it drops.
                    drop(term.request_clipboard_read(request));
                }
                SessionEvent::Exited(code) => {
                    self.exited.set(true);
                    term.update_state(|state| {
                        state.exited = Some(code);
                        state.pid = 0;
                        state.process.clear();
                    });
                    term.emit(TerminalEvent::Exited { code });
                    term.emit(TerminalEvent::CloseRequested {
                        process_alive: false,
                    });
                }
            }
        }
        if self.views.get() > 0 {
            self.paint();
        }
        if recheck {
            self.check_process();
        } else {
            self.schedule_process_check();
        }
    }

    /// Work out the rows that changed and hand the canvases those that
    /// differ.
    fn paint(&self) {
        let session = self.session.borrow();
        let Some(session) = session.as_ref() else {
            return;
        };
        let metrics = self.metrics.get();
        if metrics.cell_width <= 0. {
            return;
        }
        let focused = self.focused.get();
        let mut changed = Vec::new();
        let caret;
        let output;
        let screen_rows;
        {
            let mut term = session.term();
            let rows = term.screen_lines();
            screen_rows = rows;
            let offset = term.grid().display_offset();
            let mode = *term.mode();
            let point = term.grid().cursor.point;
            let cursor_row = (offset == 0 && mode.contains(TermMode::SHOW_CURSOR))
                .then(|| (point.line.0 + offset as i32) as usize)
                .filter(|row| *row < rows);
            let shape = term.cursor_style().shape;
            caret = (
                PAD_X + point.column.0 as f32 * metrics.cell_width,
                PAD_Y + (point.line.0 + offset as i32).max(0) as f32 * metrics.cell_height,
            );
            let selection = term.selection.as_ref().and_then(|s| s.to_range(&term));
            let mut all = self.full.replace(false)
                || offset != self.last_offset.replace(offset)
                || focused != self.last_focus.replace(focused)
                || selection != *self.last_selection.borrow();
            // Rows drawn again for the view's sake (mounted, scrolled back,
            // focus, a selection) are no news to a reader.
            output = !all;
            *self.last_selection.borrow_mut() = selection;
            let mut damaged = vec![false; rows];
            match term.damage() {
                alacritty_terminal::term::TermDamage::Full => all = true,
                alacritty_terminal::term::TermDamage::Partial(lines) => {
                    for line in lines {
                        if let Some(row) = damaged.get_mut(line.line) {
                            *row = true;
                        }
                    }
                }
            }
            let was = self.last_cursor.replace(cursor_row);
            for row in [was, cursor_row].into_iter().flatten() {
                if let Some(row) = damaged.get_mut(row) {
                    *row = true;
                }
            }
            self.look.with_untracked(|look| {
                for (row, damaged) in damaged.into_iter().enumerate() {
                    if !(all || damaged) {
                        continue;
                    }
                    let cursor = (cursor_row == Some(row)).then_some(CursorAt {
                        column: point.column.0,
                        shape,
                        focused,
                    });
                    changed.push((
                        row,
                        paint::row(&term, row, look, metrics, cursor, selection.as_ref()),
                    ));
                }
            });
            term.reset_damage();
        }
        self.fit_slots(screen_rows);
        // Worked out only while a screen reader listens.
        let listening = self
            .root
            .borrow()
            .as_ref()
            .is_some_and(NodeHandle::assistive_technology);
        let every_row = changed.len() == screen_rows;
        let shown: Vec<Rc<RowPaint>> = if every_row || (output && listening) {
            let rows = self.rows.borrow();
            self.slots
                .borrow()
                .iter()
                .map(|slot| rows[*slot].with_untracked(Rc::clone))
                .collect()
        } else {
            Vec::new()
        };
        let news = if output && listening {
            self.news_of(&shown, &changed)
        } else {
            String::new()
        };
        self.runtime.batch(|| {
            // Every row worked out again (output scrolled, most often): the
            // slots of lines still on screen move up with them.
            if every_row {
                self.scroll_slots(shift(&shown, &changed));
            }
            let rows = self.rows.borrow();
            let slots = self.slots.borrow();
            for (row, paint) in changed {
                if let Some(signal) = slots.get(row).map(|slot| &rows[*slot])
                    && signal.with_untracked(|shown| **shown != paint)
                {
                    signal.set(Rc::new(paint));
                }
            }
            self.caret.set(caret);
            if !news.is_empty() {
                self.publish_news(news);
            }
        });
    }

    /// Tell readers `news` with the frame's accessibility update: added to
    /// what is waiting for it, so that two reads before a frame are both
    /// heard, once each. Without a frame clock (no window), at once.
    fn publish_news(&self, news: String) {
        let waiting = {
            let mut pending = self.pending_news.borrow_mut();
            if !pending.is_empty() {
                pending.push('\n');
            }
            pending.push_str(&news);
            pending.clone()
        };
        match &*self.news_wake.borrow() {
            Some(wake) if self.frames.borrow().is_some() => {
                wake.send(());
            }
            _ => {
                self.pending_news.borrow_mut().clear();
                self.news.set(waiting);
            }
        }
    }

    /// What a reader hears of rows `changed` from what `rows` show: new
    /// lines and what lines gained, less the shell's echo of what was typed.
    fn news_of(&self, shown: &[Rc<RowPaint>], changed: &[(usize, RowPaint)]) -> String {
        if changed.is_empty() {
            return String::new();
        }
        let mut after: Vec<&str> = shown.iter().map(|paint| paint.text.as_str()).collect();
        let before: Vec<&str> = after.clone();
        for (row, paint) in changed {
            if let Some(text) = after.get_mut(*row) {
                *text = &paint.text;
            }
        }
        let news = crate::news::news(&before, &after);
        crate::news::less_echo(&mut self.typed.borrow_mut(), news)
    }

    /// Slot `slot`'s paint and line, made on first use.
    fn slot(&self, slot: usize) -> (Signal<Rc<RowPaint>>, Signal<usize>) {
        let mut rows = self.rows.borrow_mut();
        let mut lines = self.lines.borrow_mut();
        while rows.len() <= slot {
            rows.push(self.runtime.signal(Rc::new(RowPaint::default())));
            let line = lines.len();
            lines.push(self.runtime.signal(line));
        }
        (rows[slot].clone(), lines[slot].clone())
    }

    /// Slots for `rows` screen rows, each showing its own row again if
    /// they were for another count.
    fn fit_slots(&self, rows: usize) {
        if self.slots.borrow().len() == rows {
            return;
        }
        for slot in 0..rows {
            self.slot(slot);
        }
        *self.slots.borrow_mut() = (0..rows).collect();
        self.rebase();
    }

    /// Each slot's line counted from the top of the screen again: lines
    /// counted from far back would be placed less exactly.
    fn rebase(&self) {
        let slots = self.slots.borrow();
        let lines = self.lines.borrow();
        for (row, slot) in slots.iter().enumerate() {
            if lines[*slot].with_untracked(|line| *line != row) {
                lines[*slot].set(row);
            }
        }
        if self.scrolled.with_untracked(|top| *top != 0) {
            self.scrolled.set(0);
        }
        self.order.set(slots.iter().copied().collect());
    }

    /// The screen scrolled by `shift` lines: each screen row is shown by the
    /// slot of the row `shift` below it, which keeps its line and what it
    /// draws, and the first `shift` slots take the lines new at the bottom.
    fn scroll_slots(&self, shift: usize) {
        /// Lines scrolled before the slots' lines are counted afresh.
        const REBASE: usize = 4096;
        let count = self.slots.borrow().len();
        if shift == 0 || shift >= count {
            return;
        }
        let top = self.scrolled.with_untracked(|top| *top) + shift;
        let mut slots = self.slots.borrow_mut();
        slots.rotate_left(shift);
        if top >= REBASE {
            drop(slots);
            self.rebase();
            return;
        }
        let lines = self.lines.borrow();
        for row in count - shift..count {
            lines[slots[row]].set(top + row);
        }
        self.scrolled.set(top);
        self.order.set(slots.iter().copied().collect());
    }

    /// Fit the grid to the view's `size` (its content box, in points).
    fn fit(&self, size: (f32, f32)) {
        self.size.set(size);
        let metrics = self.metrics.get();
        if metrics.cell_width <= 0. || size.0 <= 0. || size.1 <= 0. {
            return;
        }
        let columns = ((size.0 - 2. * PAD_X) / metrics.cell_width).floor().max(1.) as u16;
        let rows = ((size.1 - 2. * PAD_Y) / metrics.cell_height)
            .floor()
            .max(1.) as u16;
        let scale = 2.;
        self.with_session(|session| {
            session.resize(GridSize {
                columns,
                rows,
                cell_width: (metrics.cell_width * scale) as u16,
                cell_height: (metrics.cell_height * scale) as u16,
            })
        });
        if let Some(term) = self.controller() {
            let (was_columns, was_rows) = term
                .state()
                .with_untracked(|state| (state.columns, state.rows));
            if (was_columns, was_rows) != (columns, rows) {
                term.update_state(|state| {
                    state.columns = columns;
                    state.rows = rows;
                });
            }
        }
        self.full.set(true);
        self.fit_slots(usize::from(rows));
        let layout = (rows, self.generation.get());
        if self.layout.with_untracked(|shown| *shown != layout) {
            self.layout.set(layout);
        }
        self.paint();
    }

    /// Measure the cell for the font size (on mounting, and as it changes).
    fn measure(&self, scene: &zgx::scene::Scene) {
        let mut metrics = self.metrics.get();
        let (width, height) = scene.measure_text_with_font(
            "0000000000",
            metrics.font_size,
            None,
            &self.font.borrow(),
        );
        metrics.cell_width = width / 10.;
        metrics.cell_height = height.ceil();
        if metrics != self.metrics.get() {
            self.metrics.set(metrics);
            self.generation.set(self.generation.get().wrapping_add(1));
        }
    }

    /// Draw the grid in `family` from now on: the cell measured again and
    /// every row made again in it.
    fn set_font_family(&self, family: zgx::text_layout::FontFamily) {
        if self.font.borrow().family == family {
            return;
        }
        self.font.borrow_mut().family = family;
        // (Rows are made again for a new generation, in the new font, even
        // where the cell measures as it did.)
        self.generation.set(self.generation.get().wrapping_add(1));
        if let Some(scene) = self.root.borrow().as_ref().and_then(NodeHandle::scene) {
            self.measure(&scene.borrow());
            self.fit(self.size.get());
        }
    }

    /// Draw in the theme's `colors` over the palette: every row repaints,
    /// and programs asking the colours (OSC 4, 10, 11) are told these.
    fn set_colors(&self, colors: &TerminalColors) {
        let look = Look::themed(colors);
        if self.look.with_untracked(|old| *old == look) {
            return;
        }
        self.with_session(|session| session.set_palette(look.palette.clone()));
        self.look.set(look);
        self.full.set(true);
        self.paint();
    }

    fn set_font_size(&self, size: f32) {
        let size = size.clamp(6., 72.);
        let mut metrics = self.metrics.get();
        if (metrics.font_size - size).abs() < 0.01 {
            return;
        }
        metrics.font_size = size;
        self.metrics.set(metrics);
        if let Some(scene) = self.root.borrow().as_ref().and_then(NodeHandle::scene) {
            self.measure(&scene.borrow());
        }
        self.fit(self.size.get());
    }

    fn write(&self, bytes: Vec<u8>) {
        self.with_session(|session| {
            // Typing goes to the bottom, where the program is.
            let mut term = session.term();
            if term.grid().display_offset() != 0 {
                term.scroll_display(Scroll::Bottom);
            }
            drop(term);
            session.write(bytes);
        });
        self.paint();
    }

    fn mode(&self) -> TermMode {
        self.with_session(|session| *session.term().mode())
            .unwrap_or(TermMode::empty())
    }

    fn paste(&self, text: &str) {
        if !text.is_empty() {
            let bytes = keys::paste(text, self.mode());
            self.write(bytes);
        }
    }

    fn copy(&self) -> bool {
        let text = self
            .with_session(|session| session.term().selection_to_string())
            .flatten();
        match (text, &*self.platform.borrow()) {
            (Some(text), Some(platform)) if !text.is_empty() => platform.0.set_clipboard_text(text),
            _ => false,
        }
    }

    fn paste_clipboard(&self) {
        let text = self
            .platform
            .borrow()
            .as_ref()
            .and_then(|platform| platform.0.clipboard_text());
        if let Some(text) = text {
            self.paste(&text);
        }
    }

    fn focus_changed(&self, focused: bool) {
        if self.focused.replace(focused) == focused {
            return;
        }
        if self.mode().contains(TermMode::FOCUS_IN_OUT) {
            self.with_session(|session| {
                session.write(if focused {
                    b"\x1b[I".as_slice()
                } else {
                    b"\x1b[O"
                })
            });
        }
        if let Some(term) = self.controller() {
            term.update_state(|state| {
                state.focused = focused;
                if focused {
                    state.bell = false;
                }
            });
        }
        self.paint();
    }

    /// Focus came to (`has`) or left the editor or the terminal's node.
    fn focus_event(self: &Rc<Self>, root: bool, has: bool) {
        if root {
            self.root_has.set(has);
        } else {
            self.input_has.set(has);
        }
        if self.settling.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        zgx::native_view::post(move || {
            if let Some(engine) = weak.upgrade() {
                engine.settling.set(false);
                engine.focus_changed(engine.input_has.get() || engine.root_has.get());
            }
        });
    }

    /// A keyboard event at the editor, or at the terminal's node while it
    /// holds focus on the way to the editor.
    fn keyboard(self: &Rc<Self>, root: bool, cx: &mut EventContext) {
        match cx.event.clone() {
            InputEvent::KeyDown {
                key,
                modifiers,
                repeat,
            } => self.key(&key, modifiers, repeat, cx),
            InputEvent::KeyUp { key, modifiers } => self.key_up(&key, modifiers, cx),
            InputEvent::Text(text) | InputEvent::ImeCommit(text) => {
                if self.preedit.with_untracked(|shown| !shown.is_empty()) {
                    self.preedit.set(String::new());
                }
                {
                    let mut typed = self.typed.borrow_mut();
                    if typed.len() < 256 {
                        typed.push_str(&text);
                    }
                }
                self.write(text.into_bytes());
                cx.prevent_default();
            }
            InputEvent::ImePreedit { text, .. } => {
                if self.preedit.with_untracked(|shown| *shown != text) {
                    self.preedit.set(text);
                }
            }
            InputEvent::Focus => {
                self.focus_event(root, true);
                if root {
                    self.focus(true);
                }
            }
            InputEvent::Blur => self.focus_event(root, false),
            _ => {}
        }
    }

    /// The cell under (`x`, `y`) in the view, and which half of it.
    fn cell_at(&self, x: f32, y: f32) -> Option<(Point, Side, usize)> {
        let metrics = self.metrics.get();
        if metrics.cell_width <= 0. {
            return None;
        }
        self.with_session(|session| {
            let term = session.term();
            let column = ((x - PAD_X) / metrics.cell_width).max(0.);
            let row =
                (((y - PAD_Y) / metrics.cell_height).max(0.) as usize).min(term.screen_lines() - 1);
            let side = if column.fract() < 0.5 {
                Side::Left
            } else {
                Side::Right
            };
            let column = (column as usize).min(term.columns() - 1);
            let line = Line(row as i32 - term.grid().display_offset() as i32);
            (Point::new(line, Column(column)), side, row)
        })
    }

    /// A pointer event over the grid at view point (`x`, `y`).
    fn pointer(
        self: &Rc<Self>,
        event: &InputEvent,
        x: f32,
        y: f32,
        modifiers: Modifiers,
        clicks: u8,
        cx: &mut EventContext,
    ) {
        let Some((point, side, row)) = self.cell_at(x, y) else {
            return;
        };
        let mode = self.mode();
        // Shift selects even where the program reads the mouse.
        let reported = mode.intersects(TermMode::MOUSE_MODE) && !modifiers.shift;
        let report = |mouse: keys::Mouse| {
            if let Some(bytes) = keys::mouse(mouse, point.column.0, row, modifiers, mode) {
                self.with_session(|session| session.write(bytes));
            }
        };
        match *event {
            InputEvent::PointerDown { button, .. } => {
                // The node takes focus now (keys typed at once reach it),
                // the editor after the press.
                if !self.focused.get() {
                    cx.focus();
                }
                self.focus(true);
                if reported {
                    report(keys::Mouse::Press(button));
                    self.pressed.set(Some(Press::Reporting(button)));
                    cx.capture_pointer();
                    cx.prevent_default();
                    return;
                }
                // X11's and Wayland's middle click pastes what was last
                // selected anywhere.
                if button == PointerButton::Middle {
                    let text = self
                        .platform
                        .borrow()
                        .as_ref()
                        .and_then(|platform| platform.0.primary_text());
                    if let Some(text) = text {
                        self.paste(&text);
                        cx.prevent_default();
                    }
                    return;
                }
                if button != PointerButton::Primary {
                    return;
                }
                if modifiers.primary_shortcut() && self.open_link(point) {
                    cx.prevent_default();
                    return;
                }
                let kind = match clicks {
                    2 => SelectionType::Semantic,
                    n if n >= 3 => SelectionType::Lines,
                    _ => SelectionType::Simple,
                };
                self.with_session(|session| {
                    let mut term = session.term();
                    match (&mut term.selection, modifiers.shift) {
                        (Some(selection), true) => selection.update(point, side),
                        _ => term.selection = Some(Selection::new(kind, point, side)),
                    }
                });
                self.pressed.set(Some(Press::Selecting));
                cx.capture_pointer();
                cx.prevent_default();
                self.paint();
            }
            InputEvent::PointerMove { .. } => match self.pressed.get() {
                Some(Press::Selecting) => {
                    self.with_session(|session| {
                        let mut term = session.term();
                        if let Some(selection) = &mut term.selection {
                            selection.update(point, side);
                        }
                    });
                    self.paint();
                }
                Some(Press::Reporting(button)) => report(keys::Mouse::Move(Some(button))),
                None if reported => report(keys::Mouse::Move(None)),
                None => {}
            },
            InputEvent::PointerUp { button, .. } => match self.pressed.take() {
                Some(Press::Reporting(_)) => report(keys::Mouse::Release(button)),
                Some(Press::Selecting) => {
                    // A click without a drag leaves nothing selected; a
                    // selection made is the primary selection (X11, Wayland).
                    let selected = self
                        .with_session(|session| {
                            let mut term = session.term();
                            if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
                                term.selection = None;
                            }
                            term.selection_to_string()
                        })
                        .flatten()
                        .filter(|text| !text.is_empty());
                    if let (Some(text), Some(platform)) = (selected, &*self.platform.borrow()) {
                        platform.0.set_primary_text(text);
                    }
                    self.paint();
                }
                None => {}
            },
            _ => {}
        }
    }

    /// Open the hyperlink (OSC 8) at `point`, if there is one.
    fn open_link(&self, point: Point) -> bool {
        let uri = self
            .with_session(|session| {
                let term = session.term();
                term.grid()[point]
                    .hyperlink()
                    .map(|link| link.uri().to_owned())
            })
            .flatten();
        match (uri, &*self.platform.borrow()) {
            (Some(uri), Some(platform)) => platform.0.open_url(&uri),
            _ => false,
        }
    }

    fn scroll(&self, x: f32, y: f32, delta_y: f32, modifiers: Modifiers) {
        let metrics = self.metrics.get();
        if metrics.cell_height <= 0. {
            return;
        }
        let travel = self.scroll_rest.get() - delta_y / metrics.cell_height;
        let lines = travel.trunc() as i32;
        self.scroll_rest.set(travel.fract());
        if lines == 0 {
            return;
        }
        let mode = self.mode();
        if mode.intersects(TermMode::MOUSE_MODE) && !modifiers.shift {
            if let Some((point, _, row)) = self.cell_at(x, y) {
                let mut bytes = Vec::new();
                for _ in 0..lines.unsigned_abs().min(20) {
                    if let Some(report) = keys::mouse(
                        keys::Mouse::Wheel(lines > 0),
                        point.column.0,
                        row,
                        modifiers,
                        mode,
                    ) {
                        bytes.extend(report);
                    }
                }
                self.with_session(|session| session.write(bytes));
            }
            return;
        }
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) && !modifiers.shift {
            // Full-screen programs scroll by the arrows.
            let arrow = if lines > 0 {
                Key::ArrowUp
            } else {
                Key::ArrowDown
            };
            if let Some(once) = keys::encode(&arrow, Modifiers::default(), mode) {
                let bytes = once.repeat(lines.unsigned_abs() as usize);
                self.with_session(|session| session.write(bytes));
            }
            return;
        }
        self.with_session(|session| session.term().scroll_display(Scroll::Delta(lines)));
        self.paint();
    }

    fn focus(&self, focus: bool) {
        let input = self.input.borrow().clone();
        match input {
            // At the next safe point: from inside an event's dispatch, what
            // the dispatch does after (a press's focus) would undo it.
            Some(input) if focus => zgx::native_view::post(move || {
                input.focus();
            }),
            Some(_) => {
                if self.focused.get()
                    && let Some(root) = self.root.borrow().as_ref()
                {
                    // Focus goes to the terminal's node, which takes no keys.
                    root.focus();
                }
            }
            None => self.pending_focus.set(focus),
        }
    }

    /// A key, at the hidden editor: sent to the program, or left to the app
    /// (`⌘` chords on macOS, shortcuts it binds).
    fn key(&self, key: &Key, modifiers: Modifiers, repeat: bool, cx: &mut EventContext) {
        // An input method composing takes the keys.
        if self.preedit.with_untracked(|text| !text.is_empty()) {
            return;
        }
        let character = match key {
            Key::Character(c) => Some(c.to_lowercase()),
            _ => None,
        };
        let copy_paste_chord = if cfg!(target_os = "macos") {
            modifiers.meta && !modifiers.control && !modifiers.alt
        } else {
            modifiers.control && modifiers.shift && !modifiers.alt && !modifiers.meta
        };
        if copy_paste_chord {
            match character.as_deref() {
                Some("c") => {
                    if self.copy() {
                        cx.prevent_default();
                        cx.stop_propagation();
                    }
                    return;
                }
                Some("v") => {
                    self.paste_clipboard();
                    cx.prevent_default();
                    cx.stop_propagation();
                    return;
                }
                _ => {}
            }
        }
        let mode = self.mode();
        let event = if repeat {
            keys::KeyEvent::Repeat
        } else {
            keys::KeyEvent::Press
        };
        // The kitty keyboard protocol, once the program asks for it.
        let encoded = match keys::kitty(key, modifiers, event, cx.key_code(), mode) {
            Some(kitty) => kitty,
            None => keys::encode(key, modifiers, mode),
        };
        if let Some(bytes) = encoded {
            self.write(bytes);
            cx.prevent_default();
            cx.stop_propagation();
        }
    }

    /// A key's release, which a program reads under the kitty keyboard
    /// protocol when it asks for event types.
    fn key_up(&self, key: &Key, modifiers: Modifiers, cx: &mut EventContext) {
        if self.preedit.with_untracked(|text| !text.is_empty()) {
            return;
        }
        let mode = self.mode();
        if !mode.contains(TermMode::REPORT_EVENT_TYPES) {
            return;
        }
        if let Some(Some(bytes)) =
            keys::kitty(key, modifiers, keys::KeyEvent::Release, cx.key_code(), mode)
        {
            self.with_session(|session| session.write(bytes));
            cx.prevent_default();
        }
    }

    /// Run a Ghostty binding action's name (those apps use).
    fn binding(&self, action: &str) {
        let (name, argument) = action.split_once(':').unwrap_or((action, ""));
        let page = self
            .with_session(|session| session.term().screen_lines() as i32)
            .unwrap_or(1);
        let scroll = |by: Scroll| {
            self.with_session(|session| session.term().scroll_display(by));
            self.paint();
        };
        match name {
            "copy_to_clipboard" => {
                self.copy();
            }
            "paste_from_clipboard" | "paste_from_selection" => self.paste_clipboard(),
            "select_all" => {
                self.with_session(|session| {
                    let mut term = session.term();
                    let top = term.topmost_line();
                    let bottom = term.bottommost_line();
                    let last = term.last_column();
                    let mut selection = Selection::new(
                        SelectionType::Simple,
                        Point::new(top, Column(0)),
                        Side::Left,
                    );
                    selection.update(Point::new(bottom, last), Side::Right);
                    term.selection = Some(selection);
                });
                self.paint();
            }
            "clear_screen" => {
                self.with_session(|session| {
                    let mut term = session.term();
                    term.grid_mut().clear_history();
                    term.selection = None;
                    drop(term);
                    // The program draws its screen (a shell's prompt) again.
                    session.write(b"\x0c".as_slice());
                });
                self.full.set(true);
                self.paint();
            }
            "scroll_to_top" => scroll(Scroll::Top),
            "scroll_to_bottom" => scroll(Scroll::Bottom),
            "scroll_page_up" => scroll(Scroll::PageUp),
            "scroll_page_down" => scroll(Scroll::PageDown),
            "scroll_page_fractional" => {
                let fraction: f32 = argument.parse().unwrap_or(0.);
                scroll(Scroll::Delta(-(fraction * page as f32).round() as i32));
            }
            "scroll_page_lines" => scroll(Scroll::Delta(-argument.parse::<i32>().unwrap_or(0))),
            "increase_font_size" => {
                let by: f32 = argument.parse().unwrap_or(1.);
                self.set_font_size(self.metrics.get().font_size + by);
            }
            "decrease_font_size" => {
                let by: f32 = argument.parse().unwrap_or(1.);
                self.set_font_size(self.metrics.get().font_size - by);
            }
            "reset_font_size" => {
                let size = self
                    .controller()
                    .and_then(|term| term.options().font_size)
                    .unwrap_or(FONT_SIZE);
                self.set_font_size(size);
            }
            "text" => self.write(unescape(argument)),
            "csi" => self.write([b"\x1b[".as_slice(), argument.as_bytes()].concat()),
            "esc" => self.write([b"\x1b".as_slice(), argument.as_bytes()].concat()),
            _ => {}
        }
    }

    /// Ask again what is in front once the output settles: `PROCESS_CHECK`
    /// after the latest output, once. Timed from the first output of a
    /// burst, a check could run just after a command's echo, before the
    /// shell had started it, and a quiet command (`sleep 30`) gave no more
    /// output to check again on.
    fn schedule_process_check(self: &Rc<Self>) {
        let due = std::time::Instant::now() + PROCESS_CHECK;
        if self.process_check.replace(Some(due)).is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        let task = self.runner.borrow().spawn(Box::pin(async move {
            let mut due = due;
            loop {
                zgx::timer::sleep_until(due).await;
                let Some(engine) = weak.upgrade() else { return };
                match engine.process_check.get() {
                    // Output since: later.
                    Some(later) if later > due => due = later,
                    _ => {
                        engine.process_check.set(None);
                        engine.check_process();
                        return;
                    }
                }
            }
        }));
        *self.process_task.borrow_mut() = Some(task);
    }

    /// What runs in front: the controller's process name, pid and status,
    /// as the app's resolver sees it when it does.
    fn check_process(self: &Rc<Self>) {
        let Some(pid) = self.with_session(Session::foreground_pid) else {
            return;
        };
        if self.exited.get() {
            return;
        }
        // Seen through (an attach client to a session server): what is
        // behind it changes while the client stays in front, so ask each
        // time.
        let resolved = if pid == 0 {
            None
        } else {
            crate::front::resolve(pid)
        };
        self.resolved.set(resolved.is_some());
        if resolved.is_some() && !crate::front::pushed() {
            self.poll();
        }
        let (name, pid, status) = match resolved {
            Some(front) => {
                if self.front.borrow().as_ref() == Some(&front) {
                    return;
                }
                *self.front.borrow_mut() = Some(front.clone());
                self.pid.set(pid);
                (front.name, front.pid, front.status)
            }
            None => {
                let was_resolved = self.front.borrow_mut().take().is_some();
                self.pid.set(pid);
                // Named afresh each time: a process keeps its pid as it
                // execs another program (the shell this session forked, read
                // before it ran; `exec less`), and the pid alone left the old
                // name in front for as long as nothing else took it.
                let name = if pid == 0 {
                    String::new()
                } else {
                    crate::process_name(pid)
                };
                if !was_resolved
                    && let Some(term) = self.controller()
                    && term
                        .state()
                        .with_untracked(|state| state.pid == pid && state.process == name)
                {
                    return;
                }
                (name, pid, None)
            }
        };
        if let Some(term) = self.controller() {
            let changed = term
                .state()
                .with_untracked(|state| state.process != name || state.pid != pid);
            term.update_state(|state| {
                state.process = name.clone();
                state.pid = pid;
                state.status = status;
            });
            if changed {
                term.emit(TerminalEvent::ProcessChanged { process: name, pid });
            }
        }
    }

    /// The checks waiting on the last window's tasks, on this one's: they
    /// would end with that window, their pending flags left set, and no
    /// check was ever scheduled again (the process in front never named
    /// after a tab moved to another window and the first closed).
    fn move_timers(self: &Rc<Self>) {
        if self.process_check.take().is_some() {
            self.process_task.borrow_mut().take();
            self.schedule_process_check();
        }
        if self.polling.replace(false) {
            self.poll_task.borrow_mut().take();
            self.poll();
        }
    }

    /// Ask the resolver twice a second while it resolves this terminal and
    /// the app doesn't say when what it sees changes.
    fn poll(self: &Rc<Self>) {
        if self.polling.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        let task = self.runner.borrow().spawn(Box::pin(async move {
            loop {
                zgx::timer::sleep(Duration::from_millis(500)).await;
                let Some(engine) = weak.upgrade() else { return };
                if !engine.resolved.get() || crate::front::pushed() || engine.exited.get() {
                    engine.polling.set(false);
                    return;
                }
                engine.check_process();
            }
        }));
        *self.poll_task.borrow_mut() = Some(task);
    }
}

/// A binding's `text:` argument, with Zig-style escapes (`\n`, `\x1b`).
fn unescape(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            let (byte, used) = match bytes[i + 1] {
                b'n' => (b'\n', 2),
                b'r' => (b'\r', 2),
                b't' => (b'\t', 2),
                b'\\' => (b'\\', 2),
                b'x' if i + 4 <= bytes.len() => {
                    match std::str::from_utf8(&bytes[i + 2..i + 4])
                        .ok()
                        .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                    {
                        Some(byte) => (byte, 4),
                        None => (b'\\', 1),
                    }
                }
                _ => (b'\\', 1),
            };
            out.push(byte);
            i += used;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

impl TerminalBackend for EngineTerminal {
    fn run(&self, command: TerminalCommand) {
        let engine = &self.0;
        match command {
            TerminalCommand::Focus(focus) => engine.focus(focus),
            TerminalCommand::Input(text) => engine.write(text.into_bytes()),
            TerminalCommand::Paste(text) => engine.paste(&text),
            TerminalCommand::Binding(action) => engine.binding(&action),
            TerminalCommand::Close => {
                engine.task.borrow_mut().take();
                engine.process_task.borrow_mut().take();
                engine.poll_task.borrow_mut().take();
                // Dropping the session hangs up its program.
                engine.session.borrow_mut().take();
            }
        }
    }
    fn set_colors(&self, colors: &TerminalColors) {
        self.0.set_colors(colors);
    }
    fn needs_confirm_close(&self) -> bool {
        self.0
            .with_session(|session| {
                if session.exited().is_some() {
                    return false;
                }
                let pid = session.foreground_pid();
                match (pid != 0).then(|| crate::front::resolve(pid)).flatten() {
                    Some(front) => !front.idle,
                    None => !session.is_idle(),
                }
            })
            .unwrap_or(false)
    }
    fn snapshot(&self, max_width: u32) -> Option<std::sync::Arc<zgx::image::ImageData>> {
        self.0.root.borrow().as_ref()?.snapshot(max_width)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl Drop for EngineTerminal {
    fn drop(&mut self) {
        self.0.task.borrow_mut().take();
        self.0.process_task.borrow_mut().take();
        self.0.poll_task.borrow_mut().take();
        self.0.session.borrow_mut().take();
    }
}

/// `term` drawn by zgx (its session started on first showing): lay it out
/// like any node (give it a size or let it grow, round and clip it). Its
/// default background is transparent: the node's own shows through.
pub fn terminal_view(term: &Terminal) -> View {
    let term = term.clone();
    component(move |cx| {
        let platform = cx.try_service::<PlatformService>();
        let engine = match term.backend() {
            Some(backend) => {
                let engine = backend
                    .as_any()
                    .downcast_ref::<EngineTerminal>()
                    .map(|engine| engine.0.clone());
                if let (Some(engine), Some(runner)) = (&engine, cx.try_service::<TaskRunner>()) {
                    if let Some(platform) = &platform {
                        *engine.platform.borrow_mut() = Some(platform.clone());
                    }
                    *engine.runner.borrow_mut() = (*runner).clone();
                    engine.listen();
                    engine.move_timers();
                }
                engine
            }
            None if term.is_closed() => None,
            None => {
                let started = cx.try_service::<TaskRunner>().map(|runner| {
                    EngineTerminal::new(&term, cx.runtime(), (*runner).clone(), platform.clone())
                });
                match started {
                    Some(Ok(engine)) => {
                        let shared = engine.0.clone();
                        term.attach(Rc::new(engine));
                        Some(shared)
                    }
                    Some(Err(error)) => {
                        return message(format!("Terminal · couldn't start: {error}"));
                    }
                    None => None,
                }
            }
        };
        let Some(engine) = engine else {
            return message("Terminal".into());
        };
        if let Some(frames) = cx.try_service::<zgx::frame::FrameClock>() {
            *engine.frames.borrow_mut() = Some((*frames).clone());
        }
        follow_theme_font(cx, &engine);
        zgx::terminal::follow_theme_colors(cx, &term);
        engine.views.set(engine.views.get() + 1);
        engine.full.set(true);
        let id = engine.latest_view.get() + 1;
        engine.latest_view.set(id);
        cx.retain(Shown(Rc::downgrade(&engine), id));
        view(engine)
    })
}

/// The grid in the theme's monospaced font (`font.mono`) where the view
/// mounts, following it as it changes (a theme switched, a scope's override:
/// an app's own code font); the platform's monospaced font where the theme
/// names none.
fn follow_theme_font(cx: &mut Context, engine: &Rc<Engine>) {
    let theme = cx.theme();
    let family = move || {
        theme
            .get(zgx::theme::tok::FONT_MONO)
            .font()
            .unwrap_or(zgx::text_layout::FontFamily::Monospace)
    };
    let follow = Rc::downgrade(engine);
    let effect = engine.runtime.effect(move || {
        let family = family();
        if let Some(engine) = follow.upgrade() {
            engine.set_font_family(family);
        }
    });
    cx.retain(effect);
}

/// Keeps a view's count while it is mounted.
struct Shown(Weak<Engine>, u64);
impl Drop for Shown {
    fn drop(&mut self) {
        if let Some(engine) = self.0.upgrade() {
            engine.views.set(engine.views.get().saturating_sub(1));
            if engine.latest_view.get() == self.1 {
                engine.input.borrow_mut().take();
                engine.root.borrow_mut().take();
            }
        }
    }
}

fn message(text: String) -> View {
    div().items_center().justify_center().child(
        text_signal(move || text.clone())
            .text_size(12.)
            .text_color(rgba(0xa0a0_a0ff)),
    )
}

fn view(engine: Rc<Engine>) -> View {
    let rows = {
        let engine = engine.clone();
        keyed(
            {
                // Slots in screen order, so readers meet rows top to bottom;
                // a scroll moves slots rather than making them again.
                let layout = engine.layout.clone();
                let order = engine.order.clone();
                move || {
                    let (_, generation) = layout.get();
                    order.with(|order| {
                        order
                            .iter()
                            .map(|slot| (*slot, generation))
                            .collect::<Vec<_>>()
                    })
                }
            },
            move |(slot, _), _cx| row_view(&engine, slot),
        )
    };
    // The rows' container, moved up over their lines as the screen scrolls:
    // moving it changes no row.
    let rows = {
        let scrolled = engine.scrolled.clone();
        let metrics = engine.clone();
        div()
            .absolute()
            .left(0.)
            .top(0.)
            .w_full()
            .h_full()
            .reactive_style(move || {
                // The cell's height as the rows are laid out by.
                let _ = metrics.layout.get();
                let height = metrics.metrics.get().cell_height;
                Styles::new().translate(0., -(scrolled.get() as f32) * height)
            })
            .child(rows)
    };
    let input = {
        let value = engine.runtime.signal(String::new());
        let caret = engine.caret.clone();
        let height = engine.metrics.get().cell_height.max(1.);
        let keys = engine.clone();
        let mounted = engine.clone();
        text_input("Terminal input", value)
            .accessible_hidden()
            .absolute()
            .w(1.)
            .h(height)
            .opacity(0.)
            .reactive_style(move || {
                let (x, y) = caret.get();
                Styles::new().left(x).top(y)
            })
            .on_event(move |cx| {
                if cx.phase != EventPhase::Target {
                    return;
                }
                keys.keyboard(false, cx);
            })
            .on_mount(move |handle| {
                *mounted.input.borrow_mut() = Some(handle.clone());
                if mounted.pending_focus.take() {
                    handle.focus();
                }
            })
    };
    let preedit = {
        let text = engine.preedit.clone();
        let caret = engine.caret.clone();
        let metrics = engine.metrics.get();
        let look = engine.look.clone();
        let shown = engine.preedit.clone();
        let font = engine.clone();
        text_signal(move || text.get())
            .absolute()
            .text_size(metrics.font_size)
            .reactive_style(move || {
                // The composition in the terminal's own colours.
                let (ink, behind) = look.with(|look| {
                    let palette = &look.palette;
                    (
                        paint::color(palette.foreground),
                        paint::color(palette.background),
                    )
                });
                let (x, y) = caret.get();
                let hidden = shown.with(String::is_empty);
                // (In the grid's font: a new one makes a new generation.)
                let _ = font.layout.get();
                let family = font.font.borrow().family.clone();
                Styles::new()
                    .left(x)
                    .top(y)
                    .text_color(ink)
                    .bg(behind)
                    .font_family(family)
                    .opacity(if hidden { 0. } else { 1. })
            })
    };
    let pointer = engine.clone();
    let mounted = engine.clone();
    div()
        .focusable(true)
        .overflow_hidden()
        .cursor(Cursor::Text)
        // A document named Terminal holding its lines: assistive technology
        // drops an unnamed container's name with it (a group is one), so
        // the rows would read as the window's own.
        .role(Role::Document)
        .accessible_label("Terminal")
        .on_event(move |cx| {
            if cx.phase == EventPhase::Capture {
                return;
            }
            if cx.phase == EventPhase::Target
                && matches!(
                    cx.event,
                    InputEvent::KeyDown { .. }
                        | InputEvent::KeyUp { .. }
                        | InputEvent::Text(_)
                        | InputEvent::ImeCommit(_)
                        | InputEvent::ImePreedit { .. }
                        | InputEvent::Focus
                        | InputEvent::Blur
                )
            {
                pointer.keyboard(true, cx);
                return;
            }
            let modifiers = cx.pointer_modifiers();
            let clicks = cx.click_count();
            let event = cx.event.clone();
            let origin = pointer
                .root
                .borrow()
                .as_ref()
                .and_then(NodeHandle::observe_bounds)
                .map(|bounds| bounds.with_untracked(|b| (b.x, b.y)))
                .unwrap_or((0., 0.));
            match event {
                InputEvent::PointerDown { x, y, .. }
                | InputEvent::PointerMove { x, y }
                | InputEvent::PointerUp { x, y, .. } => {
                    pointer.pointer(&event, x - origin.0, y - origin.1, modifiers, clicks, cx);
                    if let (InputEvent::PointerMove { .. }, Some(term)) =
                        (&event, pointer.controller())
                    {
                        term.set_pointer(Some((x - origin.0, y - origin.1)));
                    }
                }
                InputEvent::PointerLeave => {
                    if let Some(term) = pointer.controller() {
                        term.set_pointer(None);
                    }
                }
                InputEvent::Scroll { x, y, delta_y, .. } => {
                    pointer.scroll(x - origin.0, y - origin.1, delta_y, modifiers);
                    cx.prevent_default();
                    cx.stop_propagation();
                }
                _ => {}
            }
        })
        .child(rows)
        .child(news_view(&engine))
        .child(preedit)
        .child(input)
        .on_mount(move |handle| {
            let engine = mounted;
            if let Some(scene) = handle.scene() {
                engine.measure(&scene.borrow());
            }
            *engine.root.borrow_mut() = Some(handle.clone());
            if let Some(size) = handle.observe_size() {
                let fit = engine.clone();
                let effect = engine.runtime.effect(move || {
                    let size = size.get();
                    fit.fit(size);
                });
                handle.retain(effect);
            }
        })
}

fn row_view(engine: &Rc<Engine>, slot: usize) -> View {
    let (paint, line) = engine.slot(slot);
    let metrics = engine.metrics.get();
    let font = engine.font.borrow().clone();
    let label = paint.clone();
    canvas(move |_| paint::draw(&paint.get(), metrics, &font))
        .absolute()
        .left(PAD_X)
        .top(PAD_Y)
        // At its line, moved without layout when the slot takes another.
        .reactive_style(move || {
            Styles::new().translate(0., line.get() as f32 * metrics.cell_height)
        })
        .w_full()
        .h(metrics.cell_height)
        .role(Role::Label)
        .accessible_label_when(move || label.with(|paint| paint.text.clone()))
}

/// How many lines the screen scrolled from `shown` to `rows` (every screen
/// row, in order): the least whose rows below match the first rows now,
/// three of them where there are. 0 where none does, or the first row is
/// unchanged. Only how much is drawn again depends on it: each row is
/// compared with what its slot drew either way.
fn shift(shown: &[Rc<RowPaint>], rows: &[(usize, RowPaint)]) -> usize {
    let count = shown.len();
    if count < 2
        || rows.len() != count
        || rows.iter().enumerate().any(|(i, (row, _))| *row != i)
        || *shown[0] == rows[0].1
    {
        return 0;
    }
    (1..count)
        .find(|&lines| (0..(count - lines).min(3)).all(|i| *shown[lines + i] == rows[i].1))
        .unwrap_or(0)
}

/// The latest output, for screen readers alone: a live region they announce
/// as it changes (the rows themselves are read as the reader moves over
/// them, not each time they change).
fn news_view(engine: &Rc<Engine>) -> View {
    let news = engine.news.clone();
    div()
        .absolute()
        .left(0.)
        .top(0.)
        .w(1.)
        .h(1.)
        .role(Role::Status)
        .live(Live::Polite)
        .accessible_label_when(move || news.get())
}

/// Whether `term`'s backend is drawn by zgx (this crate's).
pub fn is_engine_drawn(term: &Terminal) -> bool {
    term.backend()
        .is_some_and(|backend| backend.as_any().is::<EngineTerminal>())
}

/// How many times an engine-drawn terminal read its session's output and
/// state (for tests: at most once a refresh while output streams).
#[doc(hidden)]
pub fn reads(term: &Terminal) -> u64 {
    term.backend()
        .and_then(|backend| {
            backend
                .as_any()
                .downcast_ref::<EngineTerminal>()
                .map(|engine| engine.0.reads.get())
        })
        .unwrap_or(0)
}
