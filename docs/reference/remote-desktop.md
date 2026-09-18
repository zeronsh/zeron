# Remote Desktop

Zeron can open a configured Windows or Linux RDP desktop in a native right sidebar
tab. The computer running Zeron connects directly to the configured host and port.
This works in a local workspace without a Zeron account. The chat's device, cloud
backend and preview routing do not carry desktop traffic.

## Connect

1. Open a chat, open the right sidebar and choose **Remote Desktop** from the
   surface picker or the **+** menu. A Git repository is not required.
2. Choose **Add connection**. Enter a name, a DNS name or IP address, port
   (normally 3389), username and optional domain. Enter IPv6 without brackets;
   keep the port in its separate field.
3. Select the keyboard layout used by the remote desktop: English (US), Spanish
   (Spain), or Spanish (Latin America). Select Spanish for Spanish xrdp sessions;
   its Unicode input conversion depends on the negotiated server keymap.
4. Start with 1280 × 800. Choose **Save and connect**, or **Save** and connect later.
   Saving a profile alone creates no network connection.
5. Enter the remote account's password. **Remember password** is optional and
   uses your operating system's credential store. A missing, locked or failed
   keyring allows a temporary password; passwords are never written to settings.
6. If the certificate is not trusted, verify the displayed endpoint and SHA-256
   fingerprint with the server administrator. Choose **Trust once**, **Trust and
   save fingerprint**, or **Cancel**. Changed fingerprints require a new decision.
7. Click the desktop image to control it. Expand the sidebar using its existing
   expansion control when you need more space.

The target must already have an RDP server and allow this account to log in.
Windows Remote Desktop and Linux with xrdp are intended targets. A preconfigured
VPN or tunnel can provide connectivity; Zeron does not provision either one.
macOS is a supported client build, but macOS Screen Sharing is VNC and is not an
RDP destination for this feature.

## Keyboard, pointer and clipboard

Clicking the desktop captures keyboard input. While focused, ordinary shortcuts,
including Ctrl+C, Ctrl+V, arrows, navigation keys and function keys, go to the
remote session. Forms, menus and toolbar buttons remain local. Shortcuts reserved
by the operating system may remain unavailable to applications.

Press **Ctrl+Alt+Shift+Escape**, or click **Release keyboard**, to release capture.
**Ctrl+Alt+Del** sends that sequence to the remote session. Switching tabs, hiding
the panel, opening an overlay or losing window focus releases held keys and mouse
buttons. Unicode text commits are sent once; IME composition stays local until
committed. For Spanish layouts the adapter emits dead-key compositions for
accented vowels so xrdp receives accents correctly.

Clipboard exchange is explicit:

- **Send clipboard text** offers the current local text to the remote clipboard.
  Paste it in the remote application using its normal paste shortcut.
- **Copy remote text** requests the remote text and copies it locally only
  if the originating tab, connection and focus still match when the reply arrives.

Text, including empty and multiline Unicode text, is supported up to 1 MiB
(measured both as local UTF-8 and encoded UTF-16). Newlines are converted to CRLF
for RDP and back to LF locally. Images, files and NUL-containing text are rejected.
Nothing polls or automatically synchronizes the system clipboard. A stalled
request cannot be confused with a newer request; reconnect if the peer never
answers it. Large pasted text should use the explicit clipboard action; a single
native text commit is limited to 16 KiB.

## View, resolution and session lifetime

**View: Fit** scales the whole desktop with its aspect ratio intact. Black borders
are outside the remote input area. **View: 1:1** maps remote pixels to physical
screen pixels; the arrow controls pan when the desktop exceeds the visible area.
A large desktop scaled into a narrow panel naturally has smaller text.

**Resolution: fixed** is the default. **Resolution: follow panel** requests a
single-monitor size after the visible geometry is stable for 200 ms, if the server
advertises Display Control. Requests pause during panel dragging and animations.
The server's confirmed dimensions drive drawing and pointer coordinates. If a
server does not accept a request, local scaling continues. Dimensions must be at
least 200 pixels, at most 3840 per dimension, and at most 8,294,400 pixels overall
(equivalent to 3840 × 2160, including portrait orientation).

Switching tabs or chats keeps the session connected. A hidden desktop continues
processing protocol updates but stops copying and uploading presentation images.
Returning displays the current framebuffer. Opening the same profile in one chat
focuses its existing tab. Another chat may own a separate connection; the server's
policy determines whether those connections share or replace a remote login.

**Disconnect** keeps the tab for manual reconnection. Closing its tab, deleting or
archiving its owning chat, leaving its workspace context or actually closing the
window releases its connection. A pending confirmation for unsaved files keeps
the window and its sessions alive. Disconnecting closes the transport; it does
**not** request logoff on the server. There is no automatic connection restoration
or retry loop after restarting Zeron.

Profiles live in local `ui-settings.json`. The keyring namespace includes a hash
of the local data directory, profile UUID and connection identity. Changing host,
port, username or domain invalidates the previous secret association and pin.
Renaming does not. Failed password deletion remains in the settings cleanup queue;
use **Retry removing saved credentials** in a Remote Desktop tab to retry it.

