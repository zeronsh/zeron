# Forward subagent text: demo media

Live captures for "Claude subagents: forward a foreground subagent's text and thinking" (#1000).

- The same prompt went to two headless engines on one Mac, each in a fresh chat on Claude Code 2.1.310 with Haiku: Zeron v0.2.121 (before) and this branch (after). It asks for one foreground `Agent` call whose prompt is: write "The sky is blue.", run `echo hi`, write "The grass is green.". Both runs named the subagent "Sky, echo, grass sequence".
- The frames are the production `Shell` rendered offscreen by gpui (`Window::render_to_image`), attached to each engine over IPC, with the subagent's tab opened as its chip opens it. They are not an OS screen recording.
- Both tabs show the subagent's prompt twice. That is the separate bug #952 fixes.
- `00-before-after.png` puts the two tabs side by side. `01` and `02` are the tabs at full resolution. `03` and `04` are the full windows, at half size.
- The session card's device name is blurred.
