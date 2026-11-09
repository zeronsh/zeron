//! Scripted sessions on real pseudoterminals: what programs write lands in
//! exact cells, input reaches them, and their reports come back as events.
use std::{
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};
use zgx_terminal::{
    GridSize, Session, SessionEvent, SessionOptions, Term,
    alacritty_terminal::{
        grid::Dimensions,
        index::{Column, Line},
        term::cell::Flags,
    },
};
#[cfg(unix)]
use zgx_terminal::{
    Progress,
    alacritty_terminal::{
        index::Point,
        vte::ansi::{Color, NamedColor},
    },
};

/// A session and everything it reported so far.
struct Scripted {
    session: Session,
    woken: Arc<(Mutex<bool>, Condvar)>,
    events: Vec<SessionEvent>,
}
impl Scripted {
    fn run(command: &str, columns: u16, rows: u16) -> Self {
        let woken = Arc::new((Mutex::new(false), Condvar::new()));
        let wake = woken.clone();
        let session = Session::spawn(
            &SessionOptions {
                command: Some(command.into()),
                ..Default::default()
            },
            GridSize::new(columns, rows),
            move || {
                *wake.0.lock().unwrap() = true;
                wake.1.notify_all();
            },
        )
        .expect("a pty");
        Self {
            session,
            woken,
            events: Vec::new(),
        }
    }
    /// Read what the session reports until `done` holds, as the app does
    /// when woken.
    fn until(&mut self, what: &str, mut done: impl FnMut(&[SessionEvent], &Term) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.events.extend(self.session.take_events());
            if done(&self.events, &self.session.term()) {
                return;
            }
            let now = Instant::now();
            assert!(
                now < deadline,
                "timed out waiting for {what}: {:?}\n{}",
                self.events,
                self.screen()
            );
            let (lock, wake) = &*self.woken;
            let mut woken = lock.lock().unwrap();
            if !*woken {
                woken = wake.wait_timeout(woken, deadline - now).unwrap().0;
            }
            *woken = false;
        }
    }
    fn exited(&mut self) -> i32 {
        self.until("the exit", |events, _| exit_code(events).is_some());
        exit_code(&self.events).unwrap()
    }
    fn screen(&self) -> String {
        rows(&self.session.term()).join("\n")
    }
}
fn exit_code(events: &[SessionEvent]) -> Option<i32> {
    events.iter().find_map(|event| match event {
        SessionEvent::Exited(code) => Some(*code),
        _ => None,
    })
}
/// The visible rows' text, trailing blanks trimmed (a wide character's
/// spacer cell left out).
fn rows(term: &Term) -> Vec<String> {
    let grid = term.grid();
    (0..grid.screen_lines() as i32)
        .map(|line| {
            let row = &grid[Line(line)];
            let text: String = (0..grid.columns())
                .map(|column| &row[Column(column)])
                .filter(|cell| !cell.flags.contains(Flags::WIDE_CHAR_SPACER))
                .map(|cell| cell.c)
                .collect();
            text.trim_end().to_owned()
        })
        .collect()
}
fn shows(term: &Term, line: &str) -> bool {
    rows(term).iter().any(|row| row == line)
}

#[cfg(unix)]
#[test]
fn output_lands_in_exact_cells() {
    let mut run = Scripted::run(
        r"printf 'hello\r\n\033[1;31mred\033[0m \033[38;2;1;2;3mtrue\033[0m wide:\344\270\255\346\226\207!\r\n'",
        20,
        4,
    );
    assert_eq!(run.exited(), 0);
    let term = run.session.term();
    assert_eq!(rows(&term), ["hello", "red true wide:中文!", "", ""]);
    let grid = term.grid();
    let red = &grid[Point::new(Line(1), Column(0))];
    assert_eq!(red.c, 'r');
    assert_eq!(red.fg, Color::Named(NamedColor::Red));
    assert!(red.flags.contains(Flags::BOLD));
    let truecolor = &grid[Point::new(Line(1), Column(4))];
    assert!(matches!(truecolor.fg, Color::Spec(rgb) if (rgb.r, rgb.g, rgb.b) == (1, 2, 3)));
    // "red true wide:" is 14 columns: the wide characters take two each.
    let wide = &grid[Point::new(Line(1), Column(14))];
    assert_eq!(wide.c, '中');
    assert!(wide.flags.contains(Flags::WIDE_CHAR));
    assert!(
        grid[Point::new(Line(1), Column(15))]
            .flags
            .contains(Flags::WIDE_CHAR_SPACER)
    );
    assert_eq!(grid[Point::new(Line(1), Column(16))].c, '文');
    assert_eq!(grid[Point::new(Line(1), Column(18))].c, '!');
}

