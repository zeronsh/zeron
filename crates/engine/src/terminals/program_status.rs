//! Bounded, streaming OSC 7501 receiver. Observes bytes without rewriting the
//! terminal stream. Runs beside the PTY, including while no viewport is attached.

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use chrono::{DateTime, Utc};
use zeron_proto::{ProgramInputKind, ProgramState, ProgramStatus};

pub(super) const SUPPORT_REPLY: &[u8] =
    b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";
const MAX_SEQUENCE: usize = 4096;
const MAX_RECORDS: usize = 256;

#[derive(Default)]
enum Scan {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    Osc,
    OscEscape,
    String,
    StringEscape,
}

struct Record {
    status: ProgramStatus,
    updated_at: DateTime<Utc>,
    order: u64,
}

#[derive(Default)]
pub(super) struct ProgramTracker {
    scan: Scan,
    osc: Vec<u8>,
    oversized: bool,
    records: BTreeMap<String, Record>,
    clock: u64,
}

#[derive(Default)]
pub(super) struct Update {
    pub changed: bool,
    pub queries: usize,
}

impl ProgramTracker {
    pub fn feed(&mut self, bytes: &[u8]) -> Update {
        let mut update = Update::default();
        for &byte in bytes {
            match self.scan {
                Scan::Ground => {
                    if byte == 0x1b {
                        self.scan = Scan::Escape;
                    }
                }
                Scan::Escape => self.escape(byte, &mut update),
                Scan::EscapeIntermediate => match byte {
                    0x1b => self.scan = Scan::Escape,
                    0x20..=0x2f => {}
                    _ => self.scan = Scan::Ground,
                },
                Scan::Osc => match byte {
                    0x07 => self.finish(3, &mut update),
                    0x1b => self.scan = Scan::OscEscape,
                    0x18 | 0x1a => self.scan = Scan::Ground,
                    _ if self.osc.len() < MAX_SEQUENCE => self.osc.push(byte),
                    _ => self.oversized = true,
                },
                Scan::OscEscape => {
                    if byte == b'\\' {
                        self.finish(4, &mut update);
                    } else {
                        // An unrelated escape cancels the unfinished report.
                        self.escape(byte, &mut update);
                    }
                }
                Scan::String => match byte {
                    0x1b => self.scan = Scan::StringEscape,
                    0x18 | 0x1a => self.scan = Scan::Ground,
                    _ => {}
                },
                Scan::StringEscape => {
                    self.scan = if matches!(byte, b'\\' | 0x18 | 0x1a) {
                        Scan::Ground
                    } else if byte == 0x1b {
                        Scan::StringEscape
                    } else {
                        Scan::String
                    };
                }
            }
        }
        update
    }

    fn escape(&mut self, byte: u8, update: &mut Update) {
        self.scan = match byte {
            b']' => {
                self.osc.clear();
                self.oversized = false;
                Scan::Osc
            }
            // Do not interpret OSC-shaped data inside DCS/APC/PM/SOS strings.
            b'P' | b'_' | b'^' | b'X' => Scan::String,
            b'c' => {
                update.changed |= !self.records.is_empty();
                self.records.clear();
                Scan::Ground
            }
            0x1b => Scan::Escape,
            0x20..=0x2f => Scan::EscapeIntermediate,
            _ => Scan::Ground,
        };
    }

