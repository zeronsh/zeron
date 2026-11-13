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
local Menu = require("@self/menu")
local Kbd = require("@self/kbd")
local Tile = require("@self/tile")
local locale = require("@self/locale")

local M = {}

-- How many rows show before it scrolls, how long a row's place may be
-- before it's cut from its start. Its geometry is the theme's
-- (`zeron.menu.launcher`, ds/tokens): its width, its least, a tab's height, the
-- time's column and the ↵ key's slot, the same on every row (what's at a
-- row's end lines up down the list, and lighting a row moves nothing).
local ROWS, PLACE = 8, 34
local function row_h(): number
	return (ui.theme.get("zeron.menu.row") :: number?) or 28
end

-- The panel's width in this window: its own, narrowing with a small window
-- to its least (a margin of the page's gutter each side). A host that
-- feeds no window size (a headless test) gets its own.
function M.width(): number
	local own = (ui.theme.get("zeron.menu.launcher.width") :: number?) or 440
	local least = (ui.theme.get("zeron.menu.launcher.min_width") :: number?) or 320
	local margin = (ui.theme.get("space.4") :: number?) or 16
	local window = (ui :: any).window
	local state = if type(window) == "table" or type(window) == "userdata" then window.size else nil
	local ok, v = pcall(function(): any
		return if state ~= nil then state:get() else nil
	end)
	local w = if ok and type(v) == "table" and type(v.width) == "number" then v.width else 0
	if w <= 1 then
		return own
	end
	return math.max(least, math.min(own, w - 2 * margin))
end

export type Section = { id: string, label: any }
export type Entry = { item: any, i: number }

-- The list's lines: the rows, and in All (`all`) a heading over each
-- section's, by its id (a message can't be held in a state).
local function arrange(items: { any }, all: boolean): { any }
	local out = {}
	local last: string? = nil
	for i, item in items do
		if all and item.section ~= last then
			last = item.section
			table.insert(out, { heading = item.section or "", first = #out == 0 })
		end
		table.insert(out, { item = item, i = i })
	end
	return out
end
-- Their shapes, as ds/menu's `list_height` takes them.
local function shape(entries: { any }): { string }
	local lines = {}
	for _, entry in entries do
		table.insert(lines, if entry.heading then "heading" else "row")
	end
	return lines
end

-- Brings row `i` of `items` into the list's view (its `offset`), as the
-- keys light it (ds/menu `reveal`).
function M.reveal(offset: State<number>, items: { any }, all: boolean, i: number)
	local entries = arrange(items, all)
	for n, entry in entries do
		if entry.i == i then
			Menu.reveal(offset, shape(entries), n, ROWS, row_h())
			return
		end
	end
end

export type Props = {
	-- The token at the caret (its trigger and query).
	token: any,
	-- The rows in their order (a section's together, sections in order).
	shown: any,
	active: State<number>,
	-- Where its list is scrolled (`M.reveal` keeps the lit row in it).
	offset: State<number>?,
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

-- `label`'s text from `from` to `to` as spans, the part in `s`..`e` (what
-- matched) in bold.
local function spans(label: string, from: number, to: number, s: number?, e: number?): { Node }
	local out: { Node } = {}
	local function add(a: number, b: number, bold: boolean)
		if a > b then
			return
		end
		table.insert(out, ui.span {
			string.sub(label, a, b),
			literal = true,
			-- (A theme token, which a span takes: zgx's definitions allow a
			-- span's weight numbers only.)
			weight = if bold then "$font.weight.semibold" :: any else nil,
		})
	end
	if not s or not e or e < from or s > to then
		add(from, to, false)
		return out
	end
	add(from, math.max(from, s) - 1, false)
	add(math.max(from, s), math.min(to, e), true)
	add(math.min(to, e) + 1, to, false)
	return out
end

-- A name's extension (`.png`), kept whole as the rest of a long name is cut:
-- a file's or an attachment's, not a chat's title.
local function extension(label: string): number?
	local at = string.match(label, "^.+()%.[%w_%-]+$")
	if at and #label - at < 12 then
		return at
	end
	return nil
end

-- `label` with the part that matches `query` in bold (its first match,
-- whatever its case). Too long, it's cut with an ellipsis, a file's before
-- its extension (`Screenshot 2026-…png`), never clipped.
local function title(label: string, query: string, keep_ext: boolean): Node
	local s, e = nil, nil
	if query ~= "" then
		s, e = string.find(string.lower(label), string.lower(query), 1, true)
	end
	local look = { text_size = "$font.size.md", color = "$text.primary" }
	local at = if keep_ext then extension(label) else nil
	local function rich(from: number, to: number, cut: boolean): Node
		local r: { [any]: any } = { text_size = look.text_size, color = look.color }
		if cut then
			r.shrink = 1
			r.min_w = 0
			r.truncate = true
		else
			r.shrink = 0
		end
		for _, n in spans(label, from, to, s, e) do
			table.insert(r, n)
		end
		return ui.rich(r)
	end
	if not at then
		return rich(1, #label, true)
	end
	return ui.row {
		shrink = 1,
		min_w = 0,
		items = "baseline",
		rich(1, at - 1, true),
		rich(at, #label, false),
	}
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
	-- Each band (the top, the scopes, the list, the keys) is inset from the
	-- panel's edge as a menu's group is, its lines a row's: every band's text
	-- starts where the rows' tiles do and ends where their ↵ does, and each
	-- rule has the inset over it and under it.
	local function band(child: Node): Node
		return ui.column { w_full = true, shrink = 0, p = "$space.1", child }
	end
	-- The top: the trigger in the tiles' column, what's typed on the names'
	-- line (or what to type, quiet, while nothing is), how many match in the
	-- scope chosen at the end, each centred on the band's middle (each text
	-- a line of its own size high, so its box sits on its glyphs).
	local head = band(ui.row {
		h = "$zeron.menu.row",
		w_full = true,
		px = "$space.2",
		gap = "$space.2",
		items = "center",
		ui.row {
			w = "$zeron.menu.glyph.box",
			shrink = 0,
			justify = "center",
			ui.text { trigger, text_size = "$font.size.md", line_height = "$font.line.md", color = "$text.tertiary" },
		},
		ui.row {
			grow = 1,
			min_w = 0,
			gap = "$space.2",
			items = "center",
			ui.row {
				grow = 1,
				min_w = 0,
				items = "center",
				ui.show(function(): boolean
					return query() ~= ""
				end, ui.text {
					query,
					min_w = 0,
					truncate = true,
					text_size = "$font.size.md",
					line_height = "$font.line.md",
					color = "$text.primary",
				}, ui.text {
					locale.t("ds.mention_launcher.placeholder"),
					min_w = 0,
					truncate = true,
					text_size = "$font.size.md",
					line_height = "$font.line.md",
					color = "$text.tertiary",
				}),
			},
			ui.text {
				function(): string
					return count_text(props.counts()[props.scope:get()] or 0)
				end,
				shrink = 0,
				tabular = true,
				text_size = "$font.size.xs",
				line_height = "$font.line.xs",
				color = "$text.tertiary",
			},
		},
	})
	-- The scopes: All, then each section; the one chosen in the menus'
	-- selected wash (the palette's scopes'), a row's corner.
	local scope_list: { Section } = { { id = "all", label = locale.t("ds.mention_launcher.all") } }
	for _, s in props.sections do
		table.insert(scope_list, s)
	end
	local scopes: { [any]: any } = {
		w_full = true,
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
				h = "$zeron.menu.launcher.tab",
				px = "$space.2",
				gap = "$zeron.space.tight",
				items = "center",
				rounded = Menu.row_radius(),
				bg = function(): string
					return if chosen() then "$zeron.menu.selected" else "#00000000"
				end,
				hover = { bg = "$hover" },
				focus = { outline = 0, border = 0 },
				focusable = false,
				on_click = function()
					props.scope:set(s.id)
					props.active:set(1)
				end,
				ui.row {
					gap = "$zeron.space.tight",
					items = "center",
					ui.text {
						s.label,
						text_size = "$font.size.sm",
						line_height = "$font.line.sm",
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
						line_height = "$font.line.xs",
						color = "$text.tertiary",
					},
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
		return arrange(props.shown:get(), props.scope:get() == "all")
	end)
	local function row(item: any, i: number): Node
		local function lit(): boolean
			return props.active:get() == i
		end
		-- Where it is and when it last moved: one group at the row's end;
		-- the time in a column of its own where there is one (a chat's), so
		-- the places over and under line up.
		-- The lit row's text, cut short of ↵ by a short gap and faded over
		-- the space scale's 16 before that (`fade_x`, as FadeText fades): its
		-- content keeps the row's whole width, so nothing in it moves.
		local line = ui.ref()
		local function key_room(): number
			local key = (ui.theme.get("zeron.menu.launcher.key") :: number?) or 18
			local gap = (ui.theme.get("zeron.space.tight") :: number?) or 6
			return key + gap
		end
		local function full(): number?
			local w = line.width
			return if w and w > 0 then w else nil
		end
		local meta: { [any]: any } = { shrink = 100, min_w = 0, gap = "$space.2", items = "baseline", justify = "end" }
		if item.detail then
			table.insert(meta, ui.text {
				place(item.detail),
				literal = true,
				shrink = 1,
				min_w = 0,
				truncate = true,
				text_size = "$font.size.sm",
				color = "$text.tertiary",
			})
		end
		if item.side then
			table.insert(meta, ui.row {
				w = "$zeron.menu.launcher.time",
				shrink = 0,
				justify = "end",
				ui.text { item.side, literal = true, tabular = true, text_size = "$font.size.sm", color = "$text.tertiary" },
			})
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
			-- Its name and where it is, on one baseline, to the row's end; on
			-- the lit row ↵ over their end, which fades out under it.
			ui.row {
				grow = 1,
				min_w = 0,
				relative = true,
				ui.row {
					ref = line,
					grow = 1,
					min_w = 0,
					ui.row {
						grow = 1,
						min_w = 0,
						overflow = "hidden",
						mr = function(): number
							return if lit() and full() then key_room() else 0
						end,
						fade_x = function(): { number }
							return { 0, if lit() and full() then ((ui.theme.get("space.4") :: number?) or 16) else 0 }
						end,
						ui.row {
							w = full,
							shrink = 0,
							grow = 1,
							min_w = 0,
							gap = "$space.2",
							items = "baseline",
							ui.row { grow = 1, min_w = 0, title(item.label, query(), item.section ~= "chats") },
							if item.detail or item.side then ui.row(meta) else nil,
						},
					},
				},
				ui.row {
					absolute = true,
					top = 0,
					bottom = 0,
					right = 0,
					items = "center",
					pointer_events = "none",
					ui.show(lit, Kbd { keys = "enter", tone = "quiet" }),
				},
			},
		}
	end
	-- (Keyed by place, as the list's other rows are: a row outlives a
	-- re-filter with the index it was built at otherwise.)
	local rows = ui.each(placed, function(entry: any): Node
		if entry.heading then
			return Menu.Heading { labels[entry.heading] or "", first = entry.first }
		end
		return row(entry.item, entry.i)
	end, {
		key = function(entry: any): string
			return if entry.heading then `h:{entry.heading}:{entry.first}` else `r:{entry.i}:{entry.item.section or ""}:{entry.item.value}`
		end,
		gap = "$zeron.list.gap",
	})
	local function list_cap(): number
		return Menu.list_height(shape(placed:get()), ROWS, row_h())
	end
	local function legend(keys: Node, what: string): Node
		return ui.row {
			gap = "$zeron.space.tight",
			items = "center",
			shrink = 0,
			keys,
			ui.text { what, text_size = "$font.size.xs", line_height = "$font.line.xs", color = "$text.tertiary" },
		}
	end
	local foot = band(ui.row {
		h = "$zeron.menu.row",
		w_full = true,
		px = "$space.2",
		gap = "$space.4",
		items = "center",
		legend(ui.row { gap = "$zeron.space.hair", Kbd { keys = "up" }, Kbd { keys = "down" } }, locale.s("ds.mention_launcher.navigate")),
		legend(Kbd { keys = "enter" }, locale.s("ds.mention_launcher.insert")),
		legend(Kbd { keys = "tab" }, locale.s("ds.mention_launcher.switch_tab")),
	})
	-- What a scope with nothing in it (or nothing yet) says, a row's line.
	local function note(words: any, id: string?): Node
		return ui.row {
			id = id,
			h = "$zeron.menu.row",
			w_full = true,
			px = "$space.2",
			items = "center",
			ui.text { words, text_size = "$font.size.sm", color = "$text.tertiary" },
		}
	end
	return ui.column {
		id = "mention-launcher",
		-- (A press in it, a row's, a tab's or its blank room's, leaves the
		-- draft the keys: the list hangs from it.)
		focus_on_press = false,
		w_full = true,
		gap = 0,
		head,
		band(ui.row(scopes)),
		Menu.Rule(),
		Menu.List {
			id = "mention-list",
			before = {
				ui.show(props.loading, note(locale.t("ds.mention_launcher.loading"))),
				ui.show(function(): boolean
					return not props.loading() and #props.shown:get() == 0
				end, note(function(): string
					return props.empty(props.scope:get(), query())
				end, "mention-empty")),
			},
			lines = rows,
			h = list_cap,
			offset = props.offset,
			role = "listbox",
			label = locale.t("ds.mention_launcher.results_label"),
		},
		Menu.Rule(),
		foot,
	}
end

return M
