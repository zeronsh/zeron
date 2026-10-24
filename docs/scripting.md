# Zeron scripts

A script is one Luau file dropped into Zeron's scripts folder. It reacts to
what your agents do, adds commands to the command palette, shows notices, and
can hand the agent new tools. No build, no restart: save the file and Zeron
reloads it.

Scripts run inside Zeron, next to the engine that drives the agents. They never
see the agent's process, only the stream Zeron already sees: the request going
in, the tool calls and the result coming out. That is also where the power is.
A script can rewrite a prompt before Claude Code gets it, react the moment a
turn ends, and register a tool that the agent calls through Zeron's own MCP
server.

## Where scripts live

| Platform | Folder |
| --- | --- |
| Windows | `%LOCALAPPDATA%\Zeron\scripts` |
| macOS, Linux | `~/.zeron/scripts` |
| `ZERON_DATA_DIR` set | `$ZERON_DATA_DIR/scripts` |

Zeron creates the folder on first start. Settings → Scripts lists every
script with its status, a switch to turn it off without deleting the file,
and Reload and Open folder buttons. The command palette (`mod-k`) has the
same **Reload scripts** and **Open scripts folder** entries.

Every `*.luau` (or `*.lua`) file at the top level of the folder is one script.
Its name is the file name without the extension. Files starting with `.` or
`_` are ignored. Subfolders are not read.

## Your first script

```lua
zeron.on("turn_done", function(turn)
    zeron.notify(string.format("Done in %.0fs", turn.seconds), "success")
end)

zeron.command("Say hi", function()
    zeron.notify("Hi from " .. zeron.script.name)
end)
```

Save it as `hi.luau`. The next finished turn shows a toast, and **Say hi**
appears in the command palette. Edit the file and the script reloads with a
fresh state. Delete it and it is gone.

A script's top level runs once at load. Register handlers there; keep state in
local variables or in `zeron.store` when it should survive a reload.

## API

Everything lives on the `zeron` table.

### `zeron.on(event, handler)`

Listen for an event. Handlers get one table argument (see Events). Several
handlers for the same event run in registration order, across scripts in file
name order.

### `zeron.command(label, handler)`

Add a command to the palette under its label. The handler takes no arguments.

### `zeron.notify(text, level?)`

Show a toast. `level` is `"info"` (default), `"success"` or `"error"`.

### `zeron.tool{ name, description, input, run }`

Give the agent a tool. Zeron advertises it through the MCP server it attaches
to every run, so Claude Code, Codex and the others can call it like any other
tool. The agent reads `description` to decide when to use it, so write it the
way you would brief a colleague.

```lua
zeron.tool {
    name = "remind_me",
    description = "Show the user a reminder after a delay.",
    input = {
        text = { type = "string", description = "What the reminder says" },
        seconds = { type = "number", description = "How long to wait", optional = true },
    },
    run = function(args)
        zeron.after(args.seconds or 60, function()
            zeron.notify(args.text)
        end)
        return "Reminder set"
    end,
}
```

`name` is lowercase letters, digits and underscores. `input` maps argument
names to JSON Schema snippets; a bare string is a shorthand for `{ type = ... }`,
and `optional = true` makes an argument optional. Pass a full schema with
`type = "object"` to use it as is. `run` receives the arguments plus `chat`,
the id of the chat whose agent called it. Return a string for the agent to
read, a table to send as JSON, or nothing for `"done"`. A Lua error becomes a
tool error the agent sees.

Agents list tools once when a session starts, so a tool from a new script is
offered from the next session on.

### `zeron.after(seconds, fn)` and `zeron.every(seconds, fn)`

Run `fn` once after a delay, or repeatedly. Both return a function that
cancels the timer. Timers die with the script on reload.

### `zeron.store.get(key, default?)` and `zeron.store.set(key, value)`

A small key-value store that survives reloads and restarts, saved as JSON per
script under `scripts/.state/`. Values are strings, numbers, booleans and
tables of those. Setting `nil` removes a key.

### `zeron.chats()`

The open chats: a list of `{ id, title, cwd, status }` where `status` is
`"idle"`, `"working"`, `"awaitingInput"` or `"errored"`.

### `zeron.send(chat, text)`

Send a message to a chat by id or exact title. An idle chat starts a turn; a
working chat is steered. A chat waiting on a question raises an error.

### `zeron.script`, `zeron.version`, `print`

`zeron.script.name` and `zeron.script.path` describe the running script.
`zeron.version` is Zeron's version. `print` writes to Zeron's log file in the
`logs` folder next to `scripts`.

## Events

