-- A segmented control: one of a few options, in a filled track with a flat
-- thumb that springs to the chosen one, each segment as wide as its label
-- (the thumb taking the chosen one's place and width). For a choice of two or three
-- that sits beside other controls (a title bar's), where a switch needs a
-- label to say what it means.
-- `Segmented { value = state, options = { { value = false, label = "Light" }, { value = true, label = "Dark" } } }`.

-- (A label may be a message, `ui.t`, drawn as it is.)
export type Option = { value: any, label: string | Message | () -> string }
export type Props = {
	value: State<any>,
	options: { Option },
	-- Each segment's width, if they should all be one (equal, so the thumb's
	-- travel is even); else each as wide as its label.
	segment: number?,
	-- The whole width it's given, shared equally by its segments (a
	-- dialog's choice across its column). Otherwise the track is exactly as
	-- wide as its segments, wherever it stands: a column that stretches its
	-- children never leaves room past the last one, so the thumb sits the
	-- same inset from every edge of the track.
	fill: boolean?,
	-- The track's id (its thumb's: `<id>-thumb`), for what measures them.
	id: string?,
	-- What assistive technology calls the group.
	label: Label?,
}

local H = 26
-- The track's corner (the thumb's is this less its inset).
local TRACK_RADIUS = "$radius.md"

-- A segment's ink: the chosen one's full; the others quieter, coming up
-- under the pointer. On the segment, so its label takes it (a hover there
-- can restyle it; a colour on the label itself couldn't). Focused from the
-- keys, zgx's control ring, inset.
local motion = require("@self/motion")
local control = require("@self/control")

local recipe = ui.variants {
	base = {
		cursor = "pointer",
		rounded = "$radius.sm",
		focus = { outline = 1, outline_offset = -1, outline_color = "$focus.ring" },
	},
	variants = {
		chosen = {
			[true] = { color = "$text.primary" },
			[false] = { color = "$text.secondary", hover = { color = "$text.primary" } },
		},
	},
	defaults = { chosen = false },
}

return function(props: Props): Node
	local value = props.value
	local fixed = props.segment
	-- The thumb's inset in its track.
	local PAD = ui.theme.get("zeron.space.hair") :: number
	local index = ui.computed(function(): number
		local v = value:get()
		for i, option in props.options do
			if option.value == v then
				return i - 1
			end
		end
		return 0
	end)
	local segments: { Node } = {}
	local refs: { any } = {}
	for _, option in props.options do
		local function chosen(): boolean
			return value:get() == option.value
		end
		local ref = ui.ref()
		table.insert(refs, ref)
		table.insert(
			segments,
			ui.div {
				ref = ref,
				role = "radio",
				checked = chosen,
				focusable = true,
				variants = recipe { chosen = chosen },
				w = if props.fill then nil else fixed,
				-- (Filling: equal shares, from nothing, whatever each says.)
				grow = if props.fill then 1 else nil,
				basis = if props.fill then 0 else nil,
				min_w = if props.fill then 0 else nil,
				px = if fixed or props.fill then nil else "$space.3",
				shrink = if props.fill then 1 else 0,
				h_full = true,
				items = "center",
				justify = "center",
				on_click = function()
					value:set(option.value)
				end,
				on_key = function(e)
					if e.key == " " or e.key == "Enter" then
						value:set(option.value)
						return true
					end
					return false
				end,
				ui.text {
					option.label,
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
				},
			}
		)
	end
	-- Each segment's laid out width, where the chosen one starts, and
	-- whether they have widths yet (the thumb waits for them, so it doesn't
	-- spring in from nothing as the control opens).
	local function width(i: number): number
		return refs[i].width or 0
	end
	local function measured(): boolean
		for i in refs do
			if width(i) <= 0 then
				return false
			end
		end
		return true
	end
	local SPRING: SpringOptions = { stiffness = 380, damping = 30 }
	local track: ContainerProps = {
		role = "radiogroup",
		id = props.id,
		label = props.label,
		h = H,
		shrink = 0,
		-- Its width: the column's (filling), else its segments' and its
		-- inset each side, so nothing stretches it past them.
		w_full = props.fill or nil,
		w = if props.fill
			then nil
			elseif fixed then fixed * #props.options + PAD * 2
			else function(): number?
				if not measured() then
					return nil
				end
				local sum = PAD * 2
				for i in refs do
					sum += width(i)
				end
				return sum
			end,
		-- The thumb's containing block. It sits in a `ui.show`'s wrapper, so
		-- without this it places against the nearest `relative` ancestor
		-- outside (a FilesPane's box), not this track.
		relative = true,
		bg = "$zeron.secondary.bg",
		-- The thumb, under the labels: flat, the selected wash alone (as a
		-- sidebar's selected row); concentric with the track (its radius less
		-- the inset).
		ui.show(
			measured,
			ui.div {
				id = if props.id then `{props.id}-thumb` else nil,
				absolute = true,
				left = PAD,
				top = PAD,
				h = H - PAD * 2,
				rounded = ui.raw(control.nest(TRACK_RADIUS, PAD), "the track's corner less the thumb's inset: concentric"),
				bg = "$zeron.segmented.thumb",
				w = motion.spring(ui.computed(function(): number
					return width(index:get() + 1)
				end), SPRING),
				x = motion.spring(ui.computed(function(): number
					local x = 0
					for i = 1, index:get() do
						x += width(i)
					end
					return x
				end), SPRING),
			}
		),
		table.unpack(segments),
	}
	control.contain(track, { radius = TRACK_RADIUS, inset = "$zeron.space.hair" })
	return ui.row(track)
end
