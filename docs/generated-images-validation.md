# Generated images in conversation threads

Codex `imageGeneration` uses `savedPath` as its only source. The engine imports a
PNG, JPEG, WebP, or GIF of at most 24 MiB into the active profile's uploads root
before publishing the event. Neither the original path nor `result` enters the
journal or session document. Source files remain owned by Codex.

The transcript reads through the existing attachment RPC/cache. It tries the
message's device first, then the chat host and local device, without duplicate
candidates. Raster decoding accepts at most 4096 × 4096 pixels and 64 MiB of
allocation, verifies the actual raster MIME type, and retains one static 8-bit
frame downsampled to at most 2048 pixels on either axis. The lightbox uses this
same bounded preview; the original file remains on the host. Generated cache
entries and in-flight loads are keyed by device, path and declared MIME policy,
separately from generic attachments and upload aliases. Their LRU budget is
64 MiB including encoded bytes and estimated CPU/GPU copies, independent of
the legacy attachment budget. Generated history is never shielded from eviction.
The lightbox is shared with user
attachments. Media bytes stay on the host; an offline host shows an unavailable
placeholder and uses the existing 2–15 second retry ladder.

## Automated checks

```sh
cargo test -p zeron-proto -p zeron-doc
cargo test -p zeron-engine generated_image
cargo test -p zeron-engine --test e2e --test device_routing --test workspace_sync
cargo test -p zeron-harness codex
cargo test -p zeron-harness --test codex
cargo test -p zeron-ui transcript
cargo test -p zeron-ui attachments
cd edge && npm test
```

The engine fixture covers repeated completions, restart-style Loro export/import,
removal of the original Codex source, replay after removal, and negative journal
assertions. Upload tests cover supported signatures, size/path/symlink rejection,
source replacement during copying, atomic cleanup and idempotent destinations.
The two-device routing test reads the imported file locally and via the relay,
and verifies that the original source cannot be read remotely.

`crates/ui/tests/fixtures/generated-images.json` supplies loaded, loading and
unavailable entries. Transcript tests cover their rows, copy/timestamps, owner
and metadata corrections, cache reuse, retry scheduling, and clicking through to
the lightbox with Escape restoring focus. The chunk-reader test verifies the
RPC target, MIME validation and dimension limits. These are structural and
interaction checks; they are not visual snapshot approval.

## Optional real-provider smoke (consumes quota)

These tests stay ignored in normal runs. They require an authenticated Codex
installation and a model/account provisioned for image generation.

```sh
cargo test -p zeron-harness --test codex real_image_generation_smoke -- --ignored
cargo test -p zeron-engine --test e2e real_image_generation_profile_smoke -- --ignored
```

The harness smoke checks that the real provider returns an existing saved file;
the engine smoke also requires the persisted path to be under profile uploads.
No real-provider smoke or manual visual check was performed as part of the
implementation's automated tests.

## Manual visual check

Run a normal Codex chat in both light and dark themes, including a narrow window:

1. Generate a goblin PNG. Check the live chip, resolved chip, inline image,
   rounded corners, contained aspect ratio, and lightbox click/Escape focus.
2. Generate two images with text before and after. Check order, spacing and
   timestamp placement. Small images should keep their natural size.
3. Change chat and return, then restart. The durable image should reappear.
4. Open the thread on another client. Disconnect the owning host and check
   the unavailable placeholder without affecting other message parts.
5. Exercise the fake provider's quota-error and missing-path scenarios. Neither
   should leave an unresolved chip or produce an empty image part.

For the generated turn, inspect the profile journal/document for the uploads
reference and absence of the original Codex path, inline `result`, data URLs or
image Base64. Other user-authored content may legitimately contain those strings.

## Implementation validation (2026-09-14)

- Proto/document, generated-image engine tests, e2e/device routing, Codex fake
  integration, transcript and attachment tests pass. Edge's 54 unit/workerd
  tests and TypeScript typecheck pass.
- The workspace run passed 1,826 tests (21 intentionally ignored) with
  `--skip online_runtime_shutdown_stops_edge_workers_and_retires_the_graph`.
  The unfiltered run and an isolated retry failed that shutdown assertion;
  the same failure was reproduced three times using a test binary built from
  the original `1ab04195` checkout. No shutdown behavior was changed here.
- `cargo fmt --all -- --check` reports pre-existing formatting differences in
  10 files; each affected file also fails formatting in the original checkout.
- Strict `cargo clippy --workspace --all-targets -- -D warnings` stops on
  pre-existing `zeron-theme` lints. A full run with `--cap-lints warn` completes
  and preserves the existing warnings for review.

The full workspace command used to continue past the confirmed baseline failure:

```sh
cargo test --workspace -- --skip online_runtime_shutdown_stops_edge_workers_and_retires_the_graph
```

## iOS client

The iOS session mirror recognizes the same `image` part metadata. Generated
images form independent transcript rows with the message's device as the first
source and the chat host as fallback. They preserve aspect ratio, have rounded
corners, and open the shared attachment lightbox. Loading and unavailable states
use the existing attachment cache and relay; visible rows retry failed sources.
The generation tool keeps its ordinary lifecycle presentation.

Generated downloads validate the declared and actual raster MIME type and the
24 MiB encoded-file limit. ImageIO reads dimensions before decoding, rejects
sources exceeding 4096 pixels on either axis, and decodes one static frame at
most 2048 pixels on its longest side off the main actor. The cache accounts for
decoded pixel memory and separates validated generated images from generic
attachment entries. Geometry changes notify the native transcript table.

`GeneratedImageTests` covers metadata decoding, malformed references, row identity
and device/content corrections, source ordering, raster limits, and cache policy
isolation. `TranscriptLayoutTests.testGeneratedImageArrivalResizesTheRowAndKeepsTheTailVisible`
covers a placeholder becoming a loaded image in the native scrolling transcript.
These XCTest cases were added on Linux; they have **not** been run in Xcode or on
an iPhone. Changed Swift files passed a tree-sitter syntax parse and `git diff
--check`; this does not replace Swift type checking or simulator validation.

On a Mac with the project dependencies installed and `SIMULATOR_UDID` set:

```sh
xcodebuild test -project apps/ios/Zeron.xcodeproj -scheme Zeron \
  -destination "platform=iOS Simulator,id=$SIMULATOR_UDID" \
  -only-testing:ZeronTests/GeneratedImageTests \
  -only-testing:ZeronTests/TranscriptLayoutTests
```

For the device check, generate an image on a desktop host and open its chat on
an iPhone. Verify inline display, tap-to-preview, returning to the chat, and scroll
position during loading. Reopen on a client without cached bytes while the host
is offline, then reconnect the host and verify retry recovery. Also check light
and dark appearances and a portrait image whose displayed height reaches the cap.

## Desktop security regressions

`cargo test --locked -p zeron-ui --lib generated_image -- --test-threads=1`
includes policy/alias/load-claim isolation, MIME corrections, actual format
validation, static-frame downsampling, and a 100-image cache-history budget test.
Existing transcript tests verify that generated history is not protected from
eviction, while ordinary user attachment protection remains unchanged.
