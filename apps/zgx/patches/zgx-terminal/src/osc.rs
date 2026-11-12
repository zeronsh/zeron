//! The operating system commands the screen model leaves to its embedder,
//! read from the same bytes it parses: the working directory (OSC 7),
//! shell-integrated command ends (OSC 133), progress (OSC 9;4) and desktop
//! notifications (OSC 9, OSC 777); and pictures, which it passes over: the
//! kitty graphics protocol's commands (APC `G`) and sixel images (DCS `q`).

/// What one of those commands says.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Osc {
    /// OSC 7: the shell's working directory, a path.
    Pwd(String),
    /// OSC 133;C: a command started.
    CommandStarted,
    /// OSC 133;D: a command finished, with its exit code when given.
    CommandFinished(Option<i32>),
    /// OSC 9;4: progress, as the session reports it.
    Progress(crate::Progress),
    /// OSC 9 or OSC 777;notify: a desktop notification.
    Notification { title: String, body: String },
    /// APC `G`: a graphics command, what follows the `G`.
    Graphics(Vec<u8>),
    /// DCS `q`: a sixel image, its parameters, the `q` and its data.
    Sixel(Vec<u8>),
    /// Part of a picture's body, from this offset in the bytes read to the
    /// one given with it: kept here, and nothing the screen model need read
    /// (it passes an APC's bytes over, and a sixel image's are only drawn
    /// here), so it is spared them.
    Body(usize),
}

/// Longer commands are another kind (a clipboard's contents, an image):
/// nothing this reads is that long.
const LIMIT: usize = 4096;
/// A graphics command's most (a picture is sent in chunks of 4096
/// characters; this allows a program sending one whole).
const GRAPHICS_LIMIT: usize = 64 << 20;

/// The bytes after an escape that start what is read here: an OSC (`]`),
/// an APC (`_`) and a DCS (`P`). A table, so telling one costs a load
/// whatever the set.
const STARTS: [bool; 256] = {
    let mut starts = [false; 256];
    starts[b']' as usize] = true;
    starts[b'_' as usize] = true;
    starts[b'P' as usize] = true;
    starts
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    /// In a command, collecting.
    Osc,
    /// An escape in a command: `\` ends it (ST).
    OscEscape,
    /// In an application program command (APC) or a device control string
    /// (DCS), collecting it if it is a picture.
    Apc,
    /// An escape in one.
    ApcEscape,
}

