--!strict
-- The validation contract's main probe (apps/zgx/plugins-dev/imgui-win-probe).
--
-- It opens the native windows the contract's window, lifecycle, input and
-- docking rows name, by id and with the options each row measures, and it
-- offers one probe action per row: the menu items below, and the same
-- actions through the `probe_run` tool for a scripted probe that cannot
-- click a menu.
--
-- The host allows eight native windows per plugin (contract §5). Four open
-- at launch — Probe A, Probe B, the inspector and the bare window — leaving
-- four slots free, so every other window is one action away and opens with a
-- plain `open_window` and no close first (`open_bounds`, `open_drag`,
-- `open_quiet`, `open_left`, `open_right`, `open_both_sides`, `open_dup`,
-- `open_reuse`). README.md maps every row to the window and action it uses.
--
-- What the probe reports about itself, without needing a window: the
-- `probe_status` tool returns its own JSON (windows, frames, counters,
-- closes, timer ticks, store keys). The timer keeps changing even with no
-- window open, so the lifecycle rows can see the plugin still working.
--
-- Run it (the gallery needs no engine and no sign-in):
--   ZERON_ZGX_DEV_PLUGINS=<this folder> ZERON_ZGX_PLUGIN_LOG=1 \
--     target/debug/zeron-zgx.exe gallery

local zeron = require("./zeron")("imgui-win-probe")

local id = "imgui-win-probe"
local ticks = 0
local blow_up = false
local last_refusal = ""
local last_action = ""

-- Everything the probe says about itself is keyed by window id, the way the
-- contract's rows read it: a counter and a frame count per window, never one
-- global value (`VAL-WIN-016`, `VAL-WIN-017`).
local frames: { [string]: number } = {}
local counters: { [string]: number } = {}
local closes: { [string]: number } = {}
local live: { [string]: any } = {}

local function say(what: string)
	print(`probe: {what}`)
end

local function copy(map: { [string]: number }): { [string]: number }
	local out: { [string]: number } = {}
	for key, value in map do
		out[key] = value
	end
	return out
end

local function open_ids(): { string }
	local ids: { string } = {}
	for window in live do
		table.insert(ids, window)
	end
	table.sort(ids)
	return ids
end

local function shown(window: string): boolean
	local handle = live[window]
	if not handle then
		return false
	end
	local ok, value = pcall(function()
		return handle:is_open()
	end)
	return ok and value == true
end

-- Only the ids that still read open: a close the plugin asked for may reach
-- its own close callback a frame later, and `probe_status` must not count it
-- meanwhile.
local function live_ids(): { string }
	local ids: { string } = {}
	for window in live do
		if shown(window) then
			table.insert(ids, window)
		end
	end
	table.sort(ids)
	return ids
end

local function other_open(closed: string): any
	local ids = open_ids()
	for _, window in ids do
		if window ~= closed then
			return live[window]
		end
	end
	return nil
end

-- Every window draws the same probe body: its own id, whether it still
-- reads open, its frame count, its own click counter, the timer's value and
-- a Ping button that increments that window's counter alone. That is what
-- `VAL-WIN-016`, `VAL-WIN-017` and the lifecycle rows read.
local function draw(window: string, view: any)
	frames[window] = (frames[window] or 0) + 1
	if blow_up then
		-- `VAL-LIFE-011`: the host must close the windows and keep Zeron up.
		error("probe render failure")
	end
	view.text(`window {window}`)
	view.text(`open {tostring(shown(window))}`)
	view.text(`frame {frames[window]}`)
	view.text(`clicks {counters[window] or 0}`)
	view.text(`tick {ticks}`)
	if view.button("Ping") then
		counters[window] = (counters[window] or 0) + 1
		say(`clicked:{window}:{counters[window]}`)
	end
end

