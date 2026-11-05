-- A new chat: the page under the composer as it floats a little over the
-- page's middle (screens/chat/composer). The shell draws the wallpaper
-- behind it; here, only a slot for what plugins put over the canvas (a
-- greeting, suggestions), above where the composer rises to.

local shared = require("@app/screens/chat/shared")
local EmptyState = require("@ds/empty_state")
local route = require("@app/screens/shell/route")
local boundary = require("@app/screens/shell/boundary")

-- No project yet (Zeron's first launch): one thing to do, in the
-- composer's place (the shell holds the composer back meanwhile).
local function first_project(): Node
	return EmptyState {
		icon = "folder-with-files",
		title = ui.t("chat.home.title") :: any,
		body = ui.t("chat.home.body") :: any,
		action = {
			label = ui.t("chat.home.action") :: any,
			icon = "plus",
			on_click = function()
				local add: any = boundary.load("@app/screens/shell/add_project")
				if add then
					add.open()
				end
			end,
		},
	}
end

local function Home(): Node
	-- Files carried over it attach to the composer, as on a session.
	local zone = shared.drop_zone("home")
	return ui.column {
		id = "chat-home",
		hook = "chat.home",
		w_full = true,
		h_full = true,
		min_h = 0,
		items = "center",
		justify = "center",
		on_file_hover = zone.on_file_hover,
		on_file_drop = zone.on_file_drop,
		drop_when = zone.drop_when,
		on_drag_over = zone.on_drag_over,
		on_drag_leave = zone.on_drag_leave,
		on_hover = zone.on_hover,
		on_drop = zone.on_drop,
		-- Over the composer's place: what plugins add to a new chat. (Not
		-- while there's no project: its empty margin would push the ask down.)
		ui.show(function(): boolean
			return not route.needs_project:get()
		end, ui.slot("chat.home", { w_full = true, items = "center", gap = "$space.3", mb = "$space.8" })),
		-- On the window's middle, not the middle of what's under the title
		-- strip: up by half the strip (the shell's 38).
		ui.show(function(): boolean
			return route.needs_project:get()
		end, ui.div { mt = ui.raw(-38, "half the title strip up: centred on the window, as the eye reads it"), first_project() }),
	}
end

return Home
