# Ask: a modal opening closes anchored popups, and never draws under one

**Priority: medium (worked around in Zeron for its own popups).** Open the composer's Work in dropdown, then press ⌘K (or anything that opens the palette, a dialog or the lightbox). The dropdown stays open and draws over the palette's device list.

**Seen on:** zgx `a2c9cf1` (Zeron's pin), macOS.

**Why:** the dropdown reached past the window's bottom edge, so zgx showed it in a popup window of its own (`Scene::set_popup`). A popup window sits above the main window, so nothing in the main window can draw over it, `ui.modal` included. Also, nothing closes the popup when a modal opens: `ui.modal` leaves anchored popovers and menus open. `Ui::dismiss_popups` closes only those shown in popup windows, and it runs only on app or window focus loss.

**Ask:** as AppKit does when a sheet or panel comes up:

1. A `ui.modal` opening closes the open anchored popups (popovers, menus, submenus; the `Ui::popups` set, in-window and popup-window alike) that aren't inside the modal itself.
2. If a popup has to stay open under a modal, the modal draws above it, e.g. by showing the modal in a popup window above it. A popup window must never cover a modal opened after it.

**Zeron meanwhile:** ds/popups. `glass.style` registers every ds popup's `open` state. The palette, dialog and lightbox wrap their props in `Popups.modal`, which closes those popups as the modal opens and calls the host's `popups.dismiss` (`Ui::dismiss_popups`) for zgx's own popup windows. A zgx-built popup that isn't in a popup window (zgx's own in-window menus) can't be closed from a host. (1) would replace all of this.

**Repro:** `cargo test --bin zeron-zgx popups_tests` in apps/zgx: Work in open, then `shell.palette`. With ds/popups' registration off, the panel stays open under the palette.
