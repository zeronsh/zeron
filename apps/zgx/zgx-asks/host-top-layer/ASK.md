# Ask: a host-owned top layer no script can cover

**Priority: high (security).** A template in preview, or a plugin, can paint over the controls the person must decide on.

**Seen on:** zgx `f81d5e99` (Zeron's pin), macOS. Lane: zgx security follow-up (0.1.10), shared with the plugin-isolation ask (review R8(d)).

**Today.** Zeron draws its own toasts in the window's bottom-right corner (src/plugins/host_bar.rs): "Previewing <name>" with Keep and Revert while a template runs as a preview, a switch the person didn't make, signing keys, safe mode at launch; and on Linux and Windows the menu bar with View → Use Default Interface. The toasts are a host `layer` portal at the window's root, which re-raises itself to the root's last child after every layout (`keep_on_top`): a script's `ui.modal` or `ui.layer` opened later raises itself, which lays the root out again, and the host's goes back over it (`a_script_s_modal_or_layer_can_t_cover_the_toast_or_take_its_clicks`). That holds within the window. An anchored popup (popover, menu) on macOS and Windows is a window of its own over the main one, so a script's can still cover the toasts.

**Ask:** a view the host mounts that paints and hit-tests above everything the scripts draw (modals, layers, fixed and absolute nodes, popups), e.g. `cx.render_host_layer(view)` or `ScriptHost::host_layer(view)`. Or confirm, with a test, that composing at `cx.render` already holds.

**Zeron, the day it lands:** mount `host_bar`'s toasts there, drop `keep_on_top`, and extend the test to a script's anchored popover over the corner.
