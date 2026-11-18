--!strict
-- A store plugin's own words drawn as Markdown (its README, a version's
-- notes), bounded: the author's text can't make the page fail to draw or
-- freeze it. Every place the store's text goes through ds/markdown goes
-- through here (screens/plugins/listing: About, a version's notes, the
-- update sheet's notes).
--
-- Why: zgx gives a script call 50 ms (docs/scripting.md) and stops one that
-- runs past it, so a text whose build is one long call doesn't draw at all;
-- and a stopped call can't be caught to draw something else (a `pcall`
-- catches the stop, but the call stays past its budget: nothing after it
-- runs). So the text is cut before it's built, by what it costs:
--   - a piece ends at a blank line once past `PIECE` (64 KiB), at any line
--     end once past twice that, or once its lines cost `UNITS` (about
--     15 ms of build at Zeron's pin, measured: a line of prose or a
--     heading 1, a list item 1 + its indent / 4, a quoted line 1 + its
--     depth / 8, a table row 7), never inside a code fence;
--   - past `MOST` pieces, or a fence still open past `PIECE`, the rest is
--     plain text in the code face (it costs well under a millisecond), after
--     one quiet line saying so.
-- Each piece is built in a call of its own: the first at once, the rest as
-- Show more asks (`more`), or one a frame (`auto`).
--
--   untrusted.View { text, what = "readme" | "notes", small?, auto?,
--     image?, heading_step?, drop_title?, picture_radius?, more_id?,
--     first_lines?, only_first?, handle? }
--   untrusted.pieces(text) -> { { plain: boolean, text: string } }
--
-- `first_lines`: the first piece also ends at this many lines that aren't
-- blank (a fold that shows the start alone lays out about what it shows:
-- a line is 20 high or more). `only_first`: that piece alone, nothing to
-- show more. `handle` (`untrusted.handle()`, made in the component that
-- draws it): whether there's more and the way to show the next piece, for
-- a control drawn outside the text (none inside).

local FoldButton = require("@ds/fold_button").FoldButton
local Markdown = require("@ds/markdown")

local M = {}

M.PIECE = 64 * 1024
M.UNITS = 1500
M.MOST = 32

