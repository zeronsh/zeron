-- What stands between two lines of code: a comment being written, a thread
-- on a line, an agent's note. Each is a band across the pane, in the code
-- rather than over it: the lines below move down to make room, it scrolls
-- with them, and the code's own sideways scroll leaves it where it is. It
-- has no surface of its own: the editor's shows through, a hairline over and
-- under marks where the code stops and starts, and what stands in it is the
-- comments' own parts (`ds/comment`). Its words start on the code's text
-- edge, past the gutter, so the eye runs down one margin.
--
--   InlineBlock { inset = 52, … anything … }
--   Composer { location = "ds/badge.luau:42", value = draft, on_submit = send, on_cancel = close }
--   Thread { location = "ds/badge.luau:42", comments = { … }, draft = reply, me = me }
--   Note { from = "Claude", body = "This loop runs twice.", actions = { { label = "Apply", on_click = … } } }
--
-- `inset` is where the text starts: a number, or a function for one that
-- moves (a code editor's gutter grows with its numbers). `CodeEditor`'s
-- `blocks` calls each block's `view` with `{ inset, joined }`; a diff passes
-- its own. `joined`: the block above it stands right against it, so it draws
-- no top hairline of its own, and the two never make a double line.
--
-- In a list of a diff's lines, one goes in as a row like any other:
--
--   table.insert(rows, InlineBlock.Thread { inset = 76, … })
--
-- `Add { on_click = … }` is the + that starts one, the same in a code editor's
-- gutter and a diff's: the design system's small icon button, quiet until
-- touched. It wants a slot of its own (`Add.SLOT` wide) at the start of the
-- gutter: never over a line number, and nothing moves as it shows.

local Button = require("@self/button")
local IconButton = require("@self/icon_button").IconButton
local Markdown = require("@self/markdown")
local comment = require("@self/comment")
local locale = require("@self/locale")

local M = {}

export type Inset = number | () -> number
-- The scroll a block stands in, for one that must be seen as it opens (a
-- comment being written): its `offset` state, and `frame`, the viewport's
-- box (`{ y, height }` in window points) as a function (the scroll's
-- owner records it in an `on_resize`, whose element has a `y`; a ref's
-- element and a scroll node report none), or an element to read it from.
export type Frame = { y: number?, height: number? }
export type Reveal = { offset: State<number>, frame: (() -> (Frame | Element)?) | Element }
export type Props = {
	inset: Inset?,
	joined: boolean?,
	reveal: Reveal?,
	[number]: Child,
}

-- Room kept under a revealed block.
local ROOM = 16

-- The block `el` brought into its scroll's view, with ROOM under it (or
-- over it, if it stands above the view): the scroll's offset moves by
-- what is out of sight, no more. Both measure in window points.
function M.reveal(el: Element, where: Reveal)
	local f = where.frame
	local frame = if type(f) == "function" then f() else f
	if not frame then
		return
	end
	local by, bh, sy, sh = el.y, el.height, frame.y, frame.height
	if not (by and bh and sy and sh) then
		return
	end
	local below = (by + bh + ROOM) - (sy + sh)
	local above = sy - (by - ROOM)
	local was = where.offset:peek()
	if below > 0 then
		where.offset:set(was + below)
	elseif above > 0 then
		where.offset:set(math.max(0, was - above))
	end
end

-- The room a gutter keeps for the +.
M.SLOT = 24

-- The + beside a line. Other props go to its button.
function M.Add(props: { on_click: () -> (), label: string?, [any]: any }): Node
	local p = table.clone(props) :: any
	p.icon = "plus"
	-- (A string, so it's its tooltip too: read as it's built.)
	p.label = props.label or locale.s("ds.inline_block.add_comment")
	p.size = "sm"
	p.tone = "quiet"
	return IconButton(p)
end

