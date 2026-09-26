# Prompt drafts

A new-session canvas is an independent draft until its first send. Desktop shows
nonempty drafts in the theme-accented **Drafts** section above Pinned after the user
leaves the canvas for a new session or an existing conversation. Typing autosaves
an unlisted canvas on every device. New session preserves the previous canvas; opening a draft restores its destination, run
configuration, prompt, images and Appshot context. Existing conversation composers
continue using their existing per-chat draft behavior.

## Persistence and synchronization

- `promptDrafts` registry rows contain the preferred revision, fractional
  `orderKey`, visibility and discard/send markers. Moving a draft only changes its order.
  Making a draft visible is permanent: delayed autosaves cannot hide it again.
- `draftRevisions` rows contain immutable ancestry, a short preview, target and
  creation time. Every surviving revision head is recoverable. Concurrent edits
  appear as separate entries; consuming one head preserves the other heads.
- Full content and image bytes live outside registry rows. Desktop uses SQLite
  snapshots plus a durable publication outbox; iOS uses profile-scoped files and
  an atomic outbox. Content is limited to 2 MiB; individual assets to 32 MiB.
- Publication uploads immutable content to `/draft-content/{org}/{object}` before
  publishing its registry references. Asset IDs are SHA-256 hashes and successful
  uploads are cached. Offline edits are visible locally while publication retries.
- Content and registry rooms use the authenticated user's organization/profile.
  A host going offline does not remove drafts already uploaded to Edge.
- Desktop keeps a profile-scoped recovery outbox until the engine acknowledges a
  save. Failed RPCs survive navigation and restart, and replay follows revision
  ancestry. If local recovery storage fails, an in-memory copy remains retryable
  while the application is running. Profiles cannot replay each other's requests.
  A separate canvas checkpoint allows an interrupted editor to reappear as a
  recoverable draft on restart; successful sends and navigation clear it.
- Attachment RPCs transfer 1 MiB chunks. Loading a draft fetches its content first
  and then its assets, avoiding oversized WebSocket frames even for large images.
- Desktop saves after 300 ms of inactivity and flushes on navigation and normal
  application quit. iOS also flushes when entering the background.

The ordering allocator is shared with sidebar pins. Draft ordering is independent
of pin ordering. The sidebar applies pending moves optimistically and accepts
watch/response snapshots by monotonically increasing engine revision.

## First send

The engine reserves an exact draft revision with the registry Durable Object.
Same-revision retries return the same identity; a competing revision is rejected.
Chat, command and first-message IDs are derived from the draft ID. The command is
persisted before the draft is consumed, and replaying it does not append another
command. Failed sends keep their draft content available.

An uncertain send can be retried with the reserved revision. Editing after a send
attempt creates a new draft identity and retains the original attempt. Reservations
are not silently reassigned to different content. Merged command copies receive
the same terminal outcome, including copies arriving after execution. Discard is
permanent and cannot be undone by delayed movement or publication. Immutable content and ancestry are
retained; this version does not garbage-collect draft history or shared assets.

## Rollout and validation

Deploy the Edge routes and registry claim support before using cross-device draft
publication. Update the desktop engine/UI and native iOS client. Desktop capability
negotiation uses `prompt-drafts-v1` and prevents sending to unsupported hosts.
This change does not deploy Edge automatically.

Regression coverage includes dense ordering, offline move convergence, concurrent
heads after send, durable discard, content/asset restart recovery, immutable
retries, cross-user/org isolation in real R2, serialized claims in real SQLite,
preserving independent composer canvases, failure recovery across navigation,
large attachments over real WebSocket RPC, late command duplicates, and
out-of-order revision publication. Native Swift tests cover registry
projection and restored Appshot context; they require Xcode to run.