-- Opens one window and wires its render and close callbacks. A refusal is
-- kept (`last_refusal`, `probe_status`) and said, never raised: the row that
-- asks for a window the host refuses must see the sentence, not a broken
-- plugin.
local function open(window: string, title: string, options: { [string]: any }): any
	options.id = window
	options.title = title
	local handle: any = nil
	local ok, why = pcall(function()
		handle = zeron.imgui.open_window(options)
	end)
	if not ok or handle == nil then
		last_refusal = tostring(why)
		say(`open:{window} refused: {last_refusal}`)
		return nil
	end
	live[window] = handle
	counters[window] = counters[window] or 0
	frames[window] = frames[window] or 0
	handle:on_render(function(view: any)
		draw(window, view)
	end)
	-- Runs once per close, from the plugin's own close() or the OS chrome.
	-- It logs the line `VAL-WIN-013` counts and retitles a sibling so a
	-- probe can see the callback did its work, not just that it ran.
	handle:on_close(function(closed: string)
		closes[closed] = (closes[closed] or 0) + 1
		live[closed] = nil
		say(`closed:closecall {closed}`)
		local sibling = other_open(closed)
		if sibling then
			pcall(function()
				sibling:set_title("closed 1")
			end)
		end
	end)
	say(`opened:{window}:{title}`)
	return handle
end

local function close_window(window: string)
	local handle = live[window]
	if handle and shown(window) then
		pcall(function()
			handle:close()
		end)
	end
end

