-- Mentions in the draft: `@` a file or folder, `/` a command, `$` a skill
-- (Zeron's triggers, crates/ui/src/composer.rs: `@` and `/` after a space
-- or at a start, `$` not before a digit, none inside code). As one is
-- typed, a popover opens at the caret with what matches, its first row
-- lit; ↑ and ↓ move (the list scrolling to keep it in view), Enter or Tab
-- take the row (with no row, Enter sends the draft), Escape (or a click
-- away from it) puts it away for that token.
-- It shows while the caret is in a token, the field has the keys, the
-- window is at the front and the composer is in view, unless that token's
-- list was put away; the caret leaving the token, or a trigger typed, ends
-- that. Hidden is never put away: the window going to the back, Settings
-- over the page, the field losing the keys. A row chosen lands in the draft
-- as its token and a space: a file by its name alone (as Zeron's chips name
-- it), the shortest end of its path that tells it from another of that
-- name; the path it stands for is kept beside the draft (`refs`).
--
-- A token in the draft is styled in type alone (zgx's text has no inline
-- chips, and a wash made a selection of every mention): its trigger greyed
-- as a markdown marker is, a file's name in the text's ink a weight up, a
-- command's or a skill's in the accent. Quiet at rest, plainly a
-- reference, calm in a long sentence.
--
--   local m = mentions.use { field = field, value = draft, sources = { ["@"] = files, ["/"] = commands, ["$"] = skills } }
--   ui.popover(m.popover(anchor))    -- the pill as the anchor
--   on_key = function(e) if m.keys(e) then return true end … end
--   on_caret = m.on_caret, highlight = mentions.highlight

local Icon = require("@self/icon")
local FileIcon = require("@self/file_icons")
local FadeText = require("@self/fade_text")
local Menu = require("@self/menu")
local Skeleton = require("@self/skeleton")
local Launcher = require("@self/mention_launcher")

local M = {}
-- Prints the caret and each choice to the host's log (debugging the keys).
M.debug = false

local tokens = require("@self/mention_tokens")
local locale = require("@self/locale")

export type Trigger = tokens.Trigger
export type Item = tokens.Item
export type Sources = tokens.Sources
export type Token = tokens.Token

-- The words' side, for callers that have the one module: the token at a
-- caret, a pick, the list's filter, a file's own name, the highlight.
M.token = tokens.token
M.pick = tokens.pick
M.filter = tokens.filter
M.short = tokens.short
M.highlight = tokens.highlight

-- How many rows show before the list scrolls, a row's height, the panel's
-- width.
local ROWS, ROW, WIDTH = 7, 32, 320

