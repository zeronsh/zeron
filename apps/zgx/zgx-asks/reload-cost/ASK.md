# Ask: a reload that compiles only what changed, and only when bytes change

**Priority: high for anyone developing an app** (Zeron's developers reload all day: every save in `ds/`, `screens/` or `account/` reloads the whole window, a 50-70% CPU burst each). No effect on shipped bundles.

**Today.**
1. **A file rewritten with the same bytes reloads the app.** `ScriptHost::stamps` compares modification times only, so `touch`, an editor's save without changes, a formatter that changes nothing, or a tool copying a file back all reload.
2. **Each reload compiles every module again**, not just the one that changed: in the repro, 195 of 253 samples (77%) in `reload_if_changed` are `luau_compile` for unchanged modules. Resolving every `require` again (`realpath`, `getattrlist`) is a few percent more.

In Zeron's main window (about 170 modules, a long chat open) one reload is about 65 ms in `ScriptHost::reload`, half of it compiling, then the rebuild frame (about 850 M instructions: 90 ms on a free core, 350 ms on a busy one), then about 23 ms in `AccessibilityTree::update`.

**Repro:** `repro/run.sh /path/to/zgx-run` (`make_app.py` writes a 150-module app; `run.sh` counts reloads per case and samples 10 reloads):

    1 reload(s)  touch mods/m7.luau (same bytes)
    0 reload(s)  a new file in mods/ (not required)
    1 reload(s)  control: m7.luau's bytes change
    sampled: reload_if_changed 253 samples, luau_compile 195 of them

**Ask:** on a changed mtime, compare a content hash before reloading; cache bytecode by path and content hash so a reload compiles only what changed; cache require resolution within a reload. (An incremental accessibility update after a reload would take the last 23 ms.)

**Seen on:** zgx `b3371f8` (Zeron's pin), macOS.
