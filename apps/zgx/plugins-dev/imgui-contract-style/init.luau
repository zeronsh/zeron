--!strict
-- The validation contract's styling fixture (apps/zgx/plugins-dev/imgui-contract-style).
--
-- One 720x480 window draws whatever the current screen asks for, and prints
-- `imgui.style()` — theme and all fifteen tokens — on every frame it draws,
-- so a validator reads the look and the pixels of the same frame.
--
--   ZERON_ZGX_DEV_PLUGINS=<this folder> ZERON_ZGX_PLUGIN_LOG=1 \
--     target/debug/zeron-zgx.exe gallery
--
-- Screens: defaults, light, colours, rounding, window_rounding, padding,
-- spacing, border, text_size, font, reset.

local zeron = require("./zeron")("imgui-contract-style")

local imgui = zeron.imgui

local SCREENS = {
	"defaults",
	"light",
	"colours",
	"rounding",
	"window_rounding",
	"padding",
	"spacing",
	"border",
	"text_size",
	"font",
	"reset",
}

local TOKENS = {
	"$background",
	"$text.primary",
	"$text.secondary",
	"$text.tertiary",
	"$border",
	"$accent",
	"$accent.hover",
	"$accent.text",
	"$danger",
	"$warning",
	"$success",
	"$selection",
	"$hover",
	"$pressed",
	"$focus.ring",
}

local screen = "defaults"
local frames = 0
local chosen = "alpha"

local serif = imgui.font { path = "assets/InstrumentSerif-Regular.ttf", size = 20 }
local window: any = nil
local actions: any = nil

local function say(what: string)
	print(`style: {what}`)
end

-- The whole look, as the plugin reads it, one line per frame.
local function report()
	local look = imgui.style()
	local parts = {}
	for _, token in TOKENS do
		table.insert(parts, `{token}={tostring(look.colours[token])}`)
	end
	say(
		`style frame={frames} theme={look.theme} dpi={tostring(look.dpi_scale)} frame_rounding={tostring(look.frame_rounding)} window_rounding={tostring(look.window_rounding)} padding={tostring(look.window_padding.x)}x{tostring(look.window_padding.y)} spacing={tostring(look.item_spacing.x)}x{tostring(look.item_spacing.y)} border={tostring(look.border_size)} text={tostring(look.text_size)} font={tostring(look.font)}`
	)
	say(`tokens {table.concat(parts, " ")}`)
end

-- What every screen draws after its own style is set: text, a separator, a
-- selected row, a button, a checkbox, a bar and a child.
local function content(ui: any)
	ui.text("Zeron 123")
	ui.separator()
	if ui.selectable("alpha", chosen == "alpha") then
		chosen = "alpha"
	end
	if ui.selectable("beta", chosen == "beta") then
		chosen = "beta"
	end
	if ui.button("Save") then
		say("saved")
	end
	ui.checkbox("Remember", true)
	ui.progress_bar(0.5)
	ui.begin_child("region", { width = 240, height = 80, border = true })
	ui.text("inside")
	ui.end_child()
end

local function apply()
	if screen == "defaults" or screen == "reset" then
		imgui.reset_style()
	elseif screen == "light" then
		imgui.reset_style()
		imgui.set_style { theme = "light" }
	elseif screen == "colours" then
		imgui.reset_style()
		imgui.set_style {
			colours = {
				["$background"] = "#101820",
				["$text.primary"] = "#ffcc00",
				["$border"] = "#3355ff",
				["$selection"] = "#552288",
			},
		}
	elseif screen == "rounding" then
		imgui.reset_style()
		imgui.set_style { frame_rounding = 8 }
	elseif screen == "window_rounding" then
		imgui.reset_style()
		imgui.set_style { window_rounding = 14 }
	elseif screen == "padding" then
		imgui.reset_style()
		imgui.set_style { window_padding = { x = 24, y = 24 } }
	elseif screen == "spacing" then
		imgui.reset_style()
		imgui.set_style { item_spacing = { x = 20, y = 14 } }
	elseif screen == "border" then
		imgui.reset_style()
		imgui.set_style { border_size = 3 }
	elseif screen == "text_size" then
		imgui.reset_style()
		imgui.set_style { text_size = 22 }
	elseif screen == "font" then
		imgui.reset_style()
		imgui.set_style { font = serif }
	end
end

local function draw()
	frames += 1
	apply()
	report()
end

window = imgui.open_window {
	id = "style",
	title = "Contract Style",
	size = { width = 720, height = 480 },
}
window:on_render(function(ui)
	draw()
	content(ui)
end)

local function set_screen(name: string)
	if not table.find(SCREENS, name) then
		say(`no screen {name}`)
		return
	end
	screen = name
	say(`screen {name}`)
end

actions = imgui.open_window {
	id = "actions",
	title = "Probe Actions",
	size = { width = 420, height = 1000 },
}
actions:on_render(function(view)
	view.text("Probe Actions")
	view.text(`screen {screen}`)
	for _, name in SCREENS do
		if view.button(name) then
			set_screen(name)
		end
	end
end)

zeron.tool {
	name = "probe_run",
	run = function(args: any): any
		local action = if type(args) == "table" and type(args.action) == "string" then args.action else ""
		set_screen(action)
		return { action = action, screen = screen, ok = true }
	end,
}

zeron.tool {
	name = "probe_status",
	run = function(): any
		local look = imgui.style()
		return {
			plugin = "imgui-contract-style",
			screen = screen,
			frames = frames,
			theme = look.theme,
			colours = look.colours,
			frame_rounding = look.frame_rounding,
			window_rounding = look.window_rounding,
			window_padding = look.window_padding,
			item_spacing = look.item_spacing,
			border_size = look.border_size,
			text_size = look.text_size,
			font = look.font,
		}
	end,
}

say(`ready screens={#SCREENS}`)