# Ask: let `on_click` keep an `href` from opening

**Priority: medium.** Zeron's "Open links in Zeron" setting routes a chat's link into its own browser panel; today the span has to drop `href`, losing the link's cursor, context menu and role.

**Today.** A `ui.span { href = … }` opens with the system handler after `on_click`, whatever `on_click` returns (repro: the handler runs, then the browser opens too).

**Repro:** `zgx run repro` (`ZGX_CHECK=0`).

**Ask:** `on_click` returning `false` keeps the href from opening (the span stays a link otherwise), as the DOM's `preventDefault`; or a span-level `open_links = false`, or a host hook `ScriptHost::open_link(url) -> bool`.

**Seen on:** zgx `b3371f8` (Zeron's pin), macOS. From the browser chat's notes.
