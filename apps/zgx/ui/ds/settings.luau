-- Settings' building blocks: a page, its sections, their groups and rows.
-- A page is a column at a reading width, its title and a line under it.
-- A section is a run of related settings under a small title (the design
-- system's SectionTitle) and a line saying what they are for. A group is a
-- card of rows, a hairline between each, edge to edge. A row says what a
-- setting is (its name and a line under it) with its control at its end;
-- stacked, the control goes under the text (a text field, a list). A row
-- can also lead somewhere (`on_click`, a chevron at its end).
--
--   Settings.Page { title = "Git", description = "…",
--     Settings.Section { title = "Commits",
--       Settings.Group {
--         Settings.Row { label = "Sign commits", description = "…", control = Switch { … } },
--         Settings.Row { label = "Author", stacked = true, control = TextField { … } },
--       },
--     },
--   }

local locale = require("@self/locale")
-- (By the alias the shell mounts the menus' host by: one instance per path.)
local Links = require("@ds/links")
local Icon = require("@self/icon")
local Tile = require("@self/tile")
local control = require("@self/control")
local material = require("@self/material")
local SectionTitle = require("@self/section_title")
local Hint = require("@self/hint")
local FadeText = require("@self/fade_text")
local PrivateText = require("@self/private_text")
local look = require("@self/look")
local Tabs = require("@self/tabs")
local SearchField = require("@self/search_field")
local EmptyState = require("@self/empty_state")
local Button = require("@self/button")
local skeleton = require("@self/skeleton")

local M = {}

-- The least a row's words keep before its control drops under them, so a
-- narrow window never squeezes them to a letter a line.
local MIN_TEXT = 180
-- The chevron at the end of a row that leads somewhere.
local CHEVRON = 14
-- Where the search glyph's ink starts inside its box at a small field's
-- size (its drawing's side bearing, measured): what a Catalog's search
-- moves it by to stand its ink on the rows' edge.
local SEARCH_INK = 1.5

-- Where a row's words start, from a card's outer edge: its 16 inset, past
-- the card's 1 edge. A page's title, a section's, and whatever else stands
-- over or under a card (a page's tabs, a hint) start their words here, so
-- the words line up down one edge and the cards hang past it.
M.TEXT_INSET = 17

-- That inset as a padding, less `pad` (points of the node's own between its
-- edge and its words: a tab's label pad), so its ink lands on the edge.
function M.inset(pad: number?): any
	return ui.raw(M.TEXT_INSET - (pad or 0), "a row's 16 inset, past the card's 1 edge")
end

-- (Each word a component here takes may be a message, `ui.t`: drawn as it
-- is, or read where only a string will do, so it follows the locale.)
export type PageProps = {
	title: control.Live<string> | Message,
	description: (control.Live<string> | Message)?,
	-- Something at the title's end (a search, a Reset): beside the
	-- description's last line, or with `trail_top`, centred on the title's
	-- line at the page's top right (a device choice, which the whole page
	-- is about). Short of room, it goes under the title either way.
	trail: Node?,
	trail_top: boolean?,
	-- Something that stands beside the page's groups (a profile's card, a
	-- preview of what the fields make): a column of `aside_w` at the right,
	-- its top on the first group's; `aside_top`, over the groups instead,
	-- centred (a window too narrow for two columns). A state or function
	-- moves it as the width crosses, in place: nothing is built again.
	aside: Node?,
	aside_w: number?,
	aside_top: control.Live<boolean>?,
	[number]: Child,
}

-- An aside's column, by default.
local ASIDE = 180

function M.Page(props: PageProps): Node
	local children: { any } = {}
	for _, c in ipairs(props :: any) do
		table.insert(children, c)
	end
	-- The groups take the page's whole measure, but beside an aside, where
	-- they take what it leaves.
	local function top(): boolean
		return control.value(props.aside_top) == true
	end
	local groups = ui.column {
		grow = 1,
		min_w = 0,
		w_full = if props.aside then top else true,
		gap = "$space.6",
		table.unpack(children),
	}
	local body: Node
	if props.aside then
		-- Beside the groups, a column of its own at the right, its top on
		-- theirs; over them, on the page's middle (its own box, centred), a
		-- step of the page's rhythm above them. One box either way, turned
		-- (the aside after the groups, so over them is a reversed column).
		body = ui.div {
			w_full = true,
			gap = "$space.6",
			direction = function(): Direction
				return if top() then "column_reverse" else "row"
			end,
			items = function(): Align
				return if top() then "stretch" else "start"
			end,
			groups,
			ui.column {
				shrink = 0,
				-- (Over the groups, the page's whole width.)
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
		body = groups
	end
	return ui.column {
		w_full = true,
		-- (A theme number, read as it is built: max_w takes points.)
		max_w = ui.theme.get("zeron.settings.measure") :: number?,
		gap = "$space.6",
		ui.row {
			-- Inset to the rows' text, as a section's title is: the page's
			-- words line up down one edge, the cards hang past it.
			px = M.inset(),
			gap = "$space.3",
			items = "end",
			-- Short of room, what's at its end goes under the title.
			flex_wrap = true,
			ui.column {
				grow = 1,
				-- (From nothing, at least its least: a long line under the
				-- title wraps beside what's at its end, which goes under only
				-- when the words would have less than their least.)
				basis = 0,
				min_w = MIN_TEXT,
				gap = "$space.1",
				ui.text { props.title, heading = 1, text_size = "$font.size.xl", line_height = "$font.line.xl", weight = "$font.weight.semibold", color = "$text.primary" },
				if props.description
					then ui.text { props.description, text_size = "$font.size.md", line_height = 20, color = "$text.secondary", wrap = true }
					else nil,
			},
			if props.trail and props.trail_top
				then ui.row {
					-- (Its own top, a title's line high, it in the middle: its
					-- centre on the title's.)
					self = "start",
					min_h = ui.theme.get("font.line.xl") :: number?,
					items = "center",
					props.trail,
				}
				else props.trail,
		},
		body,
	}
end

export type SectionProps = {
	title: (string | Message)?,
	description: (string | Message)?,
	trail: Node?,
	[number]: Child,
}

function M.Section(props: SectionProps): Node
	local children: { any } = {}
	for _, c in ipairs(props :: any) do
		table.insert(children, c)
	end
	local head: Node? = nil
	if props.title or props.description then
		head = ui.column {
			-- Inset to the rows' text (past the card's edge), so a section's
			-- title stands over what it names.
			px = M.inset(),
			gap = "$space.1",
			if props.title then SectionTitle { props.title, trail = props.trail, heading = 2 } else nil,
			if props.description then Hint { props.description } else nil,
		}
	end
	return ui.column {
		w_full = true,
		gap = "$space.2",
		role = "group",
		label = props.title,
		head,
		table.unpack(children),
	}
end

-- The hairline between a group's rows, from the card's edge to its edge
-- (a row that comes and goes brings its own: `ui.show(on, ui.column {
-- Settings.Rule(), row })`).
function M.Rule(): Node
	return ui.div {
		-- (What a test counts a card's hairlines by.)
		class = "settings-rule",
		h = 1,
		w_full = true,
		shrink = 0,
		bg = "$zeron.settings.rule",
		aria_hidden = true,
	}
end

-- A card of rows, a hairline between each, running from the card's edge to
-- its edge: the rule splits the card, it doesn't underline the text.
export type GroupProps = { [number]: Child }

function M.Group(props: GroupProps): Node
	local items: { any } = {}
	local n = 0
	for _, c in ipairs(props :: any) do
		if n > 0 then
			table.insert(items, M.Rule())
		end
		table.insert(items, c)
		n += 1
	end
	return ui.column {
		w_full = true,
		rounded = "$radius.xl",
		bg = material.pick("$zeron.settings.tint", "$zeron.settings.card"),
		border = 1,
		border_color = "$zeron.settings.edge",
		overflow = "hidden",
		table.unpack(items),
	}
end

-- A row in a card, washed edge to edge under the pointer: no inset and no
-- corner of its own (the card's corner clips the first and last, so the
-- only curve is the card's), as the board's rows and every settings list's
-- are. The row's own controls take their clicks; the wash only says which
-- row they're on. `over`, the caller's, says while the pointer's on it (a
-- row showing its actions then). Never a padded card with a rounded wash
-- inside it.
M.CatalogRow = ui.component(function(props: { row: Node, over: State<boolean>? }): Node
	local over = props.over or ui.use_state(false)
	return ui.column {
		class = "catalog-row",
		w_full = true,
		bg = function(): string
			return if over:get() then "$hover" else "#00000000"
		end,
		on_hover = function(inside: boolean)
			over:set(inside)
		end,
		props.row,
	}
end)
local CatalogRow = M.CatalogRow

-- A Group of rows that come and go: `Settings.Each(list, row, { key = … })`
-- (`list` a state or computed, `row` as `ui.each` builds one, `key` each
-- item's identity). Each row is its item's: one arriving or leaving leaves
-- the rest the same nodes, and the hairline before a row follows whether it
-- is the first. Empty, it is no card at all. `wash`: each row washed edge
-- to edge under the pointer (`CatalogRow`), for rows that are things in a
-- list (sessions, plugins) rather than settings.
export type EachOptions = { key: (item: any) -> any, wash: boolean? }

function M.Each(list: State<{ any }>, row: (item: any, i: number, now: State<any>) -> Node, opts: EachOptions): Node
	local key = opts.key
	local function first(item: any): boolean
		local head = list:get()[1]
		return head ~= nil and key(head) == key(item)
	end
	return ui.show(
		function(): boolean
			return #list:get() > 0
		end,
		ui.column {
			w_full = true,
			rounded = "$radius.xl",
			bg = material.pick("$zeron.settings.tint", "$zeron.settings.card"),
			border = 1,
			border_color = "$zeron.settings.edge",
			overflow = "hidden",
			ui.each(list, function(item: any, i: number, now: State<any>): Node
				return ui.column {
					w_full = true,
					ui.show(
						function(): boolean
							return not first(item)
						end,
						M.Rule()
					),
					if opts.wash then CatalogRow { row = row(item, i, now) } else row(item, i, now),
				}
			end, {
				key = key,
				w_full = true,
			}),
		}
	)
end

-- `Each` for a list that can run to thousands (a row's height known, so
-- none is measured): the same card and hairlines, its rows in a `ui.list`
-- that scrolls with the page, as tall as all of them, and builds only those
-- the page and the window show. `height(item)`: the row's own, its hairline
-- counted here; `text(item)`, its plain text for a selection running over
-- rows not built.
export type EachListOptions = {
	key: (item: any) -> any,
	height: (item: any) -> number,
	text: ((item: any) -> string)?,
}

function M.EachList(list: State<{ any }>, row: (item: any, i: State<number>, now: State<any>) -> Node, opts: EachListOptions): Node
	local text = opts.text
	return ui.show(
		function(): boolean
			return #list:get() > 0
		end,
		ui.column {
			w_full = true,
			rounded = "$radius.xl",
			bg = "$zeron.settings.card",
			border = 1,
			border_color = "$zeron.settings.edge",
			overflow = "hidden",
			ui.list {
				data = list,
				key = opts.key,
				heights = function(): { number }
					local out: { number } = {}
					for i, item in list:get() do
						out[i] = opts.height(item) + (if i > 1 then 1 else 0)
					end
					return out
				end,
				row_text = if text
					then function(i: number): string
						local item = list:peek()[i]
						return if item ~= nil then text(item) else ""
					end
					else nil,
				w_full = true,
				virtualize = "ancestor",
				row = function(item: any, i: State<number>, now: State<any>): Node
					return ui.column {
						w_full = true,
						ui.show(
							function(): boolean
								return i:get() > 1
							end,
							M.Rule()
						),
						row(item, i, now),
					}
				end,
			},
		}
	)
end

-- How tall a `truncate`d Row with a line under its name stands (each one line,
-- so its height is known; a line with nothing to say still takes its height):
-- its padding round its words or its `lead`, at the row's least. In the
-- window's text size, which scales the lines.
function M.truncated_row_h(lead: number?): number
	local scaled = look.text_theme() or {}
	local function token(name: string, default: number): number
		return scaled[name] or (ui.theme.get(name) :: number?) or default
	end
	local words = token("font.line.md", 18) + token("zeron.space.hair", 2) + token("font.line.sm", 16)
	return math.max(token("zeron.settings.row", 52), 2 * token("space.3", 12) + math.max(words, lead or 0))
end

export type RowProps = {
	label: string | Message | () -> string,
	description: (string | Message | () -> string)?,
	icon: Icon.Name?,
	-- In the icon's place: anything of the row's own (a tile, a face).
	lead: Node?,
	-- Right after the name, on its line ("You", "In use").
	badge: Node?,
	control: Node?,
	-- The control under the text, full width.
	stacked: boolean?,
	-- The row leads somewhere: the whole row takes the click, a chevron
	-- (or `value`, what is chosen there, then the chevron) at its end.
	on_click: (() -> ())?,
	-- Where it leads is a web address (About's Release notes): its context
	-- menu is a link's (ds/links: Open, the click; Copy link).
	href: (string | () -> string)?,
	value: (string | Message | () -> string)?,
	-- A value, state or function, followed (a role that can't change it).
	disabled: Props_disabled,
	-- The name in the danger's ink (Sign out, Delete account).
	danger: boolean?,
	-- The name and its line under it each on one line, fading at the end
	-- where they don't fit, and the control kept beside them (an address, a
	-- path, a list of repositories): never wrapped under.
	truncate: boolean?,
	-- The name, or the line under it, is an email address: blurred while
	-- addresses are hidden (ds.PrivateText).
	private_label: boolean?,
	private_description: boolean?,
	-- Who made it, on a line of its own under the name, over the
	-- description, in the same quiet ink (a plugin's, a template's).
	byline: (string | Message | () -> string)?,
	-- A second line under the description, in its tertiary ink, wrapped
	-- whole even in a truncating row (what a built-in plugin is allowed).
	detail: (string | Message | () -> string?)?,
	-- At the detail line's end, while it shows: a small action of its own
	-- (a plugin's Update), so the row's control stays the same on every row.
	detail_action: Node?,
}

function M.Row(props: RowProps): Node
	local text = ui.column {
		grow = 1,
		min_w = 0,
		gap = "$zeron.space.hair",
		ui.row {
			gap = "$space.2",
			items = "center",
			min_w = 0,
			if props.private_label
				then PrivateText {
					props.label,
					truncate = props.truncate,
					grow = if props.truncate then 1 else nil,
					text_size = "$font.size.md",
					line_height = "$font.line.md",
					weight = "$font.weight.medium",
					color = if props.danger then "$zeron.tone.danger.ink" else "$text.primary",
				}
				elseif props.truncate
				then FadeText {
					props.label,
					-- (With a badge, only as wide as the name, so the badge sits
					-- right after it; it still shrinks to fade.)
					grow = if props.badge then nil else 1,
					shrink = 1,
					min_w = 0,
					text_size = "$font.size.md",
					line_height = "$font.line.md",
					weight = "$font.weight.medium",
					color = if props.danger then "$zeron.tone.danger.ink" else "$text.primary",
				}
				else ui.text {
					props.label,
					text_size = "$font.size.md",
					line_height = "$font.line.md",
					weight = "$font.weight.medium",
					color = if props.danger then "$zeron.tone.danger.ink" else "$text.primary",
					wrap = true,
				},
			props.badge,
		},
		if props.byline and props.truncate
			then FadeText { props.byline, min_w = 0, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary" }
			elseif props.byline
			then ui.text { props.byline, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary", wrap = true }
			else nil,
		if props.description and props.private_description
			then PrivateText {
				props.description,
				truncate = props.truncate,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = "$text.tertiary",
			}
			elseif props.description and props.truncate
			then FadeText { props.description, min_w = 0, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary" }
			elseif props.description
			then ui.text { props.description, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary", wrap = true }
			else nil,
		if props.detail
			then ui.show(function(): boolean
				local d = control.value(props.detail)
				return type(d) == "string" and d ~= ""
			end, ui.row {
				w_full = true,
				gap = "$space.3",
				items = "start",
				ui.text {
					function(): string
						return control.value(props.detail) or ""
					end,
					grow = 1,
					min_w = 0,
					text_size = "$font.size.sm",
					line_height = "$font.line.sm",
					color = "$text.tertiary",
					wrap = true,
				},
				props.detail_action,
			})
			else nil,
	}
	local lead: Node? = props.lead
	if not lead and props.icon then
		lead = ui.row {
			w = 28,
			h = 28,
			shrink = 0,
			items = "center",
			justify = "center",
			rounded = "$radius.lg",
			bg = "$zeron.secondary.bg",
			color = "$text.secondary",
			Icon { name = props.icon, size = 16 },
		}
	end
	local trail: Node? = props.control
	if props.on_click then
		trail = ui.row {
			gap = "$space.1",
			items = "center",
			shrink = 0,
			color = "$text.tertiary",
			if props.value then ui.text { props.value, text_size = "$font.size.md", color = "$text.secondary" } else nil,
			-- The chevron's stroke ends at 15/24 of its box (the icon's own
			-- drawing), so the box is moved out by the rest: its ink, not its
			-- box, sits on the edge the row's other controls end on.
			ui.div { y = -0.5, x = CHEVRON * (1 - 15 / 24), Icon { name = "alt-arrow-right", size = CHEVRON } },
		}
	end
	local function disabled(): boolean
		return control.value(props.disabled) == true
	end
	local row: { [any]: any } = {
		w_full = true,
		min_h = "$zeron.settings.row",
		px = "$space.4",
		py = "$space.3",
		gap = if props.stacked then "$space.3" else "$space.4",
		disabled = props.disabled,
		opacity = if props.disabled
			then function(): any
				return if disabled() then "$zeron.disabled" else nil
			end
			else nil,
	}
	if props.on_click then
		row.role = "button"
		row.label = props.label
		row.focusable = function(): boolean
			return not disabled()
		end
		row.cursor = "pointer"
		row.on_click = props.on_click
		local href = props.href
		if href then
			row.context_menu = Links.outside_menu(function(): string?
				return if type(href) == "function" then href() else href
			end, props.on_click)
		end
		row.hover = { bg = "$hover" }
		row.active = { bg = "$pressed" }
		-- Inside the card's clip, the ring sits over the row's edge.
		row.focus = { outline = "$zeron.focus.width", outline_color = "$zeron.focus.ring", outline_offset = -2 }
	end
	if props.stacked then
		row.direction = "column"
		row.items = "stretch"
		table.insert(row, ui.row { gap = "$space.3", items = "center", lead or text, if lead then text else nil })
		if trail then
			table.insert(row, trail)
		end
	else
		row.direction = "row"
		row.items = "center"
		-- (A row that leads somewhere keeps its chevron beside the text, which
		-- wraps instead: a lone chevron on a line of its own reads as broken.)
		local keeps_trail = props.truncate or props.on_click ~= nil
		row.flex_wrap = not keeps_trail
		-- A lead sits closer to its name than the control does, as in a
		-- stacked row and an option card, so names line up across all three.
		-- (Their row holds the least width, and the control wraps past it;
		-- truncated, the name gives way instead.)
		-- (From no width of its own: a long line under the name wraps beside
		-- the control rather than pushing it under the words.)
		table.insert(row, ui.row {
			grow = 1,
			basis = 0,
			min_w = if keeps_trail then 0 else MIN_TEXT,
			gap = "$space.3",
			items = "center",
			lead,
			text,
		})
		if trail then
			-- (A theme number, read as it is built: max_w takes points.)
			table.insert(row, ui.row { shrink = 0, max_w = ui.theme.get("zeron.settings.control") :: number?, items = "center", justify = "end", trail })
		end
	end
	return ui.row(row)
end

-- A settings window's own list of pages, down its side: an icon and a name
-- a row, the chosen one on the flat selected wash; `heading`s between runs.
-- (`class`: what plugins place a row by.)
-- (`tile`: a tile in the icon's place, a plugin's: its name's initial on
-- its hue, as its row on the Plugins page has it.)
-- (A list given as a table may name its rows and headings with messages,
-- `ui.t`; one from a state or a function names them with strings, read
-- from messages there: a state holds no message.)
export type NavItem = { value: any?, label: (string | Message)?, icon: Icon.Name?, tile: { label: string, hue: string?, svg: string? }?, heading: (string | Message)?, class: string? }
export type NavProps = {
	value: State<any>,
	-- (A state or a function for pages that come and go: each row stays its
	-- page's, keyed by its value, or its heading.)
	items: { NavItem } | State<{ NavItem }> | () -> { NavItem },
	-- Takes the choice instead of setting `value` (a router's).
	on_choose: ((value: any) -> ())?,
	w: number?,
	-- Its width, shared and kept (the main sidebar's): dragged at its right
	-- edge, between `min_w` and `max_w`, as the sidebar's is. Over `w`.
	width: State<number>?,
	min_w: number?,
	max_w: number?,
	-- Room kept over the rows, in points: the title bar's strip, where the
	-- list runs the window's full height with the window's buttons in it.
	top: number?,
	-- Standing in a card: no hairline at rest, only under the pointer at its edge.
	card: boolean?,
	-- Rows over `head`, always shown ("Back").
	lead: { NavItem }?,
	-- Over the rows, under the strip and `lead` (a search field).
	head: Node?,
	-- While true, the rows (not `lead`, not `head`) are put away: a
	-- search's results, drawn in `head`, stand in their place.
	hide_rows: (() -> boolean)?,
	-- After the rows (what others add to the list).
	foot: Node?,
	-- At its very foot, under everything: you and settings (ds.AccountRow),
	-- as the main sidebar's foot has them.
	account: Node?,
}

function M.Nav(props: NavProps): Node
	local function row(item: NavItem, lead_row: boolean): Node
		if item.heading then
			return ui.row {
				h = "$zeron.menu.heading",
				px = "$space.2",
				pt = "$space.2",
				items = "center",
				ui.text { item.heading, text_size = "$font.size.xs", weight = "$font.weight.medium", color = "$text.tertiary" },
			}
		else
			local function on(): boolean
				return props.value:get() == item.value
			end
			-- (A page is a tab of the list's, the open one selected: one Tab
			-- stop, the arrows between them. `lead` rows are buttons.)
			return ui.row {
				class = item.class,
				role = if lead_row then "button" else "tab",
				label = item.label,
				selected = if lead_row then nil else on,
				focusable = true,
				cursor = "pointer",
				w_full = true,
				h = "$zeron.menu.row",
				px = "$space.2",
				gap = "$space.2",
				items = "center",
				rounded = "$radius.md",
				bg = function()
					return if on() then "$zeron.selected.flat.bg" else "#00000000"
				end,
				color = function()
					return if on() then "$text.primary" else "$text.secondary"
				end,
				hover = { color = "$text.primary" },
				focus = { outline = "$zeron.focus.width", outline_color = "$zeron.focus.ring", outline_offset = -2 },
				on_click = function()
					if props.on_choose then
						props.on_choose(item.value)
					else
						props.value:set(item.value)
					end
				end,
				if item.tile
					then Tile {
						label = item.tile.label,
						-- (Its own icon, a glyph on the quiet wash; else its initial on its hue.)
						hue = if item.tile.svg then nil else item.tile.hue,
						size = 16,
						inside = if item.tile.svg
							then ui.svg { code = item.tile.svg, w = 10, h = 10, shrink = 0, tint = "$text.primary", aria_hidden = true }
							else nil,
					}
					elseif item.icon then ui.div { shrink = 0, y = -0.5, Icon { name = item.icon, size = 16 } }
					else nil,
				ui.text { item.label or "", text_size = "$font.size.md" },
			}
		end
	end
	local leads: { Node } = {}
	for _, item in props.lead or {} do
		table.insert(leads, row(item, true))
	end
	local rows: { Node } = {}
	local items = props.items
	if type(items) == "table" and getmetatable(items :: any) == nil then
		for _, item in items :: { NavItem } do
			table.insert(rows, row(item, false))
		end
	else
		local list = if type(items) == "function" then ui.computed(items :: () -> { NavItem }) else items
		table.insert(
			rows,
			-- (A page row each: `ui.each` passes more than the item.)
			ui.each(list :: any, function(item: NavItem): Node
				return row(item, false)
			end, {
				key = function(item: NavItem): any
					return item.value or `heading:{item.heading}`
				end,
				w_full = true,
				gap = "$zeron.list.gap",
			})
		)
	end
	-- The pages: one Tab stop (the open page), ↑ and ↓ between them.
	local function pages(): Node
		return ui.column {
			w_full = true,
			gap = "$zeron.list.gap",
			role = "tablist",
			label = locale.t("ds.settings.nav.pages"),
			focus_group = "vertical",
			table.unpack(rows),
		}
	end
	-- The pages scroll under what stays (Back, the search, what's added
	-- under them) when the window is too short for them all.
	local function scrolled(): Node
		return ui.scroll {
			w_full = true,
			grow = 1,
			min_h = 0,
			fade = { 12, 12 },
			fade_overflow = true,
			pages(),
		}
	end
	-- Its width: the shared one, dragged at its edge, or a fixed one.
	local width = props.width
	local lo, hi = props.min_w or 0, props.max_w or math.huge
	local edge_over = ui.state("settings_nav_edge_over", false)
	local dragging = ui.state("settings_nav_dragging", false)
	local resizing, from_x, from_w = false, 0, 0
	-- (The list's name is the tab list's: nothing round it says it again.)
	return ui.column {
		w = if width
			then function(): number
				return math.clamp(width:get(), lo, hi)
			end
			else props.w or 200,
		h_full = true,
		min_h = 0,
		shrink = 0,
		relative = true,
		px = "$space.2",
		pb = "$space.2",
		-- Under the strip, the rows keep their usual inset from it.
		pt = if props.top then ui.raw(props.top + (ui.theme.get("space.2") :: number), "the title bar's strip over the rows, then the list's own inset") else "$space.2",
		gap = "$zeron.list.gap",
		-- (Its hairline down its right is drawn over its edge, as the main
		-- sidebar's is, taking no width: the two lists' rows and feet stand
		-- at the same places, nothing shifting between them.)
		ui.div {
			absolute = true,
			right = 0,
			top = 0,
			bottom = 0,
			w = 1,
			pointer_events = "none",
			bg = function(): string
				if edge_over:get() or dragging:get() then
					return "$border.strong"
				end
				return if props.card then "#00000000" else "$border.subtle"
			end,
		},
		if #leads > 0 then ui.column { w_full = true, gap = "$zeron.list.gap", table.unpack(leads) } else nil,
		props.head,
		-- (While a search's results stand in the pages' place, an empty space
		-- grows in theirs, so what's under them, you and settings, keeps to
		-- the foot.)
		if props.hide_rows
			then ui.show(function(): boolean
				return not (props.hide_rows :: () -> boolean)()
			end, scrolled(), ui.div { grow = 1, min_h = 0 })
			else scrolled(),
		props.foot,
		if props.account then ui.div { w_full = true, shrink = 0, pt = "$space.1", props.account } else nil,
		-- Its edge: dragged, it resizes, following the pointer at once, as
		-- the main sidebar's does. (Pressed itself, it keeps the pointer.)
		if width
			then ui.div {
				absolute = true,
				right = 0,
				top = 0,
				bottom = 0,
				w = 6,
				cursor = "col_resize",
				label = locale.t("ds.settings.nav.resize"),
				on_hover = function(inside: boolean)
					edge_over:set(inside)
				end,
				on_press = function(e)
					from_x, from_w = e.x, math.clamp(width:peek(), lo, hi)
					resizing = true
					dragging:set(true)
				end,
				on_pointer_move = function(e)
					if resizing then
						width:set(math.clamp(from_w + e.x - from_x, lo, hi))
					end
				end,
				on_release = function()
					resizing = false
					dragging:set(false)
				end,
			}
			else nil,
	}
end


-- A page that lists what's installed: Plugins, Templates, laid out as the
-- GitHub plugin's pull request board is: tabs over the list (what's
-- installed, what's yours), each with its count, and a New button at the
-- rule's far end; one card of rows headed by its search (a quiet field, as
-- the board's), each row washed edge to edge under the pointer so its
-- controls read as its own across a wide row. In the card under the search,
-- as what shows changes: a skeleton the first time it loads (asked again
-- behind rows already shown, they stay), "nothing matches" for a search,
-- or the page's own empty state for a tab with nothing; the search stays
-- where it is through all of them (it keeps the keys). A state of the
-- page's own (offline) stands in the card's place. A quiet hint under it
-- all if the page has one. The rows are the page's (`row`, keyed by
-- `key`), as in `Settings.Each`, and so are its dialogs (`overlays`).
--
--   Settings.Catalog {
--     title = "Plugins", description = "…",
--     trail = Node,                                   -- optional, at the title's end (a device choice)
--     search = { value = query, placeholder = "Search plugins" },
--     tabs = { value = tab, label = "Plugins", options = { { value = "installed", label = "Installed", count = … }, … } },
--     new = { label = "New plugin", on_click = … },
--     import = { label = "Import", on_click = … },    -- optional, before New
--     notice = function() return Node end,            -- optional, above the rows
--     notice_when = function() return true end,       -- optional: while false, no room
--     status = function() return "ready" end,   -- or "loading", or a key of `states`
--     states = { offline = function() return Callout { … } end },
--     found = found, row = row, key = function(p) return p.id end,
--     none = function(tab) return EmptyState { … } end,
--     hint = { id = "plugins-hint", text = "…" },
--     overlays = { Dialog { … } },
--   }
export type CatalogProps = {
	-- (The words may be messages, `ui.t`.)
	title: string | Message,
	description: (string | Message)?,
	-- Something at the title's end, as a Page's (which device's), and
	-- whether it's on the title's line (`trail_top`, a Page's).
	trail: Node?,
	trail_top: boolean?,
	search: { value: State<string>, placeholder: string | Message },
	tabs: { value: State<string>, label: string | Message, options: { any } },
	new: { label: string | Message, on_click: () -> () }?,
	-- A second way in beside New (a file to bring in), quieter than it.
	import: { label: string | Message, on_click: () -> () }?,
	-- Something waiting for the person, between the tabs and the rows
	-- (it draws nothing when there's nothing to say).
	notice: (() -> Node)?,
	-- Whether there is one now: while false, the notice is out of the
	-- column, so it takes no room (an empty list still takes a gap).
	notice_when: (() -> boolean)?,
	status: () -> string,
	states: { [string]: () -> Node }?,
	found: State<{ any }>,
	row: (item: any, i: number, now: State<any>) -> Node,
	key: (item: any) -> any,
	none: (tab: string) -> Node,
	hint: { id: string?, text: string | Message }?,
	overlays: { Node }?,
}

-- The tabs' far end: New, and Import… before it when the page has one.
local function trail(props: CatalogProps): Node?
	local new = if props.new
		then Button {
			tone = "secondary",
			size = "sm",
			on_click = props.new.on_click,
			props.new.label,
		}
		else nil
	local import = props.import
	if not import then
		return new
	end
	return ui.row {
		gap = "$space.2",
		items = "center",
		Button { tone = "ghost", size = "sm", on_click = import.on_click, import.label },
		new,
	}
end

function M.Catalog(props: CatalogProps): Node
	local query, tab = props.search.value, props.tabs.value
	-- (What the card says in place of rows: in from its edges as the board's.)
	local function said(node: Node): Node
		return ui.column { w_full = true, py = "$space.6", px = "$space.4", node }
	end
	local function nomatch(): Node
		return said(EmptyState {
			icon = "search",
			title = locale.t("ds.settings.catalog.nothing_matches", { query = query }),
			body = locale.t("ds.settings.catalog.try_another"),
			size = "sm",
			action = {
				label = locale.t("ds.settings.catalog.clear_search"),
				on_click = function()
					query:set("")
				end,
			},
		})
	end
	-- The rows, each its item's as the list changes, a hairline over all but
	-- the first (`Settings.Each`'s, inside this card).
	local function rows(): Node
		local key = props.key
		local function first(item: any): boolean
			local head = props.found:get()[1]
			return head ~= nil and key(head) == key(item)
		end
		return ui.each(props.found, function(item: any, i: number, now: State<any>): Node
			return ui.column {
				w_full = true,
				ui.show(function(): boolean
					return not first(item)
				end, M.Rule()),
				CatalogRow { row = props.row(item, i, now) },
			}
		end, { key = key, w_full = true })
	end
	-- Under the search: the skeleton, the rows, or what it says instead.
	local function inside(key: string): Node
		if key == "loading" then
			return ui.column {
				w_full = true,
				skeleton.Row { trail = 40 },
				M.Rule(),
				skeleton.Row { title = 96, line = 280, trail = 40 },
				M.Rule(),
				skeleton.Row { title = 120, line = 200, trail = 40 },
			}
		end
		local which = string.match(key, "^list:(%a+)") or "installed"
		if string.match(key, ":nomatch$") then
			return nomatch()
		elseif string.match(key, ":none$") then
			return said(props.none(which))
		end
		return rows()
	end
	-- The search heading the card: a quiet field inset evenly, its corner the
	-- card's less its edge and that inset (for its focus ring), as the board's.
	local search = ui.div {
		w_full = true,
		shrink = 0,
		p = "$space.1",
		theme = { radius = { md = control.nest("$radius.xl", 1, "$space.1") } },
		-- (Its glyph's ink on the rows' edge, over their tiles: the rows' inset
		-- less the card's edge, this inset and the glyph's own side bearing.)
		SearchField {
			value = query,
			placeholder = props.search.placeholder,
			name = props.search.placeholder,
			size = "sm",
			quiet = true,
			w_full = true,
			pl = M.TEXT_INSET - 1 - ((ui.theme.get("space.1") :: number?) or 4) - SEARCH_INK,
		},
	}
	local function card(): Node
		return ui.column {
			id = "catalog-card",
			w_full = true,
			rounded = "$radius.xl",
			bg = material.pick("$zeron.settings.tint", "$zeron.settings.card"),
			border = 1,
			border_color = "$zeron.settings.edge",
			overflow = "hidden",
			search,
			M.Rule(),
			-- One swap for what's under it: a tab is a page of its own; within
			-- it, found or not. (Asked again with rows showing, they stay.)
			ui.presence(function(): string
				local s = props.status()
				if #props.found:get() > 0 then
					return `list:{tab:get()}`
				elseif s ~= "ready" then
					return "loading"
				end
				return if query:get() ~= "" then `list:{tab:get()}:nomatch` else `list:{tab:get()}:none`
			end, inside, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
		}
	end
	local function body(key: string): Node
		local own = props.states and props.states[key]
		if own then
			return own()
		end
		return card()
	end
	local column: { any } = {
		w_full = true,
		gap = "$space.4",
		ui.row {
			w_full = true,
			items = "end",
			gap = "$space.2",
			-- In to the rows' words, as the title is: the tab's label (its
			-- space.1 inside the tab) on that edge, its underline hanging
			-- past it as a wash does.
			Tabs {
				value = tab,
				label = props.tabs.label,
				rule = true,
				-- (The rule runs the card's width, as the board's does.)
				inset = M.inset(ui.theme.get("space.1") :: number?),
				options = props.tabs.options,
				trail = trail(props),
			},
		},
		if props.notice and props.notice_when
			then ui.show(props.notice_when, ui.column { w_full = true, (props.notice :: () -> Node)() })
			elseif props.notice then props.notice()
			else nil,
		-- The card (searched, loading or not), or a state of the page's own
		-- in its place.
		ui.presence(function(): string
			local s = props.status()
			return if props.states and props.states[s] then s else "card"
		end, body, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
	}
	if props.hint then
		table.insert(column, ui.text {
			id = props.hint.id,
			props.hint.text,
			-- In to the rows' words, as the title is.
			px = M.inset(),
			w_full = true,
			wrap = true,
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = "$text.tertiary",
		})
	end
	local page: { any } = {
		title = props.title,
		description = props.description,
		trail = props.trail,
		trail_top = props.trail_top,
		ui.column(column),
	}
	for _, overlay in props.overlays or {} do
		table.insert(page, overlay)
	end
	return M.Page(page :: any)
end

return M