/// Follows the byte stream across reads, collecting operating system
/// commands and passing over everything else.
pub(crate) struct Sniffer {
    state: State,
    command: Vec<u8>,
    /// The picture being collected (empty but in one).
    graphics: Vec<u8>,
    /// Not yet known whether it is a picture: no byte of the application
    /// program command read yet, or only parameters of the device control
    /// string.
    apc_started: bool,
    /// The string is a device control string (a sixel image, if its
    /// parameters end in `q`).
    dcs: bool,
    /// The command outgrew its limit, or isn't one kept: dropped when it
    /// ends.
    overflow: bool,
}
impl Default for Sniffer {
    fn default() -> Self {
        Self {
            state: State::Ground,
            command: Vec::new(),
            graphics: Vec::new(),
            apc_started: false,
            dcs: false,
            overflow: false,
        }
    }
}
impl Sniffer {
    /// Read `bytes`, calling `found` with each command of interest they end
    /// and where in `bytes` it ends (just past its terminator).
    pub(crate) fn feed(&mut self, bytes: &[u8], mut found: impl FnMut(usize, Osc)) {
        let mut rest = bytes;
        let offset = |rest: &[u8]| bytes.len() - rest.len();
        while !rest.is_empty() {
            match self.state {
                // Most escapes (colours, cursor moves) start nothing read
                // here: told by the byte after (`STARTS`), they are passed
                // over in this loop, without the states. An escape after
                // this one is found by the next search.
                State::Ground => loop {
                    let Some(at) = memchr::memchr(0x1b, rest) else {
                        return;
                    };
                    rest = &rest[at + 1..];
                    if rest.first().is_none_or(|&next| STARTS[usize::from(next)]) {
                        self.state = State::Escape;
                        break;
                    }
                },
                State::Escape => {
                    self.state = if rest[0] == b']' {
                        self.command.clear();
                        self.overflow = false;
                        State::Osc
                    } else if rest[0] == b'_' || rest[0] == b'P' {
                        self.graphics.clear();
                        self.overflow = false;
                        self.apc_started = true;
                        self.dcs = rest[0] == b'P';
                        State::Apc
                    } else if rest[0] == 0x1b {
                        State::Escape
                    } else {
                        State::Ground
                    };
                    rest = &rest[1..];
                }
                State::Osc => {
                    let end = rest
                        .iter()
                        .position(|&b| b == 0x07 || b == 0x1b || b == 0x18 || b == 0x1a);
                    let (body, next) = match end {
                        Some(at) => (&rest[..at], Some(rest[at])),
                        None => (rest, None),
                    };
                    if self.command.len() + body.len() > LIMIT {
                        self.overflow = true;
                    } else {
                        self.command.extend_from_slice(body);
                    }
                    match next {
                        Some(0x07) => {
                            let at = offset(rest) + body.len() + 1;
                            self.finish(|osc| found(at, osc));
                            self.state = State::Ground;
                        }
                        Some(0x1b) => self.state = State::OscEscape,
                        // CAN and SUB cancel it.
                        Some(_) => self.state = State::Ground,
                        None => return,
                    }
                    rest = &rest[body.len() + 1..];
                }
                State::OscEscape => {
                    if rest[0] == b'\\' {
                        let at = offset(rest) + 1;
                        self.finish(|osc| found(at, osc));
                        self.state = State::Ground;
                        rest = &rest[1..];
                    } else {
                        // Another sequence began: this one ended unterminated.
                        self.state = State::Escape;
                    }
                }
                State::Apc => {
                    // The parser ends one at ESC, CAN or SUB, as here.
                    let end = rest
                        .iter()
                        .position(|&b| b == 0x1b || b == 0x18 || b == 0x1a);
                    let body = &rest[..end.unwrap_or(rest.len())];
                    if self.apc_started && !body.is_empty() {
                        // Only pictures are kept: graphics commands, and
                        // device control strings ending their parameters
                        // (digits and semicolons alone) in `q`.
                        if !self.dcs {
                            self.apc_started = false;
                            self.overflow = body[0] != b'G';
                        } else if let Some(&last) =
                            body.iter().find(|&&b| !(b.is_ascii_digit() || b == b';'))
                        {
                            self.apc_started = false;
                            self.overflow = last != b'q';
                        }
                        // Parameters of a string read across reads: not kept.
                        if self.overflow {
                            self.graphics = Vec::new();
                        }
                    }
                    if !self.overflow {
                        if self.graphics.len() + body.len() > GRAPHICS_LIMIT {
                            self.overflow = true;
                            self.graphics = Vec::new();
                        } else {
                            self.graphics.extend_from_slice(body);
                            // Known a picture, its body is spared the
                            // screen model: it still reads the string's
                            // start and end, which is all it keeps of one.
                            if !self.apc_started && !body.is_empty() {
                                let from = offset(rest);
                                found(from + body.len(), Osc::Body(from));
                            }
                        }
                    }
                    match end.map(|at| rest[at]) {
                        Some(0x1b) => self.state = State::ApcEscape,
                        Some(_) => self.state = State::Ground,
                        None => return,
                    }
                    rest = &rest[body.len() + 1..];
                }
                State::ApcEscape => {
                    if rest[0] == b'\\' {
                        if !self.overflow && !self.apc_started {
                            let mut command = std::mem::take(&mut self.graphics);
                            let picture = if self.dcs {
                                Osc::Sixel(command)
                            } else {
                                command.remove(0);
                                Osc::Graphics(command)
                            };
                            found(offset(rest) + 1, picture);
                        }
                        self.state = State::Ground;
                        rest = &rest[1..];
                    } else {
                        self.state = State::Escape;
                    }
                }
            }
        }
    }
    /// The room held for graphics commands.
    #[cfg(test)]
    pub(crate) fn graphics_held(&self) -> usize {
        self.graphics.capacity()
    }
    fn finish(&mut self, found: impl FnOnce(Osc)) {
        if !self.overflow
            && let Some(osc) = parse(&self.command)
        {
            found(osc);
        }
        self.command.clear();
    }
}

