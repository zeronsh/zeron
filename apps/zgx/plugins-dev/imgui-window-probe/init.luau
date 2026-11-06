--!strict
-- A development probe for the native window host: the window ops alone,
-- without the SDK or the frame pump, so a shell probe can watch the real
-- windows behind them. Every line starts with "probe:" for the probe to
-- read back; the windows are titled "Probe A" and "Probe B".
--
-- Run it with the app on the gallery screen:
--   ZERON_ZGX_DEV_PLUGINS=<this folder> target/debug/zeron-zgx.exe gallery

local id = "imgui-window-probe"
local action = `plugin.{id}.imgui`

local function say(what: string)
	print(`probe: {what}`)
end

local function call(args: any): any
	local ok, answer = pcall(ui.call_sync, action, args)
	if not ok then
		say(`unreachable: {tostring(answer)}`)
		return { ok = false, error = tostring(answer) }
	end
	return answer or { ok = false, error = "no answer" }
end

local function report(what: string, answer: any)
	say(`{what}: ok={tostring(answer.ok)} open={tostring(answer.open)} error={tostring(answer.error)}`)
end

local a = call({ op = "open_window", id = "a", title = "Probe A", size = { width = 720, height = 480 } })
local b = call({ op = "open_window", id = "b", title = "Probe B", size = { width = 300, height = 220 } })
report("opened a", a)
report("opened b", b)

-- The five refusals: each names the field it is about, and no window.
local invalid = {
	{ name = "size", args = { op = "open_window", id = "bad-size", title = "x", size = { width = "big", height = 480 } } },
	{ name = "negative", args = { op = "open_window", id = "bad-negative", title = "x", size = { width = -10, height = 480 } } },
	{ name = "position", args = { op = "open_window", id = "bad-position", title = "x", position = { x = 100 } } },
	{ name = "title", args = { op = "open_window", id = "bad-title", title = 42 } },
	{ name = "id", args = { op = "open_window", id = "", title = "x" } },
}
for _, case in invalid do
	local answer = call(case.args)
	say(`invalid {case.name}: ok={tostring(answer.ok)} error={tostring(answer.error)}`)
end

local function at(ms: number, run: () -> ())
	ui.after(ms, run)
end

-- Hidden is not closed: is_open stays true while the window is gone.
at(1500, function()
	report("hide a", call({ op = "hide", window = "a" }))
	report("a after hide", call({ op = "is_open", window = "a" }))
end)
at(2500, function()
	report("show a", call({ op = "show", window = "a" }))
	report("state a", call({ op = "state", window = "a" }))
end)
at(2800, function()
	report("focus a", call({ op = "focus", window = "a" }))
end)
-- The title and size change on the live window.
at(3500, function()
	report("rename b", call({ op = "set_title", window = "b", title = "Renamed Probe" }))
	report("resize b", call({ op = "set_size", window = "b", size = { width = 1000, height = 640 } }))
	report("state b", call({ op = "state", window = "b" }))
end)
at(4500, function()
	report("minimize a", call({ op = "minimize", window = "a" }))
end)
at(5200, function()
	report("restore a", call({ op = "restore", window = "a" }))
	report("state a", call({ op = "state", window = "a" }))
end)
-- Closing B from the plugin: the retained handle is closed and refuses
-- every control; a fresh window then opens fine.
at(6000, function()
	report("close b", call({ op = "close", window = "b" }))
	report("b after close", call({ op = "is_open", window = "b" }))
	report("stale show b", call({ op = "show", window = "b" }))
	report("stale title b", call({ op = "set_title", window = "b", title = "again" }))
	report("opened c", call({ op = "open_window", id = "c", title = "Probe C", size = { width = 400, height = 300 } }))
end)
at(7000, function()
	report("state c", call({ op = "state", window = "c" }))
end)

-- Each window draws its own frames: asking a shown window for one draws it,
-- and a hidden one draws nothing until it is shown again.
local function frames(of: string): number
	local state = call({ op = "state", window = of })
	return tonumber(state.frames) or -1
end

at(7200, function()
	call({ op = "invalidate", window = "a" })
	call({ op = "invalidate", window = "c" })
end)
at(7900, function()
	say(`frames drawn: a={frames("a")} c={frames("c")}`)
end)
at(8100, function()
	report("hide c", call({ op = "hide", window = "c" }))
	call({ op = "invalidate", window = "a" })
	call({ op = "invalidate", window = "c" })
end)
at(8900, function()
	say(`frames hidden: a={frames("a")} c={frames("c")}`)
end)
at(9100, function()
	report("show c", call({ op = "show", window = "c" }))
	call({ op = "invalidate", window = "c" })
end)
at(9800, function()
	say(`frames resumed: c={frames("c")}`)
end)

-- With every window closed the plugin is still loaded: this timer is the
-- main window's, and keeps counting.
local beats = 0
ui.every(700, function()
	beats += 1
	say(`alive {beats}`)
end)
