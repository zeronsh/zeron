# Remote voice development testing

Remote voice is enabled by default in desktop apps and headless engines.
Real Mac/Fedora and physical iPhone acceptance is pending and will be performed
by the user. Compilation and fake-provider tests
are not evidence of subscription/media compatibility. No production rollout or
server deployment is part of this change.

## Desktop

Start the execution host's new Zeron engine/app normally. No environment flag
is required. Codex must already be installed and authenticated through
ChatGPT on that host. It needs no audio helper, microphone or speaker. Continue
using the existing registered device and account; do not register another host.

The client (the device with the microphone) needs its own standalone Codex
installation, version 0.159 or later, from the official installer: audio runs
through that installation's `codex-voice-host` helper, on macOS, Linux and
Windows alike. The client's Codex is resolved like the harness's (`CODEX_EXECUTABLE`,
PATH, the login shell's PATH, known install locations); its credentials are not
read. npm installations do not include the helper. Zeron packages contain no
Codex runtime.

```sh
ZERON_DEV_BUILD_ONLY=1 ./scripts/run-macos-dev.sh
```

To test a client without Codex, or a specific helper build, set
`ZERON_VOICE_MEDIA_DIR` to a runtime projected by
`scripts/package-voice-runtime.py` (see [its notes](../../dist/voice/README.md)).
It must be a `codex-resources/voice` directory containing `zeron-runtime.json`.

Open `target/macos-dev/Zeron Dev.app`, select **Settings → Voice → Codex voice
device**, choose the registered Fedora host and start voice from the sidebar orb.
The microphone permission belongs to this Mac. Host choice is local to these UI
settings and cannot change an active call. A host with an older build that does
not advertise the remote voice capability is not eligible. Style is validated
by the chosen host at Prepare.

Desktop uses client-owned audio for both this device and a remote execution
host. Only an explicit call opens the microphone; advertising the capability
does not start audio capture. No transcript migration or credential transfer
is needed.

### Linux and Windows desktop clients

They follow the same rule: the installed standalone Codex supplies the helper
(`codex-voice-host` / `codex-voice-host.exe` with its `.so`/DLL tree). The Linux
helper requires the host's glibc and ALSA libraries. On Windows, verify live that
the `codex.exe` installed by `install.ps1` resolves inside its package (next to
`codex-package.json`); otherwise voice reports `nativeRuntimeUnavailable`.

Offline checks:

```sh
python3 -m unittest discover -s scripts/tests -p 'test_voice_packaging.py' -v
cargo test --locked -p zeron-voice-media --lib
```

To check an actual projected runtime's initialization without opening audio
devices or contacting a provider:

```sh
ZERON_VOICE_MEDIA_DIR=/absolute/path/to/codex-resources/voice \
  cargo test --locked -p zeron-voice-media --lib \
  packaged_native_runtime_initializes_without_audio_devices -- --ignored
```

On Windows set `$env:ZERON_VOICE_MEDIA_DIR` before the same Cargo command.
Real microphone, playback, permissions, interruption and live-provider
acceptance still need to be performed on each physical platform.

## Acceptance checklist

- Mac audio and Fedora Codex, including a host without an audio device/helper.
- Bidirectional conversation, interruption, mute and immediate local hang-up.
- Two successive calls and MCP delegation to a different provider, with return
  messages addressed to the complete orchestrator chat ID.
- One final transcript copy after sync; no active call or audio on other devices.
- Host shutdown, owner disconnect, half-open connection and late callbacks.
- Cancel while preparing/negotiating, then immediately start another call.
- 20 start/stop cycles, one 15-minute call and devices on separate networks.
- Open/reopen from Settings and drag the topbar; retain existing orb smoothing.

Record versions, anonymized platform/network labels, observed outcomes and
monotonic durations. Do not record SDP, tokens, audio or private transcript text.
A failed negotiation should be investigated using the opt-in split-host smoke
and its sanitized stages; it does not imply trying an API-key fallback.

### Legal and privacy (blocks a public release)

Owner decisions, not code; see the privacy/legal section of
[codex-voice.md](codex-voice.md).

- [ ] iOS consent: App Review guideline 5.1.2(i) requires disclosing that audio
  is shared with a third-party AI (OpenAI) and explicit permission before the
  first call. The microphone string discloses it; add an in-app consent step.
- [ ] iOS privacy manifest and App Store privacy labels: decide whether audio sent
  to OpenAI and synced voice transcripts are declared in `PrivacyInfo.xcprivacy`,
  and keep App Store Connect consistent with it.
- [ ] Zeron's public privacy policy and terms (outside this repository) describe
  the OpenAI audio flow and transcript sync.
- [ ] WebRTC's embedded third-party licenses (BoringSSL, libsrtp, Opus, libyuv,
  Abseil and others) are generated from the pinned XCFramework and shipped
  with the iOS app.

