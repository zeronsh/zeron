# Ask: a remote picture that fails once is never fetched again

**Who needs it:** every picture Zeron loads from the web. That includes GitHub faces on the pull request board and in the viewer, screenshots in pull request descriptions and comments, and bots' badges. Users see faces stuck on their gradient and screenshots left blank until they reopen the page.

Seen at zgx `c9d85c65` (Zeron's pin).

## What fails today

`remote_status` (`zgx-script/src/lower.rs`) fetches a remote `src` once. If that fetch fails (a dropped connection, a 5xx, a busy server), the status signal is set to `FAILED`. The backoff (5 s, doubling, up to 5 minutes) applies only when something *asks again*. Nothing does: the status signal doesn't change, so nothing re-renders, and re-reading the same `src` string from a reactive function doesn't re-run `load_image` either. The picture stays blank for the life of the node, and of any node that shows that `src` later.

The repro is a Rust test in Zeron, `a_picture_that_fails_to_load_is_asked_for_again` (`apps/zgx/src/plugins/examples_tests.rs`). Its fetcher answers 503 the first time and the picture after that. Without Zeron's workaround, the picture is fetched once and never shows.

## Zeron's workaround (to remove once this lands)

`ui/ds/image_guard.luau` (`live`, `watch`, `probe`, `mark`): a remote picture that hasn't arrived after 6 s is asked for again, alternately as itself and with its host in capitals. Hostnames are case-insensitive, so it's the same picture, but zgx sees a new source string. It retries up to 5 times, doubling the wait. A hidden copy of the picture, laid out at its own size, tells it when the picture has arrived.

## The ask

- (a) After a failed remote fetch, zgx asks again by itself once the backoff passes (a timer that sets the status back to `LOADING` and refetches), for as long as a node still shows that `src`.
- (b) A script can see whether a picture failed or arrived (`on_load` / `on_error` on `ui.image`, as `ui.web` has), so a caller can show a retry or say it couldn't load.
