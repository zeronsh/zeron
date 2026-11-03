# Split-host Codex voice: compatibility probe

Status (2026-10-01): **G0 pending**. The diagnostic and its offline tests are
implemented. No Mac/Fedora conversation, cross-network test, MCP roundtrip or
iPhone test has been performed. These offline tests do not establish live
subscription or audio compatibility.

Current implementation: V2 client-owned audio is enabled by default in desktop
apps and headless engines; the V1 RPC remains for compatibility. No environment
flag is needed. See [testing instructions](voice-remote-testing.md).

## Run the diagnostic

From the Mac, using an explicitly selected local media helper and an already
configured SSH host with subscription-authenticated Codex:

```sh
python3 scripts/codex-voice-remote-smoke.py --live \
  --ssh-host YOUR_FEDORA_ALIAS \
  --remote-codex /absolute/path/on/fedora/bin/codex \
  --remote-cwd /absolute/existing/directory/on/fedora \
  --helper /absolute/local/voice/bin/codex-voice-host
```

The command above negotiates without opening audio devices. Add
`--devices --seconds 60` to open the **Mac's** microphone and speaker for a
bounded conversation. Speak after the audio-active notice. Interrupt the spoken
response to assess barge-in. Ctrl-C closes local audio before attempting remote
stop. A desktop terminal needs macOS microphone permission for this live probe.

The helper does not resolve a local Codex executable. Copying only its binary is
insufficient: keep its adjacent libraries, plugins and runtime metadata intact.
This probe does not package that runtime into Zeron; that is C02.

SSH runs only metadata commands and Codex app-server on Fedora, never a helper
or an audio-device command. SSH must already work noninteractively; unknown host
keys are not auto-accepted. The script neither registers a Zeron device nor
uses SSH as a proposed product transport. Product discovery and controls will
reuse registered Zeron devices and the existing authenticated relay.

`--live` explicitly opts into normal subscription usage. There is no API-key
fallback. The diagnostic checks ChatGPT auth and rejects custom provider/realtime
configuration before negotiating. Its ephemeral, read-only diagnostic thread
instructs the model not to use tools. It does not create a Zeron orchestrator or
send messages to other agents.

## What the result means

The output contains stage/error codes, platforms, helper build, remote Codex
version, transport status, canonical final-item counts and cleanup results.
It never prints SDP, ICE candidates, account fields, transcript contents or raw
provider errors. It does not write protocol traffic to files. Codex's own logging
policy on the host remains independent of this diagnostic.

`ok` requires successful negotiation and both local shutdown and the provider's
requested-close event. With `--devices`, it also requires at least one canonical
user final and assistant final received on the remote app-server connection.
These counters deduplicate by item ID and reject other thread/session IDs.
They do not independently prove audible quality, echo cancellation or barge-in.
`muteControlAccepted` means the helper acknowledged mute, not that silence was
measured. `gateG0Passed` and `mcpRoutingVerified` always remain false: the tool
cannot certify the product's full-ID MCP return path.

The `thread/realtime/listVoices` call deliberately precedes thread creation.
`voicesBeforeThread` records whether that request succeeded with the tested
binary. It does not infer compatibility with every Codex version.

A fresh app-server/thread/session is used for each run. The existing wire format
can omit a session ID on SDP/close notifications; the diagnostic never reuses a
thread for another negotiation. Production bridge generations still need the
explicit lifecycle isolation described in C04.

## Offline verification

```sh
python3 -m unittest discover -s scripts/tests -p 'test_voice_remote*.py' -v
```

The tests launch independent fake app-server/helper subprocesses using the real
JSON-lines and length-prefixed pipes. They never launch Codex, contact a provider,
open hardware or require credentials. Covered cases include:

- Started/SDP in either order, final-item deduplication and stale identities.
- Malformed and oversized SDP, with limits measured in bytes.
- Truncated/oversized helper frames and stalled helper cleanup.
- Provider closure/rejection, RPC EOF and request/negotiation deadlines.
- Lost start reply and cancellation during start, including explicit stop.
- Auth/config rejection before transport, secret-safe results and SSH quoting.

