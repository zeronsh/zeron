# Ask: lend a host package to a plugin

**Priority: medium-low.** First-party plugins (GitHub) redraw Zeron's components from primitives, so they drift from the app's look and the active template's.

**Today.** `packages(plugin = true)` returns none.

**Ask:** `Capability::Package(name)`, with `packages(plugin)` returning the packages a plugin's grants name: read-only modules run in the plugin's VM as usual. Zeron would lend the running app's `@ds` (from its template's folder).

**Seen on:** zgx `b3371f8` (Zeron's pin), macOS. From the plugin chat's notes.
