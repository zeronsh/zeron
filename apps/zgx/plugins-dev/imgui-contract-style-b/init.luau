--!strict
-- The validation contract's second styling fixture
-- (apps/zgx/plugins-dev/imgui-contract-style-b).
--
-- It sets no style at all: its window must wear Zeron's own look whatever
-- `imgui-contract-style` sets in the same session, and its frame, first item
-- origin and button corners are how a validator sees that no style crossed
-- over.

local zeron = require("./zeron")("imgui-contract-style-b")

local imgui = zeron.imgui

local frames = 0
local picture = imgui.texture { path = "assets/checker.png" }
local window: any = nil
local actions: any = nil

local function say(what: string)
	print(`style-b: {what}`)
end

window = imgui.open_window {
	id = "style-b",
	title = "Contract Style B",
	size = { width = 720, height = 480 },
}
window:on_render(function(ui)
	frames += 1
	local look = imgui.style()
	say(`style-b frame={frames} theme={look.theme} background={tostring(look.colours["$background"])} text={tostring(look.colours["$text.primary"])} frame_rounding={tostring(look.frame_rounding)} padding={tostring(look.window_padding.x)}`)
	ui.text("Zeron 123")
	if ui.button("Save") then
		say("saved")
	end
end)

actions = imgui.open_window {
	id = "actions",
	title = "Probe Actions",
	size = { width = 420, height = 1000 },
}
actions:on_render(function(view)
	view.text("Probe Actions (B)")
	view.text("nothing to set")
end)

zeron.tool {
	name = "probe_run",
	run = function(args: any): any
		local action = if type(args) == "table" and type(args.action) == "string" then args.action else ""
		return { action = action, ok = true, frames = frames }
	end,
}

zeron.tool {
	name = "probe_status",
	run = function(): any
		local look = imgui.style()
		return {
			plugin = "imgui-contract-style-b",
			frames = frames,
			theme = look.theme,
			colours = look.colours,
			frame_rounding = look.frame_rounding,
			window_padding = look.window_padding,
		}
	end,
}

say("ready")