-- Find in a chat (⌘F on the chat page): a bar over the conversation, the
-- editor's (ds/code_editor's find row): a field with its tally, ↑ and ↓,
-- and Close. It searches the messages' text as drawn (each parsed as the
-- transcript parses it, ds/markdown_find), not the window's nodes: a long
-- reply's runs away from the view aren't built, and its words are still
-- found. The transcript marks the matches (ds/markdown's `find`), builds
-- the run holding the current one and scrolls it into view.
--
--   find.open()   the bar opened and its field focused, on a chat; whether
--                 it took ⌘F (false where no chat is shown)
--   find.close()
--   find.marks()  what ds.Transcript's `find` reads
--
-- Enter or ↓ go to the next match (newer), Shift+Enter or ↑ to the one
-- before; a new search starts at the newest match, where the view
-- usually rests. Escape closes.

local ds = require("@ds")
local locale = require("@ds/locale")
local MarkdownFind = require("@ds/markdown_find")
local store = require("@app/screens/chat/store")
local route = require("@app/screens/shell/route")

local M = {}

M.shown = ui.shared("chat.find.shown", false)
M.query = ui.shared("chat.find.query", "")
-- Which match is current (its place among them, from 1; 0 before any).
M.at = ui.shared("chat.find.at", 0)
local focus = ui.shared("chat.find.focus", 0)

export type Match = { key: string, block: number, k: number, row: number }

-- A message's texts as the transcript draws them, each with the key its
-- Markdown is found by: a prompt's (`<id>:u`, its line breaks breaks), a
-- reply's text blocks (`<id>:<block id>`), or a reply's whole text
-- (`<id>:r`).
local function sources(m: any): { { key: string, text: string, breaks: boolean? } }
	if m.role == "user" then
		return { { key = `{m.id}:u`, text = m.text or "", breaks = true } }
	end
	if m.role ~= "assistant" then
		return {}
	end
	if m.blocks then
		local out = {}
		for _, b in m.blocks do
			if b.kind == "text" and b.text then
				table.insert(out, { key = `{m.id}:{b.id}`, text = b.text })
			end
		end
		return out
	end
	return { { key = `{m.id}:r`, text = m.text or "" } }
end

-- Each text's blocks, parsed once while its text stays the same.
local parsed: { [string]: { text: string, blocks: { MarkdownBlock } } } = {}
local function blocks_of(key: string, text: string, breaks: boolean?): { MarkdownBlock }
	local hit = parsed[key]
	if hit and hit.text == text then
		return hit.blocks
	end
	local list = MarkdownFind.parse(text, breaks)
	parsed[key] = { text = text, blocks = list }
	return list
end

-- Every match, in the conversation's order.
M.matches = ui.computed(function(): { Match }
	local out: { Match } = {}
	local q = string.lower(M.query:get())
	if not M.shown:get() or q == "" then
		return out
	end
	for row, m in store.messages:get() do
		for _, src in sources(m) do
			for n, b in blocks_of(src.key, src.text, src.breaks) do
				for k = 1, MarkdownFind.count(b, q) do
					table.insert(out, { key = src.key, block = n, k = k, row = row })
				end
			end
		end
	end
	return out
end)

-- A new search starts at the newest match.
ui.effect(function()
	local list = M.matches:get()
	local at = M.at:peek()
	if #list == 0 then
		if at ~= 0 then
			M.at:set(0)
		end
	elseif at < 1 or at > #list then
		M.at:set(#list)
	end
end)
local last_query = M.query:peek()
ui.effect(function()
	local q = M.query:get()
	if q ~= last_query then
		last_query = q
		local n = #M.matches:peek()
		M.at:set(n)
	end
end)

function M.current(): Match?
	local at = M.at:get()
	return M.matches:get()[at]
end

-- To the next match (`by` 1) or the one before (-1), round the ends.
function M.step(by: number)
	local n = #M.matches:peek()
	if n == 0 then
		return
	end
	local at = M.at:peek()
	M.at:set((at - 1 + by) % n + 1)
end

function M.open(): boolean
	if route.session:peek() == nil then
		return false
	end
	M.shown:set(true)
	focus:set(focus:peek() + 1)
	return true
end

function M.close()
	M.shown:set(false)
end

-- What ds.Transcript's `find` reads.
function M.marks(): any
	return {
		query = function(): string
			return if M.shown:get() then M.query:get() else ""
		end,
		current = function(): Match?
			return if M.shown:get() then M.current() else nil
		end,
	}
end

-- What the tally says: "3 of 12", "No results", or nothing yet.
local function tally(): string
	if M.query:get() == "" then
		return ""
	end
	local n = #M.matches:get()
	if n == 0 then
		return locale.s("chat.find.no_results")
	end
	return locale.s("chat.find.tally", { at = M.at:get(), n = n })
end

local function none(): boolean
	return #M.matches:get() == 0
end

-- (`label`: a catalog key, its name and its tooltip.)
local function glyph(label: string, icon: string, run: () -> (), disabled: (() -> boolean)?): Node
	return ds.IconButton { icon = icon :: any, label = ui.t(label), tooltip = locale.s(label), size = "sm", tooltip_side = "bottom", disabled = disabled, on_click = run }
end

-- The bar, over the conversation's top end: shown while open.
function M.Bar(): Node
	local input = ui.ref()
	ui.effect(function()
		if focus:get() > 0 and M.shown:peek() then
			ui.after(0, function()
				local el = input:peek()
				if el then
					el:focus()
				end
			end)
		end
	end)
	return ui.show(
		function(): boolean
			return M.shown:get()
		end,
		ui.row {
			id = "chat-find",
			role = "toolbar",
			label = ui.t("chat.find.toolbar"),
			absolute = true,
			top = 8,
			right = 16,
			w = 380,
			p = "$space.1",
			gap = "$space.1",
			items = "center",
			rounded = "$radius.lg",
			bg = "$surface.overlay",
			border = 1,
			border_color = "$border.subtle",
			shadow = "$shadow.lg",
			ui.div {
				w = "$zeron.icon_button.sm",
				shrink = 0,
				items = "center",
				justify = "center",
				color = "$text.tertiary",
				ds.Icon { name = "search" :: any, size = 14 },
			},
			ds.TextField {
				input_ref = input,
				value = M.query,
				name = ui.t("chat.find.find") :: any,
				placeholder = function(): string
					return locale.s("chat.find.find")
				end,
				size = "sm",
				grow = 1,
				select_on_focus = true,
				trailing = ui.row {
					pr = "$space.1",
					ds.FadeText {
						tally,
						literal = true,
						max_w = 120,
						tabular = true,
						text_size = "$font.size.xs",
						color = "$text.tertiary",
					},
				},
				on_key = function(e: any): boolean?
					local k = string.lower(e.key)
					if k == "enter" then
						M.step(if e.shift then -1 else 1)
						return true
					elseif k == "arrowdown" then
						M.step(1)
						return true
					elseif k == "arrowup" then
						M.step(-1)
						return true
					elseif k == "escape" then
						M.close()
						return true
					end
					return nil
				end,
			},
			glyph("chat.find.previous", "arrow-up", function()
				M.step(-1)
			end, none),
			glyph("chat.find.next", "arrow-down", function()
				M.step(1)
			end, none),
			glyph("common.close", "close", M.close),
		}
	)
end

return M