-- The band: a hairline over and under, and a column on the code's text edge.
-- What it holds has its own room; the band only frames it.
function M.InlineBlock(props: Props): Node
	local inset = props.inset
	local inner: ContainerProps = {
		w_full = true,
		gap = "$space.2",
		pt = "$space.3",
		pb = "$space.3",
		pr = "$space.4",
		-- (Where the code's text starts: the gutter's width, worked out by
		-- the editor or the diff, so geometry, not a step of the scale.)
		pl = if type(inset) == "function"
			then function(): any
				return ui.raw(inset(), "where the code's text starts, past its gutter")
			end
			else ui.raw(inset or 16, "where the code's text starts, past its gutter"),
		min_w = 0,
	}
	for _, child in ipairs(props) do
		table.insert(inner, child)
	end
	local reveal = props.reveal
	return ui.column {
		w_full = true,
		border_sides = { if props.joined then 0 else 1, 0, 1, 0 },
		border_color = "$border.subtle",
		-- Seen as it opens, and as it grows (a comment being written): at
		-- each size it takes, it brings itself into view. Not at once: a
		-- field in it takes focus a moment after it mounts and the scroll
		-- follows that, and a scroll asked for lands at the next layout; a
		-- pass reads a settled layout and moves only by what is still out of
		-- sight, so two passes a little apart never count the same twice.
		on_resize = if reveal
			then function(_size, el)
				for _, wait in { 120, 360 } do
					ui.after(wait, function()
						M.reveal(el, reveal)
					end)
				end
			end
			else nil,
		ui.column(inner),
	}
end

export type ComposerProps = Merge<Props, {
	location: string?,
	value: State<string>,
	placeholder: string?,
	submit: string?,
	on_submit: ((text: string) -> ())?,
	on_cancel: (() -> ())?,
}>

-- A comment being written: where it is over the comments' own composer,
-- open from the start. ⌘↵ sends, Escape puts it away.
function M.Composer(props: ComposerProps): Node
	return M.InlineBlock {
		inset = props.inset,
		joined = props.joined,
		reveal = props.reveal,
		if props.location then ui.row { w_full = true, min_w = 0, comment.Location(props.location) } else nil,
		comment.ReplyComposer {
			value = props.value,
			open = true,
			name = locale.t("ds.inline_block.comment"),
			placeholder = props.placeholder or locale.t("ds.inline_block.placeholder"),
			submit = props.submit,
			on_submit = props.on_submit,
			on_cancel = props.on_cancel,
		},
	}
end

export type ThreadProps = Merge<Props, comment.ThreadProps>

-- A thread on a line: the comments, with their replies, in the band.
function M.Thread(props: ThreadProps): Node
	return M.InlineBlock {
		inset = props.inset,
		joined = props.joined,
		comment.Thread {
			comments = props.comments,
			location = props.location,
			resolved = props.resolved,
			draft = props.draft,
			me = props.me,
			on_reply = props.on_reply,
			on_resolve = props.on_resolve,
		},
	}
end

export type Action = { label: string, on_click: () -> (), primary: boolean? }
export type NoteProps = Merge<Props, {
	-- Who says it (an agent's name).
	from: string,
	-- Its words, as Markdown.
	body: string,
	actions: { Action }?,
}>

-- An agent's note on a line: who says it, what it says, what can be done.
function M.Note(props: NoteProps): Node
	-- (A ghost first: its label on the text edge, its wash past it.)
	local first = (props.actions or {})[1]
	local actions: ContainerProps = {
		gap = "$space.2",
		items = "center",
		mt = "$space.1",
		ml = if first and not first.primary then ui.raw(-10, "a small button's padding") else nil,
	}
	for _, a in props.actions or {} do
		table.insert(actions, Button {
			a.label,
			size = "sm",
			tone = if a.primary then "primary" else "ghost",
			on_click = a.on_click,
		})
	end
	return M.InlineBlock {
		inset = props.inset,
		joined = props.joined,
		-- A comment's name and words, its size and ink.
		ui.text { props.from, literal = true, text_size = "$font.size.md", weight = "$font.weight.medium", color = "$text.primary" },
		ui.column {
			w_full = true,
			theme = { zeron = { prose = { size = "{zeron.prose.small.size}", line = "{zeron.prose.small.line}" } } },
			Markdown { text = props.body },
		},
		if props.actions and #props.actions > 0 then ui.row(actions) else nil,
	}
end

return M
