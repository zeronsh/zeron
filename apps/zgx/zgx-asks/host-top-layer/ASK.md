# Ask: a host-owned top layer no script can cover

**Priority: high (security).** A template in preview, or a plugin, can paint over the controls the person must decide on.

**Seen on:** zgx `f81d5e99` (Zeron's pin), macOS. Lane: zgx security follow-up (0.1.10), shared with the plugin-isolation ask (review R8(d)).

**Today.** Zeron draws its own strip above the scripts' view (src/plugins/host_bar.rs): "Agent template · Keep… / Revert" while a template runs as a preview, the notice of a switch the person didn't make, and on Linux and Windows the menu bar with View → Use Zeron's Template. The strip sits in the window's layout (`column[strip, script view]` at `cx.render`), outside anything the scripts build, so a template can't remove it. But `ui.modal` and `ui.layer` mount as portals at the window's top, so a script can still paint over it and take its clicks.

**Ask:** a view the host mounts that paints and hit-tests above everything the scripts draw (modals, layers, fixed and absolute nodes, popups), e.g. `cx.render_host_layer(view)` or `ScriptHost::host_layer(view)`. Or confirm, with a test, that composing at `cx.render` already holds.

**Zeron, the day it lands:** mount `host_bar::view` there instead of above the view, and add a test that a template's full-window `ui.modal` can't cover Keep / Revert.
