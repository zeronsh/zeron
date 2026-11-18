# Ask: keep a private element's reactive regions out of plugins' reach

**Priority: high (it breaks a promise the docs make).** `private = true` / `hook = false` says "a plugin's target never matches inside one, however the inside builds (a scroll's rows, a component mounting later)" (docs/design/plugin-hooks.md). It holds for parts built bare, and not for parts a `ui.presence`, a `ui.show` or a `ui.each` builds inside the private element: a plugin replaces them and adds beside them.

**Seen on:** zgx `b80a8af1` (Zeron's pin), macOS, headless.

**Who sees it.** Zeron marks a plugin's Settings page private (screens/shell/settings sets `hook = false` round a `private` page) so that no plugin draws into another plugin's settings. The page's rows are built in a `ui.presence` (keyed by what the plugin declares). A plugin allowed `ui.change` on `#setting-show` replaces another plugin's "Show" row on that page, and one allowed `ui.add` adds anything after any row there. Zeron's other private pages (Cloud, Billing) build parts in `ui.show` and `ui.each` too. Found while testing Zeron's plugin-drawn settings pages (src/plugins/settings_page_tests.rs).

**Today.**

| part of a `hook = false` column | replaced / added after by a plugin |
|---|---|
| built bare | no / no |
| built by a `ui.presence` branch | yes / yes |
| built by a `ui.show` | yes / yes |
| a `ui.each` row | yes / yes |

**Ask:** what a private element's regions build (presence branches, shows, each/list rows, as they build and rebuild later) is lowered sealed, as its direct children and the components inside it are (`sealed_component` in lower.rs carries the count into a component mounting later; the regions' builders don't get it). Or say so in the docs, so apps reach for `ui.sealed`, which holds.

**Repro:** `ZGX_CHECK=0 zgx test zgx-asks/private-reactive-regions/repro`. A `hook = false` column with a part built bare and one built by each region, and a plugin (`plugins/reach`, granted `override:*` in `zgx.json`) replacing each part and adding after it. It fails at `in-presence` (and, past it, `in-show` and `in-each-r1`).

**Meanwhile (Zeron):** nothing changed; Zeron's pages are as they were. (`ui.sealed` holds, but it draws what it seals in a top layer and confines a plugin's page in it, so it isn't the fix for these pages.)
