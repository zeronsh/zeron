-- A strip of tabs: the chosen tab's wash, one pill gliding to the tab that
-- takes it (the session tabs' and the panel's), and tabs that reorder by
-- dragging, as Safari's and Arc's do (the panel's).
--
-- Dragged, a tab rises a little (larger, its shadow deeper) and follows the
-- pointer along the strip, held to its line and kept inside it; the others
-- slide aside as its leading edge passes their middle, and let go it
-- settles into its place, which the strip keeps then (`commit`, once). It leaves the strip as
-- it's grabbed: one tab, the lifted one, and where it would land a gap its
-- width. Carried off the strip, the drag is the platform's again (its
-- preview follows the pointer) and the gap closes; brought back, it opens
-- where the tab is held; let go over nothing (or Escape), the tab flies back
-- into its gap. (The pointer is followed through the native drag's moves
-- over the strip: zgx gives Luau no way to start a native drag mid-gesture.)
--
--   TabStrip.Highlight {         the chosen tab's wash: first in the tabs'
--     key, order, width,         row, which is `relative`
--     content, selected,
--     h, rounded, hidden? }
--
--   local strip = TabStrip.new {
--     key = "panel_tab",          -- the payload's field naming a tab of it
--     items = () -> { string },   -- the tabs' ids in order (tracked)
--     width = (id) -> number,     -- a tab's width as laid out (a ref's)
--     content = ref,              -- the row the tabs are in (its width)
--     offset = State<number>,     -- the scroll's offset
--     selected = () -> string?,   -- the chosen tab
--     face = (id) -> Node,        -- what a tab shows, still (made afresh)
--     commit = (id, index) -> (), -- dropped on the strip: its new place
--     on_away = (away) -> (),     -- (optional) carried off the strip, or back
--     h = 28, rounded = "$radius.lg",
--   }
--   strip.order               the ids as the strip shows them: `ui.each` it
--   strip.Frame(scroll)       the scroll, with the lifted tab over it
--   strip.Highlight()         the chosen tab's wash (hidden while it's lifted)
--   strip.zone(props)         the strip's props, with its drop handlers
--   strip.press(e)            a tab's `on_press`: where it was grabbed
--   strip.begin(id)           a tab's `on_drag`, before its payload
--   strip.ended(_, accepted)  a tab's `on_drag_end`
--   strip.preview(id)         a tab's `drag_preview`: shown off the strip
--   strip.opacity(id)         a tab's: gone while it's carried (its gap)
--   strip.busy()              (untracked) a tab carried or settling
--
--   TabStrip.Wash(lit)          a tab's hover wash, fading
--   TabStrip.keys(id, list, go, ref_of)   a tab's `on_key`: arrows, Home, End

local motion = require("@self/motion")

-- A node id from a strip's key: "panel_tab" gives "panel-tab-…".
local function id_of(key: string, what: string): string
	return `{(string.gsub(key, "_", "-"))}-{what}`
end

local M = {}

local function gap(): number
	return (ui.theme.get("zeron.space.hair") :: number?) or 2
end

-- Where a tab stands in the tabs' row as `list` lays it out.
local function left_in(list: { string }, width: (string) -> number, id: string): number
	local x = 0
	for _, k in list do
		if k == id then
			return x
		end
		x += width(k) + gap()
	end
	return x
end

-- A tab's hover wash, under its content, fading in and out (`motion.hover`). (The tab is `relative`; this comes first in it.)
function M.Wash(lit: () -> boolean): Node
	local on = ui.computed(function(): number
		return if lit() then 1 else 0
	end)
	return ui.div {
		absolute = true,
		left = 0,
		top = 0,
		right = 0,
		bottom = 0,
		rounded = "$radius.lg",
		bg = "$hover",
		pointer_events = "none",
		opacity = motion.tween(on, motion.hover()),
	}
end