-- Bytes a line is read by (a line is classed by its first few, never by a
-- pattern over all of it: a cutter reads half a megabyte of lines inside
-- one call's budget).
local SPACE, TAB, NL = 32, 9, 10
local DASH, STAR, PLUS, GT, BAR, TICK, TILDE = 45, 42, 43, 62, 124, 96, 126

export type Piece = { plain: boolean, text: string }
export type Cutter = { next: (self: Cutter) -> Piece?, done: boolean }

-- What cuts a text into pieces, one at a time, each read as it's asked for
-- (so the first frame reads one piece, not the whole text).
function M.cutter(text: string, first_lines: number?): Cutter
	local len = #text
	local pos = 1
	local fenced = false
	local plain = false
	local made = 0
	local self: any = { done = len == 0 }
	function self.next(me: Cutter): Piece?
		if pos > len then
			me.done = true
			return nil
		end
		local start = pos
		local units = 0
		local lines = 0
		local most_lines = if made == 0 and first_lines then first_lines else math.huge
		local cut: number? = nil
		local as_plain = plain
		while pos <= len do
			local nl = string.find(text, "\n", pos, true) or (len + 1)
			-- (A line longer than a piece's room is no line of prose: it
			-- and the rest plain. The piece so far ends before it.)
			if not as_plain and not fenced and nl - pos > M.PIECE then
				plain = true
				if pos > start then
					cut = pos - 1
					break
				end
				as_plain = true
			end
			local i = pos
			local b = string.byte(text, i)
			while i < nl and (b == SPACE or b == TAB) do
				i += 1
				b = string.byte(text, i)
			end
			local blank = i >= nl
			local bytes = nl - start
			-- (A fence's opening or closing: three ticks or tildes.)
			if (b == TICK or b == TILDE) and string.byte(text, i + 1) == b and string.byte(text, i + 2) == b then
				fenced = not fenced
			end
			if not as_plain and not blank then
				lines += 1
				if fenced then
					units += 1
				elseif (b == DASH or b == STAR or b == PLUS) and string.byte(text, i + 1) == SPACE then
					units += 1 + (i - pos) / 4
				elseif b == GT then
					local depth, k = 0, i
					while k < nl and depth < 64 do
						local c = string.byte(text, k)
						if c == GT then
							depth += 1
						elseif c ~= SPACE then
							break
						end
						k += 1
					end
					units += 1 + depth / 8
				elseif b == BAR then
					units += 7
				else
					units += 1
				end
			end
			pos = nl + 1
			if as_plain then
				if bytes >= M.PIECE then
					cut = nl
					break
				end
			elseif fenced and bytes >= M.PIECE then
				-- (A fence that doesn't close within a piece's room: this
				-- piece and the rest plain, as a code block would show them.)
				as_plain = true
				plain = true
				cut = nl
				break
			elseif not fenced and (units >= M.UNITS or lines >= most_lines or bytes >= 2 * M.PIECE or (bytes >= M.PIECE and blank)) then
				cut = nl
				break
			end
		end
		local piece: Piece = { plain = as_plain, text = string.sub(text, start, (cut or (len + 1)) - 1) }
		made += 1
		if not as_plain and made >= M.MOST then
			plain = true
		end
		if pos > len then
			me.done = true
		end
		return piece
	end
	return self :: Cutter
end

-- A plain piece without the blank lines at its ends (where a cut falls
-- decides them, so the space round the text would too). Found byte by byte:
-- a pattern such as `%s+$` over a run of spaces is tried at every byte of it.
function M.trimmed(text: string): string
	local first, last = 1, #text
	-- (From the start: past whole blank lines, never into the first line's
	-- own indent.)
	local i = 1
	while i <= last do
		local b = string.byte(text, i)
		if b == NL then
			first = i + 1
		elseif b ~= SPACE and b ~= TAB and b ~= 13 then
			break
		end
		i += 1
	end
	while last >= first do
		local b = string.byte(text, last)
		if b ~= NL and b ~= SPACE and b ~= TAB and b ~= 13 then
			break
		end
		last -= 1
	end
	return string.sub(text, first, last)
end

-- Every piece of `text` (tests; a view cuts its own as it shows them).
function M.pieces(text: string): { Piece }
	local out: { Piece } = {}
	local c = M.cutter(text)
	while true do
		local p = c:next()
		if not p then
			break
		end
		if string.find(p.text, "%S") then
			table.insert(out, p)
		end
	end
	return out
end

-- Whether a view has more to show, and the way to show its next piece: for
-- a control drawn outside its text. Made in the component that draws both.
export type Handle = { more: State<boolean>, next: (() -> ())? }
function M.handle(): Handle
	return { more = ui.use_state(false), next = nil }
end

export type ViewProps = {
	text: string,
	what: "readme" | "notes",
	small: boolean?,
	auto: boolean?,
	image: ((src: string) -> any)?,
	heading_step: number?,
	drop_title: ((words: string) -> boolean)?,
	picture_radius: string?,
	more_id: string?,
	first_lines: number?,
	only_first: boolean?,
	handle: Handle?,
}

M.View = ui.component(function(props: ViewProps): Node
	-- (Pieces cut as they're shown: the first now, each next in the call
	-- that shows it.)
	local cutter = M.cutter(props.text, props.first_lines)
	local pieces: { Piece } = {}
	local function cut_one(): boolean
		local p = cutter:next()
		if p then
			table.insert(pieces, p)
			return true
		end
		return false
	end
	cut_one()
	local count = ui.use_state(#pieces)
	local more = ui.use_state(not cutter.done)
	local function show_next()
		if props.only_first then
			return
		end
		if cut_one() then
			count:set(#pieces)
		end
		more:set(not cutter.done)
		if props.handle then
			props.handle.more:set(not cutter.done)
		end
	end
	if props.handle then
		local handle = props.handle
		handle.next = show_next
		-- (Said once it's there: never while it's being built.)
		ui.on_mount(function()
			handle.more:set(not cutter.done)
		end)
	end
	-- (`auto`: the rest a piece a frame, each its own call.)
	if props.auto then
		ui.effect(function()
			count:get()
			if more:get() then
				ui.after(0, show_next)
			end
		end)
	end
	local first_plain: number? = nil
	local list = ui.computed(function(): { number }
		local out = {}
		for i = 1, count:get() do
			if pieces[i].plain and first_plain == nil then
				first_plain = i
			end
			table.insert(out, i)
		end
		return out
	end)
	local small = props.small == true
	return ui.column {
		w_full = true,
		min_w = 0,
		gap = if small then "$space.3" else "$space.4",
		ui.each(list, function(i: number): Node
			local p = pieces[i]
			if p.plain then
				return ui.column {
					w_full = true,
					min_w = 0,
					gap = "$space.2",
					-- (Said once, before the first piece drawn plain.)
					if i == first_plain
						then ui.text {
							class = "untrusted-plain-note",
							if props.what == "readme" then ui.t("plugins.store.plain_readme") else ui.t("plugins.store.plain_notes"),
							w_full = true,
							wrap = true,
							text_size = "$font.size.sm",
							line_height = "$font.line.sm",
							color = "$text.tertiary",
						}
						else nil,
					ui.text {
						class = "untrusted-plain",
						M.trimmed(p.text),
						literal = true,
						w_full = true,
						wrap = true,
						selectable = true,
						font = "$font.mono",
						text_size = "$font.size.sm",
						line_height = "$font.line.sm",
						color = "$text.secondary",
					},
				}
			end
			return Markdown {
				-- (Its author's words: links are links, never Zeron's chips.)
				foreign = true,
				text = p.text,
				small = props.small,
				image = props.image,
				heading_step = props.heading_step,
				drop_title = if i == 1 then props.drop_title else nil,
				picture_radius = props.picture_radius,
			}
		end, {
			key = function(i: number): number
				return i
			end,
		}),
		if props.auto or props.only_first or props.handle
			then nil
			else ui.show(function(): boolean
				return more:get()
			end, ui.row {
				id = props.more_id,
				-- (The page's one way to open more of a text in place.)
				FoldButton {
					label = function(): string
						return ui.t("plugins.store.readme_more"):get()
					end,
					open = function(): boolean
						return false
					end,
					on_click = show_next,
				},
			}),
	}
end)

return M
