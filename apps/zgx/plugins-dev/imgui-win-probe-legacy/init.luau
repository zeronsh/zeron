--!strict
-- `VAL-WIN-010` (b): the copy-edited-manifest bypass regression. This
-- fixture asks for the existing `windows` permission only — the one that
-- opens Zeron's declarative `ui.window` — and not `imgui.windows`. Its
-- declarative panel keeps working, and its attempt at a native Dear ImGui
-- window is refused: the old permission keeps its old meaning and does not
-- buy the new capability.
--
-- Run it (the log line is the refusal's text):
--   ZERON_ZGX_DEV_PLUGINS=<this folder> ZERON_ZGX_PLUGIN_LOG=1 \
--     target/debug/zeron-zgx.exe gallery

local zeron = require("./zeron")("imgui-win-probe-legacy")

local function say(what: string)
	print(`probe-legacy: {what}`)
end

-- The declarative window `windows` has always meant. It stays open, which is
-- what the row's screenshot of Zeron's own window shows.
local shown = ui.state("legacy-shown", true)

ui.window {
	id = "legacy-panel",
	title = "Legacy panel",
	width = 420,
	height = 180,
	open = shown,
	ui.column {
		ui.text("The legacy windows permission still opens declarative windows."),
		ui.text("imgui.windows is a different permission, and this plugin does not have it."),
	},
}

local ok, why = pcall(function()
	zeron.imgui.open_window { id = "x", title = "Denied" }
end)

say(`imgui refused={tostring(not ok)} error={tostring(why)}`)

local beats = 0
ui.every(1000, function()
	beats += 1
	say(`alive {beats}`)
end)
