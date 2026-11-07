-- A seam: a column's edge or a split's divider, dragged to resize what's
-- beside it (Zeron's resize handle, crates/ui shell.rs `resize_handle`).
-- It takes the pointer `REACH` points each side of its middle, under the
-- top bar (the sidebar's edge, under the page, on its own side only); its gap lights under the pointer and as it's dragged, fading
-- (ds/motion `hover`); dragged past its limit it bounces (ds/motion
-- `bounce`) and its gap dims while the drag is held there. A double click
-- puts it back (each seam says where).
--
--   local drag = seam.drag()
--   drag.at(want, least, most)   on each move: where the drag asks to be
--   drag.nudge                   points to add to what the seam moves
--   opacity = seam.lit(function() return over:get() or held:get() end, drag)

local motion = require("@self/motion")

local M = {}

-- How far each side of a seam's middle takes the pointer (Zeron's
-- `PANE_RESIZE_HITBOX_HALF_WIDTH`).
M.REACH = 10

export type Drag = {
	-- Points the seam stands past its limit as it bounces (0 at rest).
	nudge: State<number>,
	-- Whether the drag is held at a limit.
	pinned: State<boolean>,
	at: (want: number, least: number, most: number) -> (),
	release: () -> (),
}

-- A seam's drag: told where each move asks it to be, it bounces once as
-- that reaches a limit, again only after coming back inside.
function M.drag(): Drag
	local nudge = ui.use_state(0)
	local pinned = ui.use_state(false)
	local edge = 0
	local bouncing: any = nil
	local function stop()
		if bouncing then
			bouncing:cancel()
			bouncing = nil
		end
		if nudge:peek() ~= 0 then
			nudge:set(0)
		end
	end
	local function at(want: number, least: number, most: number)
		local now = if want < least - 0.5 then -1 elseif want > most + 0.5 then 1 else 0
		if now == edge then
			return
		end
		edge = now
		pinned:set(now ~= 0)
		stop()
		local params = if now ~= 0 then motion.bounce(now) else nil
		if params then
			bouncing = anime.animate(nudge, params)
		end
	end
	local function release()
		edge = 0
		if pinned:peek() then
			pinned:set(false)
		end
	end
	return { nudge = nudge, pinned = pinned, at = at, release = release }
end

-- Its lit gap's opacity: on while `on` says (under the pointer, dragged),
-- off while `drag` is held at a limit, fading either way.
function M.lit(on: () -> boolean, drag: Drag): any
	return motion.tween(
		ui.computed(function(): number
			return if on() and not drag.pinned:get() then 1 else 0
		end),
		motion.hover()
	)
end

return M
