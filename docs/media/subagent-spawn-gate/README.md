# Subagent spawn gate: demo media

Live captures for "Claude subagents: count and open only this chat's spawns" (#999).

- One prompt, `Run the fanout-bg skill.`, went at the same moment to two headless engines on one Mac, each in a fresh chat on Claude Code 2.1.310 with Haiku: Zeron v0.2.121 (before) and this branch (after). `fanout-bg` is a `context: fork` skill that starts two background agents.
- The frames are the production `Shell` rendered offscreen by gpui (`Window::render_to_image`), attached to each engine over IPC, one frame every 5 s. They are not an OS screen recording. The window is 1298 × 900 with the transcript narrowed to 640 px, so the session card docks beside it.
- `00-before-after.png` and `fanout-bg-live.gif` (5 to 50 s) put the sidebar row and the session card of both runs side by side. `01` to `06` are the same crops 15 s in, plus the transcript chip. `07` and `08` are the full windows 15 s in, at half size.
- The session card's device name is blurred.
