-- A page in the page's card: Settings' pages, the catalogs (Plugins,
-- Themes, Templates), a plugin's own page (GitHub's pull requests). One
-- layout for all of them, so moving from one to another never moves the
-- title:
--
--   * a column at the reading measure (`zeron.settings.measure`), centred
--     in the card; `width = "wide"` (`layout.WIDE`) for a page that must be
--     wider (GitHub's pull request board), its header spanning the same
--     column: its title keeps the top and the style, on the wider column's
--     edge;
--   * its title `layout.TOP` under the card's top, in Text's `headline`
--     (`font.size.xl` on `font.line.xl`, semibold), the line under it
--     `space.1` below in `body` (`font.size.md`, secondary);
--   * every word outside a card (title, line, section titles, tabs, hints)
--     on the column's edge, which is the cards' edge: only a card's rows
--     keep an inset (Settings.TEXT_INSET), inside it;
--   * `actions` on the title's line at the column's right end, centred on
--     its first line (a refresh, a device or project choice, Delete all);
--     `trail` beside the line under it (a search); short of room, under;
--   * the content `space.6` under the header, its sections `space.6`
--     apart; `tabs` (underline, flush: labels on the edge, the rule the
--     column's width) in its place, the content `space.4` under them.
--
--   Page { title = "Appearance", description = "How Zeron looks on this computer.",
--     Settings.Section { … }, … }
--   Page { title = "Pull requests", description = repo, actions = Node,
--     tabs = { value = filter, label = "Filter", options = { … } }, list }
--
-- `Frame { page }`: the page's room in its card, for a page Settings
-- doesn't host (a plugin's): it scrolls, keeps the gutters (`layout.gutter`
-- by its own width), the top and the foot, and centres the page. Settings'
-- view keeps the same values round its pages. Plugins lent the design
-- system take both as `require("@ds/page")`.

local Text = require("@self/text")
local Tabs = require("@self/tabs")
local control = require("@self/control")
local layout = require("@self/layout")

local M = {}

-- The least a title's words keep before what's at their end drops under
-- them, so a narrow window never squeezes them to a letter a line.
local MIN_TEXT = 180
-- An aside's column, by default.
local ASIDE = 180

export type Width = "regular" | "wide"

-- The column's measure, in points (max_w takes points: a theme number,
-- read as it's built).
function M.measure(width: Width?): number
	if width == "wide" then
		return layout.WIDE
	end
	return (ui.theme.get("zeron.settings.measure") :: number?) or 640
end

-- (Each word may be a message, `ui.t`, or a function or state: it follows.)
export type TabsProps = {
	value: State<any>,
	options: { any },
	label: (string | Message)?,
	-- At the rule's far end (a page's New), over the same line.
	trail: Node?,
	on_change: ((value: any) -> ())?,
	-- What tests and plugins find the tabs by.
	id: string?,
}

export type Props = {
	title: control.Live<string> | Message,
	description: (control.Live<string> | Message)?,
	-- On the title's line, at the column's right end, centred on its first
	-- line (what the whole page is about: a device, a project; a refresh).
	actions: Node?,
	-- Beside the description's last line (a search the page's list takes).
	-- Short of room, either goes under the words.
	trail: Node?,
	tabs: TabsProps?,
	width: Width?,
	-- Something that stands beside the page's content (a profile's card, a
	-- preview of what the fields make): a column of `aside_w` at the right,
	-- its top on the content's; `aside_top`, over it instead, centred (a
	-- window too narrow for two columns). A state or function moves it as
	-- the width crosses, in place: nothing is built again.
	aside: Node?,
	aside_w: number?,
	aside_top: control.Live<boolean>?,
	[number]: Child,
}

