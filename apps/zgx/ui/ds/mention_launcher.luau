-- The `@` list as a launcher (ds/composer_mentions draws it for a trigger
-- given sections): what's typed on top with how many match; the scopes
-- under it (All, then each section), Tab and Shift+Tab switching tab;
-- the rows in their sections, each under a small heading in All: one line
-- each, a menu row's, its icon in a small tile, its name (the part typed
-- in bold), where it is at its end in the muted ink (a file's folder, a
-- chat's project, what an attachment is), and ↵ on the lit one; a legend
-- of the keys at its foot. The menus' glass panel, their row and type.

local Icon = require("@self/icon")
local FileIcon = require("@self/file_icons")
local FadeText = require("@self/fade_text")
local Menu = require("@self/menu")
local Kbd = require("@self/kbd")
local Tile = require("@self/tile")
local locale = require("@self/locale")

local M = {}

-- The panel's width, how many rows show before it scrolls, how long a
-- row's place may be before it's cut from its start.
M.WIDTH = 440
local ROWS, PLACE = 8, 34
-- The time's column and the ↵ key's slot, the same on every row: what's at
-- a row's end lines up down the list, and lighting a row moves nothing.
local TIME_W, KEY_W = 34, 22
-- A row's height and its tile's: a menu row's, and its glyph's box.
local function row_h(): number
	return (ui.theme.get("zeron.menu.row") :: number?) or 28
end
local ROW_ONE = 28
local HEAD_H, SCOPES_H, FOOT_H, HEADING_H = 36, 32, 32, 24

export type Section = { id: string, label: any }
export type Entry = { item: any, i: number }

export type Props = {
	-- The token at the caret (its trigger and query).
	token: any,
	-- The rows in their order (a section's together, sections in order).
	shown: any,
	active: State<number>,
	-- "all", or a section's id.
	scope: State<string>,
	sections: { Section },
	-- How many match, by scope ("all" too).
	counts: () -> { [string]: number },
	loading: () -> boolean,
	-- What an empty scope says.
	empty: (scope: string, query: string) -> string,
	choose: (item: any) -> (),
}

-- `label` with the part that matches `query` in bold (its first match,
-- whatever its case); all of it plain without one.
local function title(label: string, query: string): Node
	local s, e = nil, nil
	if query ~= "" then
		s, e = string.find(string.lower(label), string.lower(query), 1, true)
	end
	if not s or not e then
		return FadeText { label, literal = true, shrink = 1, min_w = 0, text_size = "$font.size.md", color = "$text.primary" }
	end
	local from, to: number = s :: number, e :: number
	local rich: { [any]: any } = { shrink = 1, min_w = 0, truncate = true, text_size = "$font.size.md", color = "$text.primary" }
	if from > 1 then
		table.insert(rich, ui.span { string.sub(label, 1, from - 1), literal = true })
	end
	table.insert(rich, ui.span {
		string.sub(label, from, to),
		literal = true,
		-- (A theme token, which a span takes: zgx's definitions allow a
		-- span's weight numbers only.)
		weight = "$font.weight.semibold" :: any,
	})
	if to < #label then
		table.insert(rich, ui.span { string.sub(label, to + 1), literal = true })
	end
	return ui.rich(rich)
end

-- Where a row's item is, cut from its start when long (`…/chat/screens`):
-- its end is what tells two apart.
local function place(text: string): string
	local n = utf8.len(text) or #text
	if n <= PLACE then
		return text
	end
	local from = utf8.offset(text, n - PLACE + 2) or 1
	return "…" .. string.sub(text, from)
end

-- A row's tile: its file's icon, or its own glyph, on a quiet well (a menu
-- row's glyph box, the icon two points in).
local function tile(item: any): Node
	-- A chat in a project: its project's tile, as the sidebar draws it.
	local t = item.tile
	if t then
		return Tile { label = t.label, hue = t.hue, src = t.src, emoji = t.emoji, project = t.project, size = 18 }
	end
	local lead: Node
	if item.file or item.dir or item.file_icon then
		lead = FileIcon { name = item.file_icon or item.label, dir = item.dir, size = 14 }
	else
		lead = Icon { name = item.icon or "paperclip", size = 14 }
	end
	return ui.div {
		w = "$zeron.menu.glyph.box",
		h = "$zeron.menu.glyph.box",
		shrink = 0,
		items = "center",
		justify = "center",
		rounded = "$radius.md",
		bg = "$zeron.chip.well",
		color = "$text.secondary",
		lead,
	}
end

function M.Panel(props: Props): Node
	local function query(): string
		local t = props.token:get()
		return if t then t.query else ""
	end
	local function trigger(): string
		local t = props.token:get()
		return if t then t.trigger else "@"
	end
	local function count_text(n: number): string
		return locale.s(if n == 1 then "ds.mention_launcher.result" else "ds.mention_launcher.results", { n = n })
	end
	-- The top: what's typed, and how many match in the scope chosen.
	local head = ui.row {
		h = HEAD_H,
		w_full = true,
		shrink = 0,
		px = "$space.2",
		gap = "$space.2",
		items = "center",
		ui.row {
			grow = 1,
			min_w = 0,
			items = "center",
			ui.text { trigger, shrink = 0, text_size = "$font.size.md", color = "$text.tertiary" },
			ui.text { query, min_w = 0, truncate = true, text_size = "$font.size.md", color = "$text.primary" },
		},
		ui.text {
			function(): string
				return count_text(props.counts()[props.scope:get()] or 0)
			end,
			shrink = 0,
			tabular = true,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		},
	}
	-- The scopes: All, then each section; the one chosen in the menus'
	-- selected wash.
	local scope_list: { Section } = { { id = "all", label = locale.t("ds.mention_launcher.all") } }
	for _, s in props.sections do
		table.insert(scope_list, s)
	end
	local scopes: { [any]: any } = {
		h = SCOPES_H,
		w_full = true,
		shrink = 0,
		px = "$space.1",
		gap = "$zeron.space.hair",
		items = "center",
		role = "tablist",
	}
	for _, s in scope_list do
		local function chosen(): boolean
			return props.scope:get() == s.id
		end
		table.insert(
			scopes,
			ui.button {
				role = "tab",
				selected = chosen,
				label = s.label,
				h = 24,
				px = "$space.2",
				gap = "$zeron.space.tight",
				items = "center",
				rounded = Menu.row_radius(),
				bg = function(): string
					return if chosen() then "$hover" else "#00000000"
				end,
				hover = { bg = "$hover" },
				focus = { outline = 0, border = 0 },
				focusable = false,
				on_click = function()
					props.scope:set(s.id)
					props.active:set(1)
				end,
				ui.text {
					s.label,
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
					color = function(): string
						return if chosen() then "$text.primary" else "$text.secondary"
					end,
				},
				ui.text {
					function(): string
						return tostring(props.counts()[s.id] or 0)
					end,
					tabular = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			}
		)
	end
	-- The rows, and in All a heading over each section's.
	local labels: { [string]: any } = {}
	for _, s in props.sections do
		labels[s.id] = s.label
	end
	local placed = ui.computed(function(): { any }
		local out = {}
		local all = props.scope:get() == "all"
		local last: string? = nil
		for i, item in props.shown:get() do
			if all and item.section ~= last then
				last = item.section
				-- (By its id: a message can't be held in a state.)
				table.insert(out, { heading = item.section or "" })
			end
			table.insert(out, { item = item, i = i })
		end
		return out
	end)
	local function row(item: any, i: number): Node
		local function lit(): boolean
			return props.active:get() == i
		end
		return ui.button {
			id = `mention-{i}`,
			role = "option",
			selected = lit,
			label = item.label,
			h = "$zeron.menu.row",
			w_full = true,
			px = "$space.2",
			gap = "$space.2",
			items = "center",
			rounded = Menu.row_radius(),
			bg = function(): string
				return if lit() then "$hover" else "#00000000"
			end,
			hover = {},
			active = { bg = "$pressed" },
			focus = { outline = 0, border = 0 },
			cursor = "pointer",
			on_hover = function(inside: boolean)
				if inside then
					props.active:set(i)
				end
			end,
			on_click = function()
				props.choose(item)
			end,
			tile(item),
			ui.row { grow = 1, min_w = 0, items = "center", title(item.label, query()) },
			if item.detail
				then ui.text {
					place(item.detail),
					literal = true,
					shrink = 1,
					min_w = 0,
					truncate = true,
					text_size = "$font.size.sm",
					color = "$text.tertiary",
				}
				else nil,
			-- When it last moved (a chat's), in its column; empty for none.
			ui.row {
				w = TIME_W,
				shrink = 0,
				justify = "end",
				if item.side
					then ui.text { item.side, literal = true, tabular = true, text_size = "$font.size.xs", color = "$text.tertiary" }
					else nil,
			},
			-- The ↵ key's slot: kept on every row, the key in it on the lit one.
			ui.row {
				w = KEY_W,
				shrink = 0,
				justify = "end",
				ui.show(lit, Kbd { keys = "enter", tone = "quiet" }),
			},
		}
	end
	-- (Keyed by place, as the list's other rows are: a row outlives a
	-- re-filter with the index it was built at otherwise.)
	local rows = ui.each(placed, function(entry: any): Node
		if entry.heading then
			return Menu.Heading { labels[entry.heading] or "" }
		end
		return row(entry.item, entry.i)
	end, {
		key = function(entry: any): string
			return if entry.heading then `h:{entry.heading}` else `r:{entry.i}:{entry.item.section or ""}:{entry.item.value}`
		end,
		gap = "$zeron.list.gap",
	})
	local function body_h(): number
		local h = 0
		local n = 0
		for _, entry in placed:get() do
			if entry.heading then
				h += HEADING_H
			else
				n += 1
				if n > ROWS then
					break
				end
				h += row_h() + 2
			end
		end
		return h + 2
	end
	local function legend(keys: Node, what: string): Node
		return ui.row {
			gap = "$zeron.space.tight",
			items = "center",
			shrink = 0,
			keys,
			ui.text { what, text_size = "$font.size.xs", color = "$text.tertiary" },
		}
	end
	local foot = ui.row {
		h = FOOT_H,
		w_full = true,
		shrink = 0,
		px = "$space.2",
		gap = "$space.4",
		items = "center",
		legend(ui.row { gap = "$zeron.space.hair", Kbd { keys = "up" }, Kbd { keys = "down" } }, locale.s("ds.mention_launcher.navigate")),
		legend(Kbd { keys = "enter" }, locale.s("ds.mention_launcher.insert")),
		legend(Kbd { keys = "tab" }, locale.s("ds.mention_launcher.switch_tab")),
	}
	local function rule(): Node
		return ui.div { h = 1, w_full = true, shrink = 0, bg = "$border.subtle", aria_hidden = true }
	end
	return ui.column {
		id = "mention-launcher",
		-- (A press in it, a row's, a tab's or its blank room's, leaves the
		-- draft the keys: the list hangs from it.)
		focus_on_press = false,
		w_full = true,
		gap = 0,
		head,
		ui.row(scopes),
		rule(),
		ui.column {
			w_full = true,
			p = "$space.1",
			gap = 0,
			ui.show(props.loading, ui.row {
				h = ROW_ONE,
				px = "$space.2",
				items = "center",
				ui.text { locale.t("ds.mention_launcher.loading"), text_size = "$font.size.sm", color = "$text.tertiary" },
			}),
			ui.show(function(): boolean
				return not props.loading() and #props.shown:get() == 0
			end, ui.row {
				id = "mention-empty",
				h = "$zeron.menu.row",
				px = "$space.2",
				items = "center",
				ui.text {
					function(): string
						return props.empty(props.scope:get(), query())
					end,
					text_size = "$font.size.sm",
					color = "$text.tertiary",
				},
			}),
			ui.scroll {
				w_full = true,
				h = body_h,
				overscroll = "contain",
				role = "listbox",
				label = locale.t("ds.mention_launcher.results_label"),
				rows,
			},
		},
		rule(),
		foot,
	}
end

return M