-- A tab's `on_key`: the arrows, Home and End go to the tab beside it, or
-- at an end, and choose it (`go`), the keyboard going with them, as ds/tabs'
-- do. `list` the tabs in order, `ref_of` a tab's ref.
function M.keys(id: string, list: () -> { string }, go: (id: string) -> (), ref_of: (id: string) -> any): (e: any) -> boolean
	return function(e: any): boolean
		if e.shift or e.ctrl or e.alt or e.meta then
			return false
		end
		local tabs = list()
		local at = table.find(tabs, id)
		if not at or #tabs == 0 then
			return false
		end
		local to = if e.key == "ArrowLeft"
			then math.max(1, at - 1)
			elseif e.key == "ArrowRight" then math.min(#tabs, at + 1)
			elseif e.key == "Home" then 1
			elseif e.key == "End" then #tabs
			else nil
		if not to then
			return false
		end
		local next_id = tabs[to]
		if next_id ~= id then
			go(next_id)
			local r = ref_of(next_id)
			local el = if r then r:peek() else nil
			if el then
				el:focus()
			end
		end
		return true
	end
end

export type HighlightProps = {
	key: string,
	order: () -> { string },
	width: (id: string) -> number,
	content: any,
	selected: () -> string?,
	h: number,
	rounded: string,
	-- The chosen tab is lifted, and carries its own wash.
	hidden: (() -> boolean)?,
}

-- The chosen tab's wash, under the tabs: one pill springing to the tab chosen
-- and to its width (ds/tabs' spring). It's made once the tabs up to it are
-- measured, so it starts where it belongs; the tabs keep their own wash for
-- the pointer's hover.
function M.Highlight(props: HighlightProps): Node
	local function chosen(): string
		return props.selected() or ""
	end
	local function measured(): boolean
		local _ = props.content.width
		local id = props.selected()
		if not id then
			return false
		end
		for _, k in props.order() do
			if props.width(k) <= 0 then
				return false
			end
			if k == id then
				return true
			end
		end
		return false
	end
	-- (Placed by the box round it, the pill in the flow inside, so the presence
	-- takes the pill's size: nothing at all until it's measured.)
	local pill = ui.presence(function(): string?
		return if measured() then "on" else nil
	end, function(): Node
		return ui.div {
			id = id_of(props.key, "chosen"),
			h = props.h,
			rounded = props.rounded,
			bg = "$zeron.selected.flat.bg",
			pointer_events = "none",
			x = motion.spring(ui.computed(function(): number
				return left_in(props.order(), props.width, chosen())
			end), motion.TAB_SLIDE_SPRING),
			w = motion.spring(ui.computed(function(): number
				return props.width(chosen())
			end), motion.TAB_SLIDE_SPRING),
			opacity = function(): number
				return if props.hidden and props.hidden() then 0 else 1
			end,
		}
	end)
	return ui.div { absolute = true, left = 0, top = 0, pointer_events = "none", pill }
end

export type Props = {
	key: string,
	items: () -> { string },
	width: (id: string) -> number,
	content: any,
	offset: State<number>,
	selected: () -> string?,
	face: (id: string) -> Node,
	commit: (id: string, index: number) -> (),
	on_away: ((away: boolean) -> ())?,
	h: number,
	rounded: string,
}

-- "idle"; "lifted": carried over the strip; "away": carried off it;
-- "settling": let go, going to its place.
type Phase = "idle" | "lifted" | "away" | "settling"

function M.new(props: Props)
	local held = ui.use_state(nil :: string?)
	local phase = ui.use_state("idle" :: Phase)
	-- Its place among the tabs while carried (from 1).
	local at = ui.use_state(1)
	local from_at = 1
	-- Where it was grabbed, from its left; where the pointer is, from the
	-- strip's left.
	local grab, pointer = 0, 0
	local dropped = false
	local settle: any = nil
	local frame, ghost = ui.ref(), ui.ref()
	-- The carried tab's width, as it was grabbed: off the strip it isn't laid
	-- out, and has none to read.
	local held_w = 0
	local function width(id: string): number
		return if id == held:peek() then held_w else props.width(id)
	end

	-- The order shown: the carried tab at its place among the others; off
	-- the strip, not there at all (its gap closed).
	local order = ui.computed(function(): { string }
		local list = props.items()
		local id = held:get()
		if not id then
			return list
		end
		local away = phase:get() == "away"
		local out: { string } = {}
		for _, k in list do
			if k ~= id then
				table.insert(out, k)
			end
		end
		if #out == #list then
			-- (Closed meanwhile.)
			return list
		end
		if away then
			return out
		end
		table.insert(out, math.clamp(at:get(), 1, #out + 1), id)
		return out
	end)

	local function view_w(): number
		return frame.width or 0
	end

	-- Carried near an end of the strip, the strip scrolls under it (one
	-- steady run to that end, the tab's place followed as it goes).
	local edge_run: any = nil
	local edge_dir = 0
	local place: () -> ()
	local function stop_edge()
		if edge_run then
			edge_run:cancel()
			edge_run = nil
		end
		edge_dir = 0
	end
	local function edge_scroll()
		local view = view_w()
		local most = (props.content.width or 0) - view
		local dir = 0
		if phase:peek() == "lifted" and most > 0 then
			if pointer < motion.TAB_EDGE then
				dir = -1
			elseif pointer > view - motion.TAB_EDGE then
				dir = 1
			end
		end
		if dir == edge_dir then
			return
		end
		stop_edge()
		edge_dir = dir
		if dir == 0 then
			return
		end
		local to = if dir < 0 then 0 else most
		local far = math.abs(to - props.offset:peek())
		if far < 1 then
			return
		end
		edge_run = anime.animate(props.offset, {
			value = to,
			duration = far / motion.TAB_EDGE_SPEED * 1000,
			ease = "linear",
			onUpdate = function()
				place()
			end,
		})
	end

	-- The lifted tab at the pointer, along the strip and inside it, and its
	-- place among the others: past a neighbour's middle, it takes that
	-- neighbour's place.
	function place()
		local id = held:peek()
		if not id or phase:peek() ~= "lifted" then
			return
		end
		local w = held_w
		local x = math.clamp(pointer - grab, 0, math.max(0, view_w() - w))
		anime.utils.set(ghost, { x = x })
		-- A neighbour gives way as the lifted tab's leading edge passes its
		-- middle: its right edge for one after it, its left for one before.
		-- (Coming back takes the other edge back past it, so it doesn't flicker.)
		local from = x + props.offset:peek()
		local to = from + w
		local before, left, past = 0, 0, false
		for _, k in order:peek() do
			local kw = width(k)
			if k == id then
				past = true
			else
				local middle = left + kw / 2
				if (past and to > middle) or (not past and from >= middle) then
					before += 1
				end
			end
			left += kw + gap()
		end
		if at:peek() ~= before + 1 then
			at:set(before + 1)
		end
		edge_scroll()
	end

	-- Its phase; carried off the strip or back onto it, the strip says so.
	local function set_phase(to: Phase)
		local was = phase:peek()
		if was == to then
			return
		end
		phase:set(to)
		if props.on_away and (was == "away") ~= (to == "away") then
			props.on_away(to == "away")
		end
	end
	-- Still among the strip's tabs (one closed while carried isn't).
	local function present(id: string): boolean
		return table.find(props.items(), id) ~= nil
	end

	local function finish()
		stop_edge()
		settle = nil
		held:set(nil)
		set_phase("idle")
	end

	-- To its place, on the tabs' spring (at once, with motion reduced).
	local function settle_in()
		local id = held:peek()
		if not id then
			return finish()
		end
		stop_edge()
		set_phase("settling")
		local x = left_in(order:peek(), width, id) - props.offset:peek()
		if motion.reduced_now() then
			anime.utils.set(ghost, { x = x })
			return finish()
		end
		settle = anime.animate(ghost, {
			x = x,
			ease = motion.tab_ease(),
			onComplete = function()
				finish()
			end,
		})
	end

	local self = {}
	self.order = order

	function self.busy(): boolean
		return held:peek() ~= nil
	end

	function self.press(e: any)
		grab = e.local_x
	end

	function self.begin(id: string)
		if settle then
			settle:cancel()
			settle = nil
		end
		local list = props.items()
		from_at = table.find(list, id) or 1
		dropped = false
		-- (Where it stands, so it lifts from there.)
		held_w = props.width(id)
		local x = left_in(list, props.width, id) - props.offset:peek()
		pointer = x + grab
		anime.utils.set(ghost, { x = x })
		at:set(from_at)
		held:set(id)
		set_phase("lifted")
	end

	function self.ended(_payload: any?, accepted: boolean?)
		local id = held:peek()
		if dropped or not id then
			return
		end
		if (phase:peek() == "away" and accepted) or not present(id) then
			-- Taken elsewhere (a split's pane, another window), or closed
			-- while carried: nothing to fly back.
			return finish()
		end
		-- Let go over nothing, or cancelled (Escape): it flies back into its
		-- gap, opening again where it was, from where it left the strip.
		at:set(from_at)
		set_phase("lifted")
		settle_in()
	end

	-- The tab as it's carried: chosen or not, on the window's colour.
	local function lifted_look(id: string, shadow: string): Node
		return ui.div {
			rounded = props.rounded,
			bg = "$background",
			shadow = shadow,
			ui.row {
				w = held_w,
				h = props.h,
				rounded = props.rounded,
				bg = if props.selected() == id then "$zeron.selected.flat.bg" else "$hover",
				color = "$text.primary",
				props.face(id),
			},
		}
	end

	-- What the drag shows: off the strip, the tab as it's carried; over it,
	-- nothing (the lifted tab is the strip's own).
	function self.preview(id: string): Node
		return ui.div {
			-- (Over the window, never in its flow: zgx sets it so as the drag
			-- begins, but the opacity changing restyles it from these props.)
			absolute = true,
			opacity = function(): number
				return if phase:get() == "away" then 1 else 0
			end,
			lifted_look(id, "$shadow.sm"),
		}
	end

	function self.opacity(id: string): number
		return if held:get() == id then 0 else 1
	end

	local function carried(payload: any): boolean
		local id = held:peek()
		return id ~= nil and type(payload) == "table" and payload[props.key] == id
	end

	local function track(e: any)
		local el = frame:peek()
		local r = if el then ui.window_rect(el) else nil
		if r then
			pointer = e.x - r.x
		end
	end

	function self.zone(p: { [any]: any }): { [any]: any }
		p.drop_when = carried
		p.on_drag_over = function(_payload: any, e: any)
			track(e)
			if phase:peek() == "away" then
				set_phase("lifted")
			end
			place()
		end
		p.on_drag_leave = function()
			if phase:peek() == "lifted" then
				stop_edge()
				set_phase("away")
			end
		end
		p.on_drop = function(payload: any, e: any)
			local id = held:peek()
			if not id or not carried(payload) then
				return
			end
			-- (Closed while carried: nothing to place.)
			if not present(id) then
				dropped = true
				return finish()
			end
			track(e)
			if phase:peek() ~= "lifted" then
				set_phase("lifted")
			end
			place()
			dropped = true
			local to = at:peek()
			if to ~= from_at then
				props.commit(id, to)
			end
			settle_in()
		end
		return p
	end

	-- The lifted tab, over the strip: a still copy of it, risen.
	local lift = ui.computed(function(): number
		return if phase:get() == "lifted" then motion.TAB_LIFT else 1
	end)
	local layer = ui.div {
		-- (`#panel-tab-lifted` for "panel_tab".)
		id = id_of(props.key, "lifted"),
		ref = ghost,
		absolute = true,
		left = 0,
		top = 0,
		pointer_events = "none",
		opacity = function(): number
			local ph = phase:get()
			return if held:get() and (ph == "lifted" or ph == "settling") then 1 else 0
		end,
		scale = motion.tween(lift, motion.TAB_LIFT_TWEEN),
		ui.presence(function(): string
			return held:get() or ""
		end, function(id: string): Node
			if id == "" then
				return ui.div {}
			end
			return lifted_look(id, "$shadow.md")
		end),
	}

	function self.Frame(scroll: Node): Node
		return ui.row {
			ref = frame,
			relative = true,
			grow = 1,
			min_w = 0,
			h = props.h,
			items = "center",
			scroll,
			layer,
		}
	end

	function self.Highlight(): Node
		return M.Highlight {
			key = props.key,
			order = function(): { string }
				return order:get()
			end,
			width = width,
			content = props.content,
			selected = props.selected,
			h = props.h,
			rounded = props.rounded,
			hidden = function(): boolean
				local ph = phase:get()
				return held:get() ~= nil and held:get() == props.selected() and (ph == "lifted" or ph == "settling")
			end,
		}
	end

	return self
end

return M