-- The header: the title and the line under it on the column's edge, what's
-- at their end on its right edge.
local function header(props: Props): Node
	local line = ui.theme.get("font.line.xl") :: number?
	return ui.row {
		class = "page-header",
		w_full = true,
		gap = "$space.3",
		items = "end",
		-- Short of room, what's at its end goes under the title.
		flex_wrap = true,
		ui.column {
			grow = 1,
			-- (From nothing, at least its least: a long line under the title
			-- wraps beside what's at its end, which goes under only when the
			-- words would have less than their least.)
			basis = 0,
			min_w = MIN_TEXT,
			gap = "$space.1",
			Text { props.title :: any, style = "headline", heading = 1, wrap = true },
			if props.description then Text { props.description :: any, tone = "secondary", wrap = true } else nil,
		},
		if props.actions
			then ui.row {
				-- (Its own top, a title's line high, it in the middle: its
				-- centre on the title's.)
				self = "start",
				min_h = line,
				gap = "$space.2",
				items = "center",
				props.actions,
			}
			else nil,
		props.trail,
	}
end

function M.Page(props: Props): Node
	local children: { any } = {}
	for _, c in ipairs(props :: any) do
		table.insert(children, c)
	end
	local function top(): boolean
		return control.value(props.aside_top) == true
	end
	-- The content takes the page's whole measure, but beside an aside, where
	-- it takes what that leaves.
	local content = ui.column {
		grow = 1,
		min_w = 0,
		w_full = if props.aside then top else true,
		gap = "$space.6",
		table.unpack(children),
	}
	local body: Node
	if props.aside then
		-- Beside the content, a column of its own at the right, its top on
		-- the content's; over it, on the page's middle (its own box,
		-- centred), a step of the page's rhythm above. One box either way,
		-- turned (the aside after the content, so over it is a reversed
		-- column).
		body = ui.div {
			w_full = true,
			gap = "$space.6",
			direction = function(): Direction
				return if top() then "column_reverse" else "row"
			end,
			items = function(): Align
				return if top() then "stretch" else "start"
			end,
			content,
			ui.column {
				shrink = 0,
				w = function(): number | string
					return if top() then "100%" else (props.aside_w or ASIDE)
				end,
				items = function(): Align
					return if top() then "center" else "stretch"
				end,
				props.aside,
			},
		}
	else
		body = content
	end
	local tabs = props.tabs
	if tabs then
		body = ui.column {
			w_full = true,
			gap = "$space.4",
			ui.column {
				id = tabs.id,
				w_full = true,
				Tabs {
					value = tabs.value,
					label = tabs.label,
					options = tabs.options,
					rule = true,
					flush = true,
					trail = tabs.trail,
					on_change = tabs.on_change,
				},
			},
			body,
		}
	end
	return ui.column {
		class = "page",
		w_full = true,
		max_w = M.measure(props.width),
		gap = "$space.6",
		header(props),
		body,
	}
end

-- The room under a page's last words, in its card.
M.BOTTOM = "$space.8"

-- The page's room in its card: it scrolls (its edges fading), the page
-- `layout.TOP` under the card's top, `layout.gutter` each side by the
-- room's own width, centred. Settings' view keeps these round its pages.
export type FrameProps = { class: string?, id: string?, selectable: boolean?, [number]: Node }

M.Frame = ui.component(function(props: FrameProps): Node
	local width = ui.use_state(0)
	return ui.column {
		id = props.id,
		class = props.class,
		w_full = true,
		h_full = true,
		-- (Its words can be selected and copied, as a chat's can.)
		selectable = props.selectable,
		on_resize = function(size)
			-- (No width at all is no width: the page's column closing.)
			local w = math.floor(size.w)
			if w > 0 and w ~= width:peek() then
				width:set(w)
			end
		end,
		ui.scroll {
			w_full = true,
			h_full = true,
			fade = { 16, 16 },
			fade_overflow = true,
			ui.column {
				w_full = true,
				min_h = "100%",
				items = "center",
				px = function(): string
					return layout.gutter(width:get())
				end,
				pt = layout.TOP,
				pb = M.BOTTOM,
				props[1],
			},
		},
	}
end)

return M
