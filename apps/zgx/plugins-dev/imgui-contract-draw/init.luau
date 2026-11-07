--!strict
-- The validation contract's drawing fixture (apps/zgx/plugins-dev/imgui-contract-draw).
--
-- One 720x480 window draws exactly one row's geometry at a time: the row is
-- chosen with `screen` and can be set from the on-screen Probe Actions
-- window (one button per screen) or with the `probe_run` tool, so a
-- validator drives a row without an engine or a menu bar.
--
--   ZERON_ZGX_DEV_PLUGINS=<this folder> ZERON_ZGX_PLUGIN_LOG=1 \
--     target/debug/zeron-zgx.exe gallery
--
-- Screens: the DRAW rows (lines, rect, rect_filled, rounding,
-- rounded_filled, circle, circle_filled, text, image, polygon, clip, alpha,
-- kept, refused, off-frame, handles) and the image widget's own row.

local zeron = require("./zeron")("imgui-contract-draw")

local imgui = zeron.imgui

local SCREENS = {
	"lines",
	"rect",
	"rect_filled",
	"rounding",
	"rounded_filled",
	"circle",
	"circle_filled",
	"text",
	"image",
	"polygon",
	"clip",
	"alpha",
	"kept",
	"refused",
	"off_frame",
	"handles",
	"image_widget",
}

local screen = "lines"
local frames = 0
local kept_frames = 0
local refusal = ""

local checker = imgui.texture { path = "assets/checker.png" }
local scratch = imgui.draw_list()
local window: any = nil
local actions: any = nil
local last_colour = { r = 0.2, g = 0.4, b = 0.9 }

local function say(what: string)
	print(`draw: {what}`)
end

local function pentagon(cx: number, cy: number, radius: number): { { number } }
	local points = {}
	for step = 0, 4 do
		local angle = -math.pi / 2 + step * 2 * math.pi / 5
		table.insert(points, { cx + radius * math.cos(angle), cy + radius * math.sin(angle) })
	end
	return points
end

-- The three handles VAL-DRAW-014 refuses: a number, a path string, and
-- another plugin's handle. A handle is a table only the plugin that made it
-- can read, so another plugin's is, from here, a table whose fields mean
-- nothing to this window's registry: the same refusal. (Nothing this plugin
-- can construct is a real handle from another plugin, which is the point of
-- the encapsulation the row also asks for.)
local function refuse_handles(dl: any)
	local fake = { 42, "assets/checker.png", { __texture = 7 } }
	for _, handle in fake do
		local ok, why = pcall(function()
			dl:add_image { handle = handle, min = { 0, 0 }, max = { 10, 10 } }
		end)
		refusal = `handle {type(handle)} ok={tostring(ok)} {tostring(why)}`
		say(refusal)
	end
end

-- Off the frame: neither the frame's own list nor a kept one draws.
local function off_frame(where: string)
	local ok, why = pcall(function()
		imgui.open_window({ id = "noop", title = "noop" })
	end)
	local called, why_get = pcall(function()
		local list = scratch
		list:add_line { from = { 0, 0 }, to = { 10, 10 }, color = "#ffffff" }
	end)
	say(`off-frame {where} open ok={tostring(ok)} {tostring(why)} kept ok={tostring(called)} {tostring(why_get)}`)
	say(`off-frame-ok {where}`)
end

local function draw_text(dl: any)
	dl:add_text { pos = { 40, 40 }, color = "#ff0000", text = "Zeron 123", font_size = 20 }
	dl:add_text { pos = { 40, 120 }, color = "#ff0000", text = "Zeron 123", font_size = 20 }
end

