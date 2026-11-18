--!strict
-- A run wider than its room (a row of tabs in a narrow pane, or in a longer
-- language), scrolled sideways: each edge that hides some of it stops short
-- of a small chevron button that pages it on, the run fading into that
-- edge, so that there's more is said by a control, never by where a word
-- happens to be cut, and no word runs under the chevron. Neither shows
-- while the run fits. The keys go through the run's own controls
-- (a focused one off its edge scrolls into view, as zgx scrolls any), so
-- the chevrons are the pointer's alone: no Tab stop of their own.
--
--   Pager { id = "store-categories", back = ui.t(…), more = ui.t(…),
--     ds.Tabs { … } }

local IconButton = require("@self/icon_button").IconButton

local M = {}

-- How much of the room a page moves (the rest stays, to keep one's place).
local PAGE = 0.8
-- The fade at an edge that hides some of the run.
local FADE = 32
-- The room a chevron stands in, past the run's view: its button (22) and
-- 2 (`$space.5`).
local ROOM = "$space.5"

export type Props = {
	id: string?,
	-- What each chevron is called (assistive technology, the tooltip).
	back: string | Message,
	more: string | Message,
	[number]: Node,
}

M.Pager = ui.component(function(props: Props): Node
	local offset = ui.use_state(0)
	local view, content = ui.ref(), ui.ref()
	local function room(): number
		return view.width or 0
	end
	local function past(): number
		return math.max(0, (content.width or 0) - room())
	end
	local function hidden_before(): boolean
		return offset:get() > 0.5
	end
	local function hidden_after(): boolean
		return past() - offset:get() > 0.5
	end
	local function page(by: number)
		offset:set(math.clamp(offset:peek() + by * room() * PAGE, 0, past()))
	end
	local function chevron(side: "left" | "right"): Node
		local before = side == "left"
		return ui.show(if before then hidden_before else hidden_after, ui.row {
			class = if before then "pager-back" else "pager-more",
			absolute = true,
			top = 0,
			bottom = 0,
			left = if before then 0 else nil,
			right = if before then nil else 0,
			items = "center",
			IconButton {
				icon = if before then "alt-arrow-left" else "alt-arrow-right",
				label = if before then props.back else props.more,
				size = "sm",
				focusable = false,
				on_click = function()
					page(if before then -1 else 1)
				end,
			},
		})
	end
	local children: { any } = {}
	for _, c in ipairs(props :: any) do
		table.insert(children, c)
	end
	return ui.div {
		id = props.id,
		relative = true,
		w_full = true,
		min_w = 0,
		-- (The run's view stopping short of a chevron while it shows: the
		-- chevron stands in this room, placed on the box's own edge.)
		pl = function(): any
			return if hidden_before() then ROOM else 0
		end,
		pr = function(): any
			return if hidden_after() then ROOM else 0
		end,
		ui.scroll_x {
			ref = view,
			offset = offset,
			w_full = true,
			fade = { FADE, FADE },
			fade_overflow = true,
			ui.row { ref = content, shrink = 0, items = "center", table.unpack(children) },
		},
		chevron("left"),
		chevron("right"),
	}
end)

return M