    fn finish(&mut self, framing: usize, update: &mut Update) {
        self.scan = Scan::Ground;
        if self.oversized || self.osc.len() + framing > MAX_SEQUENCE {
            return;
        }
        if self.osc == b"7501;?" {
            update.queries += 1;
        } else if self.osc == b"133;A" || self.osc.starts_with(b"133;A;") {
            update.changed |= self.end_command();
        } else if let Some(body) = self.osc.strip_prefix(b"7501;") {
            // Validate the entire report before changing any record.
            if let Some(report) = parse(body) {
                match report {
                    Report::Clear(id) => {
                        let before = self.records.len();
                        self.records.retain(|key, _| {
                            !(id.is_empty()
                                || key == &id
                                || key
                                    .strip_prefix(&id)
                                    .is_some_and(|tail| tail.starts_with('/')))
                        });
                        update.changed |= self.records.len() != before;
                    }
                    Report::Status(status) => {
                        self.clock = self.clock.saturating_add(1);
                        if let Some(record) = self.records.get_mut(&status.id) {
                            record.order = self.clock;
                            if record.status == status {
                                return;
                            }
                        } else if self.records.len() == MAX_RECORDS {
                            let oldest = self
                                .records
                                .iter()
                                .min_by_key(|(_, r)| r.order)
                                .map(|(id, _)| id.clone())
                                .unwrap();
                            self.records.remove(&oldest);
                        }
                        self.records.insert(
                            status.id.clone(),
                            Record {
                                status,
                                updated_at: DateTime::from_timestamp_millis(
                                    Utc::now().timestamp_millis(),
                                )
                                .unwrap(),
                                order: self.clock,
                            },
                        );
                        update.changed = true;
                    }
                }
            }
        }
    }

    /// Process exit and OSC 133 A end live records, never invent success.
    /// Completed/error records survive until cleared or acknowledged by the UI.
    pub fn end_command(&mut self) -> bool {
        let before = self.records.len();
        self.records.retain(|_, record| {
            matches!(
                record.status.state,
                ProgramState::Done | ProgramState::Error
            )
        });
        self.records.len() != before
    }

    pub fn summary(
        &self,
        seen_at: Option<DateTime<Utc>>,
    ) -> Option<(ProgramStatus, DateTime<Utc>)> {
        let record = self
            .records
            .values()
            .filter(|record| {
                !matches!(
                    record.status.state,
                    ProgramState::Done | ProgramState::Error
                ) || seen_at.is_none_or(|seen| record.updated_at > seen)
            })
            .min_by(|a, b| {
                a.status
                    .state
                    .attention_rank()
                    .cmp(&b.status.state.attention_rank())
                    .then_with(|| b.updated_at.cmp(&a.updated_at))
                    .then_with(|| a.status.id.cmp(&b.status.id))
            })?;
        let mut status = record.status.clone();
        let mut parent = status.id.as_str();
        while status.app.is_none() && !parent.is_empty() {
            parent = parent.rsplit_once('/').map_or("", |(parent, _)| parent);
            status.app = self.records.get(parent).and_then(|r| r.status.app.clone());
        }
        Some((status, record.updated_at))
    }
}

enum Report {
    Status(ProgramStatus),
    Clear(String),
}

fn valid_id(id: &str) -> bool {
    id.len() <= 128
        && id.split('/').count() <= 8
        && id.split('/').all(|segment| {
            !segment.is_empty()
                && segment.len() <= 32
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
        })
}

fn text(
    value: Option<&&str>,
    encoded_limit: usize,
    decoded_limit: usize,
) -> Result<Option<String>, ()> {
    let Some(value) = value else { return Ok(None) };
    if value.len() > encoded_limit {
        return Err(());
    }
    let bytes = STANDARD
        .decode(value.as_bytes())
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(value.as_bytes()))
        .map_err(|_| ())?;
    if bytes.len() > decoded_limit {
        return Err(());
    }
    let value = String::from_utf8(bytes).map_err(|_| ())?;
    if value.chars().any(|c| c.is_control()) {
        return Err(());
    }
    // These strings appear outside the terminal grid. Disarm bidi overrides.
    Ok(Some(
        value
            .chars()
            .filter(|c| {
                !matches!(*c,
                    '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
                )
            })
            .collect(),
    ))
}

