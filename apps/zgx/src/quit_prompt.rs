//! Quitting with "Keep Zeron running after you quit" off (Settings →
//! General; the engine's `crates/engine/src/keep_running.rs`).
//!
//! On (the default) nothing happens here: the engine keeps running, as it
//! always has. Off: the app tells its engine it quit, and the engine stops
//! once nothing is pending. Work still running is never ended silently:
//! the app asks first — "N chats are still working. Keep running in the
//! background until they finish, or stop them now?" — and only "Stop them
//! now" stops them (each chat's own Stop). Anything that keeps the question
//! from being asked (no engine, no answer) leaves the engine running.
//!
//! The question's words are the scripts' (`ui.t`, in the language the app
//! shows), handed over as they change (`app.quit_words`): by the time the
//! app quits, its scripts are gone.

use std::cell::RefCell;
use std::time::Duration;

use serde_json::{Value as Json, json};

/// The question, in the app's language: the message for one chat and for
/// many (its count written as [`COUNT_MARK`]), the detail, and the buttons.
#[derive(Debug, Clone, PartialEq)]
pub struct Words {
    pub one: String,
    pub other: String,
    pub detail: String,
    pub keep: String,
    pub stop: String,
}

/// The count the scripts word "many" with, replaced by the real one.
pub const COUNT_MARK: &str = "999999";

impl Default for Words {
    fn default() -> Self {
        Self {
            one: "1 chat is still working.".into(),
            other: format!("{COUNT_MARK} chats are still working."),
            detail: "Keep running in the background until they finish, or stop them now?".into(),
            keep: "Keep Running".into(),
            stop: "Stop Them Now".into(),
        }
    }
}

impl Words {
    pub fn message(&self, working: usize) -> String {
        if working == 1 {
            self.one.clone()
        } else {
            // (However the scripts' number formatting grouped it.)
            ["999,999", "999\u{a0}999", "999\u{202f}999", "999 999", "999.999", COUNT_MARK]
                .iter()
                .fold(self.other.clone(), |text, mark| text.replace(mark, &working.to_string()))
        }
    }
}

thread_local! {
    static WORDS: RefCell<Words> = RefCell::new(Words::default());
}

/// `app.quit_words`: the scripts' words for the question.
pub fn install(host: &zgx_script::ScriptHost) {
    host.action("app.quit_words", |args| {
        let field = |key: &str| {
            args.field(key)
                .map(crate::auth::text)
                .filter(|text| !text.is_empty())
        };
        WORDS.with(|words| {
            let mut words = words.borrow_mut();
            let defaults = Words::default();
            *words = Words {
                one: field("one").unwrap_or(defaults.one),
                other: field("other").unwrap_or(defaults.other),
                detail: field("detail").unwrap_or(defaults.detail),
                keep: field("keep").unwrap_or(defaults.keep),
                stop: field("stop").unwrap_or(defaults.stop),
            };
        });
    });
}

/// What the person chose for work still running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    KeepRunning,
    StopNow,
}

/// What to tell the engine, given what it said: nothing (it keeps running),
/// or `AppQuitting` with whether to stop the work — asking the person
/// only when there's work to stop.
pub fn decide(check: &Json, ask: impl FnOnce(usize) -> Choice) -> Option<Json> {
    if check["keepRunning"].as_bool() != Some(false) {
        return None;
    }
    let working = check["working"].as_u64().unwrap_or(0) as usize;
    let stop_work = working > 0 && ask(working) == Choice::StopNow;
    Some(json!({ "stopWork": stop_work }))
}

/// After the app's windows have closed, as it quits.
pub fn after_quit() {
    let Some((port, data_dir)) = crate::engine::get().ipc_endpoint() else {
        return;
    };
    let call = |method: &str, params: Json| {
        crate::engine::rpc::call_blocking(port, &data_dir, method, params, Duration::from_secs(5))
    };
    let check = match call("QuitCheck", json!({})) {
        Ok(check) => check,
        // No engine, or one too old to know the setting: it runs on.
        Err(error) => {
            eprintln!("zeron-zgx: quit: the engine didn't say what's running ({error}); it keeps running");
            return;
        }
    };
    let words = WORDS.with(|words| words.borrow().clone());
    let Some(quitting) = decide(&check, |working| ask(&words, working)) else {
        return;
    };
    if let Err(error) = call("AppQuitting", quitting) {
        eprintln!("zeron-zgx: quit: couldn't tell the engine ({error}); it keeps running");
    }
}

/// The question, as a native alert (macOS). Elsewhere work keeps running
/// until it finishes: nothing is stopped without asking.
#[cfg(target_os = "macos")]
fn ask(words: &Words, working: usize) -> Choice {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    let string = |text: &str| -> *mut Object {
        let text = std::ffi::CString::new(text.replace('\0', "")).unwrap_or_default();
        // SAFETY: a NUL-terminated UTF-8 string; NSString copies it.
        unsafe { msg_send![class!(NSString), stringWithUTF8String: text.as_ptr()] }
    };
    // SAFETY: AppKit on the main thread (where the app's run ended), with
    // its shared application still there; NSAlert is modal and returns the
    // button pressed (1000 for the first).
    unsafe {
        let alert: *mut Object = msg_send![class!(NSAlert), new];
        let _: () = msg_send![alert, setMessageText: string(&words.message(working))];
        let _: () = msg_send![alert, setInformativeText: string(&words.detail)];
        let _: *mut Object = msg_send![alert, addButtonWithTitle: string(&words.keep)];
        let _: *mut Object = msg_send![alert, addButtonWithTitle: string(&words.stop)];
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, activateIgnoringOtherApps: true];
        let pressed: isize = msg_send![alert, runModal];
        let _: () = msg_send![alert, release];
        if pressed == 1001 { Choice::StopNow } else { Choice::KeepRunning }
    }
}

#[cfg(not(target_os = "macos"))]
fn ask(_words: &Words, _working: usize) -> Choice {
    Choice::KeepRunning
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_the_setting_on_nothing_is_said_or_asked() {
        let asked = std::cell::Cell::new(false);
        let check = json!({ "keepRunning": true, "working": 3 });
        assert_eq!(decide(&check, |_| { asked.set(true); Choice::StopNow }), None);
        assert!(!asked.get());
        // An engine that doesn't say keeps running too.
        assert_eq!(decide(&json!({}), |_| Choice::StopNow), None);
    }

    #[test]
    fn off_with_nothing_running_the_engine_stops_without_asking() {
        let check = json!({ "keepRunning": false, "working": 0 });
        let quitting = decide(&check, |_| panic!("asked with nothing running"));
        assert_eq!(quitting, Some(json!({ "stopWork": false })));
    }

    #[test]
    fn off_with_work_running_the_person_decides() {
        let check = json!({ "keepRunning": false, "working": 2 });
        let mut asked_about = 0;
        let keep = decide(&check, |n| { asked_about = n; Choice::KeepRunning });
        assert_eq!(asked_about, 2);
        assert_eq!(keep, Some(json!({ "stopWork": false })));
        assert_eq!(decide(&check, |_| Choice::StopNow), Some(json!({ "stopWork": true })));
    }

    #[test]
    fn the_count_is_written_into_the_message() {
        let words = Words::default();
        assert_eq!(words.message(1), "1 chat is still working.");
        assert_eq!(words.message(4), "4 chats are still working.");
        let grouped = Words { other: "999,999 chats are still working.".into(), ..Words::default() };
        assert_eq!(grouped.message(12), "12 chats are still working.");
    }
}
