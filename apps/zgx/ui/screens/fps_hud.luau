-- A frame stats panel over the main window in the style of Metal's
-- performance HUD (`ZERON_FPS_HUD=1`, src/fps_hud.rs): the chip, the
-- window's pixels, scale, how its frames show and the display's rate; FPS,
-- the gap between frames (Pre), GPU and CPU time each with its range, the
-- footprint with its peak; and a graph of the last frames' gaps (blue) and
-- GPU times (green) under a refresh's budget. It updates with the app's
-- frames; clicks go through it.

local W = 276
local PAD = 10
local SIZE = 12
local CHAR = SIZE * 0.6
local LINE = 16
local GRAPH_H = 44

local WHITE = "#ffffff"
local DIM = "#ffffff59"
local RED = "#ff6b6b"
local BLUE = "#82b1ff"
local GREEN = "#8fe388"
local ORANGE = "#ffc073"

type Stats = {
	device: string,
	width: number,
	height: number,
	scale: number,
	path: string,
	refresh: number,
	budget: number,
	fps: number,
	fps_range: { number },
	pre: number,
	pre_range: { number },
	gpu: number,
	gpu_range: { number },
	cpu: number,
	cpu_range: { number },
	mem: number,
	mem_peak: number,
	pre_line: { number },
	gpu_line: { number },
}

-- A row's pieces, each in its colour, laid out a monospace cell at a time.
type Piece = { text: string, color: string }

local function row(label: string, value: number, range: { number }, value_color: string, low: string, high: string): { Piece }
	return {
		{ text = label, color = WHITE },
		{ text = string.format(" %6.2f", value), color = value_color },
		{ text = " [", color = WHITE },
		{ text = string.format("%6.2f", range[1] or 0), color = low },
		{ text = " ", color = WHITE },
		{ text = string.format("%6.2f", range[2] or 0), color = high },
		{ text = "]", color = WHITE },
	}
end

local function rows(s: Stats): { { Piece } }
	return {
		row("FPS:", s.fps, s.fps_range, WHITE, RED, WHITE),
		row("Pre:", s.pre, s.pre_range, BLUE, BLUE, RED),
		row("GPU:", s.gpu, s.gpu_range, GREEN, GREEN, GREEN),
		row("CPU:", s.cpu, s.cpu_range, ORANGE, ORANGE, ORANGE),
		{
			{ text = "Mem:", color = WHITE },
			{ text = string.format(" %.2fMB", s.mem), color = WHITE },
			{ text = string.format(" [%.2fMB]", s.mem_peak), color = GREEN },
		},
	}
end

local HEIGHT = PAD + LINE * 7 + 6 + GRAPH_H + PAD

return function(view: any): any
	local stats = ui.state("debug_frame_stats", false :: any)
	-- Asked at every tick; the host answers when the app drew a frame since
	-- (and once a second at rest), nil otherwise, so the panel draws only
	-- with the app's frames.
	-- (30 a second at most: the panel's own drawing is work in the frames
	-- it measures.)
	ui.every(33, function()
		local answer = ui.call_sync("debug.frame_stats", nil)
		if answer then
			stats:set(answer)
		end
	end)
	local function draw(g: any, w: number, _h: number)
		local s = stats:get() :: Stats?
		local text = function(t: string, x: number, y: number, color: string, align: string?)
			g:text(t, x, y, { font = "mono", size = SIZE, color = color, align = align or "start", baseline = "top" })
		end
		if not s then
			text("waiting for frames", PAD, PAD, DIM)
			return
		end
		-- The device, the window's pixels; scale, how frames show, the rate.
		text(s.device, PAD, PAD, WHITE)
		text(string.format("[%dx%d]", s.width, s.height), w - PAD, PAD, WHITE, "end")
		text(string.format("%.1fx %s", s.scale, s.path), PAD, PAD + LINE, WHITE)
		text(string.format("%dHz", math.floor(s.refresh + 0.5)), w - PAD, PAD + LINE, WHITE, "end")
		for i, pieces in rows(s) do
			local x = PAD
			local y = PAD + LINE * (i + 1)
			for _, piece in pieces do
				text(piece.text, x, y, piece.color)
				x += #piece.text * CHAR
			end
		end
		-- The graph: twice the budget at its top, the budget halfway.
		local top = PAD + LINE * 7 + 6
		local left, right = PAD, w - PAD
		local bottom = top + GRAPH_H
		local function y_of(ms: number): number
			return bottom - math.min(GRAPH_H, GRAPH_H * ms / (2 * s.budget))
		end
		g:line(left, bottom, right, bottom, WHITE, 1)
		g:line(left, top, left, bottom, DIM, 1)
		g:line(right, top, right, bottom, DIM, 1)
		g:line(left, y_of(s.budget), right, y_of(s.budget), DIM, 1)
		-- One path a line: a stroke each, not one a segment.
		local function plot(values: { number }, color: string)
			local n = #values
			if n < 2 then
				return
			end
			local step = (right - left) / (n - 1)
			local path = ui.path()
			path:move(left, y_of(values[1]))
			for i = 2, n do
				path:line(left + (i - 1) * step, y_of(values[i]))
			end
			g:stroke(path, color, 1.5)
		end
		plot(s.gpu_line, GREEN)
		plot(s.pre_line, BLUE)
	end
	return ui.overlay {
		w_full = true,
		h_full = true,
		view,
		ui.div {
			absolute = true,
			top = ui.raw(8, "fps panel: in the title bar's corner"),
			right = ui.raw(16, "fps panel"),
			w = ui.raw(W, "fps panel"),
			pointer_events = "none",
			z = 100,
			rounded = ui.raw(10, "fps panel"),
			-- Metal's HUD: black, translucent, whatever the theme.
			bg = ui.raw("#000000a6", "fps panel: Metal's HUD look"),
			ui.canvas {
				w_full = true,
				h = ui.raw(HEIGHT, "fps panel"),
				height = HEIGHT,
				draw = draw,
			},
		},
	}
end