## Automated coverage

`voice-tests.yml` includes the new media/coordinator crates and default
fake-provider remote-engine tests. These require no live provider or microphone.
The new V2 controls travel over the existing relay and remain ephemeral; no audio
frames or volume-meter stream are added to sync or the command ledger.

## iOS

Build the Rust core with `scripts/ios/build-core.sh iphonesimulator` (or `iphoneos`
for a physical device). Open `apps/ios/Zeron.xcodeproj` and use the **Zeron** scheme.
Sign a physical-device build using your existing development team. No Codex
executable or OpenAI credentials are installed in the iOS app.

Voice appears by itself once a registered execution host advertises
`voice-client-media-v1` (all current engines advertise it; calls require
authenticated Codex on the host); the `-remote-voice` launch argument still
forces it on for development.
Tap the waveform at the end of the **New session** bar to call the chosen host —
or the only one online — and hold it to pick the host and voice. On iPad it sits
beside compose in the sidebar toolbar. **More → Settings → Voice** keeps the same
choices. Microphone permission is requested after checking the host capability.

During a call the bar becomes a live strip (orb, clock, mute, hang-up) and an
open session shows the live orb in its navigation bar; both open the full-screen
stage. The stage has the desktop orb, caption, transcript button (marked when
Codex is waiting for an answer) and mute/route/end. Mute and hang-up act locally
first. Like a phone call: in hand audio plays on the loudspeaker, held to the ear
the proximity sensor turns the screen off and moves it to the earpiece, and a
headset or car route always wins. Locking the screen keeps the call (background
audio); an interruption such as an incoming call ends it, without automatic
restart. Losing a headset re-routes instead of ending the call.

`-demo -voice-preview [-voice-stage]` (debug builds) plays a scripted call for
screenshots and UI work without audio, host or relay.

Offline iOS lifecycle tests (no microphone permission or provider):

```sh
scripts/ios/build-core.sh iphonesimulator
ZERON_SKIP_CORE=1 xcodebuild -project apps/ios/Zeron.xcodeproj -scheme Zeron \
  -destination 'platform=iOS Simulator,name=Zeron iPhone 17 Pro' \
  ARCHS=arm64 ONLY_ACTIVE_ARCH=YES CODE_SIGN_IDENTITY=- \
  -only-testing:ZeronTests/RemoteVoiceLifecycleTests test
```

Use an available ARM64 simulator name on your machine. The Rust build script
currently creates an ARM64 simulator archive; a generic x86_64 simulator build
cannot link that archive. Keep simulator ad-hoc signing enabled so it can launch.

## Local validation record

On macOS ARM64, the iOS simulator build and all six native lifecycle tests
passed (including the shared orb frames). Rust tests cover incompatible hosts before permission, cancellation
while an offer is stalled, a half-open heartbeat, local meters, owner drop,
duplicate prepare/negotiation, late controls and bounded UniFFI callbacks.
The full remote flow also passed with two engines over the WebSocket test relay
and a fake Codex package whose audio helper was removed. The test checks that
closing the owner releases the call without breaking the shared connection.

Live sound, Wi-Fi/cellular behavior, Bluetooth, battery/CPU, latency, cross-network
provider acceptance and real MCP delegation are still the user's acceptance tests.
These live checks remain pending; default enablement does not establish that
all of those environments have been verified.
