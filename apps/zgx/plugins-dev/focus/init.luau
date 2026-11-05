-- Focus: a button placed after the title bar's actions ("ui.add", at
-- #titlebar-actions), and, while it's on, the sidebar filtered to the
-- sessions working or waiting for you ("ui.change" on #sidebar, which is
-- how a plugin may set the sidebar's filter: it's one virtual list). Plugins can't reach Zeron's design system's modules,
-- so the button is built from zgx's own pieces in Zeron's theme tokens.

local zeron = require("./zeron")("focus")
local filter = ui.shared("sidebar.filter", nil :: any)

local LIVE = { "working", "input", "approval", "failed", "done" }

local function on(): boolean
	local f = filter:get()
	return type(f) == "table" and f.by == "Focus"
end

local function toggle()
	if on() then
		filter:set(nil)
	else
		filter:set({ statuses = LIVE, by = "Focus" })
		zeron.toast("Showing what's working or needs you")
	end
end

zeron.place {
	at = "#titlebar-actions",
	after = function(): Node
		return ui.button {
			id = "focus-toggle",
			label = "Focus",
			role = "switch",
			checked = on,
			h = "$zeron.island.control",
			px = "$space.2",
			rounded = "$radius.full",
			cursor = "pointer",
			bg = function(): string
				return if on() then "$hover" else "#00000000"
			end,
			color = function(): string
				return if on() then "$text.primary" else "$text.secondary"
			end,
			hover = { bg = "$hover", color = "$text.primary" },
			active = { bg = "$pressed" },
			on_click = toggle,
			ui.text { "Focus", text_size = "$font.size.sm", weight = "$font.weight.medium" },
		}
	end,
}