fn parse(command: &[u8]) -> Option<Osc> {
    let text = std::str::from_utf8(command).ok()?;
    let (number, rest) = text.split_once(';').unwrap_or((text, ""));
    match number {
        "7" => pwd(rest).map(Osc::Pwd),
        "133" => {
            let mut fields = rest.split(';');
            match fields.next()? {
                "C" => Some(Osc::CommandStarted),
                "D" => Some(Osc::CommandFinished(
                    fields.next().and_then(|code| code.parse().ok()),
                )),
                _ => None,
            }
        }
        "9" => {
            if let Some(progress) = rest.strip_prefix("4;").or((rest == "4").then_some("")) {
                return Some(Osc::Progress(crate::Progress::parse(progress)));
            }
            // ConEmu's other OSC 9 commands are numbered; a notification
            // is text.
            let numbered = rest.split(';').next().is_some_and(|first| {
                !first.is_empty() && first.bytes().all(|b| b.is_ascii_digit())
            });
            (!numbered && !rest.is_empty()).then(|| Osc::Notification {
                title: String::new(),
                body: rest.to_owned(),
            })
        }
        "777" => {
            let mut fields = rest.splitn(3, ';');
            (fields.next()? == "notify").then(|| Osc::Notification {
                title: fields.next().unwrap_or("").to_owned(),
                body: fields.next().unwrap_or("").to_owned(),
            })
        }
        _ => None,
    }
}

