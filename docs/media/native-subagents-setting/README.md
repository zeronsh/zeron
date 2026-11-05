# Native subagents setting: demo media

Captures for "Providers: a switch to turn native subagents off" (#1003).

- `01` and `02`: Settings → Providers → Claude Code on a headless engine built from this branch, before and after a click on the switch. After the click the engine answered `GetNativeSubagents` with `{"off": ["claude-code"], "switchable": ["claude-code"]}` and wrote `"nativeSubagentsOff": ["claude-code"]` to its `harness-prefs.json`. `00-setting.png` stacks the two at half size.
- `03`: the session card of a chat on that engine, Claude Code 2.1.310 with Haiku. Its first turn, with the switch off, asked for a subagent; its Claude process ran with `--disallowedTools=Agent,Task,Workflow`, the agent delegated through Zeron's `create_chat`, and the side chat "ls entry count" is the result. Its second turn, after the switch went back on, ran in a new process without the flag and used Claude's own `Agent` tool: "1 finished" under Subagents.
- The frames are the production `Shell` rendered offscreen by gpui (`Window::render_to_image`), attached to the engine over IPC. They are not an OS screen recording.
- The crops stop above the provider's Accounts section, and the session card's device name is blurred.

The engine and the UI were both built from this branch's code commit, `f3e9d09a`, on v0.2.124. Only Claude Code is switchable: Codex's details have no Delegation section.