A failed helper exchange permanently invalidates that pipe and reaps its child;
a late reply cannot satisfy another command. Requests, signaling, queues and
item tracking are bounded. Cleanup stops local media first, then requests remote
stop, then closes/reaps only the diagnostic's own subprocesses.

Observed locally: 17 tests passed on macOS ARM64. This is control/protocol
coverage, **not live provider compatibility evidence**.

## Local runtime inventory

Read-only inspection of the installed standalone package on 2026-10-01:

| Property | Observed value |
| --- | --- |
| Codex package | `0.160.0`, layout version `1` |
| Target | `aarch64-apple-darwin` |
| Source/helper build | `a956835d020762cb2b570053af06f643a11c0ecc` |
| Helper SHA-256 | `e408413b79d76e518966046ce1d7758523c606844269fe4dd56446bed80c1062` |
| Helper size | 9,900,112 bytes |
| Complete voice subtree | 37 files, 21,237,467 bytes |
| Dynamic libraries | 18 |
| GStreamer plugins | 7 |
| License files | 7 |
| Metadata/notices | `manifest.json`, `runtime.json`, `sources.json`, `NOTICE.md` |
| Package manifest hashes | All entries verified, no mismatches |
| Helper code signature | `codesign --verify --strict` succeeded |

`otool -L` confirms adjacent `@rpath` dependencies on libffi, GLib/GIO/GObject,
GStreamer, libintl, Opus, PCRE2 and zlib, plus macOS system libraries/frameworks.
The seven plugins are app, audioconvert, audioresample, coreelements, opus, rtp
and rtpmanager. The package sources inventory identifies GStreamer 1.28.6,
GLib 2.88.3, Opus 1.6.1, libffi 3.8.0, PCRE2 10.47, zlib 1.3.2 and
proxy-libintl 0.5, with upstream archive hashes. Meson 1.12.0 and Ninja 1.13.2
are recorded build inputs, not runtime libraries.

Bundled notice files cover LGPL-2.1, Opus, PCRE2, libffi, proxy-libintl, sljit and
zlib. The package's `NOTICE.md` identifies the upstream Codex
`third_party/voice/` build/projection/package scripts and references
`manifest.json` for the source commit. This inventories existing provenance.
Zeron does not redistribute this runtime: the desktop client of a remote call
runs the helper of its own standalone Codex installation, as local voice does.
Redistributing it would make Zeron responsible for the LGPL source offer, the
Apache NOTICE, the Windows Visual C++ runtime terms and security updates of the
media stack. `scripts/package-voice-runtime.py` remains a development tool that
projects a pinned package for `ZERON_VOICE_MEDIA_DIR`. The helper only
initializes from a `codex-resources/voice` directory: anywhere else it exits
with code 23 on `initializeRuntime`, which the client reports as an unavailable
runtime. Projection never re-signs: upstream already signs the helper and
libraries with OpenAI's Developer ID, hardened runtime and a secure timestamp.
No binaries or credentials were copied into this repository.

## Remaining G0 evidence

Remaining live acceptance (the user requested proceeding with implementation
and will run these checks after it):

1. Supply the Fedora SSH alias, absolute Codex path and working directory, with
   compatible subscription authentication already configured.
2. Run negotiation and an actual bidirectional conversation, with media on Mac
   and app-server on Fedora. Record sanitized versions/platforms and whether the
   devices are on different networks. Verify Fedora has no audio-device access.
3. Inspect the actual media contract: negotiated codecs, ICE gathering, required
   data-channel messages/readiness, barge-in and echo behavior. The presence of
   an Opus library alone does not establish the negotiated codec contract.
4. Validate canonical finals and a real Zeron MCP delegation/return using the
   full orchestrator ID. The standalone no-tools diagnostic cannot establish
   this: it must be paired with a Zeron-hosted harness test. Do not interpret a
   generic promoted-item notification as proof of MCP success.
5. Record any provider restriction before committing the dependent product
   architecture. Runtime redistribution is avoided: clients use their installed
   standalone Codex helper.

G0–G4 remain pending live validation. C02–C15 code is implemented,
including the shared coordinator, remote engine control, desktop media runtime,
UniFFI bridge and native iOS UI/audio. Automated tests cover a fake Codex host
through two engines and the WebSocket relay; they do not establish live provider
acceptance. A physical iPhone is required at G3/G4.
