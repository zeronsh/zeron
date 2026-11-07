# Attachments for chats hosted on Cloud

Status: implemented; the lifecycle rule below is applied by hand before the edge deploys.

## The problem

Attachment bytes used to travel only over a direct peer link to the chat's host. A Cloud
machine may be asleep when someone sends an image. A wake that took long, or a sender that
went offline, lost the message: the machine's wait cap (15 minutes from when the message was
sent) rejected it with "attachments never arrived".

## The flow

1. **The sender holds the message.** A send to a Cloud-hosted chat goes into a durable outbox
   (`held_sends` in the local store) instead of the chat's doc. The host can't see it. The
   send state shows as usual: the message's echo, "Sending", the percent ring.
2. **The bytes go up.** `PUT /blob/{chatId}/attachments/{uploadId}`, with retries on the
   existing transfer backoff, cut short by network and system wakes. The outbox and the staged
   bytes survive restarts, so an upload resumes after one.
3. **The message goes out.** Only after every file is stored does it enter the doc (with its
   clocks starting then) and the host get its wake. The sender can go offline from here on.
4. **The machine fetches on wake.** Its drain sees a `pending://` ref whose bytes aren't on
   disk and looks in the store. Found: it writes the file where the ref resolves, re-drains
   the same way an `UploadCommit` does, and deletes the object.

Later sends of the same chat queue behind a held one, so nothing overtakes it. Controls
(Stop, answering a question) don't wait; they go straight out.

## The 15-minute cap

It now applies only when nothing is on the way. A machine past the cap asks the store first,
and only a definite "nothing there" rejects the message. While the store has the bytes, or
can't be asked (an outage, a token refreshing), the message keeps waiting, bounded by the
command's own expiry. An expired message can still be retried by the user.

## How long a message waits

A command to a Cloud-hosted chat expires after **7 days**; every other chat keeps its day
(`zeron_doc::command_ttl_ms`). A machine can be asleep, or waiting on its person to pick a
size, for days, and the message should still run, once, when it comes back. The sender stamps
the expiry, so a host of any version honors it. Past it, the message reads as expired in its
chat, with a retry.

An old message is about to run when its machine comes back, so the composer says so first.
`WatchWaitingMessages {chatId}` lists the chat's messages not yet taken: each one's `sentAt`,
`expiresAt`, sender, and whether it's still held here uploading. On mobile, a pending send's
`sent_at_ms` carries the same age.

## Failure

The outbox retries by itself for the same 7 days. It stops on a permanent answer (too large,
refused, the store full, the staged file gone) or when the week runs out, and the message then shows as failed in its
chat with the reason and a retry ("Not delivered: … — click to retry"). A failed send
doesn't hold up later ones. One nobody retries is dropped after a week, since there's no way
yet to dismiss one.

## What keeps the peer path

| Sender | Host | Edge | What happens |
| --- | --- | --- | --- |
| any | a user's own computer | any | peer path, as before |
| old | any | any | peer path; a new machine also looks in the store, finds nothing, and waits as before |
| new | Cloud, reported in, no `attachment-store-v1` | any | peer path, as before |
| new | Cloud, not reported in yet | new | store first, then the peer path too, in case it's old |
| new | Cloud, `attachment-store-v1` | new | store only |
| new | Cloud | old (404 on the PUT) | the message goes out and the bytes take the peer path |

The capability is `zeron_proto::capabilities::ATTACHMENT_STORE_V1`. An old engine ignores it,
and never looks in the store.

## Access

Objects live at `attachments/{userId}/{chatId}/{uploadId}`, where the user id comes from the
caller's verified token, so only that user's bearers can name them. A Cloud machine's token
is admitted only for a chat it hosts, or a delegate it started
(`edge/src/runner-access.ts`). Every read and write needs a bearer. The size limit is the
engine's existing upload cap, 32 MiB, refused from the `content-length` before anything is
stored. Each object carries a SHA-256 that R2 checks on the way in and the machine checks on
the way out.

## Quota

What one user has stored and not yet collected is capped: **1 GiB** and **500 files** by
default, set with the edge's `ATTACHMENT_QUOTA_BYTES` and `ATTACHMENT_QUOTA_OBJECTS`. An upload
past either gets `507 quota_exceeded`, and the message shows as failed in its chat ("too many
files are waiting for Cloud machines to collect them; retry once one has") with a retry. It's
never dropped. A file frees its space when its machine collects and deletes it, or when the
lifecycle rule expires it. The check runs before each upload, so uploads racing each other can
pass it together. The 32 MiB limit per file is unchanged.

## Retention

- **After delivery:** the machine deletes the object as soon as its copy is on disk.
- **Fallback:** an R2 lifecycle rule expires anything left under `attachments/` after
  **14 days**: the 7 days a message can wait, and a week more for a retry once it has expired
  (a phone keeps no copy of its own after the upload, so its retry relies on this one). A missed
  delete only costs storage until then.

The rule is bucket configuration, not code, so it has to be applied once per bucket:

```sh
npx wrangler r2 bucket lifecycle add zeron-staging-blobs attachments-expire attachments/ --expire-days 14
npx wrangler r2 bucket lifecycle add comet-native-blobs attachments-expire attachments/ --expire-days 14
```

**Cost.** Objects normally live for minutes. Assuming 1,000 attachments a day at 2 MB:
about 30,000 writes a month ($0.14), about 30,000 reads ($0.01), deletes free, no egress,
and storage rounds to nothing. If every delete failed, the 14-day rule caps storage at about
28 GB ($0.42 a month), and no one user can hold more than their quota. These are estimates from R2's published prices, not measurements. The
normal case fits inside R2's free allowance.

## Mobile

The iOS and Android apps have no engine: `crates/client` writes to the chat's doc itself. So
`crates/client/src/live/held.rs` runs the same outbox over the same store and table, with the
escort stash as its staged bytes. The app's retry and failed state are the existing ones.

## A row already on the machine's disk

A queue restored from disk stays frozen on a user's computer: restarting the app shouldn't send
anything. A Cloud machine restarts on every wake, so its queue thaws by itself
(`DocHost::thaw_recovered_queue`), once boot recovery is through and after any turn it brings back.
Sent first, the row would read to that turn as the chat having moved on, and the turn wouldn't run.

## How it was attached doesn't matter

Plain sends, app captures and sends queued during a live turn all stage locally and go
through the store. A capture names its image in an XML attribute, so the host rewrites a ref
in both spellings (`uploads::rewrite_pending_ref`). A queued send waits in the outbox as a row:
its message shows in the transcript until the queue panel has it. Over a peer link nothing
changed: those two still upload straight to the host first.