local function draw_screen(ui: any)
	local dl = ui.get_window_draw_list()
	if screen == "lines" then
		dl:add_line { from = { 40, 60 }, to = { 240, 60 }, color = "#ff0000", thickness = 3 }
		dl:add_line { from = { 40, 80 }, to = { 240, 200 }, color = "#00c000", thickness = 1 }
	elseif screen == "rect" then
		dl:add_rect { min = { 40, 40 }, max = { 240, 140 }, color = "#00a0ff", thickness = 2 }
	elseif screen == "rect_filled" then
		dl:add_rect_filled { min = { 40, 40 }, max = { 240, 140 }, color = "#ff8800" }
	elseif screen == "rounding" then
		dl:add_rect_rounded { min = { 40, 40 }, max = { 240, 140 }, rounding = 16, color = "#00a0ff", thickness = 2 }
		dl:add_rect_rounded { min = { 40, 160 }, max = { 240, 260 }, rounding = 0, color = "#00a0ff", thickness = 2 }
	elseif screen == "rounded_filled" then
		dl:add_rect_rounded_filled { min = { 40, 40 }, max = { 240, 140 }, rounding = 16, color = "#3366ff" }
	elseif screen == "circle" then
		dl:add_circle { center = { 140, 120 }, radius = 60, color = "#ffffff", thickness = 2 }
	elseif screen == "circle_filled" then
		dl:add_circle_filled { center = { 140, 120 }, radius = 60, color = "#22cc88" }
	elseif screen == "text" then
		draw_text(dl)
	elseif screen == "image" then
		dl:add_image { handle = checker, min = { 40, 40 }, max = { 104, 104 } }
	elseif screen == "polygon" then
		dl:add_poly_filled { points = { { 60, 220 }, { 140, 60 }, { 220, 220 } }, color = "#ff00ff" }
		dl:add_poly_line { points = pentagon(430, 150, 70), color = "#00ffff", thickness = 2, closed = true }
	elseif screen == "clip" then
		dl:push_clip_rect { min = { 100, 100 }, max = { 200, 180 } }
		dl:add_rect_filled { min = { 0, 0 }, max = { 720, 480 }, color = "#ff0000" }
		dl:pop_clip_rect()
	elseif screen == "alpha" then
		dl:add_rect_filled { min = { 40, 40 }, max = { 200, 200 }, color = "#0000ff" }
		dl:add_rect_filled { min = { 100, 100 }, max = { 260, 260 }, color = "#ff000080" }
	elseif screen == "kept" then
		kept_frames += 1
		if kept_frames == 31 then
			scratch:clear()
			scratch:add_circle_filled { center = { 500, 300 }, radius = 30, color = "#00ff00" }
		else
			scratch:clear()
			scratch:add_line { from = { 40, 40 }, to = { 240, 240 }, color = "#00ff00", thickness = 2 }
		end
		ui.draw(scratch)
	elseif screen == "refused" then
		refuse_handles(dl)
		dl:add_rect_filled { min = { 300, 300 }, max = { 400, 400 }, color = "#00ff00" }
	elseif screen == "handles" then
		last_colour = ui.color_edit3("Tint", last_colour, { display_mode = "rgb" })
		local look = imgui.style()
		say(
			`printed-handles window {type(window)} {tostring(window)} texture {type(checker)} {tostring(checker)} list {type(scratch)} {tostring(scratch)} colour {type(last_colour)} {tostring(last_colour)}`
		)
		say(`printed-handles theme {look.theme} colour {tostring(last_colour.r)} {tostring(last_colour.g)} {tostring(last_colour.b)}`)
		dl:add_rect_filled { min = { 40, 40 }, max = { 120, 120 }, color = "#3366ff" }
	elseif screen == "image_widget" then
		local width, height = imgui.texture_size(checker)
		say(`texture_size {width} {height}`)
		ui.image { handle = checker, width = 64, height = 64 }
		ui.dummy { width = 4, height = 4 }
	end
end

local function draw(view: any)
	frames += 1
	draw_screen(view)
end

window = imgui.open_window {
	id = "draw",
	title = "Contract Drawing",
	size = { width = 720, height = 480 },
}
window:on_render(draw)

local function set_screen(name: string)
	if not table.find(SCREENS, name) then
		say(`no screen {name}`)
		return
	end
	screen = name
	kept_frames = 0
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
		if action == "off_frame" then
			off_frame("tool")
		else
			set_screen(action)
		end
		return { action = action, screen = screen, ok = true }
	end,
}

zeron.tool {
	name = "probe_status",
	run = function(): any
		return { plugin = "imgui-contract-draw", screen = screen, frames = frames, refusal = refusal }
	end,
}

zeron.every(5.0, function()
	off_frame("timer")
end)

say(`ready screens={#SCREENS}`)