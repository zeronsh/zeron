# Typed fork chip: demo media

Live captures for "Claude subagents: give a typed /skill's fork a chip and a tab".

- One chat on a headless engine built from this branch, Claude Code 2.1.310 with Haiku. The prompt was `/f-slow-steps`, a temporary `context: fork` skill whose fork runs three 20 s `python3` steps in the foreground, one after the other, then replies `steps done`.
- The frames are the production `Shell` rendered offscreen by gpui (`Window::render_to_image`), attached to the engine over IPC, with the fork's tab opened the way its chip opens it. They are not an OS screen recording. The window is 1298 × 900.
- `01-running.png`, 35 s after the prompt: the session card lists the fork under Subagents, running. Its tab holds the skill's prompt and a working trailer, because the CLI holds the fork's transcript until the fork is done.
- `02-done.png`, after the fork: its row is under "1 finished", done, and its tab holds the two thoughts, the three steps (opened by a click) and the reply. The whole fork reached the tab 76 s after the prompt.
- `00-running-done.png` puts the two side by side at half size. `03` and `04` are the full windows at those two moments, at half size.
- The session card's device name is blurred.

The engine and the UI were both built from this branch's code commit, `065e554f`, on v0.2.124.
