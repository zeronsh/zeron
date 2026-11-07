--!strict
-- A 3D viewer in a native window: a lattice of cubes drawn by the GPU,
-- depth-tested and lit, under a title bar the plugin draws itself.
--
-- Dear ImGui draws in two dimensions, so the host takes the mesh and the
-- camera and draws them with wgpu in the window's own target: a depth buffer
-- decides what hides what, the fragment shader lights each face, and the
-- result sits in the rect this window reserves for it. The plugin owns the
-- geometry, the shading palette and the camera; the title bar is the
-- plugin's too — `titlebar = "hidden"` leaves no OS chrome, so the bar below
-- is drawn with buttons and dragged with the window's own drag gesture.
--
-- The camera lives in `libs/orbit.luau`, a library of the plugin's own,
-- reached with `imgui.library("orbit")`: it is handed this plugin's imgui
-- entry point and is loaded once.
--
--   ZERON_ZGX_DEV_PLUGINS=apps/zgx/plugins-dev/imgui-3d-viewer \
--     apps/zgx/target/debug/zeron-zgx.exe gallery

local zeron = require("./zeron")("imgui-3d-viewer")

local imgui = zeron.imgui
local orbit = imgui.library("orbit")

local window = imgui.open_window {
	id = "viewer",
	title = "3D Viewer",
	size = { width = 900, height = 620 },
	titlebar = "hidden",
}

-- The bar's height, the buttons' size and the window's own margin, in points.
local BAR = 38
local BUTTON = 34
local MARGIN = 14

-- One cube: four corners a face, six faces, outward normals so the light
-- finds each face on its own. White, so the scene's tint colours it.
local function cube(size: number): any
	local h = size / 2
	local faces = {
		{ n = { 0, 0, 1 }, c = { { -h, -h, h }, { h, -h, h }, { h, h, h }, { -h, h, h } } },
		{ n = { 0, 0, -1 }, c = { { h, -h, -h }, { -h, -h, -h }, { -h, h, -h }, { h, h, -h } } },
		{ n = { 1, 0, 0 }, c = { { h, -h, h }, { h, -h, -h }, { h, h, -h }, { h, h, h } } },
		{ n = { -1, 0, 0 }, c = { { -h, -h, -h }, { -h, -h, h }, { -h, h, h }, { -h, h, -h } } },
		{ n = { 0, 1, 0 }, c = { { -h, h, h }, { h, h, h }, { h, h, -h }, { -h, h, -h } } },
		{ n = { 0, -1, 0 }, c = { { -h, -h, -h }, { h, -h, -h }, { h, -h, h }, { -h, -h, h } } },
	}
	local vertices = {}
	local indices = {}
	for _, face in faces do
		local base = #vertices / 10
		for _, corner in face.c do
			table.insert(vertices, corner[1])
			table.insert(vertices, corner[2])
			table.insert(vertices, corner[3])
			table.insert(vertices, face.n[1])
			table.insert(vertices, face.n[2])
			table.insert(vertices, face.n[3])
			table.insert(vertices, 1)
			table.insert(vertices, 1)
			table.insert(vertices, 1)
			table.insert(vertices, 1)
		end
		for _, step in { 0, 1, 2, 0, 2, 3 } do
			table.insert(indices, base + step)
		end
	end
	return imgui.mesh { vertices = vertices, indices = indices }
end

local lattice = cube(0.62)

-- Where the cubes sit: three by three by three, a step apart.
local PLACES = {}
for x = -1, 1 do
	for y = -1, 1 do
		for z = -1, 1 do
			table.insert(PLACES, { x = x * 1.65, y = y * 1.65, z = z * 1.65, step = x + y + z })
		end
	end
end

local frames = 0
local clock = 0
local last_tick = os.clock()
local last_click = 0

local function channels(colour: string): { number }
	local digits = string.sub(colour, 2, 7)
	return {
		tonumber(string.sub(digits, 1, 2), 16) or 255,
		tonumber(string.sub(digits, 3, 4), 16) or 255,
		tonumber(string.sub(digits, 5, 6), 16) or 255,
	}
end

