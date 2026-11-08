-- One progress and one complete geometry per frame. The editor stays
-- compact until a whole line fits above the controls, then takes its final
-- expanded width in one step. It never wraps through interpolated insets.
local Layout = require("@ds/composer_layout")
local M = {}

local function mix(a: number, b: number, p: number): number
	return a + (b - a) * p
end

function M.frame(from: Layout.Geometry, to: Layout.Geometry, progress: number): Layout.Geometry
	local p = math.clamp(progress, 0, 1)
	if p == 1 then
		return to
	end
	local g = table.clone(to)
	g.height = mix(from.height, to.height, p)
	g.header_height = mix(from.header_height, to.header_height, p)
	g.controls = {
		leading = {
			left = mix(from.controls.leading.left, to.controls.leading.left, p),
			bottom = mix(from.controls.leading.bottom, to.controls.leading.bottom, p),
			height = to.controls.leading.height,
		},
		trailing = {
			right = mix(from.controls.trailing.right, to.controls.trailing.right, p),
			bottom = mix(from.controls.trailing.bottom, to.controls.trailing.bottom, p),
			height = to.controls.trailing.height,
		},
		voice = {
			left = mix(from.controls.voice.left, to.controls.voice.left, p),
			right = mix(from.controls.voice.right, to.controls.voice.right, p),
			bottom = mix(from.controls.voice.bottom, to.controls.voice.bottom, p),
		},
	}
	-- Animate the vertical envelope continuously, deriving the viewport
	-- from the current container and reserved space. Switching width must
	-- not also jump the text vertically or leave it over the buttons.
	local top = mix(from.text.top, to.text.top, p)
	local from_bottom = from.height - Layout.BORDER * 2 - from.text.top - from.text.height
	local to_bottom = to.height - Layout.BORDER * 2 - to.text.top - to.text.height
	local bottom = mix(from_bottom, to_bottom, p)
	local available = math.max(0, g.height - Layout.BORDER * 2 - top - bottom)
	local controls = math.max(g.controls.leading.bottom + g.controls.leading.height,
		g.controls.trailing.bottom + g.controls.trailing.height)
	local expand = to.expanded and bottom >= controls - 0.001
		and available >= math.min(to.line_height, to.text.height) - 0.001
	local left = if expand then to.text.left else to.compact_text.left
	local right = if expand then to.text.right else to.compact_text.right
	if not expand then
		-- Keep compact text and controls beside each other, including themes
		-- with deeply inset expanded controls. Their vertical anchors move.
		g.controls.leading.left = to.compact_inset
		g.controls.trailing.right = to.compact_inset
	end
	g.text = {
		left = left, right = right,
		width = math.max(0, to.width - Layout.BORDER * 2 - left - right),
		top = top, height = available,
	}
	g.expanded = expand
	g.mode = if expand then "expanded" else "compact"
	return g
end

-- Changes in natural content height at the viewport cap, or in hysteresis
-- metadata, don't change the destination and must not restart a run.
function M.same_destination(a: Layout.Geometry, b: Layout.Geometry): boolean
	if a.width ~= b.width or a.height ~= b.height or a.mode ~= b.mode or a.expanded ~= b.expanded
		or a.header_height ~= b.header_height then
		return false
	end
	for name, value in a.text do
		if (b.text :: any)[name] ~= value then
			return false
		end
	end
	for key, anchors in a.controls do
		for name, value in anchors do
			if (b.controls :: any)[key][name] ~= value then
				return false
			end
		end
	end
	return true
end

function M.use(target: ReadOnlyState<Layout.Geometry>, snap: State<number>?)
	local motion = require("@self/motion")
	local clock = require("@self/composer_clock")
	local initial = target:peek()
	local origin = ui.use_state(initial)
	local destination = ui.use_state(initial)
	local progress = ui.use_state(1)
	local active = ui.use_state(false)
	local frame = ui.computed(function(): Layout.Geometry
		return M.frame(origin:get(), destination:get(), progress:get())
	end)
	local run: any = nil
	local alive, generation = true, 0
	local snap_seen = if snap then snap:peek() else nil
	local snap_until = 0
	local function cancel()
		generation += 1
		if run then
			run:cancel()
			run = nil
		end
		active:set(false)
	end
	local function settle(next: Layout.Geometry)
		cancel()
		ui.batch(function()
			origin:set(next)
			destination:set(next)
			progress:set(1)
		end)
	end
	ui.effect(function()
		local next = target:get()
		local reduced = motion.reduced()
		if snap then
			local n = snap:get()
			if n ~= snap_seen then
				snap_seen = n
				-- Keep C3's context-settle window; replacing deadlines is C6.
				snap_until = clock.now() + 400
			end
		end
		if reduced or clock.now() < snap_until then
			settle(next)
			return
		end
		if M.same_destination(destination:peek(), next) then
			destination:set(next)
			return
		end
		local shown = frame:peek()
		if M.same_destination(shown, next) then
			settle(next)
			return
		end
		cancel()
		local own = generation
		ui.batch(function()
			origin:set(shown)
			destination:set(next)
			progress:set(0)
			active:set(true)
		end)
		run = anime.animate(progress, {
			value = 1, duration = 220, ease = "out(3)",
			onComplete = function()
				if alive and generation == own then
					run = nil
					active:set(false)
				end
			end,
		})
	end)
	ui.on_cleanup(function()
		alive = false
		cancel()
	end)
	return { frame = frame, origin = origin, destination = destination, progress = progress, active = active }
end

return M