fn parse(body: &[u8]) -> Option<Report> {
    let body = std::str::from_utf8(body).ok()?;
    let mut pairs = BTreeMap::new();
    for pair in body.split(':') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key.len() > 16 {
            return None;
        }
        // Limits apply to every pair, including overwritten duplicates. An
        // oversized optional field discards the report rather than disappearing.
        match key {
            "msg" if value.len() > 2732 => return None,
            "title" if value.len() > 256 => return None,
            "app" if value.len() > 32 => return None,
            "id" if value.len() > 128
                || value.split('/').count() > 8
                || value.split('/').any(|segment| segment.len() > 32) =>
            {
                return None;
            }
            _ => {}
        }
        if key.is_empty()
            || !key.bytes().all(|b| b.is_ascii_lowercase())
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.,+/=-".contains(&b))
        {
            continue;
        }
        match key {
            "msg" => {
                text(Some(&value), 2732, 2048).ok()?;
            }
            "title" => {
                text(Some(&value), 256, 192).ok()?;
            }
            _ => {}
        }
        pairs.insert(key, value);
    }
    let id = match pairs.get("id") {
        Some(id) if valid_id(id) => (*id).to_owned(),
        Some(_) => return None,
        None => String::new(),
    };
    let message = text(pairs.get("msg"), 2732, 2048).ok()?;
    let title = text(pairs.get("title"), 256, 192).ok()?;
    let state = match *pairs.get("state")? {
        "idle" => ProgramState::Idle,
        "working" => ProgramState::Working,
        "done" => ProgramState::Done,
        "blocked" => ProgramState::Blocked,
        "error" => ProgramState::Error,
        "clear" => return Some(Report::Clear(id)),
        _ => return None,
    };
    let kind = if state == ProgramState::Blocked {
        match pairs.get("kind").copied() {
            Some("permission") => Some(ProgramInputKind::Permission),
            Some("question") => Some(ProgramInputKind::Question),
            Some("auth") => Some(ProgramInputKind::Auth),
            _ => None,
        }
    } else {
        None
    };
    let progress = pairs
        .get("progress")
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<u8>().ok())
        .filter(|n| *n <= 100)
        .filter(|_| state.active());
    let app = pairs
        .get("app")
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 32
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
        })
        .map(|s| (*s).to_owned());
    Some(Report::Status(ProgramStatus {
        id,
        state,
        kind,
        progress,
        app,
        title,
        message,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(body: &str) -> Vec<u8> {
        format!("\x1b]7501;{body}\x1b\\").into_bytes()
    }

    #[test]
    fn fragmented_reports_queries_and_terminators() {
        let bytes = report("state=blocked:kind=permission:msg=QXBwcm92ZT8");
        for split in 0..=bytes.len() {
            let mut tracker = ProgramTracker::default();
            tracker.feed(&bytes[..split]);
            tracker.feed(&bytes[split..]);
            let (status, _) = tracker.summary(None).unwrap();
            assert_eq!(status.state, ProgramState::Blocked);
            assert_eq!(status.message.as_deref(), Some("Approve?"));
            assert_eq!(status.kind, Some(ProgramInputKind::Permission));
        }
        let mut tracker = ProgramTracker::default();
        assert_eq!(tracker.feed(b"\x1b]7501;?\x07").queries, 1);
        assert!(tracker.summary(None).is_none());
        tracker.feed(b"\x1b]7501;state=working:progress=25\x07");
        assert_eq!(tracker.summary(None).unwrap().0.progress, Some(25));
    }

    #[test]
    fn children_inherit_app_replace_fields_and_clear_only_their_subtree() {
        let mut tracker = ProgramTracker::default();
        for body in [
            "state=idle:app=claude-code",
            "state=working:id=build",
            "state=blocked:id=build/test:kind=question",
            "state=done:id=builder",
        ] {
            tracker.feed(&report(body));
        }
        assert_eq!(
            tracker.summary(None).unwrap().0.app.as_deref(),
            Some("claude-code")
        );
        tracker.feed(&report("state=working:id=build/test"));
        assert!(tracker.records["build/test"].status.kind.is_none());
        tracker.feed(&report("state=clear:id=build"));
        assert_eq!(tracker.records.len(), 2);
        assert!(tracker.records.contains_key("builder"));
        tracker.feed(&report("state=clear"));
        assert!(tracker.summary(None).is_none());
    }

    #[test]
    fn invalid_and_oversized_reports_are_atomic_and_bounded() {
        let mut tracker = ProgramTracker::default();
        tracker.feed(&report("state=working"));
        for body in [
            "state=done:id=bad/id/",
            "state=done:msg=AA==",
            "state=done:msg=_",
            "state=new-state",
            "state=done:abcdefghijklmnopq=x",
            "state=done:msg=AA==:msg=b2s=",
            "state=done:app=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:app=ok",
            "state=done:id=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:id=ok",
        ] {
            assert!(!tracker.feed(&report(body)).changed, "{body}");
            assert_eq!(
                tracker.summary(None).unwrap().0.state,
                ProgramState::Working
            );
        }
        assert!(
            !tracker
                .feed(&report(&format!(
                    "state=done:msg={}",
                    STANDARD.encode("x".repeat(2049))
                )))
                .changed
        );
        assert!(
            !tracker
                .feed(&report(&format!(
                    "state=done:unknown={}",
                    "x".repeat(MAX_SEQUENCE)
                )))
                .changed
        );
        assert!(tracker.osc.len() <= MAX_SEQUENCE);
        for n in 0..300 {
            tracker.feed(&report(&format!("state=done:id=job{n}")));
        }
        assert_eq!(tracker.records.len(), MAX_RECORDS);
        assert!(!tracker.records.contains_key("job0"));
    }

    #[test]
    fn prompt_exit_and_reset_have_protocol_lifetimes() {
        let mut tracker = ProgramTracker::default();
        for body in [
            "state=done:id=a",
            "state=error:id=b",
            "state=working:id=c",
            "state=blocked:id=d",
        ] {
            tracker.feed(&report(body));
        }
        tracker.feed(b"\x1b[?1049h\x1b[?1049l\x1b[!p");
        assert_eq!(tracker.records.len(), 4);
        assert!(tracker.feed(b"\x1b]133;A\x07").changed);
        assert_eq!(tracker.records.len(), 2);
        tracker.feed(&report("state=working:id=c"));
        assert!(tracker.end_command());
        assert_eq!(tracker.records.len(), 2);
        assert!(tracker.feed(b"\x1bc").changed);
        assert!(tracker.summary(None).is_none());
    }

    #[test]
    fn seen_errors_do_not_hide_new_results_and_still_supply_app_inheritance() {
        let mut tracker = ProgramTracker::default();
        tracker.feed(&report("state=error:app=cargo"));
        let seen = Utc::now();
        tracker.records.get_mut("").unwrap().updated_at = seen;
        tracker.feed(&report("state=done:id=build"));
        tracker.records.get_mut("build").unwrap().updated_at = seen + chrono::Duration::seconds(1);
        let (summary, _) = tracker.summary(Some(seen)).unwrap();
        assert_eq!(summary.state, ProgramState::Done);
        assert_eq!(summary.app.as_deref(), Some("cargo"));
        assert!(
            tracker
                .summary(Some(seen + chrono::Duration::seconds(1)))
                .is_none()
        );
        assert_eq!(tracker.records.len(), 2);
    }

    #[test]
    fn unrelated_control_strings_and_repeated_done_do_not_create_activity() {
        let mut tracker = ProgramTracker::default();
        tracker.feed(b"\x1bPpayload \x1b]7501;state=done\x07\x1b\\");
        assert!(tracker.summary(None).is_none());
        assert!(tracker.feed(&report("state=done")).changed);
        let before = tracker.summary(None);
        assert!(!tracker.feed(&report("state=done")).changed);
        assert_eq!(tracker.summary(None), before);
    }
}
