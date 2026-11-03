-- An engine older than the one on disk (src/engine_update.rs says, as
-- shared state `engine.update`: `{ stale, ours }`): one this app started is
-- started again on its own while nothing runs in it; while something does,
-- a quiet chip offers it ("Restart to update"). One this app didn't start is
-- never touched: the chip says so, and how to update it.
--
--   Update.Watch()      what starts an idle engine of this app's again:
--                       mounted once, always (the shell's root), nothing
--                       drawn
--   Update.Notice()     the chip, above the sidebar's footer (nothing, and
--                       no room, when there's nothing to say)
--
-- Busy: a chat working or waiting on an answer, or a terminal open in a
-- panel (starting the engine again ends its shells).
--
-- The beta also asks whether the engine serves this interface at all
-- (`engine.interface`: `{ supported }`): an engine from before it (the
-- stable app's, still running) is restarted as a stale one of this app's
-- is, into the bundle's. One that still can't once restarted needs a newer
-- Zeron: the chip says so and opens About & updates, where turning beta off
-- goes back to Stable.

local sessions = require("@account/sessions")
local Icon = require("@ds/icon")
local FadeText = require("@ds/fade_text")
local locale = require("@ds/locale")

local M = {}

local update = ui.shared("engine.update", { stale = false, ours = false })
local interface = ui.shared("engine.interface", { supported = true })

-- An engine that doesn't serve this interface (the beta's check).
local function unsupported(): boolean
	local i = interface:get()
	return i ~= nil and i.supported == false
end
-- A restart asked of this stale engine already (once each: the engine that
-- comes back is a new one, and says again).
local asked = ui.state("engine_update_asked", false)

-- Whether anything runs in the engine now.
local function busy(): boolean
	local now = sessions.now:get()
	for _, c in now.chats or {} do
		if c.status == "working" or c.status == "input" then
			return true
		end
	end
	local ok, panel = pcall(function(): any
		return (require :: any)("@app/screens/panel/state")
	end)
	if ok and panel and panel.sessions then
		for _, s in panel.sessions:get() do
			for _, t in s.tabs do
				if t.kind == "terminal" then
					return true
				end
			end
		end
	end
	return false
end

local function stale(): boolean
	local u = update:get()
	return (u ~= nil and u.stale == true) or unsupported()
end

local function ours(): boolean
	local u = update:get()
	return (u ~= nil and u.ours == true) or unsupported()
end

-- The host's restart (a test stands in for it).
function M.call_restart(): any
	return ui.call("engine.restart", {})
end

local function restart()
	asked:set(true)
	M.call_restart():after(function() end, function(err: any)
		print(`zeron-zgx: the engine didn't restart: {err}`)
		asked:set(false)
	end)
end

M.Watch = ui.component(function(): Node
	-- Idle and this app's: started again at once, nothing shown.
	ui.effect(function()
		if not stale() then
			if asked:peek() then
				asked:set(false)
			end
			return
		end
		if ours() and not busy() and not asked:peek() then
			restart()
		end
	end)
	return ui.div { w = 0, h = 0, absolute = true, pointer_events = "none" }
end)

M.Notice = ui.component(function(): Node
	-- Which chip, if any: one this app may restart, or one it may not.
	local function which(): string
		-- Restarted, and still not one for this interface.
		if unsupported() and asked:get() then
			return "old"
		end
		if not stale() or asked:get() then
			return ""
		end
		if not ours() then
			return "theirs"
		end
		return if busy() then "ours" else ""
	end
	return ui.presence(which, function(kind: string): Node
		if kind == "" then
			return ui.div { h = 0, w_full = true }
		end
		if kind == "old" then
			return M.Old()
		end
		local mine = kind == "ours"
		return ui.row {
			id = "engine-update",
			role = if mine then "button" else "status",
			label = if mine then ui.t("engine.update.restart") else ui.t("engine.update.out_of_date"),
			-- (A tooltip takes a string: read as the chip is made.)
			tooltip = if mine then locale.s("engine.update.restart_tip") else locale.s("engine.update.theirs_tip"),
			w_full = true,
			-- The sidebar foot's piece, as the Star banner under it is
			-- (screens/shell/sidebar `star`): a row's height and corner, its
			-- glyph and words at the banner's sizes and inset; no ×.
			h = ui.raw(30, "a sidebar row's height (ds/sidebar's SLOT)"),
			shrink = 0,
			px = "$space.2",
			gap = "$zeron.space.tight",
			items = "center",
			rounded = "$radius.lg",
			bg = "$zeron.tone.neutral.wash",
			hover = if mine then { bg = "$zeron.tone.neutral.edge" } else nil,
			color = "$zeron.tone.neutral.ink",
			cursor = if mine then "pointer" else nil,
			on_click = if mine then restart else nil,
			-- (An update's arrow while it's ours to restart, else a note.)
			ui.div { shrink = 0, y = 0.5, Icon { name = if mine then "arrow-up" else "info-circle", size = 12 } },
			FadeText {
				if mine then ui.t("engine.update.restart") else ui.t("engine.update.short"),
				grow = 1,
				min_w = 0,
				text_size = "$font.size.xs",
				weight = "$font.weight.medium",
			},
		}
	end, { w_full = true })
end)

-- An engine too old for this interface, once restarting didn't bring a newer
-- one: the way back to Stable is About & updates.
function M.Old(): Node
	return ui.row {
		id = "engine-update",
		role = "button",
		label = ui.t("engine.update.old"),
		tooltip = locale.s("engine.update.old_tip"),
		w_full = true,
		h = ui.raw(30, "a sidebar row's height (ds/sidebar's SLOT)"),
		shrink = 0,
		px = "$space.2",
		gap = "$zeron.space.tight",
		items = "center",
		rounded = "$radius.lg",
		bg = "$zeron.tone.neutral.wash",
		hover = { bg = "$zeron.tone.neutral.edge" },
		color = "$zeron.tone.neutral.ink",
		cursor = "pointer",
		on_click = function()
			local ok, route = pcall(function(): any
				return (require :: any)("@app/screens/shell/route")
			end)
			if ok and route then
				(route :: any).settings("about")
			end
		end,
		ui.div { shrink = 0, y = 0.5, Icon { name = "info-circle", size = 12 } },
		FadeText {
			ui.t("engine.update.short_old"),
			grow = 1,
			min_w = 0,
			text_size = "$font.size.xs",
			weight = "$font.weight.medium",
		},
	}
end

return M
