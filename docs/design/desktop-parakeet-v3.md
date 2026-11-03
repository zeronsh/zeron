# Desktop Parakeet v3 dictation

Authorized implementation brief: `/Users/gaelcado/zeron/evidence/voice/desktop-parakeet-v3-handoff-2026-09-27.md`.

Dictation is device-local and opt-in. Settings → Voice provides the optional model download, microphone selection, privacy notice and lifecycle actions; supported languages are documented in the desktop dictation reference. A microphone beside the composer utilities starts/stops recording; a rebindable composer shortcut uses the existing keymap. Stop inserts an editable transcription; only explicit Send/Queue can submit.

Use a pinned native Rust ONNX adapter for NVIDIA Parakeet TDT 0.6B v3, with a checksummed, pinned upstream conversion downloaded separately. CPU execution provides a common desktop baseline. Capture and inference remain outside the engine, RPC, documents and sync. Audio is bounded and transient. Model downloads use staging and checksum verification before activation. Cancellation invalidates work; only one inference owner may run at a time.

Reuse the historical editor lifecycle design selectively: the input owns a generation and selected UTF-8 range; a changed draft, IME, undo, navigation or focus transition invalidates it. Inference never mutates a different draft. Finalization is bounded, and failure/timeout never submits. Submission intent must retain normal versus modified Send semantics.

Parakeet TDT is an offline model. Start with final transcription on Stop; investigate previews without promising native streaming. Preserve text on errors and empty recognition. Match existing GPUI settings widgets, semantic theme colors, accessible buttons, and composer density.

Implementation sequence:
1. Verify runtime/model APIs, pin manifest and attribution; implement model lifecycle and bounded native transcription/capture.
2. Adapt input lifecycle and deterministic tests to current composer, including Send/Queue and shortcut configuration.
3. Add settings lifecycle and composer recording/finalizing/error presentation with production widgets.
4. Run formatting, focused checks/tests, actual English/French runtime samples and packaged native UI verification in an isolated development profile. Record performance and limitations; prepare a local PR description. No publishing.
