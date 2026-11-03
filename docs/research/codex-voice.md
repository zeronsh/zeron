# Native Codex subscription voice

Zeron connects a local Codex chat to the installed Codex voice runtime. Voice
uses ChatGPT authentication and the normal Codex plan budget. Codex may use
additional credits according to the user's account settings and usage limits.
There is no OpenAI API-key fallback, separate public Realtime API client, or
promise of an included-quota-only spending cap.

Sources checked 2026-10-01:

- [App-server](https://learn.chatgpt.com/docs/app-server)
- [Voice](https://learn.chatgpt.com/docs/features/voice)
- [Pricing](https://learn.chatgpt.com/docs/pricing#how-much-does-voice-cost)
- [Official Codex source, rust-v0.159.0](https://github.com/openai/codex/tree/377f7f557a6bdea0f3a2d26d4d899c66db4789d0/codex-rs),
  commit `377f7f557a6bdea0f3a2d26d4d899c66db4789d0`: app-server realtime schemas,
  `core/src/realtime_conversation.rs`, `realtime-webrtc`, and `voice-host`.

## Transport and authentication

Codex 0.159's WebSocket audio path calls `realtime_api_key` and requires API-key
credentials, including an environment-key fallback. That path is unsuitable for
this integration. Native subscription voice uses **WebRTC V3** instead:

1. Prepare or reuse an idle native thread with the normal Zeron MCP server.
   No initial `turn/start`, empty user message, title request, or submitted draft.
   Load the initial `account/read` snapshot before attaching the voice router:
   Codex announces initial authentication with `account/updated`, which must not
   be mistaken for a later identity change. Subsequent updates still retire voice.
2. Check `account/read` on that same process for `chatgpt` authentication, reject
   custom providers and realtime backend overrides in the effective project
   configuration (`config/read` with the resolved thread cwd), validate the
   provider returned by `thread/start` or `thread/resume`, inspect ordinary
   usage permission, and obtain `thread/realtime/listVoices`. Exhausted ordinary
   quota alone does not reject permitted credits; the native backend makes the
   final usage and spend-control decision.
3. Resolve the helper inside the physical standalone Codex package (layout 1,
   version 0.159 or later). Its protocol/build handshake must succeed.
4. Initialize the helper and gather its SDP offer. Call `thread/realtime/start`
   with WebRTC, V3, audio output, the lease session id, startup context, and
   `clientManagedHandoffs: false`. Codex owns background task delegation.
5. The request response acknowledges submission. Independently wait for both
   `thread/realtime/started` and `thread/realtime/sdp`; validate session/version.
6. Apply the answer and wait for transport readiness. Open default local audio
   devices while muted/suppressed. Only enable them for an attached owner.

Audio, Opus, resampling, echo cancellation, reference audio, native interruption,
and bounded playout remain in Codex's helper. No PCM or SDP reaches the UI,
document, sync journal, or owner RPC stream. Helper stderr is discarded; errors
and diagnostics use typed rejection reasons. Child processes are killed and
reaped on cancellation, including when a helper control reply is stalled.

The earlier `zeron-audio` PCM/AEC prototype remains isolated and tested offline;
it is not a dependency of the released UI or the subscription voice path.
`voice-experimental` is retained as an empty compatibility build feature. The
Codex voice control is available in ordinary desktop builds.

## Lifecycle, transcript and MCP

One engine lease owns local voice. Tokens are redacted from Debug. Ownership is
exclusive, generation checked, ephemeral, and cancelled when its scoped stream
is dropped. A five-second unattached-owner watchdog prevents abandoned starts
from creating a provider call. A successor waits for native stop and its terminal
`requested` notification from that single stop; a spontaneous terminal cannot
satisfy the barrier and leave our terminal queued for a successor. Late events
cannot acquire the successor's generation. Native
capture termination is independent of the actor's pending control I/O. The
stdout reader also cancels capture on identity changes, overflow and EOF; it
does not wait for a blocked audio-control reply. Identity retirement invalidates
pending start reservations as well as existing leases. The physical Codex
release is pinned before spawning app-server so an installer symlink change
cannot select a different helper for a warm runtime. A reader identity change
also permanently invalidates the voice bridge before any new token can be
installed. A fresh explicit request can renew that idle runtime after cleanup;
continuing delegated work is preserved and must finish before renewal.

Voice protects the warm runtime from idle reaping and updates. Voice stop leaves
delegated Codex work running. Account/profile retirement, engine/window owner
loss, audio failure and provider termination close voice; navigating between
threads does not. An agent question keeps voice open: the stage reports it and
links to the session transcript, where the usual input UI answers it. Reconnection and
microphone resumption require a fresh user action. Changes to the chat host,
configuration or checkout invalidate pending starts and active ownership;
workspace monitoring also covers synced host/configuration changes.

Canonical `thread/realtime/item/completed` transcript segments are committed by
the serialized document owner, deduplicated by session/item identity. Legacy
transcript events and partial text never become durable messages. Only a newly
inserted canonical final updates sidebar activity and preview; replay does not
bump the timestamp. Native BEM
promotion uses the existing Codex task transcript; it does not resubmit a spoken
request or create another task response. Native `turn/started` publishes the
engine turn boundary before its text/tools, including on an idle bootstrap.

Every session is an orchestrator. `thread/realtime/start` carries one short
English instruction, as `realtimeStartInstructions` for the backing Codex model
and as a developer `initialItems` entry for the voice model: use the Zeron MCP
to create, message and monitor chats, and delegate coding work to them.

The voice agent uses the unchanged upstream Zeron MCP. Creating chats, sending
messages and delegating tasks use the normal provider selection, authentication
and device routing. A Codex voice session can create a Grok, Claude Code or other
available provider session; those children use their own provider credentials.
Codex/ChatGPT authentication and local-host checks apply to starting voice itself,
not to the sessions it controls through MCP. Ending voice does not restrict
continuing tasks or their children.

## Interface and packaging

Voice starts from the microphone in the sidebar footer, next to Settings. Each
session creates a fresh projectless Codex chat (cwd `~`) on this device whose id
starts with `voice-orchestrator-`. That prefix keeps it out of the sidebar,
jump slots, command palette, archive and completion notifications, and the
engine drops it as a `parentChatId`: chats the orchestrator creates are ordinary
top-level sessions. The chat is deleted again if startup fails before a lease
exists. It uses the composer's Codex model when Codex is selected, otherwise
Codex's default. The composer is never replaced or suspended: the user keeps typing in
and switching between threads while voice runs.

While a session is live the microphone becomes a small orb. Pressing it toggles
a stage over the conversation area (the whole window when the sidebar is
collapsed); the titlebar cluster stays above it, and picking another thread,
a new session or Settings steps it aside without ending voice. The stage shows
the orb extracted from Bezel at `6141af9c16f7353cdf36003f7404e0a94566a163`
(magnified), over the new-thread hero artwork with its soft bottom fade and a
live caption. A video-call style bar at the bottom carries the session
clock, microphone mute (with a live input ring), a next-session voice menu,
transcript, a red End button and the way back to the chats (also Escape).
Bezel is not a dependency. MIT notices accompany the extracted component.
The orb tracks listening, speaking, task work and pending questions, uses the
theme and reduced-motion setting, and stops hidden/background timers.

Native voice ids from the provider catalog can be selected for the next session;
only that preference is persisted. Devices use the operating system defaults.
Codex's current helper protocol does not expose device selection.

The native helper/runtime is supplied by the standalone Codex installation,
not redistributed by Zeron, on macOS, Linux and Windows; the desktop client of
a remote call uses its own installation the same way. npm/CLI-only
installations without those resources report `nativeRuntimeUnavailable`. macOS production/dev bundles declare microphone
usage and audio-input entitlements. The viewport verifies microphone authorization
and requests it when voice is explicitly started, before creating a provider
session or opening devices. Denied access shows a settings hint. Cargo builds
embed the microphone purpose string in the executable's `__TEXT,__info_plist`
section, so direct development launches can also request access. Missing
metadata shows a rebuild hint instead of requesting permission and being
terminated by macOS. `scripts/run-macos-dev.sh` builds the
signed development bundle; `ZERON_DEV_BUILD_ONLY=1` prepares it without launching.
No new DSP DLLs or C++ build dependency are
added to the production UI. Apache-2.0 attribution for the adapted native helper
protocol is included in `THIRD_PARTY_NOTICES.md`.

## Privacy, notices and legal

Data flow:

- Audio goes from the device with the microphone (Codex's helper on desktop,
  WebRTC on iOS) directly to OpenAI, under the user's own ChatGPT-authenticated
  Codex account and OpenAI's terms. It never transits Zeron servers.
- Remote calls exchange SDP signaling and controls over the authenticated
  owner-only device relay; they are ephemeral and never persisted.
- Canonical final transcript text is stored in the orchestrator chat's session
  document and syncs like any chat, through Zeron's relay. Orchestrator chats
  are hidden from the sidebar but are not deleted when a call ends.
- Usage is billed to the user's Codex plan or credits by OpenAI.

The microphone purpose strings (`dist/macos/Info*.plist`,
`apps/ios/Zeron/Info.plist`) state that voice sessions stream audio to OpenAI;
dictation stays on device.

Notices: `THIRD_PARTY_NOTICES.md` covers the Apache-2.0 Codex protocol
adaptation (with its upstream NOTICE and an OpenAI trademark/non-affiliation
statement), the MIT orb chain (Bezel, gpui-thinking-orbs, thinking-orbs) and
WebRTC for iOS. macOS, Windows and Linux packages ship `LICENSE` and
`THIRD_PARTY_NOTICES.md`; the iOS app ships `Voice/WebRTC-LICENSE.txt` and
`Voice/ThinkingOrbs-LICENSE.txt`. No Codex binaries are redistributed.

Pending before a public release (owner decisions, not code):

- iOS: App Review guideline 5.1.2(i) requires disclosing that personal data is
  shared with a third-party AI and obtaining explicit permission before doing
  so. The permission string discloses it; an explicit in-app consent step
  before the first call is not implemented.
- iOS privacy manifest (`PrivacyInfo.xcprivacy`) and App Store privacy labels
  declare no collected data. Decide whether audio sent to OpenAI and synced
  voice transcripts must be declared.
- Zeron's public privacy policy and terms (outside this repository) should
  describe the OpenAI audio flow and transcript sync.
- WebRTC's embedded third-party licenses (BoringSSL, libsrtp, Opus, libyuv,
  Abseil and others) are referenced, not reproduced; generate the full list
  from the pinned XCFramework before distribution.

## Validation and practical limits

Offline CI never starts a provider session or microphone. Fake app-server and
helper peers exercise actual engine RPC, independent start acknowledgements,
SDP signaling, owner attachment, native controls, transcript replay, task
continuation, stop/restart and authentication rejection. Synthetic audio tests
remain offline. Cross-platform checks compile the native bridge without loading
hardware; signed package/device trials require their actual operating systems.

Read-only discovery:

```sh
python3 scripts/codex-voice-probe.py
```

Explicit opt-in connectivity smoke (consumes normal Codex voice usage, does not
open microphone devices):

```sh
python3 scripts/codex-voice-smoke.py --live
```

Optional device/playout check opens default devices with the microphone **muted**
and requests a short greeting:

```sh
python3 scripts/codex-voice-smoke.py --live --devices
```

Evidence on 2026-10-01, Linux, installed Codex 0.159.0: ChatGPT authentication,
packaged helper initialization, real V3 WebRTC negotiation, transport readiness
and native stop passed. No API fallback was used. The optional device trial
failed at opening audio devices: this environment exposes a network output
monitor, without a usable default microphone. No real speech or acoustic echo
trial is claimed. macOS/Windows signed package and physical-device trials also
remain unverified; protocol/fixture success does not establish those results.

Implementation review and local test results are recorded in the ignored
`.personal/codex-voice/` working notes. No push, deployment or remote PR is part
of this task.