| Event | When | Fields |
| --- | --- | --- |
| `before_run` | A new turn is about to start | `chat`, `prompt`, `model`, `reasoning`, `cwd`, `harness`, `sandbox` |
| `run_start` | The agent process is up | `chat`, `harness`, `model`, `cwd`, `prompt` |
| `tool_call` | The agent invoked a tool | `chat`, `id`, `kind`, plus the tool's own fields |
| `tool_result` | A tool finished | `chat`, `id`, `error`, `output` |
| `turn_done` | The turn ended | `chat`, `status`, `error`, `prompt`, `seconds` |
| `error` | The harness reported an error | `chat`, `message` |
| `input_requested` | The agent asked the user a question | `chat`, `id`, `questions` |
| `unload` | The script is about to be replaced or removed | none |

`before_run` is the one event a script can change. Edit `prompt`, `model` or
`reasoning` on the table and the turn runs with the new values; the other
fields are read-only. Steers into a running turn do not pass through it.

```lua
zeron.on("before_run", function(run)
    if run.cwd:find("work") then
        run.prompt = run.prompt .. "\n\nFollow CONTRIBUTING.md."
        run.reasoning = "high"
    end
end)
```

`tool_call.kind` is one of `exec`, `readFile`, `writeFile`, `editFile`,
`applyPatch`, `search`, `glob`, `webFetch`, `webSearch`, `todo`, `mcp` or
`unknown`. Each kind carries its own fields: `command` for `exec`, `path` for
the file kinds, `pattern` for `search` and `glob`, `url` for `webFetch`,
`query` for `webSearch`, `server` and `tool` for `mcp`, `name` and `input` for
`unknown`.

`turn_done.status` is `"completed"`, `"interrupted"` or `"errored"`.
`harness` values are `claude-code`, `codex`, `cursor`, `devin`, `grok`,
`hermes`, `pi`, `opencode` and `antigravity`. Subagent traffic is not
delivered; only the chat's own turn is.

## Limits

Scripts are sandboxed. There is no file, network or process access, no
`require`, and the standard library is `string`, `table`, `math`, `utf8`,
`bit32`, `os` (time and date only) and `coroutine`. Each script has its own
Lua state with 64 MB of memory. A handler or command may run for 250 ms and a
tool for 1 s; past that it is stopped with an error, so precompute at the top
level and keep handlers short. A script file is at most 256 KB.

One script failing never affects another. A script that fails to load shows
the error as a toast, contributes no commands or tools, and gets a red mark in
Settings → Scripts that opens the full error; fix the file and it loads again.

## Troubleshooting

- A toast prefixed with the script name is that script's last error. The same
  error is not shown twice in a row; check the log for repeats.
- Nothing happens on save: the file must be at the top level of the folder
  with a `.luau` or `.lua` extension, and not start with `.` or `_`.
- A tool does not show up for the agent: start a new session. Tools are listed
  once per session.
- `print` output is in Zeron's log file, under `logs` in the data folder.

## Examples

Ready-to-copy scripts live in [docs/scripting/examples](scripting/examples):
`watchdog.luau` (prompt rules per project, a risky-command guard, turn
stats, two commands and a tool), `remind.luau` (a tool the agent calls to
schedule a notice) and `brief.luau` (a toggle that trims every prompt).

Count turns and report on demand:

```lua
local turns = zeron.store.get("turns", 0)

zeron.on("turn_done", function()
    turns += 1
    zeron.store.set("turns", turns)
end)

zeron.command("Turns so far", function()
    zeron.notify(turns .. " turns")
end)
```

Warn when the agent runs a destructive command:

```lua
zeron.on("tool_call", function(call)
    if call.kind == "exec" and call.command:find("rm %-rf") then
        zeron.notify("rm -rf in " .. call.chat, "error")
    end
end)
```

Chain two chats: when one finishes, brief another:

```lua
zeron.on("turn_done", function(turn)
    if turn.status == "completed" then
        for _, chat in zeron.chats() do
            if chat.title == "Reviewer" then
                zeron.send(chat.id, "A turn just finished in " .. turn.chat .. ". Review the diff.")
            end
        end
    end
end)
```

## How it works

The engine hosts one Luau state per script (`crates/engine/src/scripts.rs`,
via mlua) on a dedicated thread, so a script never blocks a run and the UI
never waits on a script. `Inner::publish` in `sessions.rs` forwards the agent
events a script can observe; `drive_run` sends the request through
`before_run` right after it attaches Zeron's MCP server. The folder is
watched through the shared `FsWatchHub`; a change reloads only the scripts
whose files changed.

The UI learns about scripts over one stream, `WatchScripts`, whose frames are
either a snapshot (folder, scripts, their commands and tools) or a notice.
Palette entries and the settings page call `RunScriptCommand`,
`ReloadScripts` and `SetScriptEnabled`; a switched-off script is remembered
in `scripts/.state/settings.json` and never loaded. The MCP server
appends the engine's `ListScriptTools` to `tools/list` and routes unknown tool
names through `CallScriptTool`. Wire types are in
`crates/proto/src/scripts.rs`; the engine advertises the `scripts-v1`
capability so older viewports skip the stream.
