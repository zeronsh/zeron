# macOS composer dictation

The microphone button and **⌘⇧D** dictate into the current message draft. The
button is keyboard-focusable (Enter/Space) and exposes an accessible name,
shortcut, toggle state, and Click action. Native accessibility announcements cover
permission, listening, finalizing, and stopped/error transitions. Start/permission,
listening, finalizing, and failure
states have visible text. Focus returns to the editor after using the control.

Speech uses the viewing Mac's microphone and current system locale, including
for chats hosted on another device. Zeron checks
[`supportsOnDeviceRecognition`](https://developer.apple.com/documentation/speech/sfspeechrecognizer/supportsondevicerecognition)
and sets `requiresOnDeviceRecognition`. Unsupported locales/devices report an
unavailable state; there is no network recognition fallback. Audio buffers go
only to Apple's on-device recognizer. They are never saved, attached, sent to an
agent, added to a document, or synchronized.

Each partial replaces one range anchored at the initial selection, with one
undo step for the whole dictation. Moving the selection, typing, IME composition,
undo, or accepting a completion stops capture first and preserves the latest
partial. Dictation cannot start over marked IME text. Empty results do not erase
selected text. Stop allows up to two seconds for a final correction, then keeps
the latest partial. Send and Save to queue finish once before continuing; an
error preserves the draft and cancels the pending send. Escape commits the latest
partial immediately and cancels a pending send.

Changing chats/drafts, replacing the composer with a question, cancelling a
queue edit, closing/switching windows, or deactivating Zeron stops capture.
Permission callbacks and queued submit events cannot modify a replacement draft.
The native bridge owns its frameworks on the UI thread; the engine/RPC/schema
have no voice fields.

## Development and packaging

Use `scripts/run-macos-dev.sh`, which launches the **Zeron Dev.app** bundle with
its own privacy identity. A bare `cargo run -p zeron` executable deliberately
reports dictation unavailable before requesting permission; it is not supported
microphone/permission evidence. Both development and packaged plist templates
contain microphone/speech usage descriptions. Both signing paths include the
[hardened-runtime audio-input entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.device.audio-input).

To build the development bundle without launching it:

```sh
ZERON_DEV_BUILD_ONLY=1 scripts/run-macos-dev.sh
```

`CARGO_TARGET_DIR` may point to a shared Cargo cache. Bundle output remains in
this checkout's `target/macos-dev` or `target/package` directory.

Deterministic checks need no microphone or speech permissions:

```sh
cargo check -p zeron-ui
cargo test -p zeron-ui --lib dictation
cargo test -p zeron-ui --lib composer::
cargo test -p zeron-ui --lib queue::
```

## Hardware verification

Fake-recognizer tests prove range/edit/send/lifecycle behavior, not real Speech
availability or permission prompts. For both Zeron Dev.app and the packaged
Zeron.app, verify on a Mac with an on-device-supported language:

1. Launch the bundle through LaunchServices. Grant both permissions; prompts
   should identify the corresponding Zeron bundle, not the terminal. Dictate
   several words, then stop and correct the draft.
2. Repeat with selected text, emoji, multiple partial corrections, undo/redo,
   manual editing, IME, completion, an attachment, a queued edit, and a remote
   chat. Send while speaking: exactly one final draft should enter the ordinary
   text/attachment path.
3. Deny each permission and retry after enabling it in Privacy & Security.
   Try an unavailable on-device language: Zeron must not start cloud recognition.
4. Switch chats/windows, close the window, deactivate the app, and disconnect
   the microphone. Capture must stop and the latest draft must survive.
5. Check compact/expanded layout, keyboard focus, and VoiceOver controls/status.
   The GPUI accessibility tree exposes the status and native notifications request
   announcements; actual VoiceOver behavior needs
   real VoiceOver verification.

A signed bundle inspection alone does not establish any of these runtime results.
