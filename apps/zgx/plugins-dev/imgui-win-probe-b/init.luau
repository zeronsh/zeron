--!strict
-- The validation contract's second plugin (apps/zgx/plugins-dev/imgui-win-probe-b).
--
-- It opens a window with the *same* id the main probe uses (`inspector`) at
-- a different size and position, so `VAL-WIN-006` and `VAL-LIFE-009` can see
-- that a window id is scoped to the plugin that opened it: the two windows
-- are two native windows, and closing, hiding or resizing one leaves the
-- other untouched. Its own counter and frame count let a probe see it keeps
-- rendering while the other plugin's window closes.
--
-- Run it beside the main probe (the gallery needs no engine):
--   ZERON_ZGX_DEV_PLUGINS="<probe>;<probe-b>" ZERON_ZGX_PLUGIN_LOG=1 \
--     target/debug/zeron-zgx.exe gallery
-- (`:` separates the folders on macOS and Linux, `;` on Windows.)

local zeron = require("./zeron")("imgui-win-probe-b")

local id = "imgui-win-probe-b"
local frames = 0
local counter = 0
local closes = 0
local last_action = ""
local window: any = nil

local function say(what: string)
	print(`probe-b: {what}`)
end

local function draw(view: any)
	frames += 1
	view.text("window inspector")
	view.text("plugin B")
	view.text(`frame {frames}`)
	view.text(`clicks {counter}`)
	if view.button("Ping") then
		counter += 1
		say(`clicked:{counter}`)
	end
end

local function open(titled: string, width: number, height: number)
	local handle: any = nil
	local ok, why = pcall(function()
		handle = zeron.imgui.open_window {
			id = "inspector",
			title = titled,
			size = { width = width, height = height },
			-- B's own place: far enough from A's inspector that the two can
			-- never be mistaken for one window sharing geometry.
			position = { x = 500, y = 300 },
			-- `VAL-LIFE-009`: each plugin's inspector comes back at its own
			-- size after a restart.
			remember = true,
		}
	end)
	if not ok or handle == nil then
		say(`open refused: {tostring(why)}`)
		return nil
	end
	window = handle
	handle:on_render(draw)
	handle:on_close(function(closed: string)
		closes += 1
		window = nil
		say(`closed:closecall {closed}`)
	end)
	return handle
end

open("B Inspector", 700, 500)

-- Reopening waits for the host's close to finish (a native window leaves its
-- map on the next loop turn). A timer made inside a tool's task dies with the
-- task, so this script's own timer drains the queue.
local pending: { () -> () } = {}

local function later(run: () -> ())
	table.insert(pending, run)
end

ui.every(100, function()
	if #pending == 0 then
		return
	end
	local todo = pending
	pending = {}
	for _, run in todo do
		local ok, why = pcall(run)
		if not ok then
			say(`deferred failed: {why}`)
		end
	end
end)

local actions: { [string]: () -> () } = {}

-- `VAL-WIN-006`: closing B's window is B's own action; A's window is a
-- different window with the same id.
actions.close_inspector = function()
	local handle = window
	if handle then
		pcall(function()
			handle:close()
		end)
	end
end

actions.reopen = function()
	actions.close_inspector()
	later(function()
		if window == nil then
			open("B Inspector", 700, 500)
		end
	end)
end

-- `VAL-WIN-006`'s second run: both windows titled `Inspector`, so no title
-- distinguishes them and only the window ids' scoping can.
actions.retitle_inspector = function()
	local handle = window
	if handle then
		handle:set_title("Inspector")
	end
end

actions.retitle_b = function()
	local handle = window
	if handle then
		handle:set_title("B Inspector")
	end
end

-- `VAL-LIFE-009` names 1100x800 for B's inspector.
actions.resize_1100 = function()
	local handle = window
	if handle then
		handle:set_size({ width = 1100, height = 800 })
	end
end

actions.bump = function()
	counter += 1
end

local function run_action(action: string)
	local one = actions[action]
	if not one then
		error(`imgui-win-probe-b has no probe action "{action}"`, 2)
	end
	last_action = action
	one()
end

local function menu(action: string, label: string)
	zeron.menu {
		kind = "session",
		id = `probe-b-{action}`,
		label = `Probe B: {label}`,
		run = function()
			run_action(action)
		end,
	}
end

menu("close_inspector", "close inspector")
menu("reopen", "reopen inspector")
menu("retitle_inspector", "inspector titled Inspector")
menu("retitle_b", "inspector titled B Inspector")
menu("resize_1100", "inspector 1100x800")
menu("bump", "bump counter")

zeron.tool {
	name = "probe_status",
	run = function(): any
		local open_now = window ~= nil
		if window then
			local ok, value = pcall(function()
				return window:is_open()
			end)
			open_now = ok and value == true
		end
		return {
			plugin = id,
			version = "1",
			windows = if open_now then { "inspector" } else {},
			open = if open_now then 1 else 0,
			frames = frames,
			counter = counter,
			closed = closes,
			last_action = last_action,
		}
	end,
}

zeron.tool {
	name = "probe_run",
	run = function(args: any): any
		local action = if type(args) == "table" and type(args.action) == "string" then args.action else ""
		run_action(action)
		return { action = action, ok = true }
	end,
}
