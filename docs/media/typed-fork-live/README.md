# Typed fork live tab: demo media

Live captures for "Claude subagents: fill a typed fork's tab while the CLI holds it".

- The same prompt, `/f-slow-steps`, went at the same moment to two headless engines on one Mac, each in a fresh chat on Claude Code 2.1.310 with Haiku: one built from this PR's base, which gives the fork its chip and tab (before), and one from this branch (after). `/f-slow-steps` is a temporary `context: fork` skill whose fork runs three 20 s `python3` steps in the foreground, one after the other, then replies `steps done`.
- The frames are the production `Shell` rendered offscreen by gpui (`Window::render_to_image`), attached to each engine over IPC, one frame every 2 s, with the fork's tab opened the way its chip opens it. They are not an OS screen recording. The window is 1298 × 900.
- `00-before-after.png` puts the two tabs side by side 35 s after the prompt. The base's holds the skill's prompt and a working trailer. This branch's holds the fork's first thought, its first line of text and step 1, whose result came at 32 s. `01` and `02` are those tabs at full resolution, `03` and `04` the full windows at half size.
- `typed-fork-live.gif` is this branch's tab from 6 to 93 s, each frame captioned with its time after the prompt. The fork's first step reached it at 11 s, and the fork was done at 81 s. The base's tab got the whole fork at once, at 76 s.
- The session card's device name is blurred.

Each engine and its UI were built from that branch's code commit: `e535bd26` for this branch, `065e554f` for the base, both on v0.2.124.