#[cfg(unix)]
#[test]
fn input_reaches_the_program_and_it_echoes() {
    let mut run = Scripted::run("cat", 20, 4);
    run.session.write(b"abc\r".as_slice());
    run.until("the echo and cat's copy", |_, term| {
        rows(term)[..2] == ["abc", "abc"]
    });
    // ^D at a line's start: end of input.
    run.session.write(b"\x04".as_slice());
    assert_eq!(run.exited(), 0);
}

#[cfg(unix)]
#[test]
fn ctrl_c_interrupts_the_foreground_program() {
    let mut run = Scripted::run("sh -c 'echo ready; sleep 30'", 20, 4);
    run.until("ready", |_, term| shows(term, "ready"));
    // A ^C before `sleep` exists reaches only the shells, which (dash)
    // catch it and act once their command ends: 30 s on. Wait for it to be
    // in the session's process group.
    let group = run.session.pid().to_string();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !std::process::Command::new("pgrep")
        .args(["-g", &group, "-x", "sleep"])
        .stdout(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
    {
        assert!(Instant::now() < deadline, "sleep never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    run.session.write(b"\x03".as_slice());
    // SIGINT: 128 + 2.
    assert_eq!(run.exited(), 130);
}

#[cfg(unix)]
#[test]
fn resizing_tells_the_program() {
    let mut run = Scripted::run("sh -c 'stty size; read line; stty size'", 40, 5);
    run.until("the first size", |_, term| shows(term, "5 40"));
    run.session.resize(GridSize::new(50, 7));
    run.session.write(b"\r".as_slice());
    run.until("the new size", |_, term| shows(term, "7 50"));
    assert_eq!(run.exited(), 0);
    let term = run.session.term();
    assert_eq!((term.grid().columns(), term.grid().screen_lines()), (50, 7));
}

#[cfg(unix)]
#[test]
fn exit_codes_and_reports_come_back_as_events() {
    let mut run = Scripted::run(
        r"printf '\033]2;Building\007\033]7;file://host/tmp/a%%20b\007\007\033]133;C\007\033]9;4;1;42\007\033]133;D;3\007\033]777;notify;CI;green\007'; exit 3",
        20,
        4,
    );
    assert_eq!(run.exited(), 3);
    let events: Vec<String> = run
        .events
        .iter()
        .map(|event| match event {
            SessionEvent::CommandFinished { exit_code, .. } => format!("finished {exit_code:?}"),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        events,
        [
            r#"Title("Building")"#,
            r#"Pwd("/tmp/a b")"#,
            "Bell",
            &format!("{:?}", SessionEvent::Progress(Progress::Set(42))),
            "finished Some(3)",
            r#"Notification { title: "CI", body: "green" }"#,
            "Exited(3)",
        ]
    );
}

#[cfg(unix)]
#[test]
fn queries_are_answered_and_the_foreground_is_known() {
    // Device attributes (DA1): the screen model answers through the pty,
    // a VT220 with sixel graphics.
    let mut run = Scripted::run(
        r#"exec sh -c 'stty raw -echo; printf "\033[c"; dd bs=1 count=11 2>/dev/null | od -An -c | tr -d " \n"; echo; exec sleep 30'"#,
        30,
        4,
    );
    run.until("the answer", |_, term| shows(term, r"033[?62;4;22c"));
    let deadline = Instant::now() + Duration::from_secs(10);
    while zgx_terminal::process_name(run.session.foreground_pid()) != "sleep" {
        assert!(Instant::now() < deadline, "sleep never came to the front");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A picture a program sends in its output (the kitty graphics protocol)
/// is placed on the cells at the cursor and kept for the view, the cursor
/// moves past it, and the program is answered through the pty.
#[cfg(unix)]
#[test]
fn a_program_places_a_picture_and_is_answered() {
    // 2 by 2 black pixels, RGB (twelve zero bytes in base64).
    let mut run = Scripted::run(
        r#"exec sh -c 'stty raw -echo; printf "ab\033_Ga=T,f=24,s=25,v=30,i=5,q=2;AAAAAAAAAAAAAAAA\033\\"; printf "\033_Ga=T,f=24,s=2,v=2,i=5;AAAAAAAAAAAAAAAA\033\\"; dd bs=1 count=11 2>/dev/null | tr "\033\\\\" "E/"; exec sleep 30'"#,
        30,
        4,
    );
    run.until("the answer", |_, term| {
        rows(term).iter().any(|row| row.contains("E_Gi=5;OKE/"))
    });
    let term = run.session.term();
    // The first, with too few pixels for 25 by 30, was refused (quietly);
    // the second placed at the cursor, one cell.
    let placed: Vec<_> = term
        .placements()
        .iter()
        .map(|p| {
            (
                p.image,
                p.point.line.0,
                p.point.column.0,
                p.lines,
                p.columns,
            )
        })
        .collect();
    assert_eq!(placed, [(5, 0, 2, 1, 1)]);
    let image = run.session.image(5).expect("kept");
    assert_eq!((image.width(), image.height()), (2, 2));
    assert_eq!(image.pixels(), [0, 0, 0, 255].repeat(4));
}

/// A sixel image a program prints is placed at the cursor, which then
/// stays in its column (on the line of the image's last band).
#[cfg(unix)]
#[test]
fn a_program_draws_a_sixel_image() {
    // 4 red columns of six pixels.
    let mut run = Scripted::run(r"printf 'ab\033Pq#1;2;100;0;0!4~\033\\cd'; sleep 30", 30, 4);
    run.until("the text after it", |_, term| shows(term, "abcd"));
    let term = run.session.term();
    let placed: Vec<_> = term
        .placements()
        .iter()
        .map(|p| (p.point.line.0, p.point.column.0, p.lines, p.columns))
        .collect();
    assert_eq!(placed, [(0, 2, 1, 1)]);
    let image = run.session.image(term.placements()[0].image).expect("kept");
    assert_eq!((image.width(), image.height()), (4, 6));
    assert_eq!(image.pixels()[..4], [255, 0, 0, 255]);
}

#[cfg(windows)]
#[test]
fn a_windows_program_writes_exact_cells_and_exits() {
    let mut run = Scripted::run("cmd.exe /d /c echo hello& exit 3", 30, 4);
    assert_eq!(run.exited(), 3);
    assert!(shows(&run.session.term(), "hello"), "{}", run.screen());
}

#[cfg(windows)]
#[test]
fn a_windows_program_gets_input_and_resizes() {
    let mut run = Scripted::run("cmd.exe /d /q /k prompt $G", 40, 5);
    run.session.write(b"echo typed\r".as_slice());
    run.until("the echo", |_, term| shows(term, "typed"));
    run.session.resize(GridSize::new(60, 8));
    run.session.write(b"mode con\r".as_slice());
    run.until("the new size", |_, term| {
        rows(term)
            .iter()
            .any(|row| row.trim_start().starts_with("Columns:") && row.ends_with("60"))
    });
    run.session.write(b"exit 7\r".as_slice());
    assert_eq!(run.exited(), 7);
}

#[cfg(windows)]
#[test]
fn ctrl_c_reaches_a_windows_program() {
    // As when an app starts from a parent ignoring Ctrl+C (an agent's
    // shell): inherited, the shell would never get it.
    // SAFETY: no handler, TRUE: this process ignores Ctrl+C.
    unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(None, 1) };
    // Rows enough for ping's statistics whole: on six, ConPTY's repaint
    // of them could leave their heading above the screen, and the check
    // below failed with ping's statistics and "Control-C" plainly shown.
    let mut run = Scripted::run("cmd.exe /d /c ping -t 127.0.0.1", 80, 24);
    // A reply: ping is in its loop, its Ctrl+C handler set.
    run.until("ping replying", |_, term| {
        rows(term).iter().any(|row| row.starts_with("Reply from"))
    });
    run.session.write(b"\x03".as_slice());
    // `ping -t` ends only on Ctrl+C, and cmd /c ends with it.
    run.exited();
    let shown = {
        let term = run.session.term();
        shows_prefix(&term, "Ping statistics") && shows_prefix(&term, "Control-C")
    };
    assert!(shown, "{}", run.screen());
}
#[cfg(windows)]
fn shows_prefix(term: &Term, prefix: &str) -> bool {
    rows(term)
        .iter()
        .any(|row| row.trim_start().starts_with(prefix))
}

/// Ptys opened at once from many threads all open: macOS's `openpty` isn't
/// safe to call concurrently (16 threads failed about once in 4,000 calls,
/// errno -6, and a terminal didn't start: "Unknown error: -6"), so the fork
/// opens one at a time.
#[cfg(unix)]
#[test]
fn ptys_opened_at_once_from_many_threads_all_open() {
    use zgx_terminal::alacritty_terminal::{event::WindowSize, tty};
    let size = WindowSize {
        num_lines: 24,
        num_cols: 80,
        cell_width: 8,
        cell_height: 16,
    };
    let threads: Vec<_> = (0..16)
        .map(|_| {
            std::thread::spawn(move || {
                (0..600)
                    .filter_map(|_| match tty::open_pty(size) {
                        // Used as a terminal is, then closed: a byte through it.
                        Ok((controller, child)) => {
                            use std::io::{Read, Write};
                            let (mut controller, mut child) =
                                (std::fs::File::from(controller), std::fs::File::from(child));
                            child.write_all(b"z").ok()?;
                            let mut byte = [0];
                            controller.read_exact(&mut byte).ok()?;
                            None
                        }
                        Err(error) => Some(error.to_string()),
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let failed: Vec<String> = threads
        .into_iter()
        .flat_map(|thread| thread.join().unwrap())
        .collect();
    assert!(
        failed.is_empty(),
        "{} of 9600 didn't open: {:?}",
        failed.len(),
        &failed[..failed.len().min(3)]
    );
}

/// A program that prints and exits at once: all it printed is on the
/// screen as the session reports its exit. ConPTY renders a program's last
/// output after the program has gone; a pty hands it on through the
/// kernel's buffer, Linux's on a work queue of its own.
#[test]
fn a_program_that_prints_and_exits_at_once_shows_all_it_printed() {
    #[cfg(unix)]
    let command = "i=1; while [ $i -le 400 ]; do echo \"line $i\"; i=$((i+1)); done; \
                   printf 'the last line'";
    #[cfg(windows)]
    let command = "cmd.exe /d /c (for /l %i in (1,1,400) do @echo line %i)& echo the last line";
    let started = std::time::Instant::now();
    let mut run = Scripted::run(command, 80, 24);
    run.exited();
    let shown = rows(&run.session.term())
        .iter()
        .any(|row| row.starts_with("the last line"));
    assert!(shown, "{}", run.screen());
    // Windows: the output pipe ends as the pseudoconsole closes (our ends of
    // its pipes closed once it had its own), so the exit comes as soon as
    // the last output is read, not after the drain's five seconds.
    #[cfg(windows)]
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "the exit waited for the drain's deadline: {:?}",
        started.elapsed()
    );
    #[cfg(not(windows))]
    let _ = started;
}

/// What an app links this crate with still infers its comparisons of
/// `usize` and `i32`: no crate in the graph compares them with a type of its
/// own (alacritty_terminal did, from the primitive's side; our fork doesn't).
#[test]
fn usize_compares_only_with_itself() {
    let rows: std::collections::BTreeMap<usize, ()> = (0..5).map(|i| (i, ())).collect();
    let range: std::ops::Range<usize> = 1..3;
    let outside: Vec<usize> = rows
        .keys()
        .filter(|i| !range.contains(i))
        .copied()
        .collect();
    assert_eq!(outside, [0, 3, 4]);
    let lines = [-1i32, 0, 2];
    let range: std::ops::Range<i32> = 0..2;
    assert_eq!(lines.iter().filter(|line| range.contains(line)).count(), 1);
}

/// The cells of `line` as (character, wide, spacer) up to the first blank
/// run of three.
#[cfg(unix)]
fn cells(term: &Term, line: i32) -> Vec<(char, bool, bool)> {
    let row = &term.grid()[Line(line)];
    (0..term.grid().columns())
        .map(|column| {
            let cell = &row[Column(column)];
            (
                cell.c,
                cell.flags.contains(Flags::WIDE_CHAR),
                cell.flags.contains(Flags::WIDE_CHAR_SPACER),
            )
        })
        .collect()
}

#[cfg(unix)]
#[test]
fn an_emoji_asked_for_with_vs16_takes_two_cells() {
    // ❤ (text by default) with VS16, then without; ✔ with VS16 at the end
    // of a line of 8; an emoji wide already (😀) with VS16; and VS16 after
    // a letter, which has no emoji to show; and ❤ with VS16 inserted (IRM)
    // before `XY`.
    let mut run = Scripted::run(
        "printf 'a\\342\\235\\244\\357\\270\\217b\\342\\235\\244c\\r\\n1234567\\342\\234\\224\\357\\270\\217z\\r\\n\\360\\237\\230\\200\\357\\270\\217d e\\357\\270\\217f\\r\\n\\342\\235\\244\\r\\357\\270\\217g\\r\\nXY\\r\\033[4h\\342\\235\\244\\357\\270\\217\\033[4l\\r\\n'",
        8,
        7,
    );
    assert_eq!(run.exited(), 0);
    let term = run.session.term();
    let grid = term.grid();
    let line = cells(&term, 0);
    assert_eq!(line[0], ('a', false, false));
    assert_eq!(line[1], ('❤', true, false), "{line:?}");
    assert_eq!(line[2], (' ', false, true));
    assert_eq!(
        line[3],
        ('b', false, false),
        "the cursor went past both cells"
    );
    assert_eq!(line[4], ('❤', false, false), "without VS16, one cell");
    assert_eq!(line[5], ('c', false, false));
    let heart = &grid[Point::new(Line(0), Column(1))];
    assert_eq!(heart.zerowidth(), Some(&['\u{fe0f}'][..]));
    // No room at the end of the line: it starts the next, a spacer where
    // it was.
    let line = cells(&term, 1);
    assert_eq!(line[7].0, ' ');
    assert!(
        grid[Point::new(Line(1), Column(7))]
            .flags
            .contains(Flags::LEADING_WIDE_CHAR_SPACER),
        "{line:?}"
    );
    let line = cells(&term, 2);
    assert_eq!(
        &line[..3],
        [('✔', true, false), (' ', false, true), ('z', false, false)]
    );
    // Wide already: as it was. After a letter: nothing changes.
    let line = cells(&term, 3);
    assert_eq!(
        &line[..3],
        [('😀', true, false), (' ', false, true), ('d', false, false)]
    );
    assert_eq!(&line[4..6], [('e', false, false), ('f', false, false)]);
    // Inserted: what the insert shifted along is shifted on past the
    // spacer, not written over.
    let line = cells(&term, 5);
    assert_eq!(
        &line[..4],
        [
            ('❤', true, false),
            (' ', false, true),
            ('X', false, false),
            ('Y', false, false)
        ],
        "{line:?}"
    );
    // VS16 after the cursor went back to the start: not the character just
    // written, so nothing widens (`g` then writes over the ❤).
    let line = cells(&term, 4);
    assert_eq!(
        &line[..2],
        [('g', false, false), (' ', false, false)],
        "{line:?}"
    );
}

/// The top-left `count` characters of `line`: each cell's character with
/// those joined to it, and whether it's wide (spacers left out).
#[cfg(unix)]
fn clusters(term: &Term, line: i32, count: usize) -> Vec<(String, bool)> {
    let row = &term.grid()[Line(line)];
    (0..term.grid().columns())
        .map(|column| &row[Column(column)])
        .filter(|cell| !cell.flags.contains(Flags::WIDE_CHAR_SPACER))
        .take(count)
        .map(|cell| {
            let mut text = String::from(cell.c);
            text.extend(cell.zerowidth().into_iter().flatten());
            (text, cell.flags.contains(Flags::WIDE_CHAR))
        })
        .collect()
}

#[cfg(unix)]
#[test]
fn grapheme_clusters_take_one_cell_or_two_until_mode_2027_is_reset() {
    // A family (ZWJ), a flag and ⌚ with VS15, then the same with mode 2027
    // reset (`CSI ? 2027 l`).
    let line = r"\360\237\221\250\342\200\215\360\237\221\251\342\200\215\360\237\221\247\360\237\207\272\360\237\207\270\342\214\232\357\270\216x";
    let mut run = Scripted::run(
        &format!("printf '{line}\\r\\n\\033[?2027l{line}\\r\\n'"),
        20,
        4,
    );
    assert_eq!(run.exited(), 0);
    let term = run.session.term();
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
    assert_eq!(
        clusters(&term, 0, 4),
        [
            (family.to_owned(), true),
            ("\u{1f1fa}\u{1f1f8}".to_owned(), true),
            ("\u{231a}\u{fe0e}".to_owned(), false),
            ("x".to_owned(), false),
        ]
    );
    // Reset: each character takes its own width (zero-width ones joined to
    // the one before), as `wcwidth` counts them.
    assert_eq!(
        clusters(&term, 1, 7),
        [
            ("\u{1f468}\u{200d}".to_owned(), true),
            ("\u{1f469}\u{200d}".to_owned(), true),
            ("\u{1f467}".to_owned(), true),
            ("\u{1f1fa}".to_owned(), false),
            ("\u{1f1f8}".to_owned(), false),
            ("\u{231a}\u{fe0e}".to_owned(), true),
            ("x".to_owned(), false),
        ]
    );
}

/// readline counts with `wcwidth`: with mode 2027 reset its cursor stays on
/// the character it edits across an emoji with VS16 (❤️, one column to it).
#[cfg(target_os = "linux")]
#[test]
fn a_readline_edit_across_an_emoji_keeps_its_cursor_with_mode_2027_reset() {
    if std::process::Command::new("bash")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    const LEFT: &str = "\x1b[D";
    for (lefts, shown) in [(4, "[aX\u{2764}bc]"), (5, "[Xa\u{2764}bc]")] {
        let mut run = Scripted::run(
            "printf '\\033[?2027l'; exec env LANG=C.UTF-8 LC_ALL=C.UTF-8 INPUTRC=/dev/null PS1='$ ' bash --norc --noprofile -i",
            30,
            6,
        );
        run.until("the prompt", |_, term| {
            rows(term).iter().any(|row| row == "$")
        });
        run.session
            .write("echo [a\u{2764}\u{fe0f}bc]".as_bytes().to_vec());
        run.until("the line", |_, term| {
            rows(term).iter().any(|row| row.ends_with("bc]"))
        });
        run.session
            .write(format!("{}X", LEFT.repeat(lefts)).into_bytes());
        let edited = format!("$ echo {shown}");
        run.until("the edit", |_, term| rows(term)[0] == edited);
        run.session.write(b"\r".as_slice());
        run.until("bash's echo", |_, term| {
            rows(term).iter().any(|row| row == shown)
        });
        run.session.write(b"exit\r".as_slice());
    }
}
