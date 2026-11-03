-- What runs wider than a reading column: a reply's table, an agent's
-- visual (and a picture follows the column's measure). Prose keeps to the
-- column (the transcript's measure); a block whose content wants more takes
-- the page's spare width first, centred on the column, only as wide as its
-- content and never past the page. Wider still, it scrolls sideways: a thin
-- rail along its foot while the pointer is over it (and as it scrolls), its
-- edges fading where it runs on. A code block keeps to the column, however
-- long its lines, and scrolls sideways the same way.
--
-- The page's room comes from above: a transcript's reply provides `wide`
-- (`Room`: the room either side of the column, the column's width, and the
-- conversation's measure), and `Wide.context()` reads it in a component's
-- build. Where nothing provides it (a file's preview, the gallery, a
-- thought's words, a list's or a quote's insides), a block keeps to its
-- column.
--
--   local room = Wide.context()
--   ui.column { self = "center", w = Wide.width(room, function() return natural end), … }
--
-- `Wide.Scroll` is the sideways view (a table's, a code block's):
-- `ui.scroll_x` with the rail over it (its sibling, so the edge fade
-- doesn't fade it, as ds/rail says) and the wheel: the wheel over it, Shift
-- held or not, scrolls it sideways while it can still move that way; at its
-- end, a gesture goes to the page.

local Rail = require("@self/rail")

local M = {}

export type Room = { room: number, column: number, measure: number }

-- The name it's provided by.
M.KEY = "wide"

-- The edges' fade, where the content runs on past them: clear enough to
-- read as "more this way", short enough to leave the words beside it.
M.FADE = { 24, 48 }

-- The page's room around the column here, if a reader provides it.
function M.context(): State<Room>?
	local ok, got = pcall(ui.use_context, M.KEY, false)
	if ok and got then
		return got :: any
	end
	return nil
end

-- How wide a block runs whose content is `natural` points (its frame's
-- included): the column at least, the page at most. A size worked out in
-- layout (`clamp(…)` of the column, `100%`), so it follows the column as the
-- window changes, without waiting a frame. With `self = "center"` on the
-- block, a wider one stands out evenly on both sides. With no room, the
-- column's width.
function M.width(room: State<Room>?, natural: () -> number): any
	if not room then
		return "100%"
	end
	local r = room :: State<Room>
	return ui.computed(function(): string
		local spare = math.floor(2 * r:get().room)
		if spare <= 0 then
			return "100%"
		end
		local n = math.ceil(natural())
		return `clamp(100%, {n}px, calc(100% + {spare}px))`
	end)
end

export type ScrollProps = {
	-- What scrolls.
	child: Node,
	-- Told the content's width as it's laid out (its natural width, unless
	-- `fill` stretched it).
	on_content: ((w: number) -> ())?,
	-- At least this wide, so a short content still fills its frame (a
	-- table's rows run to its edge).
	fill: (() -> number?)?,
	-- The box's own: a class, an id, its width (the column's by default) and
	-- place across it, its corner and edge.
	class: string?,
	w: any?,
	self: string?,
	id: string?,
	rounded: any?,
	border: number?,
	border_color: string?,
}

-- A view that scrolls sideways: the rail and the wheel as above.
M.Scroll = ui.component(function(props: ScrollProps): Node
	local offset = ui.use_state(0)
	local view = ui.use_state(0)
	local extent = ui.use_state(0)
	local hovered = ui.use_state(false)
	local function limit(): number
		return math.max(0, extent:peek() - view:peek())
	end
	local function wheel(e: any): boolean?
		local dx, dy = e.dx or 0, e.dy or 0
		-- Sideways already (a trackpad's swipe, a tilted wheel): the view's
		-- own scrolling has it.
		if dy == 0 or math.abs(dx) >= math.abs(dy) then
			return nil
		end
		local lim = limit()
		if lim < 1 then
			return nil
		end
		local at = offset:peek()
		local to = math.clamp(at + dy, 0, lim)
		-- At its end that way: the page's. (zgx keeps a gesture with what
		-- took its first event, as macOS does: one the page took stays the
		-- page's as a block slides under the pointer, and one this took
		-- stops at its end rather than running on into the page.)
		if math.abs(to - at) < 0.5 then
			return nil
		end
		offset:set(to)
		return true
	end
	local fill = props.fill
	return ui.div {
		class = props.class,
		id = props.id,
		relative = true,
		w = props.w or "100%",
		self = props.self,
		min_w = 0,
		overflow = "hidden",
		rounded = props.rounded,
		border = props.border,
		border_color = props.border_color,
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		on_wheel = wheel,
		ui.scroll_x {
			w_full = true,
			offset = offset,
			fade_x = M.FADE,
			fade_overflow = true,
			on_resize = function(size: { w: number, h: number })
				if math.abs(size.w - view:peek()) >= 0.5 then
					view:set(size.w)
				end
			end,
			ui.row {
				-- (Its own width, never squeezed to the view's: the view
				-- scrolls over it.)
				shrink = 0,
				min_w = if fill
					then function(): number
						return fill() or 0
					end
					else nil,
				on_resize = function(size: { w: number, h: number })
					if math.abs(size.w - extent:peek()) >= 0.5 then
						extent:set(size.w)
					end
					if props.on_content then
						props.on_content(size.w)
					end
				end,
				props.child,
			},
		},
		Rail {
			class = "wide-rail",
			axis = "x",
			offset = offset,
			extent = extent,
			view = view,
			show = function(): boolean
				return hovered:get()
			end,
		},
	}
end)

return M