export type Use = {
	field: any,
	value: State<string>,
	-- What each trigger lists: a table, or a state of one (a search's
	-- results, arriving as the query is typed).
	-- (`any`: a union with the state type is past luau-lsp's type functions.)
	sources: any,
	-- A heading over each trigger's list.
	titles: { [string]: string }?,
	-- What an empty list says, by trigger.
	empty: { [string]: string }?,
	-- Called with the item taken (after it lands in the draft).
	-- (And the token it is in the draft: a file's is its shortest name.)
	on_choose: ((item: Item, trigger: Trigger, token: string) -> ())?,
	-- Whether a trigger opens anything (an agent's "Use $ for skills"
	-- off: `$` is ordinary text); by default, every one.
	enabled: ((trigger: Trigger) -> boolean)?,
	-- Whether its composer is in view (by default, it is).
	in_view: (() -> boolean)?,
	-- Whose draft it is (a composer that moves between chats: the chat): a
	-- list put away is that draft's alone.
	key: (() -> any)?,
	-- A trigger whose list is a launcher (ds/mention_launcher): its
	-- sections, in their order, each item saying its `section`. All shows
	-- each section's first few; a section's scope, all of its.
	sections: { [string]: { Launcher.Section } }?,
}

-- How many of a section All shows.
local ALL_EACH = 6

-- Wires the mentions to a draft: returns what the composer needs.
-- (Called as a component is built: its states are the component's own.)
function M.use(opts: Use)
	local function sources(): any
		local given: any = opts.sources
		if type(given) == "userdata" then
			return given:get()
		end
		return given
	end
	local caret = ui.use_state(0)
	-- Whether the field has the keys (its own focus and blur say so).
	local focused = ui.use_state(false)
	-- The token whose list was put away (Escape, a click away from it), by
	-- where it starts, for each draft (`key`): that token's alone, see below.
	local dismissed = ui.use_state({} :: { [any]: number })
	local function key_now(): any
		return if opts.key then opts.key() else ""
	end
	-- Where the token put away in the draft shown starts (-1: none).
	local function away_from(): number
		return dismissed:get()[key_now()] or -1
	end
	local function dismiss(key: any, from: number?)
		local all = dismissed:peek()
		if all[key] ~= from then
			local next = table.clone(all)
			next[key] = from
			dismissed:set(next)
		end
	end
	local active = ui.use_state(1)
	local token = ui.computed(function(): Token?
		local t = M.token(opts.value:get(), caret:get())
		if t and opts.enabled and not opts.enabled(t.trigger) then
			return nil
		end
		return t
	end)
	-- A launcher's scope ("all", or a section's id): All as each token
	-- opens, kept while it's typed at.
	local scope = ui.use_state("all")
	local function sections_of(t: Token?): { Launcher.Section }?
		return if t and opts.sections then opts.sections[t.trigger] else nil
	end
	-- What matches, and a launcher's by section, in the sections' order.
	local matched = ui.computed(function(): { [string]: { Item } }?
		local t = token:get()
		local secs = sections_of(t)
		if not t or not secs then
			return nil
		end
		local by: { [string]: { Item } } = {}
		for _, item in M.filter(sources()[t.trigger] or {}, t.query) do
			local id = (item :: any).section or ""
			by[id] = by[id] or {}
			table.insert(by[id], item)
		end
		return by
	end)
	local shown = ui.computed(function(): { Item }
		local t = token:get()
		if not t then
			return {}
		end
		local by = matched:get()
		local secs = sections_of(t)
		if by and secs then
			local out: { Item } = {}
			local only = scope:get()
			for _, sec in secs do
				if only == "all" or only == sec.id then
					for n, item in by[sec.id] or {} do
						if only == "all" and n > ALL_EACH then
							break
						end
						table.insert(out, item)
					end
				end
			end
			return out
		end
		local items = sources()[t.trigger]
		if not items then
			return {}
		end
		return M.filter(items, t.query)
	end)
	-- How many match in each scope (All: all of them, not just those shown).
	local function counts(): { [string]: number }
		local out: { [string]: number } = { all = 0 }
		for id, list in matched:get() or {} do
			out[id] = #list
			out.all += #list
		end
		return out
	end
	-- A token of its own (another place, another trigger): All again.
	local scope_for = ""
	ui.effect(function()
		local t = token:get()
		local at = if t then `{t.trigger}{t.from}` else ""
		if at ~= scope_for then
			scope_for = at
			if scope:peek() ~= "all" then
				scope:set("all")
			end
		end
	end)
	local function loading(): boolean
		local t = token:get()
		return t ~= nil and sources()[t.trigger] == nil
	end
	-- Whether the list could show at all: the field has the keys, the window
	-- is at the front, and the composer is in view (Settings standing over
	-- the page, it has nothing to hang from). Any of these gone hides the
	-- list; none of them puts it away.
	local function live(): boolean
		if not focused:get() or ui.window.active:get() == false then
			return false
		end
		return not opts.in_view or opts.in_view()
	end
	-- (The same, read without following it.)
	local function live_now(): boolean
		if not focused:peek() or ui.window.active:peek() == false then
			return false
		end
		return not opts.in_view or opts.in_view()
	end
	-- How many times the window has gone to the back: a popover closed as it
	-- went is told from a press away by it (see the popover's `on_close`),
	-- whenever the next frame comes (none may, until it's back).
	local away = 0
	ui.effect(function()
		if ui.window.active:get() == false then
			away += 1
		end
	end)
	-- The list shows while the caret is in a token whose list wasn't put
	-- away, wherever it could show. Nothing else decides it.
	local function open(): boolean
		local t = token:get()
		return t ~= nil and away_from() ~= t.from and live()
	end
	-- Where the list is scrolled (the launcher's or the list's: one shows).
	local offset = ui.use_state(0)
	-- The lit row brought into view, as the keys move it: past the rows
	-- that show, the list scrolls with it.
	local function reveal(i: number)
		local t = token:peek()
		if sections_of(t) then
			Launcher.reveal(offset, shown:peek(), scope:peek() == "all", i)
			return
		end
		local lines = {}
		for _ = 1, #shown:peek() do
			table.insert(lines, "row")
		end
		Menu.reveal(offset, lines, i, ROWS, ROW)
	end
	-- A new token (or a new query) lights the first row again, the list at
	-- its top.
	ui.effect(function()
		token:get()
		active:set(1)
		offset:set(0)
	end)
	-- A list put away stays away for its token alone: the caret out of it
	-- (moved, the token deleted), or its trigger typed again, and the next
	-- token's list opens. Typed further at, it stays away. (Another chat's draft shown in
	-- its place: that draft's own, kept with it.)
	local was_text = opts.value:peek()
	local was_key = key_now()
	ui.effect(function()
		local t = token:get()
		local text = opts.value:get()
		local key = key_now()
		-- (The draft just as it was with the token's trigger put in: that
		-- trigger was typed.)
		local typed = t ~= nil
			and #text == #was_text + 1
			and string.sub(text, 1, t.from) .. string.sub(text, t.from + 2) == was_text
		local moved = key ~= was_key
		was_text, was_key = text, key
		local gone = dismissed:peek()[key]
		if gone == nil or moved then
			return
		end
		if t == nil or t.from ~= gone or typed then
			dismiss(key, nil)
		end
	end)
	-- What each token in the draft stands for (a file's name, its path).
	-- A token that leaves the draft (replaced, deleted, edited) takes its
	-- path with it.
	local refs = ui.use_state({} :: { [string]: string })
	ui.effect(function()
		local now = opts.value:get()
		local kept = refs:peek()
		local stale: { string } = {}
		for tok in kept do
			if not string.find(now, tok, 1, true) then
				table.insert(stale, tok)
			end
		end
		if #stale > 0 then
			local next = table.clone(kept)
			for _, tok in stale do
				next[tok] = nil
			end
			refs:set(next)
		end
	end)
	local function choose(item: Item)
		local t = token:peek()
		local el = opts.field:peek()
		if not t or not el then
			return
		end
		local now = opts.value:peek()
		local from, to, text, tok = M.pick(now, t.to, item, sources())
		if not from or not to or not text or not tok then
			return
		end
		if M.debug then
			local a, b = el:selection_bytes()
			print(`[mentions] choose from {from} to {to} (field selection {a}..{b}) text {string.sub(now, 1, 80)}`)
		end
		-- The path the new token stands for, in place of the old one's.
		local old = string.sub(now, from + 1, to)
		local next = table.clone(refs:peek())
		next[old] = nil
		if item.file and item.value ~= string.sub(tok, 2) then
			next[tok] = item.value
		end
		refs:set(next)
		el:set_selection_bytes(from, to)
		el:replace_selection(text)
		if M.debug then
			local a, b = el:selection_bytes()
			print(`[mentions] after: selection {a}..{b} text {string.sub(opts.value:peek(), 1, 80)}`)
		end
		-- The caret is past what was put in (the field says so on its next
		-- caret event; said here too, so the list goes with the token at
		-- once). The token's session is over; the next trigger opens one.
		caret:set(from + #text)
		if opts.on_choose then
			opts.on_choose(item, t.trigger, tok)
		end
	end
	local function keys(e: any): boolean
		if not open() then
			return false
		end
		local list = shown:peek()
		-- A launcher's Tab and Shift+Tab: the next scope, the one before.
		local secs = sections_of(token:peek())
		if secs and e.key == "Tab" then
			local order = { "all" }
			for _, sec in secs do
				table.insert(order, sec.id)
			end
			local at = table.find(order, scope:peek()) or 1
			scope:set(order[(at - 1 + (if e.shift then -1 else 1)) % #order + 1])
			active:set(1)
			offset:set(0)
			return true
		end
		if e.key == "ArrowDown" or e.key == "ArrowUp" then
			if #list == 0 then
				return true
			end
			local step = if e.key == "ArrowDown" then 1 else -1
			local i = (active:peek() - 1 + step) % #list + 1
			active:set(i)
			reveal(i)
			return true
		elseif e.key == "Enter" or e.key == "Tab" then
			local item = list[active:peek()]
			if item then
				choose(item)
				return true
			end
			-- Nothing lit (no row matches, or the list is still loading):
			-- Enter is the composer's again, so it sends, as Zeron's does
			-- (composer.rs `enter_outcome`). A last word like `/tmp`,
			-- `$HOME` or `@someone` once ate it, and nothing went.
			return e.key == "Tab"
		elseif e.key == "Escape" then
			local t = token:peek()
			if t then
				dismiss(key_now(), t.from)
			end
			return true
		end
		return false
	end
	local function on_caret(_line: number, _column: number, _anchor: number, focus: number)
		if M.debug then
			print(`[mentions] caret {focus} text {string.sub(opts.value:peek(), 1, 80)}`)
		end
		caret:set(focus)
	end
	-- The field's focus and blur (`on_focus`, `on_blur`).
	local function on_focus(now: boolean)
		if focused:peek() ~= now then
			focused:set(now)
		end
	end
	local function descendant(): string?
		return if open() and shown:get()[active:get()] then `mention-{active:get()}` else nil
	end
	local function row(item: Item, i: number): Node
		local lead: Node
		if item.file then
			lead = FileIcon { name = item.label, dir = item.dir, size = 16 }
		else
			lead = Icon { name = item.icon or "command", size = 16 }
		end
		return ui.button {
			id = `mention-{i}`,
			role = "option",
			selected = function()
				return active:get() == i
			end,
			label = item.label,
			h = ROW,
			w_full = true,
			px = "$space.2",
			gap = "$space.2",
			items = "center",
			rounded = Menu.row_radius(),
			bg = function()
				return if active:get() == i then "$hover" else "#00000000"
			end,
			hover = {},
			active = { bg = "$pressed" },
			focus = { outline = 0, border = 0 },
			cursor = "pointer",
			on_hover = function(inside: boolean)
				if inside then
					active:set(i)
				end
			end,
			on_click = function()
				choose(item)
			end,
			ui.div { w = 18, h = 18, shrink = 0, items = "center", justify = "center", color = "$text.secondary", lead },
			FadeText { item.label, literal = true, shrink = 0, max_w = 180, text_size = "$font.size.md", color = "$text.primary" },
			if item.detail
				then FadeText { item.detail, literal = true, grow = 1, text_size = "$font.size.sm", color = "$text.tertiary" }
				else nil,
		}
	end
	local function popover(anchor: Node): { [any]: any }
		local function title(): string
			local t = token:get()
			if not t then
				return ""
			end
			local titles = opts.titles or {}
			return titles[t.trigger]
				or locale.s(if t.trigger == "@" then "ds.composer_mentions.files" elseif t.trigger == "/" then "ds.composer_mentions.commands" else "ds.composer_mentions.skills")
		end
		local function empty(): string
			local t = token:get()
			if not t then
				return ""
			end
			local says = opts.empty or {}
			local what = says[t.trigger]
				or locale.s(
					if t.trigger == "@" then "ds.composer_mentions.what.files"
					elseif t.trigger == "/" then "ds.composer_mentions.what.commands"
					else "ds.composer_mentions.what.skills"
				)
			return locale.s(if t.query == "" then "ds.composer_mentions.none_available" else "ds.composer_mentions.none_matching", { what = what })
		end
		-- keyed by position, not by item: a keyed row outlives a re-filter with the
		-- index it was built at, so the lit row never matched, and two skills
		-- installed in two places share a name
		local placed = ui.computed(function(): { { item: Item, i: number } }
			local out = {}
			for i, item in shown:get() do
				table.insert(out, { item = item, i = i })
			end
			return out
		end)
		local rows = ui.each(placed, function(entry: { item: Item, i: number }): Node
			return row(entry.item, entry.i)
		end, {
			key = function(entry: { item: Item, i: number }): string
				return tostring(entry.i)
			end,
			gap = "$zeron.list.gap",
		})
		local skeleton: { Node } = {}
		for _ = 1, 3 do
			table.insert(skeleton, Skeleton.Row { lead = 18, line = 0 })
		end
		return Menu.panel {
			id = "mention-popover",
			open = open,
			-- zgx closed it: a press away from it (the person putting it
			-- away), or the window going to the back, a dialog opening over
			-- it, the window minimized (hidden, not put away). zgx closes
			-- popovers before it says the window is at the back, so this is
			-- told once that event is over, before the next frame: still
			-- where it could show, with the same token, it was a press away.
			on_close = function()
				local t = token:peek()
				if not t then
					return
				end
				local from, aways, key = t.from, away, key_now()
				ui.after(0, function()
					local now = token:peek()
					if now and now.from == from and away == aways and key_now() == key and live_now() then
						dismiss(key, from)
					end
				end)
			end,
			at_caret = true,
			autofocus = false,
			w = function(): number
				return if sections_of(token:get()) then Launcher.width() else WIDTH
			end,
			placement = { side = "top", align = "start", offset = 8 },
			anchor = anchor,
			-- A launcher's trigger (`@`): the launcher; the others, the list.
			ui.show(function(): boolean
				return sections_of(token:get()) ~= nil
			end, Launcher.Panel {
				token = token,
				shown = shown,
				active = active,
				offset = offset,
				scope = scope,
				sections = ((opts.sections or {}) :: any)["@"] or {},
				counts = counts,
				loading = loading,
				empty = function(which: string, q: string): string
					return locale.s(
						if q == "" then `ds.mention_launcher.empty.{which}` else `ds.mention_launcher.none.{which}`,
						{ q = q }
					)
				end,
				choose = choose,
			}, ui.column {
				id = "mention-panel",
				-- (A press in it leaves the draft the keys: the list hangs
				-- from it.)
				focus_on_press = false,
				w_full = true,
				gap = 0,
				Menu.List {
					id = "mention-list",
					before = {
						Menu.Heading { title, first = true },
						ui.show(loading, ui.column { gap = "$zeron.list.gap", table.unpack(skeleton) }),
						ui.show(function()
							return not loading() and #shown:get() == 0
						end, ui.row {
							id = "mention-empty",
							h = ROW,
							px = "$space.2",
							items = "center",
							ui.text { empty, text_size = "$font.size.sm", color = "$text.tertiary" },
						}),
					},
					lines = rows,
					h = function(): number
						local lines = {}
						for _ = 1, #shown:get() do
							table.insert(lines, "row")
						end
						return Menu.list_height(lines, ROWS, ROW)
					end,
					offset = offset,
					role = "listbox",
					label = title,
				},
			}),
		}
	end
	return {
		token = token,
		shown = shown,
		active = active,
		open = open,
		keys = keys,
		on_caret = on_caret,
		on_focus = on_focus,
		descendant = descendant,
		choose = choose,
		popover = popover,
		refs = refs,
	}
end

return M
