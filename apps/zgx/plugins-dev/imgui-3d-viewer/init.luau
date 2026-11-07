--!strict
-- A 3D viewer in a native window: a cube you can turn with the pointer.
--
-- Dear ImGui draws in two dimensions, so the cube is projected here, in
-- Luau: eight corners, six faces, a perspective divide and a painter's sort
-- (farthest first), each face shaded from the accent the window wears. Drag
-- the viewport to rotate it; the auto-spin stops the moment you do.
--
--   ZERON_ZGX_DEV_PLUGINS=apps/zgx/plugins-dev/imgui-3d-viewer \
--     apps/zgx/target/debug/zeron-zgx.exe gallery

local zeron = require("./zeron")("imgui-3d-viewer")

local imgui = zeron.imgui

local window = imgui.open_window {
	id = "viewer",
	title = "3D Viewer",
	size = { width = 760, height = 560 },
}

-- One unit cube, its corners and the six faces as corner indices, each with
-- the shade it takes (the light sits up and to the left, in front).
local CORNERS = {
	{ -1, -1, -1 },
	{ 1, -1, -1 },
	{ 1, 1, -1 },
	{ -1, 1, -1 },
	{ -1, -1, 1 },
	{ 1, -1, 1 },
	{ 1, 1, 1 },
	{ -1, 1, 1 },
}

local FACES = {
	{ corners = { 1, 2, 3, 4 }, shade = 0.52, normal = { 0, 0, -1 } },
	{ corners = { 5, 6, 7, 8 }, shade = 1.15, normal = { 0, 0, 1 } },
	{ corners = { 1, 5, 6, 2 }, shade = 0.68, normal = { 0, -1, 0 } },
	{ corners = { 2, 6, 7, 3 }, shade = 0.92, normal = { 1, 0, 0 } },
	{ corners = { 3, 7, 8, 4 }, shade = 0.80, normal = { 0, 1, 0 } },
	{ corners = { 4, 8, 5, 1 }, shade = 0.74, normal = { -1, 0, 0 } },
}

local yaw = 0.6
local pitch = -0.35
local spin = true
local zoom = 1.0
local frames = 0

local function clamp(value: number, low: number, high: number): number
	return math.max(low, math.min(high, value))
end

-- Turn a point about the vertical axis, then the horizontal one.
local function rotate(point: { number }, turn: number, tilt: number): { number }
	local cos_yaw, sin_yaw = math.cos(turn), math.sin(turn)
	local x = point[1] * cos_yaw - point[3] * sin_yaw
	local z = point[1] * sin_yaw + point[3] * cos_yaw
	local cos_pitch, sin_pitch = math.cos(tilt), math.sin(tilt)
	local y = point[2] * cos_pitch - z * sin_pitch
	local depth = point[2] * sin_pitch + z * cos_pitch
	return { x, y, depth }
end

local function rotate_normal(normal: { number }, turn: number, tilt: number): { number }
	return rotate(normal, turn, tilt)
end

-- A point in the box `(left, top)..(right, bottom)` on screen, its depth
-- scaled by the perspective divide.
local function project(point: { number }, centre: { number }, radius: number): (number, number, number)
	local focal = 4.0
	local depth = point[3] + focal
	local scale = radius * focal / (depth * 2.2)
	return centre[1] + point[1] * scale, centre[2] - point[2] * scale, depth
end

-- `#rrggbb` to its channels, and back with each one moved by `factor`.
local function channels(colour: string): { number }
	local digits = string.sub(colour, 2, 7)
	return {
		tonumber(string.sub(digits, 1, 2), 16) or 255,
		tonumber(string.sub(digits, 3, 4), 16) or 255,
		tonumber(string.sub(digits, 5, 6), 16) or 255,
	}
end

local function shaded(rgb: { number }, factor: number): string
	local function channel(value: number): string
		local level = math.floor(clamp(value * factor, 0, 255) + 0.5)
		return string.format("%02x", level)
	end
	return `#{channel(rgb[1])}{channel(rgb[2])}{channel(rgb[3])}`
end

local function cube(ui: any, left: number, top: number, right: number, bottom: number)
	local accent = imgui.style().colours["$accent"] or "#8b7cf6"
	local rgb = channels(accent)
	local draw = ui.get_window_draw_list()
	local centre = { (left + right) / 2, (top + bottom) / 2 }
	local radius = math.min(right - left, bottom - top) / 2 * 0.8 * zoom

	local turned = {}
	for index, corner in CORNERS do
		turned[index] = rotate(corner, yaw, pitch)
	end
	local camera = {}
	for index, face in FACES do
		local normal = rotate_normal(face.normal, yaw, pitch)
		-- Faces the camera can see, farthest first.
		if normal[3] > 0.02 then
			local points = {}
			local depth = 0
			for _, at in face.corners do
				local x, y, z = project(turned[at], centre, radius)
				table.insert(points, { x, y })
				depth += z
			end
			table.insert(camera, { points = points, shade = face.shade * (0.75 + 0.35 * normal[3]), depth = depth })
		end
	end
	table.sort(camera, function(a: any, b: any): boolean
		return a.depth > b.depth
	end)
	for _, face in camera do
		draw:add_poly_filled { points = face.points, color = shaded(rgb, face.shade) }
		draw:add_poly_line {
			points = face.points,
			color = shaded(rgb, face.shade * 1.5),
			thickness = 2,
			closed = true,
		}
	end
end

window:on_render(function(ui)
	frames += 1
	if spin then
		local step = 0.9 / 60
		yaw += step
		if yaw > math.pi * 2 then
			yaw -= math.pi * 2
		end
	end

	local state = window:state()
	local width = math.max(320, (tonumber(state.width) or 760) - 48)
	local height = math.max(220, (tonumber(state.height) or 560) - 200)

	ui.text("3D Viewer")
	ui.text_colored(
		`yaw {string.format("%.2f", yaw)}  pitch {string.format("%.2f", pitch)}  zoom {string.format("%.2f", zoom)}  frame {frames}`,
		"#8a8a8a"
	)
	ui.text_colored("drag the viewport to turn the cube", "#8a8a8a")

	local bar = ui.invisible_button { id = "viewport", size = { width = width, height = height } }
	local min, max = bar.min, bar.max
	if bar.active and ui.is_mouse_dragging() then
		local dx, dy = ui.mouse_delta()
		if dx ~= 0 or dy ~= 0 then
			yaw += dx * 0.012
			pitch = clamp(pitch + dy * 0.012, -1.45, 1.45)
			spin = false
		end
	end
	cube(ui, min.x, min.y, max.x, max.y)

	if ui.button(if spin then "Stop" else "Spin") then
		spin = not spin
	end
	ui.same_line()
	if ui.button("Reset") then
		yaw = 0.6
		pitch = -0.35
		zoom = 1.0
		spin = true
	end
	ui.same_line()
	zoom = ui.slider_float("zoom", zoom, { min = 0.5, max = 2.0 })
end)

-- A tool so the window can be checked without a pointer.
zeron.tool {
	name = "viewer_status",
	run = function(): any
		return { yaw = yaw, pitch = pitch, zoom = zoom, spin = spin, frames = frames }
	end,
}