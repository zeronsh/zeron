# Fork settles copied subagents: demo media

Live captures for "Fork: settle the running subagent chips a fork copies" (#1001).

- The same scenario ran on two headless engines on one Mac, on Claude Code 2.1.310 with Haiku: Zeron v0.2.121 (before) and this branch (after). A chat starts one background `Agent` that runs `sleep 70`. Right after its turn ends, the chat is forked (`ForkSideChat`).
- Before: the fork, captured about an hour later, still lists "Sleep for 70 seconds" as running, and its chip still spins.
- After: the fork's copy reads `failed` as soon as `ForkSideChat` returns, and stays settled. The source keeps its own chip, which went from running to done once the subagent finished.
- The frames are the production `Shell` rendered offscreen by gpui (`Window::render_to_image`), attached to each engine over IPC. They are not an OS screen recording.
- `00-before-after.png` puts the two forks' session cards side by side. `01` to `04` are the cards and the transcript chips at full resolution. `05` and `06` are the full windows, at half size.
- The session card's device name is blurred.
