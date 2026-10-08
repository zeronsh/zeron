# Native iOS voice endpoint (development)

Physical-device acceptance is delegated to the user; G3/G4 remain pending.
The initial endpoint is foreground-only and does not contain Codex or credentials.

The dependency is pinned to [stasel/WebRTC 150.0.0](https://github.com/stasel/WebRTC/blob/150.0.0/Package.swift),
whose Swift package declares binary SHA-256
`f9890492b0016e4c88ab20f07867b8b420054caedc8a692b2ec6ac041f3cf6b2`.
Xcode verifies this artifact checksum and records the resolved revision. Retain
WebRTC's bundled notices when distributing (`Voice/WebRTC-LICENSE.txt` is copied
from the pinned XCFramework and included as an app resource, as is the orb's MIT
notice `Voice/ThinkingOrbs-LICENSE.txt`; see `THIRD_PARTY_NOTICES.md` and the
privacy/legal section of [codex-voice.md](codex-voice.md)). The package wraps the upstream native
SDK rather than reimplementing its codecs, echo cancellation or audio device.

Contract reference: the [pinned Codex helper transport](https://github.com/openai/codex/blob/a956835d020762cb2b570053af06f643a11c0ecc/codex-rs/voice-host/src/transport.rs)
adds an audio track, creates an ordered `oai-events` data channel, gathers ICE into
the offer, and waits for the channel to open after applying the answer. It discards
incoming channel message bodies and rejects remotely created channels. The iOS
endpoint follows those behaviors. This source inspection does not establish that
subscription negotiation, interruption or MCP work on an actual iPhone.

The endpoint keeps its audio track and manual audio device disabled until the
host confirms the lease. Mute disables the local track without a network roundtrip.
Close disables audio and releases the peer before remote cleanup. The WebRTC audio
configuration is installed globally (`setWebRTCConfiguration`), because WebRTC
re-applies its global configuration when the audio unit starts and would otherwise
drop the app's category options mid-call. Output follows the proximity sensor
(loudspeaker in hand, earpiece at the ear) unless a headset, Bluetooth or car route
is active. The call survives the screen locking (background audio). An interruption
or failed connection closes it; it never resumes automatically. Cancellation
invalidates pending continuations so late SDK callbacks cannot return a second
result or reactivate media.

The orb is the desktop's: `zeron-orb` computes geometry, clock, audio response and
crossfades for both, and iOS only paints the frames (`OrbView`).

Required live checks: iPhone on Wi-Fi and cellular, Fedora and Mac hosts, actual
bidirectional sound, barge-in/AEC, Bluetooth and route changes, background during
startup and active speech, denied permission, full-ID MCP delegation and unique
host-side transcripts. Measure package size, CPU and latency on the target device;
none are claimed by an offline build.

Pending before App Store submission (owner decisions, also tracked in the
[acceptance checklist](voice-remote-testing.md#legal-and-privacy-blocks-a-public-release)):

- Consent: guideline 5.1.2(i) requires disclosing that audio is shared with a
  third-party AI (OpenAI) and explicit permission before doing so. The
  microphone purpose string discloses it; an in-app consent step before the
  first call is not implemented.
- Privacy manifest: `PrivacyInfo.xcprivacy` declares no collected data. Decide
  whether audio sent to OpenAI and synced voice transcripts must be declared,
  consistently with the App Store privacy labels.
