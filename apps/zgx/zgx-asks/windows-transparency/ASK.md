# Ask: report Windows' Transparency effects as `reduced_transparency`

**Priority: high.** Users see it. With Windows' *Transparency effects* off
(Settings → Personalization → Colors), an app's popovers, menus, tooltips
and toasts still draw translucent: the model picker over a new chat's
wallpaper shows the picture through it. macOS's Reduce transparency is
honoured; Windows' switch isn't seen at all.

**Today.** `system_colors::preferences` (`crates/zgx-desktop/src/
system_colors.rs`) reads macOS's
`accessibilityDisplayShouldReduceTransparency`, but on Windows (and Linux)
it returns `reduced_transparency: false` whatever the system says. So
`ui.prefers.reduced_transparency` is always false there, and
`preference_tokens`' opaque glass (`glass.tint` → `surface.raised`,
`glass.blur` 0) is macOS-only too.

**Repro** (`zgx run repro/main.luau`, on Windows): it shows
`ui.prefers.reduced_transparency` and a frosted card. Turn Transparency
effects off: it still says `false`, and the card stays translucent.

**Ask:**

1. On Windows, read `EnableTransparency` (DWORD; 0 is off) under
   `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`
   (or WinRT `UISettings::AdvancedEffectsEnabled`) as `reduced_transparency`,
   and read it again when it changes. The caption subclass already wakes on
   `WM_SETTINGCHANGE` (`settings_changed` → `system_colors::sync`); Windows
   sends it with `"ImmersiveColorSet"` when the switch flips.
2. Apply the same opaque-glass tokens there as on macOS.
3. (Linux, while there: GNOME has no such switch; KDE's is per-theme.
   Nothing to do unless a desktop reports one.)

**Zeron meanwhile:** `apps/zgx/src/transparency.rs` reads the same value
and waits on the key (`RegNotifyChangeKeyValue`), as the shared value
`system.reduced_transparency`; `ui/ds/material.luau` `see_through()` reads
it with `ui.prefers.reduced_transparency` and Zeron's own Frosted/Opaque
setting. Once zgx reports it, the host's watcher and the shared value go,
and `see_through()` reads `ui.prefers` alone.

**Seen on:** zgx `99a50ad5`, unchanged at `e46ff6fc` (Zeron's pin); Windows 11 26200.
