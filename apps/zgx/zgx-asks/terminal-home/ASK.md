# Ask: `ui.term { cwd = "~" }` on Windows

**Priority: medium.** On Windows every terminal Zeron opened failed to start
("couldn't start: The directory name is invalid"): the panel's terminal tabs
and an agent's sign-in terminal alike.

**Today.** `ui.term`'s options (zgx-script/src/term.rs:184) expand a leading
`~` from `HOME` alone. Windows has no `HOME` (its home is `USERPROFILE`), so
`cwd = "~"` stays the literal path `~`, which `CreatePseudoConsole`'s
`CreateProcessW` refuses as the working directory. Zeron now asks its own
host for the folder on Windows (apps/zgx/src/terminal.rs `bridge_dir`,
`ui.call_sync("terminal.cwd")`) and passes the profile folder itself.

**Repro:** on Windows, `zgx run` a script with
`ui.terminal { term = ui.term("t", { cwd = "~" }), grow = 1 }`: the terminal
shows the error instead of PowerShell.

**Ask:** expand `~` from the platform's home folder: `USERPROFILE` on Windows
(or `std::env::home_dir`, which reads it there), `HOME` elsewhere. Zeron would
drop `terminal.cwd` and pass `"~"` again.

**Seen on:** zgx `99a50ad5` and `e46ff6fc` (Zeron's pin), Windows 11 26200.
