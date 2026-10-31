# Background process screenshots

Real Linux desktop application captures taken on 2026-10-28 from this branch, built with `cargo build -p zeron`. The application ran in Xvfb (Mesa lavapipe software Vulkan) at a 1280 × 800 window size against a separate local engine with temporary data directories, one empty demo project, and the mock harness with `ZERON_MOCK_BACKGROUND=1` (its turn leaves three scripted processes running, which print output and honor Stop). Input was driven with `xdotool`; images were captured from the X display with ImageMagick `import`. They are unedited screenshots, not mockups, and are not bundled into the application.

- `collapsed.png`: after the turn, the composer's **Background** tray says three processes are running and lists their commands. Collapsed is the default.
- `expanded.png`: one row per process: the command, the agent's description of it, how long it has run, and its Stop button.
- `output.png`: clicking a row opens the tail of that process's output, here the doc-test watcher's last run.
- `stopping.png`: Stop on the log watcher: the row dims and reads **Stopping…** until the host reports it gone.
- `stopped.png`: the process has left the list (the count reads 2); the other two keep running.
