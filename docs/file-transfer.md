# Device file transfer

Send files and folders straight from one of your engines to another — a
laptop to your phone, a VPS to your desktop — over a temporary tunnel, with
no size limit. An agent on a remote machine can hand you what it built
("send me the apk") and it shows up on your phone, ready to open.

Every engine sends and receives: desktop Linux, macOS and Windows, a
`zeron headless` server, and the phone's on-device engine. The code lives in
`crates/transfer` (protocol, sender, receiver, relay pipe) and
`crates/engine/src/file_transfers.rs` (tunnels, recipient lookup).

## Trust and acceptance

Peers are the account's own authenticated devices — the remote-workspace
trust boundary of ARCHITECTURE.md §1. A transfer from one of them is accepted
into the inbox unless the receiving device turned on **Ask before accepting
files from my other devices** (`FileTransferSettings.requireConfirmation`);
then it waits in `awaitingAcceptance` until that device's user accepts or
declines. The receiver trusts nothing in the sender's manifest (see below).
Over P2P the sender's device id is the one the signaling coordinator
stamped; over the relay the DeviceRoom admits only the account's devices and
the claimed `fromDeviceId` must be one of them.

Received items land in `{home}/Zeron Transfers/<sender device name>/` (the
inbox root is a per-device setting). An item whose name is taken gets a
` (2)` suffix; nothing is ever overwritten. A sender may name an explicit
`destination`; the receiver accepts it only if it is an existing folder
inside its home folder, its inbox, or one of its projects, and not inside a
`.git` folder.

## Transport

1. **P2P first.** The sender opens mux streams to service `file-transfer:v1`
   through the preview networking stack (`docs/preview-networking.md`): the
   edge's PreviewRoom relays only SDP, and the bytes travel over one ordered,
   reliable WebRTC DataChannel between the two engines, DTLS-authenticated by
   the fingerprints exchanged over the authenticated signaling socket. The
   peer id the receiver sees is the one the coordinator stamped. The mux's
   per-stream 64 KiB credit window is the backpressure; a transfer uses one
   control lane and one data lane on the same connection (parallel mux
   streams share one SCTP association and measured slower), with one block
   of read-ahead so disk reads and hashing overlap the send.
2. **Relay fallback.** If no direct path pairs within 8 s (or P2P is
   unavailable), the lanes run over the DeviceRoom relay instead: each lane
   is a byte pipe made of RPCs on the existing device link — a
   `FileTransferPipe` stream carries receiver → sender bytes, and ordered
   `FileTransferPipeWrite` calls (≤ 48 KiB raw, base64, sequence-numbered,
   12 in flight) carry sender → receiver bytes. A write is acknowledged only
   once the receiver has taken its bytes, which is the relay's backpressure.
   Every message stays within the relay's message limits.
3. **Teardown.** A tunnel paired for a transfer is closed once its streams
   are idle; a pre-existing preview connection is left alone.

The protocol below is identical over both: a lane is just an ordered byte
stream.

## Protocol

Frames are `u32 BE length ‖ u8 kind ‖ body`: kind 1 is a JSON control
message, kind 2 a data block `u32 file ‖ u64 offset ‖ sha256[32] ‖ bytes`.

```
sender                                   receiver
  ── Hello{v, transferId, sessionId, lane: control} ──▶
  ── Offer{entryCount, fileCount, totalBytes, digest, destination?} ──▶
  ── Manifest{entries[≤2048]}* , ManifestEnd ──▶
                                          validate · decide · lay out
  ◀── Pending (every 10 s while the user decides) ──
  ◀── Accept{have: [{file, blocks: [[start,end]…], done}]} ──
  ── Hello{lane: data} + Block* on the data lane(s) ──▶
  ── Digest{file, sha256} (whole file, computed alongside) ──▶
  ◀── Progress{doneBytes} (2/s) · Resend{file, blocks?} ──
  ◀── Complete ──   (or Cancel / Decline / Error from either side)
```

- **Manifest.** A flat pre-order list of `{path, kind: file|dir|symlink,
  size, mode, target?}` with `/`-separated relative paths. The receiver
  refuses empty names, `.`/`..`, absolute paths, backslashes, NUL or control
  characters (plus names invalid on Windows when it runs there), duplicates,
  and any entry whose parent is not a directory listed before it — so
  nothing can land outside the destination or be written through a symlink
  or file. Symlinks travel as symlinks (target verbatim, created last, never
  followed while writing); sockets, FIFOs and devices are skipped and
  counted (`skipped`). Top-level symlinks the user picks are followed.
  Unix permission bits are kept (folders stay owner-writable).
- **Blocks.** Files move in 1 MiB blocks read from disk at their offset —
  nothing is buffered beyond one block per lane. The receiver checks each
  block's SHA-256, writes it at its offset into a hidden
  `.<name>.zeron-<id>.part` (opened `O_NOFOLLOW`), and records the block as
  verified. A bad block is requested again.
