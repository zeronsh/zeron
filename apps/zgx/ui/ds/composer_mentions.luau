-- Mentions in the draft: `@` a file or folder, `/` a command, `$` a skill
-- (Zeron's triggers, crates/ui/src/composer.rs: `@` and `/` after a space
-- or at a start, `$` not before a digit, none inside code). As one is
-- typed, a popover opens at the caret with what matches, its first row
-- lit; ↑ and ↓ move, Enter or Tab take the row (with no row, Enter sends
-- the draft), Escape puts the popover
-- away until the token is typed at again. A row chosen lands in the draft
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
}

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
	local dismissed = ui.use_state(-1)
	local active = ui.use_state(1)
	local token = ui.computed(function(): Token?
		local t = M.token(opts.value:get(), caret:get())
		if t and opts.enabled and not opts.enabled(t.trigger) then
			return nil
		end
		return t
	end)
	local shown = ui.computed(function(): { Item }
		local t = token:get()
		if not t then
			return {}
		end
		local items = sources()[t.trigger]
		if not items then
			return {}
		end
		return M.filter(items, t.query)
	end)
	local function loading(): boolean
		local t = token:get()
		return t ~= nil and sources()[t.trigger] == nil
	end
	local function open(): boolean
		local t = token:get()
		-- (Its composer in view: Settings standing over the page, it has nothing to
		-- hang from.)
		if opts.in_view and not opts.in_view() then
			return false
		end
		return t ~= nil and dismissed:get() ~= t.from
	end
	-- The window going to the back closes every popover, as AppKit's do: not a
	-- dismissal. Back at the front, the list that was open opens again. (Which comes
	-- first, the window's state or the popover's closing, isn't for this to rely on.)
	local closed_from, closed_at = -1, -math.huge
	local away_from = -1
	local function closed(from: number)
		dismissed:set(from)
		closed_from, closed_at = from, os.clock()
		if ui.window.active:peek() == false then
			away_from = from
		end
	end
	local was_active = ui.window.active:peek()
	ui.effect(function()
		local now = ui.window.active:get()
		if now == was_active then
			return
		end
		was_active = now
		if not now then
			if os.clock() - closed_at < 0.5 then
				away_from = closed_from
			end
			return
		end
		local t = token:peek()
		if t and away_from == t.from and dismissed:peek() == t.from then
			dismissed:set(-1)
		end
		away_from = -1
	end)
	-- A new token (or a new query) lights the first row again.
	ui.effect(function()
		token:get()
		active:set(1)
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
		-- The caret is past the token now (the field says so on its next
		-- caret event; meanwhile, nothing to show).
		dismissed:set(t.from)
		if opts.on_choose then
			opts.on_choose(item, t.trigger, tok)
		end
	end
	local function keys(e: any): boolean
		if not open() then
			return false
		end
		local list = shown:peek()
		if e.key == "ArrowDown" or e.key == "ArrowUp" then
			if #list == 0 then
				return true
			end
			local step = if e.key == "ArrowDown" then 1 else -1
			active:set((active:peek() - 1 + step) % #list + 1)
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
				dismissed:set(t.from)
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
		local rows = ui.each(shown, row, {
			key = function(item: Item): string
				return item.value
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
			on_close = function()
				local t = token:peek()
				if t then
					closed(t.from)
				end
			end,
			at_caret = true,
			autofocus = false,
			w = WIDTH,
			placement = { side = "top", align = "start", offset = 8 },
			anchor = anchor,
			ui.column {
				id = "mention-panel",
				w_full = true,
				p = "$space.1",
				gap = 0,
				Menu.Heading { title },
				ui.show(loading, ui.column { gap = "$zeron.list.gap", table.unpack(skeleton) }),
				ui.show(function()
					return not loading() and #shown:get() == 0
				end, ui.row {
					h = ROW,
					px = "$space.2",
					items = "center",
					ui.text { empty, text_size = "$font.size.sm", color = "$text.tertiary" },
				}),
				ui.scroll {
					w_full = true,
					h = function(): number
						local n = #shown:get()
						return math.min(n, ROWS) * (ROW + 2) + 2
					end,
					overscroll = "contain",
					role = "listbox",
					label = title,
					rows,
				},
			},
		}
	end
	return {
		token = token,
		shown = shown,
		active = active,
		open = open,
		keys = keys,
		on_caret = on_caret,
		descendant = descendant,
		choose = choose,
		popover = popover,
		refs = refs,
	}
end

return M