-- The four the contract needs at once: `VAL-WIN-001` (inspector, 720x480),
-- `VAL-WIN-005`/`VAL-WIN-016`/`VAL-WIN-017` (a and b, distinct windows) and
-- `VAL-WIN-019` (bare, the plugin's own chrome). Four slots stay free, so
-- the rest open one action away with no close first. `remember` is on only
-- where a row measures restored geometry: inspector, left and right.
open("a", "Probe A", { size = { width = 520, height = 360 } })
open("b", "Probe B", { size = { width = 420, height = 300 } })
open("inspector", "Probe Inspector", {
	size = { width = 720, height = 480 },
	position = { x = 120, y = 80 },
	remember = true,
})
open("bare", "Probe Bare", {
	size = { width = 600, height = 400 },
	titlebar = "hidden",
	decorations = false,
})

-- What the plugin keeps between runs, under the keys the architecture and
-- the lifecycle rows name: `imgui.windows.<window id>.*`.
local quiet_visible = zeron.store("imgui.windows.quiet.visible", false)
local quiet_shown = quiet_visible:get() == true
-- The choice the row measures is recorded; the window is always opened from
-- the recorded value, so a restart reads the same answer.
quiet_visible:set(false)
local inspector_prefs = zeron.store("imgui.windows.inspector.prefs", {})
inspector_prefs:set({ title = "Probe Inspector", size = { width = 720, height = 480 } })
local drag_prefs = zeron.store("imgui.windows.drag.prefs", {})
drag_prefs:set({ bar = 32 })

-- Work a probe action leaves for after the host has finished the close it
-- asked for (a native window leaves the host's map on the next loop turn).
-- A timer made inside a tool's task dies with that task, so the queue is
-- drained by this script's own timer instead.
local pending: { () -> () } = {}

local function later(run: () -> ())
	table.insert(pending, run)
end

-- A value that keeps changing whether a window is open or not: the lifecycle
-- rows read it through `probe_status` while every window is closed.
ui.every(250, function()
	ticks += 1
	if ticks % 20 == 0 then
		say(`tick {ticks}`)
	end
end)

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

-- `VAL-LIFE-007`: close and reopen the inspector by its own id; its counter
-- and remembered geometry survive, ImGui's transient state does not.
actions.reopen = function()
	close_window("inspector")
	later(function()
		if not shown("inspector") then
			open("inspector", "Probe Inspector", {
				size = { width = 720, height = 480 },
				position = { x = 120, y = 80 },
				remember = true,
			})
		end
	end)
end

-- `VAL-LIFE-011`: the next render callback raises; every shown window has
-- one, so every one of the plugin's windows fails the way the row expects.
actions.blow_up = function()
	blow_up = true
	say("blow_up armed: the next frame raises probe render failure")
end

-- `VAL-WIN-006`: A's plugin menu action closes A's inspector.
actions.close_inspector = function()
	close_window("inspector")
end

-- `VAL-WIN-006`: the same id and geometry as plugin B's inspector (400x300),
-- then the run repeated with no title distinguishing the two windows.
actions.inspector_as_a = function()
	local handle = live["inspector"]
	if handle then
		handle:set_title("A Inspector")
		handle:set_size({ width = 400, height = 300 })
	end
end

actions.inspector_unnamed = function()
	local handle = live["inspector"]
	if handle then
		handle:set_title("Inspector")
	end
end

-- `VAL-LIFE-009` names 700x500 at (100,120) for this fixture's inspector;
-- the title and size change in place, the HWND does not.
actions.inspector_700 = function()
	local handle = live["inspector"]
	if handle then
		handle:set_size({ width = 700, height = 500 })
	end
end

-- `VAL-WIN-011`: each of the five controls, from a probe action.
actions.hide_inspector = function()
	local handle = live["inspector"]
	if handle then
		handle:hide()
	end
end

actions.show_inspector = function()
	local handle = live["inspector"]
	if handle then
		handle:show()
	end
end

actions.focus_inspector = function()
	local handle = live["inspector"]
	if handle then
		handle:focus()
	end
end

actions.minimize_inspector = function()
	local handle = live["inspector"]
	if handle then
		handle:minimize()
	end
end

actions.restore_inspector = function()
	local handle = live["inspector"]
	if handle then
		handle:restore()
	end
end

-- `VAL-WIN-012`: title and size change on the live window, never a new one.
actions.rename_inspector = function()
	local handle = live["inspector"]
	if handle then
		handle:set_title("Renamed Probe")
	end
end

actions.resize_inspector = function()
	local handle = live["inspector"]
	if handle then
		handle:set_size({ width = 1000, height = 640 })
	end
end

-- The counter the button increments, reachable without the pointer.
actions.bump_inspector = function()
	counters["inspector"] = (counters["inspector"] or 0) + 1
end

-- Windows beyond the four launch windows, each a one-action-away open with
-- no close first: the four free slots absorb it, and two-window actions still
-- fit under the host's live cap.

-- `VAL-WIN-004`: min_size and max_size.
actions.open_bounds = function()
	open("bounds", "Probe Bounds", {
		size = { width = 600, height = 400 },
		min_size = { width = 400, height = 300 },
		max_size = { width = 900, height = 600 },
		resizable = true,
	})
end

-- `VAL-WIN-019` to `VAL-WIN-022`: the plugin's own 32-point bar, hidden
-- chrome, edges that resize.
actions.open_drag = function()
	open("drag", "Probe Drag", {
		size = { width = 640, height = 420 },
		titlebar = "hidden",
		decorations = false,
		resizable = true,
	})
end

-- `VAL-LIFE-016`: visible = false, recorded under its store key.
actions.open_quiet = function()
	open("quiet", "Probe Quiet", {
		size = { width = 480, height = 360 },
		visible = quiet_shown,
	})
end

-- `VAL-LIFE-009`: each side remembers its own size, never the other's.
actions.open_left = function()
	open("left", "Probe Left", {
		size = { width = 500, height = 400 },
		remember = true,
	})
end

actions.open_right = function()
	open("right", "Probe Right", {
		size = { width = 900, height = 700 },
		remember = true,
	})
end

actions.open_both_sides = function()
	open("left", "Probe Left", {
		size = { width = 500, height = 400 },
		remember = true,
	})
	open("right", "Probe Right", {
		size = { width = 900, height = 700 },
		remember = true,
	})
end

-- `VAL-WIN-007`: dup is open, and a second open under the same id is refused
-- with the id in the sentence; the plugin keeps running.
actions.open_dup = function()
	open("dup", "Probe Dup", { size = { width = 380, height = 280 } })
	local ok, why = pcall(function()
		zeron.imgui.open_window({ id = "dup", title = "Second Dup" })
	end)
	last_refusal = if ok then "" else tostring(why)
	say(`dup refused={tostring(not ok)} error={last_refusal}`)
end

-- `VAL-WIN-008`: a closed id is reusable; the old handle stays rejected.
actions.open_reuse = function()
	open("reuse", "Probe Reuse", { size = { width = 400, height = 300 } })
end

actions.reuse_cycle = function()
	local first = live["reuse"]
	if not first then
		first = open("reuse", "Probe Reuse", { size = { width = 400, height = 300 } })
	end
	close_window("reuse")
	later(function()
		if not shown("reuse") then
			open("reuse", "Probe Reuse Again", { size = { width = 400, height = 300 } })
		end
	end)
end

-- `VAL-WIN-009`: the live cap, driven to it. Everything closes first so the
-- loop's own eight are the whole population the probe counts.
actions.n_loop = function()
	for _, window in open_ids() do
		close_window(window)
	end
	later(function()
		for i = 1, 12 do
			local handle = open(`n{i}`, `Probe N{i}`, {
				size = { width = 240 + i * 20, height = 200 + i * 10 },
			})
			if not handle then
				say(`limit stopped at {i - 1}: {last_refusal}`)
				break
			end
		end
	end)
end

-- `VAL-WIN-009`: close one of the eight and open one more: the cap is a live
-- count, not a latch.
actions.n_more = function()
	local highest = 0
	for window in live do
		local n = tonumber(string.match(window, "^n(%d+)$") or "")
		if n and n > highest then
			highest = n
		end
	end
	if highest == 0 then
		say("n_more: no n windows open; run n_loop first")
		return
	end
	close_window(`n{highest}`)
	later(function()
		open(`n{highest + 1}`, `Probe N{highest + 1}`, { size = { width = 320, height = 260 } })
	end)
end

actions.close_all_windows = function()
	for _, window in open_ids() do
		close_window(window)
	end
end

actions.open_a = function()
	open("a", "Probe A", { size = { width = 520, height = 360 } })
end

actions.open_b = function()
	open("b", "Probe B", { size = { width = 420, height = 300 } })
end

actions.open_c = function()
	open("c", "Probe C", { size = { width = 560, height = 340 } })
end

local function run_action(action: string)
	local one = actions[action]
	if not one then
		error(`imgui-win-probe has no probe action "{action}"`, 2)
	end
	last_action = action
	one()
end

-- The same actions as menu items, so a person or a probe with a pointer can
-- drive them the way the rows describe ("the probe triggers the fixture's
-- menu action `reopen`"). `probe_run` is the scripted route to the same
-- table.
local function menu(action: string, label: string)
	zeron.menu {
		kind = "session",
		id = `probe-{action}`,
		label = `Probe: {label}`,
		run = function()
			run_action(action)
		end,
	}
end

menu("reopen", "reopen inspector")
menu("blow_up", "raise in the next frame")
menu("close_inspector", "close inspector")
menu("inspector_as_a", "inspector as A Inspector 400x300")
menu("inspector_unnamed", "inspector titled Inspector")
menu("inspector_700", "inspector 700x500")
menu("rename_inspector", "rename inspector")
menu("resize_inspector", "resize inspector 1000x640")
menu("bump_inspector", "bump inspector counter")
menu("hide_inspector", "hide inspector")
menu("show_inspector", "show inspector")
menu("focus_inspector", "focus inspector")
menu("minimize_inspector", "minimize inspector")
menu("restore_inspector", "restore inspector")
menu("open_bounds", "open bounds")
menu("open_drag", "open drag")
menu("open_quiet", "open quiet")
menu("open_left", "open left")
menu("open_right", "open right")
menu("open_both_sides", "open left and right")
menu("open_dup", "open dup and try it twice")
menu("open_reuse", "open reuse")
menu("reuse_cycle", "close and reopen reuse")
menu("n_loop", "open up to the window cap")
menu("n_more", "close one and open one more")
menu("close_all_windows", "close every window")
menu("open_a", "open a")
menu("open_b", "open b")
menu("open_c", "open c")

zeron.tool {
	name = "probe_status",
	run = function(): any
		local ids = live_ids()
		return {
			plugin = id,
			version = "1",
			windows = ids,
			open = #ids,
			frames = copy(frames),
			counters = copy(counters),
			closed = copy(closes),
			ticks = ticks,
			last_action = last_action,
			last_refusal = last_refusal,
			store = {
				quiet_visible = quiet_visible:get(),
				inspector_prefs = inspector_prefs:get(),
				drag_prefs = drag_prefs:get(),
			},
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

say(`ready windows={table.concat(open_ids(), ",")} ticks={ticks}`)
