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
local function same_shape(a: Layout.Geometry, b: Layout.Geometry): boolean
	if a.height ~= b.height or a.mode ~= b.mode or a.expanded ~= b.expanded
		or a.header_height ~= b.header_height then
		return false
	end
	for name, value in a.text :: { [string]: number } do
		if name ~= "width" and (b.text :: any)[name] ~= value then
			return false
		end
	end
	for key, anchors in a.controls :: { [string]: { [string]: number } } do
		for name, value in anchors do
			if (b.controls :: any)[key][name] ~= value then
				return false
			end
		end
	end
	return true
end

function M.same_destination(a: Layout.Geometry, b: Layout.Geometry): boolean
	return a.width == b.width and a.text.width == b.text.width and same_shape(a, b)
end

-- Height given to the textarea inside its presented clip (`shown.text.height`).
-- While growing (or on frame 0 before a pending edit's target publishes), the
-- textarea holds at least the height needed to lay out the draft without
-- caret-reveal scrolling the top line off-screen. Settled, it equals
-- `shown.text.height` so accessibility, hit-testing and scrollbars match the
-- visible field exactly.
function M.editor_height(shown: Layout.Geometry, dest: Layout.Geometry, pending: Layout.Sample?, live_width: number?): number
	local h = math.max(shown.text.height, dest.text.height)
	local capped = dest.natural_height > dest.height + 0.01
	local expanded_need = dest.text.height
	local opening_to_expanded = not shown.expanded and dest.expanded
	local width = if live_width and live_width > 0 then live_width else dest.width
	local compact_w = if width > 0
		then math.max(0, width - Layout.BORDER * 2 - shown.compact_text.left - shown.compact_text.right)
		else shown.text.width

	if pending then
		local pending_h = math.min(Layout.TEXT_MAX_VIEWPORT, math.max(pending.wrapped_height, pending.line_height))
		if pending.wrapped_height > Layout.TEXT_MAX_VIEWPORT + 0.01 then
			capped = true
		end
		if not shown.expanded and not dest.expanded and (
			(compact_w > 0 and pending.unwrapped_width > compact_w)
			or pending.wrapped_height > pending.line_height + 0.01
		) then
			opening_to_expanded = true
			expanded_need = math.max(expanded_need, Layout.TEXT_MIN_VIEWPORT, pending_h)
		else
			expanded_need = math.max(expanded_need, pending_h)
		end
		h = math.max(h, expanded_need)
	end

	-- While compact -> expanded still holds compact width (waiting for room
	-- above the controls), an uncapped draft can wrap to more lines at compact
	-- width than at expanded width. Give the editor enough height to hold those
	-- temporary wrapped lines at scroll.y = 0; the outer clip masks to
	-- `shown.text.height`.
	if opening_to_expanded and not capped and compact_w > 0 then
		local expanded_w = Layout.measurement(width).width
		if expanded_w > compact_w then
			local lh = math.max(1, if pending then pending.line_height else dest.line_height)
			local compact_fit = math.ceil(expanded_need * (expanded_w / compact_w) / lh) * lh + lh * 2
			h = math.max(h, compact_fit)
		end
	end

	return h
end

function M.use(target: ReadOnlyState<Layout.Geometry>, snap: State<number>?, settlement: ReadOnlyState<{ revision: number, status: string }>?)
	local motion = require("@self/motion")
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
	local settling = ui.use_state(false)
	local cancel_ack: (() -> ())? = nil
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
		if cancel_ack then cancel_ack(); cancel_ack = nil end
		if snap then
			local n = snap:get()
			if n ~= snap_seen then
				snap_seen = n
				settling:set(true)
			end
		end
		if reduced then
			settle(next)
			settling:set(false)
			return
		end
		if settling:peek() then
			local measured = if settlement then settlement:get() else { revision = 0, status = "ready" }
			settle(next)
			if measured.status == "ready" then
				local context, revision = snap_seen, measured.revision
				cancel_ack = ui.after(0, function()
					local current = if settlement then settlement:peek() else measured
					if alive and context == snap_seen and current.status == "ready"
						and current.revision == revision and M.same_destination(target:peek(), next) then
						settling:set(false)
					end
				end)
			end
			return
		end
		if M.same_destination(destination:peek(), next) then
			destination:set(next)
			return
		end
		local shown = frame:peek()
		-- Layout already applies width; an otherwise unchanged frame needs no run.
		if same_shape(shown, next) then
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
		if cancel_ack then cancel_ack() end
		cancel()
	end)
	return { frame = frame, origin = origin, destination = destination, progress = progress, active = active, settling = settling }
end

return M