/// The path of OSC 7's `file://host/path` (percent-decoded), or a bare path.
fn pwd(url: &str) -> Option<String> {
    let path = match url.strip_prefix("file://") {
        Some(rest) => &rest[rest.find('/')?..],
        None if url.starts_with('/') => url,
        // `kitty-shell-cwd://` and others name a host the same way.
        None => {
            let (_, rest) = url.split_once("://")?;
            &rest[rest.find('/')?..]
        }
    };
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = bytes.get(i + 1..i + 3)
            && let Ok(hex) = std::str::from_utf8(hex)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            decoded.push(byte);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    let path = String::from_utf8_lossy(&decoded).into_owned();
    // A Windows path in a URL: `/C:/Users`.
    #[cfg(windows)]
    let path = match path.as_bytes() {
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => path[1..].to_owned(),
        _ => path,
    };
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Progress;

    fn sniff(chunks: &[&[u8]]) -> Vec<Osc> {
        let mut sniffer = Sniffer::default();
        let mut found = Vec::new();
        for chunk in chunks {
            sniffer.feed(chunk, |at, osc| {
                if let Osc::Body(from) = osc {
                    assert!(from < at && !chunk[from..at].contains(&0x1b));
                    return;
                }
                assert!(
                    matches!(chunk[at - 1], 0x07 | b'\\'),
                    "ends after its terminator"
                );
                found.push(osc)
            });
        }
        found
    }

    #[test]
    fn commands_end_with_bel_or_st_across_reads() {
        assert_eq!(
            sniff(&[b"ls\r\n\x1b]7;file://host/Users/me/My%20Files\x07$ "]),
            [Osc::Pwd("/Users/me/My Files".into())]
        );
        assert_eq!(
            sniff(&[b"\x1b]13", b"3;D;", b"2\x1b", b"\\x"]),
            [Osc::CommandFinished(Some(2))]
        );
        assert_eq!(
            sniff(&[b"\x1b]133;C\x07out\x1b]133;D\x07"]),
            [Osc::CommandStarted, Osc::CommandFinished(None)]
        );
    }

    #[test]
    fn progress_and_notifications() {
        assert_eq!(
            sniff(&[b"\x1b]9;4;1;42\x07\x1b]9;4;3\x07\x1b]9;4;0\x07\x1b]9;4;2;10\x07"]),
            [
                Osc::Progress(Progress::Set(42)),
                Osc::Progress(Progress::Indeterminate),
                Osc::Progress(Progress::Remove),
                Osc::Progress(Progress::Error(Some(10))),
            ]
        );
        assert_eq!(
            sniff(&[b"\x1b]9;Build done\x07\x1b]777;notify;CI;green\x07\x1b]9;1;2\x07"]),
            [
                Osc::Notification {
                    title: String::new(),
                    body: "Build done".into()
                },
                Osc::Notification {
                    title: "CI".into(),
                    body: "green".into()
                },
            ]
        );
    }

    /// Escapes starting nothing read here are passed over; one starting a
    /// command is seen wherever it falls: after another escape, or split
    /// from its `]` by the end of a read.
    #[test]
    fn commands_start_after_any_escape_and_across_reads() {
        assert_eq!(
            sniff(&[
                b"\x1b[0m\x1bM\x1b\x1b]7;/a\x07\x1b[1m\x1b",
                b"]7;/b\x07\x1b"
            ]),
            [Osc::Pwd("/a".into()), Osc::Pwd("/b".into())]
        );
    }

    /// Graphics commands are read whole across reads, ended by ST; other
    /// application program commands, and those cancelled, pass.
    #[test]
    fn graphics_commands_are_read_across_reads() {
        assert_eq!(
            sniff(&[b"a\x1b_Ga=T,f=100;iVBO", b"Rw0K\x1b", b"\\b\x1b_Gi=1\x1b\\"]),
            [
                Osc::Graphics(b"a=T,f=100;iVBORw0K".to_vec()),
                Osc::Graphics(b"i=1".to_vec())
            ]
        );
        assert_eq!(
            sniff(&[
                b"\x1b_",
                b"Xother\x1b\\\x1b_Gcut\x18\x1b_Gesc\x1b[0m\x1b]7;/p\x07"
            ]),
            [Osc::Pwd("/p".into())]
        );
    }

    /// Sixel images are read whole across reads, their parameters split
    /// from the rest; other device control strings pass.
    #[test]
    fn sixel_images_are_read_across_reads() {
        assert_eq!(
            sniff(&[b"\x1bP0;1", b";0q\"1;1;2;6#0", b"~~\x1b\\"]),
            [Osc::Sixel(b"0;1;0q\"1;1;2;6#0~~".to_vec())]
        );
        assert_eq!(
            sniff(&[b"\x1bPq#1!6~\x1b\\\x1bP+q544e\x1b\\\x1bP=1s\x1b\\\x1bP$qm\x1b\\"]),
            [Osc::Sixel(b"q#1!6~".to_vec())]
        );
    }

    /// A picture's body, once known one, is told apart for the screen model
    /// to be spared it, across reads: a graphics command's after its `G`, a
    /// sixel image's parameters and data. Other strings are not.
    #[test]
    fn picture_bodies_are_told_apart() {
        let mut sniffer = Sniffer::default();
        let mut bodies = Vec::new();
        let chunks: [&[u8]; 2] = [
            b"x\x1b_Ga=T;AAAA",
            b"BBBB\x1b\\y\x1bPq#1~~\x1b\\\x1b_Xno\x1b\\\x1bP+q54\x1b\\",
        ];
        for chunk in chunks {
            sniffer.feed(chunk, |at, osc| {
                if let Osc::Body(from) = osc {
                    bodies.push(chunk[from..at].to_vec());
                }
            });
        }
        assert_eq!(bodies, [&b"Ga=T;AAAA"[..], b"BBBB", b"q#1~~"]);
    }

    /// Output with no graphics command holds no room for one.
    #[test]
    fn no_graphics_commands_hold_no_room() {
        let mut sniffer = Sniffer::default();
        sniffer.feed(
            b"\x1b[31mred\x1b]0;title\x07\x1b]7;file://h/a\x07\x1b_Xapc\x1b\\\
              \x1bP+q544e\x1b\\\x1bP1$r0m\x1b\\\x1bP=1s\x1b\\",
            |_, _| {},
        );
        assert_eq!(sniffer.graphics.capacity(), 0);
    }

    #[test]
    fn other_sequences_and_long_commands_pass() {
        let long = [b"\x1b]7;file://h/".as_slice(), &[b'a'; 5000], b"\x07"].concat();
        assert_eq!(
            sniff(&[&long, b"\x1b[31mred\x1b]0;title\x07\x1b]8;;http://x\x1b\\"]),
            []
        );
        // An escape inside a command that isn't ST ends it unterminated.
        assert_eq!(
            sniff(&[b"\x1b]7;file://h/a\x1b[0m\x1b]7;file://h/b\x07"]),
            [Osc::Pwd("/b".into())]
        );
    }
}
