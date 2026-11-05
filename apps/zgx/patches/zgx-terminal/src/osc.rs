//! The operating system commands the screen model leaves to its embedder,
//! read from the same bytes it parses: the working directory (OSC 7),
//! shell-integrated command ends (OSC 133), progress (OSC 9;4) and desktop
//! notifications (OSC 9, OSC 777).

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
}

/// Longer commands are another kind (a clipboard's contents, an image):
/// nothing this reads is that long.
const LIMIT: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    /// In a command, collecting.
    Osc,
    /// An escape in a command: `\` ends it (ST).
    OscEscape,
}

/// Follows the byte stream across reads, collecting operating system
/// commands and passing over everything else.
pub(crate) struct Sniffer {
    state: State,
    command: Vec<u8>,
    /// The command outgrew `LIMIT`: dropped when it ends.
    overflow: bool,
}
impl Default for Sniffer {
    fn default() -> Self {
        Self {
            state: State::Ground,
            command: Vec::new(),
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
                State::Ground => match memchr(0x1b, rest) {
                    Some(at) => {
                        self.state = State::Escape;
                        rest = &rest[at + 1..];
                    }
                    None => return,
                },
                State::Escape => {
                    self.state = if rest[0] == b']' {
                        self.command.clear();
                        self.overflow = false;
                        State::Osc
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
            }
        }
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

fn memchr(byte: u8, bytes: &[u8]) -> Option<usize> {
    bytes.iter().position(|&b| b == byte)
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
