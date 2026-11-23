--!strict
-- The drag of an orchestrator page's seam, between its conversation and its
-- side panel: pressed at a point, moved, let go; each move asks for the
-- panel's width the pointer says (the seam is the panel's left edge, so
-- moving left widens it), held between the panel's least and as wide as
-- leaves the conversation its least. A double press puts it back. Pure (no
-- ui): the page's seam calls it, and so do the tests, with positions.
--
--   local drag = SeamDrag.new {
--     width = function() return panel_w end,      -- its width now
--     page = function() return page_w end,        -- the page's width
--     set = function(w) end,                      -- a width kept (nil: back
--                                                 -- to its share)
--     bounds = function(want, least, most) end,   -- each move, before set
--   }
--   drag.press(x, clicks)  drag.move(x)  drag.release()

local M = {}

-- The narrowest the conversation and the panel go.
M.CHAT_MIN = 360
M.PANEL_MIN = 300

-- The widest the panel goes on a page `page` wide.
function M.most(page: number): number
	return math.max(M.PANEL_MIN, page - M.CHAT_MIN - 1)
end

-- A width asked for, held within its bounds on a page `page` wide.
function M.clamp(want: number, page: number): number
	return math.floor(math.clamp(want, M.PANEL_MIN, M.most(page)))
end

export type Opts = {
	width: () -> number,
	page: () -> number,
	set: (w: number?) -> (),
	bounds: ((want: number, least: number, most: number) -> ())?,
}
export type Drag = {
	press: (x: number, clicks: number?) -> (),
	move: (x: number) -> (),
	release: () -> (),
	held: () -> boolean,
}

function M.new(opts: Opts): Drag
	local start: { x: number, w: number }? = nil
	local self = {} :: Drag
	function self.press(x: number, clicks: number?)
		if clicks == 2 then
			start = nil
			opts.set(nil)
			return
		end
		start = { x = x, w = opts.width() }
	end
	function self.move(x: number)
		local s = start
		if not s then
			return
		end
		local want = s.w - (x - s.x)
		local page = opts.page()
		if opts.bounds then
			opts.bounds(want, M.PANEL_MIN, M.most(page))
		end
		opts.set(M.clamp(want, page))
	end
	function self.release()
		start = nil
	end
	function self.held(): boolean
		return start ~= nil
	end
	return self
end

return M