- **Whole-file check and atomic finalize.** When every block of a file is
  verified and the sender's whole-file `Digest` has arrived, the receiver
  hashes the assembled part, sets its mode, fsyncs it and renames it into
  place. A mismatch requests the whole file again (at most 3 times).
- **Resume.** Every 2 s the receiver fsyncs the parts it wrote and persists
  the verified block set (`{profile}/file-transfers/incoming/<id>/`). When a
  tunnel drops, the sender redials with backoff (1 s … 30 s, P2P first each
  time) and a new session's `Accept.have` lists what is already verified, so
  only missing blocks travel. That also covers a receiver restart. A sender
  gives up after 15 minutes without progress; a receiver drops an
  interrupted transfer's partial data after 7 days.
- **Cancel.** Either side: over the control lane when one is up, otherwise
  through a relay `CancelFileTransfer {fromPeer: true}`. The receiver
  removes unfinished parts (completed files stay).
- **Free space.** A receiver refuses a transfer larger than its free space
  (minus a 64 MiB margin).

## RPC surface

All forwardable with `targetDeviceId` naming the engine that sends or holds
the transfer (so a viewer can drive any engine); the recipient of
`SendFiles` is `toDeviceId`. Not to be confused with `WatchTransfers`, the
chat-attachment upload feed.

| Method | Params → reply |
| --- | --- |
| `SendFiles` | `{toDeviceId?, chatId?, paths[], destination?}` → `{transferId, toDeviceId, toDeviceName}`. Without `toDeviceId`, the device that typed the chat's latest (non-agent) message, if it runs an engine; otherwise an error naming the devices that can receive. |
| `WatchFileTransfers` | stream of `FileTransfer[]` (newest first; progress ≤ 4 frames/s) |
| `ListFileTransfers` | unary snapshot of the same |
| `CancelFileTransfer` / `AcceptFileTransfer` / `DeclineFileTransfer` | `{transferId}` |
| `ClearFileTransfers` | `{transferId?}` — drop finished rows |
| `Get/SetFileTransferSettings` | `{requireConfirmation, inboxDir?}` |
| `FileTransferPipe` / `FileTransferPipeWrite` | engine ⇄ engine relay pipe (internal) |

Rows (`zeron_proto::FileTransfer`) carry direction, peer, state
(`preparing → connecting → [awaitingAcceptance] → transferring ⇄
reconnecting → completed | failed | cancelled | declined`), transport,
top-level items with their local paths, byte counts and throughput. The
history keeps the last 200 finished transfers per profile. Engines that
speak the protocol advertise the `file-transfer-v1` capability on their
device row.

Agents use the `send_files` MCP tool (`docs/mcp.md`).

## Clients

- **Desktop:** "Send to device…" in the file tree, a transfers indicator
  and panel (progress, cancel, accept/decline, Show in folder), an incoming
  toast, and the confirmation setting.
- **Android** (the phone's own engine — it is a regular device;
  docs/android.md § File transfers): a
  Transfers screen (Settings → Files) that polls `ListFileTransfers` over
  `host_call`, a notification per incoming transfer (with Accept/Decline when
  confirmation is on), a copy of received files in `Download/Zeron` via
  MediaStore (an APK opens the package installer), and a "Share to Zeron"
  target that stages shared files into the guest's
  `~/.zeron/outbox/<uuid>/` and sends them. To reach engines on other
  machines in development, Settings → Developer → Custom server points the
  phone's engine at a shared `zeron local-edge`.

## Limits

- One manifest holds at most 1,000,000 entries; file sizes are unbounded.
- The sender must keep its files unchanged while sending; a file that
  shrinks or changes fails that transfer with a clear error.
- Symlinks are not recreated on Windows receivers.
- macOS and Windows receivers refuse a transfer holding two names that
  differ only in letter case (their file systems can't keep them apart).
- Both engines need the edge (a signed-in account or a development/local
  edge); local-only profiles can't transfer.

## Development

`zeron local-edge --port P --token T [--bind ADDR]` serves a standalone local
edge; engines join it in `Development` scope with `ZERON_EDGE_URL=http://ADDR:P
ZERON_EDGE_TOKEN=T ZERON_USER_ID=local ZERON_ORG_ID=local zeron headless`
(docs/android.md § Development; the Android emulator reaches host loopback
as `10.0.2.2`, and the phone joins through Settings → Developer → Custom
server).

Paths and a `destination` may start with `~/`: the sending engine expands
sources against its home, the receiving engine the destination against its
own (a viewer or an agent whose chat runs in `~` can't know either).

Tests: `cargo test -p zeron-transfer` (manifest validation and traversal
refusal, chunking, block and whole-file verification, resume, cancel,
confirmation, a hostile sender) and `cargo test -p zeron-localedge --test
file_transfer` (two engines on one local edge: a folder and a 1 GiB file
over P2P and over the forced relay, each cut mid-file and resumed;
`ZERON_TRANSFER_TEST_MIB` shrinks the file, `ZERON_TRANSFER_TEST_THROUGHPUT=1`
adds uncut runs that print throughput).
