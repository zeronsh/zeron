# Remote Desktop validation record

Implementation validation on 2026-09-14. Results below are measurements of the
specified local lab, not a performance promise or a claim that the entire
cross-platform matrix has been exercised.

## Environment

- Client: Fedora Linux 44, Rust 1.98.0, AMD Ryzen 9 5950X (16 cores / 32 threads).
- Native GPUI X11: isolated Xvfb, NVIDIA RTX 3090 Vulkan adapter.
- Native GPUI Wayland: Weston nested in Xvfb, llvmpipe Vulkan adapter.
- Server: disposable Ubuntu 24.04 container, xrdp 0.9.24-4,
  xorgxrdp 1:0.9.19-1, Openbox and xterm. Only loopback TCP ports were published.
- TLS 1.3; self-signed certificate with an explicit one-session decision in the
  protocol probe and a matching certificate pin in the native UI fixture.
- Negotiated client capabilities include RemoteFX codec ID 3, drdynvc Display
  Control and cliprdr Unicode text. Audio/device/file channels are not attached.

## Executed checks

| Check | Result |
| --- | --- |
| `cargo check --locked -p zeron` | Passed |
| RDP core and controlled-peer tests | 17 passed (13 unit + 4 integration) |
| Complete `zeron-ui --lib` suite | 1,000 passed |
| Focused Remote Desktop UI tests after final UI adjustments | Passed |
| Release Linux application and native fixture | Built |
| Headless engine with no DISPLAY or WAYLAND_DISPLAY | IPC started; no RDP worker thread; clean interrupt/shutdown |
| Native synthetic image, X11 and Wayland | Passed rendered RGB/white color checks; screenshots captured |
| Native live surface, X11 and Wayland → xrdp | Visible desktop; matching-pin TLS connection; clean fixture exit |
| Native X11 input and focused close | Mouse focus, typed shell command, Enter, capture release and window close passed without retained handles |
| Actual terminal input | Spanish `niño con acento á` arrived once and correctly |
| Actual clipboard | Unicode text transferred in both directions; CRLF conversion verified |
| 20 connection/disconnection cycles | Passed in one process; Unicode clipboard and confirmed resize on every cycle |
| Dynamic resolution | Confirmed 1280×800 → 1024×768, including deactivation/reactivation and a new image |
| Initial 1920×1080 and 3840×2160 | Received correctly sized frames; clean disconnect |
| Shell/settings contracts | Profile persistence, owner isolation/deduplication, archive/context cleanup, late clipboard rejection and dirty-file window-close guard passed |
| Keyring boundary | Missing/locked store, ordered write/delete, retained deletion intent and retry tested through injected GPUI provider |
| Linux package | Built tarball with application, THIRD_PARTY_NOTICES and resolved dependency license files; no development examples |
| Packaging notices | Resolved metadata and available license files collected for 343 dependency packages |
| Script syntax / workflow | Shell syntax and workflow YAML parsed successfully |

The native input adapter keeps a weak view reference so the platform IME handler
cannot retain a desktop after closing a focused window. A regression test covers
that ownership contract.

The protocol tests also cover rejected/changed certificate pins, invalid TLS 1.2
and 1.3 signatures even with a matching pin, cancellation during TLS, TCP timeout,
input congestion/releases, resize debouncing, exact xrdp control packets, BGRA
stride/origin and preservation of incremental updates while presentation is hidden.

`cargo fmt --all -- --check` reports pre-existing formatting differences in
unmodified doc/engine/harness/browser/terminal files with this toolchain. The new
and modified RDP implementation files pass rustfmt, and `git diff --check` passes.
Those unrelated formatting changes were not included.

## Measurements

Release protocol probe, an idle xterm desktop over loopback:

| Initial size | First complete frame | Peak RSS | CPU time during a 15 s run |
| --- | --- | --- | --- |
| 1920×1080 | 0.923 s | 31,928 KiB | 0.08 s |
| 3840×2160 | 1.683 s | 105,412 KiB | 0.39 s |

Long-run results are stored alongside this record in
[`remote-desktop-measurements.json`](remote-desktop-measurements.json).
The animated protocol scenario repeatedly rewrites 42 rows of colored source-like
text in a 150-column xterm at a requested 30 updates/s. It runs at 1280×800 for
600 seconds and hides presentation for the middle third. This is a deliberately
heavy terminal repaint workload; the decoder can consume approximately one CPU
core. Hidden mode suppresses presentation copies, while protocol decoding remains
active. Publication counts and bytes refer to the RDP worker, not network RTT or
GPUI's paint latency.

The protocol run completed in 600.26 s, with RSS 28,948 KiB at 30 s and
29,204 KiB at the end (peak 29,332 KiB), two threads and at most 14 file
descriptors. It consumed 575.83 CPU seconds and presented 1,953 snapshots.
Sampled hidden publication counts remained unchanged at 1,052 while received
bytes continued increasing.

The separate 600-second native GPUI scenario renders the offline 1280×800 color,
checkerboard and moving-square fixture in an 800×600 window. It measures process
RSS while replacing and evicting images continuously. It includes synthetic frame
generation and rendering, with no RDP decoder or engine. These two scenarios must
not be added together as a measured full-application cost. GPU atlas tile counts
and device memory attributable to an individual window were not exposed by the
fixture; stable RSS alone is not a measurement of GPU allocation. Its RSS was 206,356 KiB
(201.5 MiB) at 30 s and at completion; the run exited without leaked GPUI handles
and consumed 315.06 CPU seconds over 602.18 s.

Evidence was captured under `/tmp/rdp-native-release`, `/tmp/rdp-native-live`,
`/tmp/rdp-performance`, `/tmp/rdp-gpu-stability`, `/tmp/rdp-native-input` and `/tmp/rdp-twenty-cycles-4` in the
implementation environment. Only synthetic lab content was captured. The CI
workflow retains its own native screenshots and xrdp logs as artifacts.

## Environment-dependent validation still pending

- Actual macOS build/run, Keychain prompts, Retina and mixed-DPI interaction.
  The native macOS fixture and tests are added to CI but were not run from Linux.
- Windows RDP/NLA interoperability from Linux Wayland and macOS. No Windows RDP
  endpoint or credentials were provided in this environment. Controlled TLS/NLA
  negotiation peers do not substitute for a Windows session.
- Real desktop IME/AltGr and OS-reserved shortcuts across the macOS/Windows matrix.
- Network latency/loss injection, video playback and comparison with FreeRDP on
  the same remote workload. No universal FPS or latency guarantee is inferred.

These pending items are not represented as passed by either fixture. The
implementation, local tests and CI wiring are complete; the missing hosts are
required to finish the original plan's full real-server/platform validation.