local function mix(from: { number }, to: { number }, amount: number): string
	local function channel(at: number): string
		return string.format("%02x", math.floor(from[at] + (to[at] - from[at]) * amount + 0.5))
	end
	return `#{channel(1)}{channel(2)}{channel(3)}`
end

local function models(): { any }
	local look = imgui.style()
	local accent = channels(look.colours["$accent"] or "#8b7cf6")
	local softer = channels(look.colours["$accent.hover"] or "#998cf7")
	local bright = channels(look.colours["$text.primary"] or "#e8e8ea")
	local placed = {}
	for _, place in PLACES do
		local tint = if place.step > 1
			then mix(accent, bright, 0.35)
			else if place.step < -1 then mix(accent, softer, 0.7) else mix(accent, softer, 0.3)
		table.insert(placed, {
			mesh = lattice,
			position = { place.x, place.y, place.z },
			-- Each cube turns on its own, so the lattice reads as solid up
			-- close and as a turning cloud from the outside.
			rotation = {
				clock * (0.25 + 0.05 * place.step),
				clock * (0.4 - 0.04 * place.step),
				clock * 0.15,
			},
			scale = 1,
			color = tint,
		})
	end
	return placed
end

-- The bar the plugin draws: a drag strip, and minimise, maximise and close.
-- A double click on the strip maximises or restores, as an OS bar would.
local function title_bar(ui: any, width: number): any
	local strip = ui.invisible_button {
		id = "titlebar",
		size = { width = width - BUTTON * 3 - 8, height = BAR - 8 },
	}
	if strip.active and ui.is_mouse_dragging() then
		window:begin_drag()
	end
	if strip.clicked then
		local now = os.clock()
		if now - last_click < 0.4 then
			last_click = 0
			if window:is_maximized() then
				window:restore()
			else
				window:maximize()
			end
		else
			last_click = now
		end
	end
	ui.same_line()
	if ui.small_button("_") then
		window:minimize()
	end
	ui.same_line()
	if ui.small_button("[]") then
		if window:is_maximized() then
			window:restore()
		else
			window:maximize()
		end
	end
	ui.same_line()
	if ui.small_button("x") then
		window:close()
	end
	return strip
end

window:on_render(function(ui)
	local now = os.clock()
	local dt = math.min(now - last_tick, 0.1)
	last_tick = now
	clock += dt
	frames += 1
	orbit.turn(dt)

	local state = window:state()
	local width = math.max(420, (tonumber(state.width) or 900) - MARGIN * 2)
	local height = math.max(260, (tonumber(state.height) or 620) - BAR - MARGIN * 2)

	local strip = title_bar(ui, width)
	ui.separator()

	local view = ui.scene3d {
		id = "lattice",
		width = width,
		height = height,
		camera = orbit.place(),
		models = models(),
	}
	orbit.drag(ui, view)
	orbit.clicked(view)

	-- The scene draws over this frame, so its backdrop and its labels are
	-- painted here: the cubes land on top of them.
	local look = imgui.style()
	local draw = ui.get_window_draw_list()
	draw:add_rect_filled { min = view.min, max = view.max, color = "#0a0b0f" }
	draw:add_text {
		pos = { strip.min.x + 2, strip.min.y + 7 },
		text = "3D Viewer",
		color = look.colours["$text.primary"] or "#e8e8ea",
		font_size = 16,
	}

	if ui.button(if orbit.spinning then "Stop" else "Orbit") then
		orbit.spinning = not orbit.spinning
	end
	ui.same_line()
	if ui.button("Reset") then
		orbit.reset()
	end
	ui.same_line()
	orbit.distance = ui.slider_float("distance", orbit.distance, { min = 3.5, max = 16 })
	ui.same_line()
	ui.text_colored(
		`{#PLACES} cubes · {string.format("%.2f", orbit.yaw)} / {string.format("%.2f", orbit.pitch)} · frame {frames}`,
		"#5c5c68"
	)
end)

zeron.tool {
	name = "viewer_status",
	run = function(): any
		return {
			yaw = orbit.yaw,
			pitch = orbit.pitch,
			distance = orbit.distance,
			spin = orbit.spinning,
			cubes = #PLACES,
			frames = frames,
		}
	end,
}