## Errors and supported protocol path

Errors distinguish DNS, network/timeouts, certificate verification, authentication,
protocol negotiation and active-session failures. A connected peer that never
provides graphics fails after the connection timeout instead of displaying an
indefinite black desktop. Validate the account and server configuration after an
authentication error; xrdp may instead return its own login screen after a rejected
PAM login.

The pinned client is IronRDP **0.17.0** with connector **0.10.0**, session **0.11.0**,
PDU **0.9.0**, graphics **0.9.0**, input **0.7.0**, Tokio **0.10.0**, cliprdr **0.7.0**
and displaycontrol **0.8.0**. TLS uses rustls through tokio-rustls **0.26.4**.
`Cargo.lock` is the authority for the complete resolved graph.

The initial graphics path handles bitmap/fast-path updates and advertises
RemoteFX (codec ID 3). The xrdp lab logs confirm that advertisement and a visible
working desktop; this is not a claim that every update uses RemoteFX. There is no
EGFX channel, AVC420/AVC444/H.264 decoder, OpenH264 download, GPU decoder or
zero-copy frame path. GPUI uploads BGRA images, limited to 30 publications/s per
visible session. Audio, microphone, printers, disks, USB, remote files, multi-monitor,
RemoteApp, RD Gateway and managed tunnels are outside this implementation.

The adapter handles two published-library interoperability issues: Display Control
CAPS must consume its eight-byte header, and xrdp's six-byte Deactivate All PDU
must be accepted on the negotiated IO channel. xrdp also treats client build
numbers at or below 419 as legacy; the connector supplies a modern compatibility
build value so it performs reactivation. These paths have wire-level regression
tests and are isolated in `crates/rdp/src/{display_control,resize}.rs`.

Sources: [Display Control capabilities](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpedisp/8989a211-984e-4ecc-80f3-60694fc4b476),
[IronRDP displaycontrol 0.8 source](https://docs.rs/crate/ironrdp-displaycontrol/0.8.0/source/src/client.rs),
[xrdp 0.9.24 resize implementation](https://github.com/neutrinolabs/xrdp/blob/v0.9.24/libxrdp/libxrdp.c).

## Reproduce validation

Run the normal Linux GPUI development dependencies from `.github/workflows/ui-tests.yml`,
then:

```sh
cargo check --locked -p zeron
cargo test --locked -p zeron-rdp
cargo test --locked -p zeron-ui remote_desktop
cargo test --locked -p zeron-ui settings::tests
cargo test --locked -p zeron-ui shell::tests
cargo build --release --locked -p zeron
cargo build --release --locked -p zeron-ui --example remote-desktop-fixture --features remote-desktop-fixture
scripts/test-remote-desktop.sh /tmp/rdp-native
cargo build --release --locked -p zeron-rdp --example probe
scripts/test-rdp-xrdp.sh /tmp/rdp-xrdp
```

The native fixture verifies rendered red/green/blue/white pixels, checkerboard and
motion in a real GPUI window. On Linux the script runs X11 and Wayland inside
isolated Xvfb/Weston displays. On macOS it captures only the fixture window.
Screenshots and logs remain in the supplied directory. Supplying `ZERON_RDP_HOST`,
`ZERON_RDP_PORT`, `ZERON_RDP_USER`, `ZERON_RDP_PASSWORD` and
`ZERON_RDP_CERT_SHA256` exercises the actual connection surface instead of the
synthetic image. The native fixture uses a temporary settings directory and
never starts an engine. With no output directory it stays open for interaction.

The xrdp script requires Podman (or `CONTAINER_RUNTIME=docker`), downloads/builds an
Ubuntu 24.04 fixture, publishes only `127.0.0.1:33991`, and removes its container on
exit. Its account and password are intentionally synthetic. It checks login,
Spanish text in a terminal, both clipboard directions and confirmed resize to
1024 × 768. Change `RDP_LAB_PORT` if that port is occupied. Use this script only
for the disposable lab; credentials for other hosts belong in the interactive UI.

For lifecycle/performance runs, the probe accepts `ZERON_RDP_PROBE_CYCLES=20`,
`ZERON_RDP_PROBE_SECONDS=600`, `ZERON_RDP_PROBE_BACKGROUND=1`,
`ZERON_RDP_PROBE_RESIZE=off`, `ZERON_RDP_WIDTH` and `ZERON_RDP_HEIGHT`.
Background mode hides the presentation for the middle third of the run and
asserts that its frame sequence stops. `ZERON_RDP_DIAGNOSTICS=1` emits local
aggregate protocol/publication counts every ten seconds, with no desktop,
clipboard or password contents. No additional telemetry is sent.

Release packages use the existing platform dependencies; no additional viewer,
codec binary or system TLS library is installed. Both packaging scripts include
`THIRD_PARTY_NOTICES.md` and the resolved RDP dependency notices. The `probe` and
native fixture are development examples and are not shipped in the application.

See [the validation record](remote-desktop-validation.md) for executed checks,
measurements and environments still requiring validation.
