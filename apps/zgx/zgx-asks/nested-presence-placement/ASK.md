# Ask: `replace` at a `ui.presence` branch's root

**Priority: medium.** A plugin's `ui.change` (a `replace` contribution) silently does nothing when its target is the root a `ui.presence` builder returns. Zeron's Parallax scenes plugin stopped drawing a new chat's background when the wallpaper's art moved into a presence of its own. A cousin of [show-root-placement](../show-root-placement/ASK.md), which fixed `before`/`after` at presence roots (#1264, #1291); `replace` still misses.

**Seen on:** zgx `f6506b6e6` (Zeron's pin), macOS.

**Today.** `ui.contribute { target = "#slot", replace = … }` replaces `#slot` when it's built bare, or as a child inside a presence branch (at any depth of nesting), but not when the presence builder returns `#slot` itself. No error; `zgx inspect contributions` lists the target, matching nothing. (The folder's name is the symptom as first seen, a slot two presences deep: the bisect showed nesting isn't the cause, being the branch's root is.)

**Bisect (Zeron, ds/wallpaper, Parallax scenes test):**

| `#new-chat-background` built as | Replaced |
|---|---|
| a child of the presence branch's root (Zeron before) | yes |
| the root of an inner presence's branch | no |
| the same, the inner presence without enter/exit | no |
| a child of a plain div inside the branch | yes |

**Ask:** match a `replace` at a presence branch's root, as `before`/`after` now are, and draw the replacement in its place as the presence keys change (each new branch replaced as it's built). Or refuse it with an error naming the target.

**Repro:** `ZGX_CHECK=0 zgx test zgx-asks/nested-presence-placement/repro`. An app with five slots (bare; a presence root; a child in a presence; both again inside a nested presence) and a plugin replacing each (`plugins/swap`, granted in `zgx.json`). It fails at `slot-root`: the tree shows `#slot-root` and `#slot-nested-root` still the app's, the other three swapped.

**Meanwhile (Zeron):** ds/wallpaper builds the plugin's place as a child inside its branch, never a branch's root (and `ui/tests/wallpaper.test.luau`, with the Parallax scenes test in src/plugins/examples_tests.rs, keeps it so).
