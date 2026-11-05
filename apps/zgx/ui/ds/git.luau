-- A change, as Zeron's git pages draw one: a pull request's header and its
-- tabs, a list of changed files, a diff of one file (with a + on any line
-- to comment there, and what stands under a line: a thread, a comment
-- being written), its checks folded to a line, and what happened to it in
-- order (the activity). The gallery's git journey (demos/git_parts) and
-- the GitHub plugin draw from here, with data of their own: nothing here
-- knows a pull request, a person or a line until it is given one.
--
--   local git = require("@ds/git")
--   git.header({ state = "open", number = 1142, title = "…", author = who, branch = "x", base = "main", added = 3, removed = 1 }, narrow)
--   git.diff({ path = "a.rs", status = "modified", add = 1, del = 1 }, lines, { on_comment = … })
--   git.checks_block { list = checks, open = state }
--   git.activity(events, { branch = "x", base = "main" })
--
-- The vocabulary is Zeron's (pull_requests.rs row_status: Draft, Open,
-- Approved, Changes requested, Merge conflicts; Checks · n). Every piece has
-- two widths, the window's and a side pane's (`narrow`): Zeron's right pane
-- is 360 at its narrowest (settings.rs RIGHT_PANE_MIN), and the narrow
-- layout holds there: the header stacks, the tabs' badges drop under them,
-- the diff keeps one column of line numbers.

local Badge = require("@self/badge")
local Icon = require("@self/icon")
local Text = require("@self/text")
local Tabs = require("@self/tabs")
local Select = require("@self/select")
local FadeText = require("@self/fade_text")
local progress = require("@self/progress")
local layout = require("@self/layout")
local Settings = require("@self/settings")
local Inline = require("@self/inline_block")
local people = require("@self/people")
local control = require("@self/control")
local locale = require("@self/locale")
local Syntax = require("@self/syntax")
local lex = require("@self/syntax/lex")

local M = {}

-- The pages' cards (a list of files, a diff, the checks) share one corner,
-- and a border of one: what sits against a card's edge inside it derives
-- its own corner from these (README rule 5).
M.RADIUS = "$radius.xl"
M.EDGE = 1
-- Where a card's words start from its outer edge: its edge and its
-- padding. Words over a card (a commit's head over its files) start here,
-- the card's box hanging past them (text on text, not box on box).
function M.text_inset(): number
	return ((ui.theme.get("space.3") :: number?) or 12) + M.EDGE
end
-- A card head's trailing controls: their inset from its inner edge, the
-- gap between them, and how far in from the card's outer edge they end
-- (what a band over the card ends on, so the two rows' last controls share
-- one right edge).
M.HEAD_END, M.HEAD_GAP = "$space.2", "$space.1"
function M.end_inset(): number
	return ((ui.theme.get("space.2") :: number?) or 8) + M.EDGE
end

-- A git glyph (an icon's name, or a node: a ring) in a quiet disc, as the
-- settings rows lead with one; `size` 32 (a row's) or 24 (the activity's).
function M.glyph(inside: any, tone: string?, size: number?): Node
	local d = size or 32
	return ui.row {
		w = d,
		h = d,
		shrink = 0,
		items = "center",
		justify = "center",
		rounded = "$radius.full",
		bg = if tone then `$zeron.tone.{tone}.wash` else "$zeron.secondary.bg",
		color = if tone then `$zeron.tone.{tone}.ink` else "$text.secondary",
		if type(inside) == "string" then Icon { name = inside :: any, size = if d >= 32 then 16 else 14 } else inside,
	}
end

local function quiet(text: string): Node
	return ui.text { text, literal = true, text_size = "$font.size.sm", color = "$text.tertiary" }
end

-- A pull request, as its header says it.
export type Header = {
	state: string,
	number: number,
	title: string,
	-- Who opened it, and when ("2 hours ago"; or a node of its own).
	author: people.Who?,
	opened: (string | Node)?,
	-- From which branch into which, and the size of the change.
	branch: string?,
	base: string?,
	added: number?,
	removed: number?,
	-- What stands at the header's end (a link out, a refresh): after the
	-- title, or, narrow, at the end of its first line.
	actions: Node?,
}

-- The pull request's header, as every page of it opens: its title with its
-- state and number after it, and under it who opened it, when, from which
-- branch into which, and the size of the change. Narrow, who on a line of
-- its own, the title wrapping before the badge.
function M.header(pr: Header, narrow: boolean?): Node
	local who: { Node } = {}
	if pr.author then
		table.insert(who, people.Person { id = pr.author.id, name = pr.author.name, src = pr.author.src, size = 18 } :: any)
	end
	if type(pr.opened) == "string" then
		table.insert(who, quiet(`opened {pr.opened}`))
	elseif pr.opened then
		table.insert(who, pr.opened :: Node)
	end
	local where: { Node } = {}
	if pr.branch and pr.branch ~= "" then
		table.insert(where, Badge.Badge { pr.branch, icon = "git-branch", mono = true })
		if pr.base and pr.base ~= "" then
			table.insert(where, quiet("into"))
			table.insert(where, Badge.Badge { pr.base, icon = "git-branch", mono = true })
		end
	end
	local stat = if pr.added or pr.removed then Badge.DiffStat { added = pr.added or 0, removed = pr.removed or 0 } else nil
	-- Parts on one line, a dot between each part there is.
	local function dotted(parts: { { Node } }): { Node }
		local out: { Node } = {}
		for _, part in parts do
			if #part > 0 then
				if #out > 0 then
					table.insert(out, quiet("·"))
				end
				for _, n in part do
					table.insert(out, n)
				end
			end
		end
		return out
	end
	-- The first line: the title, its state and number right after it, and
	-- the actions at the far end. (As a list with no hole: a header with no
	-- actions has none.)
	local first: { any } = { w_full = true, gap = "$space.3", items = "center" }
	table.insert(first, Text { pr.title, style = "heading", wrap = true, heading = 1, shrink = 1, min_w = 0 })
	table.insert(first, Badge.PullRequest { state = pr.state :: any, number = pr.number, size = "md" })
	table.insert(first, ui.div { grow = 1 })
	if pr.actions then
		table.insert(first, pr.actions)
	end
	local lines: { Node } = { ui.row(first) }
	-- Under it, who and when · where · how much: on one line at the
	-- window's width, who on a line of its own at a side pane's.
	local rest = if narrow then { dotted({ who }), dotted({ where, if stat then { stat } else {} }) } else { dotted({ who, where, if stat then { stat } else {} }) }
	for _, line in rest do
		if #line > 0 then
			table.insert(lines, ui.row { gap = "$space.2", items = "center", flex_wrap = true, table.unpack(line) })
		end
	end
	return ui.column { w_full = true, gap = "$space.2", table.unpack(lines) }
end

-- The page's tabs, with what stands at the far end of their rule (the
-- checks and review badges); narrow, the badges on a line under the tabs.
export type TabsProps = {
	value: State<string>,
	options: { any },
	badges: { Node },
	narrow: boolean?,
	-- The row's inset past its hairline (the cards' text edge, less a
	-- tab's own label padding, so the first label's ink lands on it).
	inset: any?,
	-- Under them at a pane's width, the badges' own (a pill's words on the
	-- same edge).
	trail_inset: any?,
}

function M.tabs(o: TabsProps): Node
	local trail = if #o.badges > 0 then ui.row { gap = "$space.2", items = "center", flex_wrap = true, table.unpack(o.badges) } else nil
	if o.narrow then
		return ui.column {
			w_full = true,
			gap = "$space.3",
			Tabs { value = o.value, label = locale.t("ds.git.pull_request") :: any, options = o.options, inset = o.inset },
			if trail then ui.div { w_full = true, pl = o.trail_inset, trail } else nil,
		}
	end
	return Tabs { value = o.value, label = locale.t("ds.git.pull_request") :: any, options = o.options, trail = trail, inset = o.inset }
end

-- A changed file: its path, its status (Zeron's: added, modified, deleted,
-- renamed, conflict) and the lines it adds and takes.
export type File = { path: string, status: string, add: number, del: number }

-- A changed file, as a list row: its status letter, its path (fading if
-- long; `label` in its place, a file's own name under its folder's
-- header), what it adds and takes; `check` puts a staging box before it.
-- `opts`: its height and side room, when it stands inset in a list (the
-- Changes list's: a 30-point row, its words on the card's column).
function M.file_row(f: File, check: Node?, label: string?, opts: { h: number?, px: any? }?): Node
	return ui.row {
		w_full = true,
		h = (opts and opts.h) or 36,
		px = (opts and opts.px) or "$space.3",
		gap = "$space.2",
		items = "center",
		check,
		Badge.FileStatus { status = f.status :: any },
		FadeText {
			label or f.path,
			grow = 1,
			-- (A name, as the file tree and the file's head have it: the
			-- interface's face, not code's.)
			text_size = "$font.size.md",
			color = if f.status == "deleted" then "$text.tertiary" else "$text.primary",
		},
		Badge.DiffStat { added = f.add, removed = f.del },
	}
end

-- A card of rows with a title row over them, as the settings' groups are
-- built, for lists that aren't settings: files, checks. `faded(i)` says
-- the hairline over row `i` is not drawn (a ringed row keeps the lines
-- next to it from crowding its ring); its room stays, so nothing moves.
function M.card(head: Node?, rows: { Node }, faded: ((i: number) -> boolean)?): Node
	local items: { any } = {}
	if head then
		table.insert(items, head)
	end
	for i, r in rows do
		if i > 1 or head then
			table.insert(items, ui.div {
				h = 1,
				w_full = true,
				shrink = 0,
				bg = "$zeron.settings.rule",
				aria_hidden = true,
				opacity = if faded
					then function(): number
						return if faded(i) then 0 else 1
					end
					else nil,
			})
		end
		table.insert(items, r)
	end
	return ui.column {
		w_full = true,
		rounded = M.RADIUS,
		bg = "$zeron.settings.card",
		border = M.EDGE,
		border_color = "$zeron.settings.edge",
		overflow = "hidden",
		table.unpack(items),
	}
end

-- The hairline across a card, edge to edge, between its parts.
function M.rule(): Node
	return ui.div { h = 1, w_full = true, shrink = 0, bg = "$zeron.settings.rule", aria_hidden = true }
end

-- A card of the pages' corner, fill and edge round what it is given: `fill`
-- grows into the room it stands in (a diff beside a list), `padded` sets its
-- words in from its edge (a description).
export type FrameProps = { id: string?, fill: boolean?, padded: boolean?, [number]: Child }

function M.frame(props: FrameProps): Node
	local kids: { any } = {}
	for _, c in ipairs(props :: any) do
		table.insert(kids, c)
	end
	return ui.column {
		id = props.id,
		w_full = true,
		h_full = if props.fill then true else nil,
		min_h = if props.fill then 0 else nil,
		px = if props.padded then "$space.4" else nil,
		py = if props.padded then "$space.3" else nil,
		rounded = M.RADIUS,
		bg = "$zeron.settings.card",
		border = M.EDGE,
		border_color = "$zeron.settings.edge",
		overflow = "hidden",
		table.unpack(kids),
	}
end

-- A chevron that turns to `angle()` (degrees) in a slot its own size: it
-- rotates about its own centre, so its ink never moves as it turns. Room
-- beside it is the caller's, outside the slot: padding inside a turning
-- box moves its centre, and the glyph swings round it.
-- (`angle`: 0 open, pointing down; -90 folded, pointing along the line.)
-- At rest it is never rotated: each state draws its own icon, and only
-- the turn between them rotates, from where it pointed into 0. A node
-- left rotated is a layer of its own, drawn apart every frame anything
-- around it moves (a panel's edge dragged): one per fold of a list.
function M.chevron(angle: () -> number, size: number?, duration: number?): Node
	local d = size or 14
	local folded = ui.computed(function(): boolean
		return angle() ~= 0
	end)
	local box = ui.ref()
	local last = folded:peek()
	ui.effect(function()
		local now = folded:get()
		if now == last then
			return
		end
		last = now
		anime.animate(box, { rotate = { if now then 90 else -90, 0 }, duration = duration or 160, ease = "outQuart" })
	end)
	return ui.div {
		ref = box,
		w = d,
		h = d,
		shrink = 0,
		items = "center",
		justify = "center",
		color = "$text.tertiary",
		Icon {
			name = function(): Icon.Name
				return if folded:get() then "alt-arrow-right" else "alt-arrow-down"
			end,
			size = d,
		},
	}
end

-- The files of the change as a list to choose from, each row a button; the
-- chosen one (the diff beside it is its) on the design system's flat
-- selection wash, the sidebar's and the settings nav's (`zeron.selected
-- .flat.bg`), and a row under the pointer on the hover wash. Each washes
-- the whole row, edge to edge, as the folders' bands do: no inset, no
-- corner of its own, no hairlines.
local ListRow = require("@self/list_row").ListRow
-- (The design system's list row, ds/list_row: the sidebar's flat wash,
-- edge to edge in its card, the one recipe every list hovers by.)
local function Selectable(props: { row: Node, chosen: () -> boolean, label: string, on_click: () -> (), class: string?, rounded: any? }): Node
	return ListRow {
		class = props.class,
		selected = props.chosen,
		label = props.label,
		on_click = props.on_click,
		ui.row { w_full = true, relative = true, props.row },
	}
end

function M.selectable(row: Node, chosen: () -> boolean, label: string, on_click: () -> (), class: string?): Node
	return Selectable { row = row, chosen = chosen, label = label, on_click = on_click, class = class }
end

-- For `M.card`: the hairline over row `i` goes when that row or the one
-- above it is the chosen one.
function M.faded(chosen: () -> number): (i: number) -> boolean
	return function(i: number): boolean
		local c = chosen()
		return c == i or c == i - 1
	end
end

function M.files_list(list: { File }, chosen: State<number>, head: Node?): Node
	local rows: { Node } = {}
	for i, f in list do
		table.insert(rows, M.selectable(M.file_row(f), function()
			return chosen:get() == i
		end, f.path, function()
			chosen:set(i)
		end))
	end
	return M.card(head, rows, M.faded(function()
		return chosen:get()
	end))
end

-- Narrow, the same choice as a select over the diff.
function M.files_select(list: { File }, chosen: State<number>): Node
	local options: { any } = {}
	for i, f in list do
		table.insert(options, { value = i, label = f.path, description = `+{f.add} −{f.del}` })
	end
	return Select { value = chosen, options = options, name = locale.t("ds.git.file") :: any, size = "sm", w_full = true }
end

-- A diff's line: what it is (" " context, "+" added, "-" removed, "hunk"
-- a hunk's header), its old and new numbers, its text.
export type Line = { kind: string, old: number?, new: number?, text: string }

-- A unified diff of one file (its text from its first `@@` on) as lines:
-- each numbered on its side, a hunk's header as it is. `max`: at most so
-- many (a long file's diff is cut there, said by the caller).
function M.parse(text: { string } | string, max: number?): { Line }
	local out: { Line } = {}
	local old, new = 0, 0
	local function each(line: string)
		if max and #out >= max then
			return
		end
		local a, c = string.match(line, "^@@ %-(%d+),?%d* %+(%d+)")
		if a then
			old, new = tonumber(a) :: number, tonumber(c) :: number
			table.insert(out, { kind = "hunk", text = line })
			return
		end
		local first = string.sub(line, 1, 1)
		if first == "\\" then
			-- "\ No newline at end of file": not a line of either side.
			return
		elseif first == "+" then
			table.insert(out, { kind = "+", new = new, text = string.sub(line, 2) })
			new += 1
		elseif first == "-" then
			table.insert(out, { kind = "-", old = old, text = string.sub(line, 2) })
			old += 1
		else
			table.insert(out, { kind = " ", old = old, new = new, text = string.sub(line, 2) })
			old += 1
			new += 1
		end
	end
	if type(text) == "string" then
		for line in string.gmatch(text .. "\n", "([^\n]*)\n") do
			each(line)
		end
	else
		for _, line in text do
			each(line)
		end
	end
	return out
end

-- A line's key, for a comment on it: the new line's number, or the old
-- one's for a line taken out ("n42", "o42").
function M.line_key(l: Line): string
	return if l.new then `n{l.new}` else `o{l.old}`
end

-- A line's text as the diff draws it: tabs at the app's tab stops
-- (changes.rs DIFF_TAB_SIZE, 4 columns).
local TAB = 4
local function expand(text: string): string
	if not string.find(text, "\t", 1, true) then
		return text
	end
	local out, col = {}, 0
	for ch in string.gmatch(text, utf8.charpattern) do
		if ch == "\t" then
			local n = TAB - col % TAB
			table.insert(out, string.rep(" ", n))
			col += n
		else
			table.insert(out, ch)
			col += 1
		end
	end
	return table.concat(out)
end
M.expand = expand

-- How diffs are read, as the person last chose (Changes' and a pull
-- request's alike, the toolbar's toggles): side by side or one over the
-- other, and long lines wrapped or scrolled.
M.prefs = {
	split = ui.persist("ds.git.diff_split", false),
	wrap = ui.persist("ds.git.diff_wrap", false),
}

-- A diff's lines coloured as their language is (ds/syntax: the editor's
-- own highlighters), worked out once per list of lines, the first time
-- one of them is drawn: each line's runs by its index (none for a hunk's
-- header). A hunk starts the lexer afresh (a comment open across one
-- isn't known); a language Zeron doesn't colour, none at all.
export type Run = { text: string, color: string }
local painted: { [any]: any } = setmetatable({}, { __mode = "k" }) :: any
local TEXT = lex.C.text
local function runs(line: string, spans: { any }): { Run }
	local out: { Run } = {}
	local at = 1
	for _, sp in spans do
		local from, to = math.max(sp[1], at), math.min(sp[2], #line)
		if from > at then
			table.insert(out, { text = string.sub(line, at, from - 1), color = TEXT })
		end
		if to >= from then
			table.insert(out, { text = string.sub(line, from, to), color = sp[3] })
			at = to + 1
		end
	end
	if at <= #line then
		table.insert(out, { text = string.sub(line, at), color = TEXT })
	end
	return out
end
function M.highlight(lines: { Line }, path: string?): { [number]: { Run } }?
	local known = painted[lines]
	if known ~= nil then
		return if known then known else nil
	end
	local h = if path then Syntax.highlighter(Syntax.language(path)) else nil
	if not h then
		painted[lines] = false
		return nil
	end
	local out: { [number]: { Run } } = {}
	local state: any = 0
	for i, l in lines do
		if l.kind == "hunk" then
			state = 0
		else
			local text = expand(l.text)
			local spans
			spans, state = h(text, state)
			out[i] = runs(text, spans)
		end
	end
	painted[lines] = out
	return out
end

-- A diff side by side: each hunk's header, then its lines paired, what
-- was taken out on the left beside what was put in on the right, a line
-- unchanged on both. `at`: each side's index in `lines` (for its colours).
export type Pair = { hunk: Line?, left: Line?, right: Line?, li: number?, ri: number? }
function M.pairs(lines: { Line }): { Pair }
	local out: { Pair } = {}
	local dels: { number } = {}
	local adds: { number } = {}
	local function flush()
		for k = 1, math.max(#dels, #adds) do
			local li, ri = dels[k], adds[k]
			table.insert(out, { left = if li then lines[li] else nil, right = if ri then lines[ri] else nil, li = li, ri = ri })
		end
		table.clear(dels)
		table.clear(adds)
	end
	for i, l in lines do
		if l.kind == "hunk" then
			flush()
			table.insert(out, { hunk = l })
		elseif l.kind == "-" then
			if #adds > 0 then
				flush()
			end
			table.insert(dels, i)
		elseif l.kind == "+" then
			table.insert(adds, i)
		else
			flush()
			table.insert(out, { left = l, right = l, li = i, ri = i })
		end
	end
	flush()
	return out
end

-- The gutter: the +'s own slot first (the design system's, so nothing
-- moves as it shows), the line numbers (36 each, room for four figures)
-- and the sign (16). Its width is where the code's text starts, and so
-- where a block under a line starts its words.
local NUM, SIGN = 36, 16
function M.gutter(narrow: boolean?): number
	return Inline.SLOT + (if narrow then NUM else NUM * 2) + SIGN
end

-- Row heights a list of a diff can know: a hunk's header, a line.
M.HUNK_H, M.LINE_H = 24, 22

-- One line of the diff. With `on_comment`, the + shows in its slot at the
-- line's start under the pointer, to say something about it.
-- The text of a line: its colours (`runs`), or plain; scrolled sideways
-- whole, or wrapped (`wrap`) to the room it has.
local function code(l: Line, runs_of: { Run }?, wrap: boolean?): Node
	local base: { [any]: any } = {
		literal = true,
		shrink = if wrap then 1 else 0,
		grow = if wrap then 1 else nil,
		min_w = if wrap then 0 else nil,
		wrap = wrap == true,
		pr = "$space.3",
		font = "$font.mono",
		text_size = "$font.size.sm",
		color = "$text.primary",
	}
	if not runs_of or #runs_of == 0 then
		base[1] = expand(l.text)
		return ui.text(base)
	end
	for k, r in runs_of do
		base[k] = ui.span { r.text, color = r.color, literal = true }
	end
	return ui.rich(base)
end
M.code = code

-- A line's mark, as a comment left on it says (`mark`: nil none, "on" a
-- comment rests on it, "lit" its chip is pointed at or a range is being
-- chosen over it): a quiet bar at its start, and, lit, the accent's wash
-- over it. Drawn over the line, taking no room.
local function mark_of(mark: (() -> string?)?): Node?
	if not mark then
		return nil
	end
	return ui.div {
		class = "diff-mark",
		absolute = true,
		left = 0,
		top = 0,
		bottom = 0,
		right = 0,
		pointer_events = false,
		aria_hidden = true,
		opacity = function(): number
			return if mark() then 1 else 0
		end,
		bg = function(): string
			return if mark() == "lit" then "$zeron.diff.comment_wash" else "#00000000"
		end,
		ui.div {
			class = "diff-mark-bar",
			absolute = true,
			left = 0,
			top = 0,
			bottom = 0,
			w = ui.raw(2, "a comment's bar at the line's start"),
			bg = "$accent",
			opacity = function(): number
				return if mark() == "lit" then 1 else 0.55
			end,
		},
	}
end
M.mark_of = mark_of

-- `slot`, `row`: what more the gutter's + and the line itself take (a
-- range dragged from the +, over lines: ds/git_changes). A drag from the +
-- keeps it there till it ends, wherever the pointer goes.

-- The +'s props, with a drag of its own kept: `dragging` set while one it
-- started goes on.
local function add_props(base: { [string]: any }, extra: { [string]: any }?, dragging: State<boolean>): { [string]: any }
	if not extra then
		return base
	end
	local p = table.clone(base)
	for k, v in extra do
		p[k] = v
	end
	local drag, ended = extra.on_drag, extra.on_drag_end
	if drag then
		p.on_drag = function(): any
			dragging:set(true)
			return drag()
		end
		p.on_drag_end = function(payload: any, accepted: boolean)
			dragging:set(false)
			if ended then
				ended(payload, accepted)
			end
		end
	end
	return p
end
local DiffLine = ui.component(function(props: { line: Line, narrow: boolean?, on_comment: (() -> ())?, runs: { Run }?, wrap: boolean?, mark: (() -> string?)?, slot: { [string]: any }?, row: { [string]: any }? }): Node
	local over = ui.use_state(false)
	local dragging = ui.use_state(false)
	local l = props.line
	local wash = if l.kind == "+" then "$zeron.tone.success.wash" elseif l.kind == "-" then "$zeron.tone.danger.wash" else nil
	local ink = if l.kind == "+" then "$zeron.diff.added" elseif l.kind == "-" then "$zeron.diff.removed" else "$text.tertiary"
	local slot: { [any]: any } = {
		w = Inline.SLOT,
		h = M.LINE_H,
		shrink = 0,
		items = "center",
		justify = "center",
		if props.on_comment
			then ui.show(function()
				return over:get() or dragging:get()
			end, Inline.Add(add_props({
				label = if l.new
					then locale.s("ds.git.comment_on_line", { n = tostring(l.new) })
					else locale.s("ds.git.comment_on_old_line", { n = tostring(l.old) }),
				on_click = props.on_comment,
			}, props.slot, dragging) :: any))
			else nil,
	}
	local numbers: { Node } = { ui.div(slot) }
	if props.narrow then
		table.insert(numbers, ui.text { tostring(l.new or l.old or ""), w = NUM, shrink = 0, align = "right", font = "$font.mono", text_size = "$font.size.xs", color = "$text.tertiary", tabular = true })
	else
		table.insert(numbers, ui.text { if l.old then tostring(l.old) else "", w = NUM, shrink = 0, align = "right", font = "$font.mono", text_size = "$font.size.xs", color = "$text.tertiary", tabular = true })
		table.insert(numbers, ui.text { if l.new then tostring(l.new) else "", w = NUM, shrink = 0, align = "right", font = "$font.mono", text_size = "$font.size.xs", color = "$text.tertiary", tabular = true })
	end
	table.insert(numbers, ui.text { if l.kind == " " then "" else l.kind, w = SIGN, shrink = 0, align = "center", font = "$font.mono", text_size = "$font.size.sm", color = ink })
	-- (The children as one list, unpacked last: an unpacked list in the
	-- middle of a table gives only its first item.) The text is never cut:
	-- the diff scrolls sideways, as the app's does (long-line wrapping is a
	-- toolbar choice there), so a long line can be read to its end. Its
	-- label is the line as a diff writes it, sign first.
	local kids: { Node } = numbers
	table.insert(kids, code(l, props.runs, props.wrap))
	local mark = mark_of(props.mark)
	if mark then
		table.insert(kids, mark)
	end
	local row: { [any]: any } = {
		class = "diff-line",
		label = `{if l.kind == " " then " " else l.kind}{l.text}`,
		w_full = true,
		-- (Wrapped, as tall as its text, its numbers on its first line.)
		h = if props.wrap then nil else M.LINE_H,
		min_h = if props.wrap then M.LINE_H else nil,
		items = if props.wrap then "start" else "center",
		py = if props.wrap then ui.raw(2, "a wrapped line's text on the line's own baseline") else nil,
		relative = true,
		bg = wash,
		on_hover = if props.on_comment
			then function(inside: boolean)
				over:set(inside)
			end
			else nil,
		table.unpack(kids),
	}
	for k, v in props.row or {} do
		row[k] = v
	end
	return ui.row(row)
end)
M.DiffLine = DiffLine

-- A hunk's header as a row of the diff, on the quiet wash. (Cut at the
-- card's edge where it runs long, as the lines are.)
-- Its ranges quiet, then the code it's in (git's context: the function or
-- the type the hunk falls inside), as the line's text is.
function M.hunk_row(l: Line): Node
	local ranges, context = string.match(l.text, "^(@@ [^@]* @@)%s*(.*)$")
	return ui.row {
		class = "diff-hunk",
		label = l.text,
		w_full = true,
		h = M.HUNK_H,
		px = "$space.3",
		gap = "$space.3",
		items = "center",
		bg = "$zeron.secondary.bg",
		ui.text { ranges or l.text, literal = true, shrink = 0, font = "$font.mono", text_size = "$font.size.xs", color = "$text.tertiary" },
		if context and context ~= ""
			then ui.text { context, literal = true, shrink = 0, font = "$font.mono", text_size = "$font.size.xs", color = "$text.secondary" }
			else nil,
	}
end

-- A pair of lines side by side (`M.pairs`): each half its number, sign
-- and text, on its own wash; a half with no line (a line only put in, or
-- only taken out) a quiet blank. Comments go on the right: what's said
-- is said of the change as it now stands.
local function half(l: Line?, side: string, runs_of: { Run }?, wrap: boolean?, slot: Node?): Node
	if not l then
		return ui.row {
			-- (Halves of one width: none of their own, the room shared.)
			w = 0,
			grow = 1,
			min_w = 0,
			min_h = M.LINE_H,
			h_full = true,
			bg = "$zeron.secondary.bg",
			aria_hidden = true,
		}
	end
	local kind = l.kind
	local wash = if kind == "+" then "$zeron.tone.success.wash" elseif kind == "-" then "$zeron.tone.danger.wash" else nil
	local ink = if kind == "+" then "$zeron.diff.added" elseif kind == "-" then "$zeron.diff.removed" else "$text.tertiary"
	local n = if side == "left" then l.old else l.new
	return ui.row {
		w = 0,
		grow = 1,
		min_w = 0,
		min_h = M.LINE_H,
		h_full = true,
		items = if wrap then "start" else "center",
		py = if wrap then ui.raw(2, "a wrapped line's text on the line's own baseline") else nil,
		overflow = "hidden",
		bg = wash,
		slot,
		ui.text { if n then tostring(n) else "", w = NUM, shrink = 0, align = "right", font = "$font.mono", text_size = "$font.size.xs", color = "$text.tertiary", tabular = true },
		ui.text { if kind == " " then "" else kind, w = SIGN, shrink = 0, align = "center", font = "$font.mono", text_size = "$font.size.sm", color = ink },
		code(l, runs_of, wrap),
	}
end

local SplitLine = ui.component(function(props: { pair: Pair, left_runs: { Run }?, right_runs: { Run }?, wrap: boolean?, on_comment: (() -> ())?, mark: (() -> string?)?, slot: { [string]: any }?, row: { [string]: any }? }): Node
	local over = ui.use_state(false)
	local dragging = ui.use_state(false)
	local p = props.pair
	local r = p.right
	local slot_props: { [any]: any } = {
		w = Inline.SLOT,
		h = M.LINE_H,
		shrink = 0,
		items = "center",
		justify = "center",
		if props.on_comment and r
			then ui.show(function()
				return over:get() or dragging:get()
			end, Inline.Add(add_props({
				label = locale.s("ds.git.comment_on_line", { n = tostring(r.new or "") }),
				on_click = props.on_comment,
			}, props.slot, dragging) :: any))
			else nil,
	}
	local slot = ui.div(slot_props)
	local function said(l: Line?): string
		return if l then `{if l.kind == " " then " " else l.kind}{l.text}` else ""
	end
	local row: { [any]: any } = {
		class = "diff-pair",
		label = if p.left == p.right then said(r) else `{said(p.left)} │ {said(r)}`,
		w_full = true,
		h = if props.wrap then nil else M.LINE_H,
		min_h = if props.wrap then M.LINE_H else nil,
		items = "stretch",
		relative = true,
		on_hover = if props.on_comment
			then function(inside: boolean)
				over:set(inside)
			end
			else nil,
		half(p.left, "left", props.left_runs, props.wrap, ui.div { w = Inline.SLOT, shrink = 0 }),
		ui.div { w = 1, shrink = 0, bg = "$zeron.settings.rule", aria_hidden = true },
		half(r, "right", props.right_runs, props.wrap, slot),
		mark_of(props.mark),
	}
	for k, v in props.row or {} do
		row[k] = v
	end
	return ui.row(row)
end)
M.SplitLine = SplitLine

-- What a block under a line is given to build itself with: where the
-- code's text starts (its words start there too), whether another block
-- stands right above it (then it draws no top hairline of its own), and
-- the scroll it stands in, for a block that must be seen as it opens.
export type BlockCtx = { inset: number, joined: boolean, reveal: Inline.Reveal? }

-- A block under a line: what kind it is (a key for it among the others on
-- the line: "thread", "composer"), and how to build it.
export type Block = { kind: string, build: (ctx: BlockCtx) -> Node }

export type Diff = {
	narrow: boolean?,
	-- What stands under a line (a thread on it, a comment being written),
	-- by its key: each a block, built with its context, top to bottom.
	after: ((key: string) -> { Block }?)?,
	-- Asked to comment on a line.
	on_comment: ((key: string, line: Line) -> ())?,
	-- The scroll the diff stands in (its element and offset), so a block
	-- opening under a line can bring itself into view.
	reveal: Inline.Reveal?,
	-- What stands at the end of the card's head (a link out).
	actions: Node?,
	-- Under the lines, inside the card (a long diff cut short, said so).
	foot: Node?,
}

-- The card's head over a file's diff: its status, its path, what it adds
-- and takes, and `actions` at its end. As tall as the files' card's head,
-- so beside it the two cards' first rows share a line.
function M.diff_head(f: File, actions: Node?): Node
	return ui.row {
		w_full = true,
		h = 40,
		shrink = 0,
		px = "$space.3",
		gap = "$space.2",
		items = "center",
		Badge.FileStatus { status = f.status :: any },
		FadeText { f.path, grow = 1, text_size = "$font.size.md", color = "$text.primary" },
		Badge.DiffStat { added = f.add, removed = f.del },
		actions,
	}
end

-- A diff of one file, unified: its hunks' headers, then their lines, each
-- with its old and new numbers, added lines on the success's wash, removed
-- on the danger's. Narrow, one column of numbers (the new line's, or the
-- old one's for a line taken out). Every line is built: for a long diff,
-- the virtualized list (ds/git_changes) draws only what is seen.
function M.diff(f: File, list: { Line }, opts: Diff?): Node
	local o: Diff = opts or {}
	-- The room the lines scroll in: a block under a line keeps to its
	-- width, not the longest line's. (The diff scrolls sideways as one
	-- column, so a block moves with it; the editor keeps its blocks outside
	-- the text's own scroll.)
	local room = ui.ref()
	local inset = M.gutter(o.narrow)
	local lines: { Node } = {}
	for _, l in list do
		if l.kind == "hunk" then
			table.insert(lines, M.hunk_row(l))
		else
			local key = M.line_key(l)
			table.insert(lines, DiffLine {
				line = l,
				narrow = o.narrow,
				on_comment = if o.on_comment
					then function()
						(o.on_comment :: any)(key, l)
					end
					else nil,
			})
			-- What stands under it, as `after` says now: each block its
			-- place's and kind's, so a comment opening under one line leaves
			-- the diff and every other block where they are. (A thread that
			-- takes a closed composer's place is a block of its own.)
			local after = o.after
			if after then
				local slots = ui.computed(function(): { string }
					local out: { string } = {}
					for i, b in after(key) or {} do
						table.insert(out, `{i}:{b.kind}`)
					end
					return out
				end)
				table.insert(
					lines,
					ui.each(slots, function(slot: string): Node
						local i = tonumber(string.match(slot, "^(%d+)")) :: number
						local b = (after(key) or {})[i]
						if not b then
							return ui.div {}
						end
						-- (16 short of the room: a band's end keeps its own padding's
						-- worth of room from the card's edge, where a line runs to it.)
						return ui.div {
							w = function(): number
								return (room.width or 600) - 16
							end,
							shrink = 0,
							self = "start",
							b.build({ inset = inset, joined = i > 1, reveal = o.reveal }),
						}
					end, {
						key = function(slot: string): string
							return slot
						end,
						w_full = true,
					})
				)
			end
		end
	end
	local rows: { Node } = {
		-- The lines scroll sideways as one, their washes running the whole
		-- way; a line past the edge is cut there, plainly (no fade: the
		-- card's edge is the edge), and scrolls into view.
		ui.scroll_x {
			id = "diff-lines",
			ref = room,
			w_full = true,
			ui.column { id = "diff-content", min_w = "100%", py = "$space.1", table.unpack(lines) },
		},
	}
	if o.foot then
		table.insert(rows, o.foot)
	end
	return M.card(M.diff_head(f, o.actions), rows)
end

-- The checks on a commit: each what it is, how it stands (passed in how
-- long, running, or what failed), and a way to its log. `state` is
-- "passing", "failing" or "running" (anything else: a quiet glyph, as
-- one skipped or cancelled); `link`, what the way to its log says
-- ("Log", "See what failed" by default); `on_open`, where it goes.
export type Check = { name: string, state: string, detail: string, link: string?, on_open: (() -> ())? }

local function check_lead(state: string, size: number?): Node
	if state == "passing" then
		return M.glyph("check-circle", "success", size)
	elseif state == "failing" then
		return M.glyph("close-circle", "danger", size)
	elseif state == "running" then
		return M.glyph(progress.ProgressRing { size = if (size or 32) >= 32 then 16 else 14, tone = "neutral", label = locale.s("ds.git.running") }, nil, size)
	end
	return M.glyph("minus", nil, size)
end

local function check_row(c: Check): Node
	local link = c.link or (if c.state == "failing" then locale.s("ds.git.see_what_failed") else locale.s("ds.git.log"))
	return Settings.Row {
		label = c.name,
		description = c.detail,
		lead = check_lead(c.state),
		control = if link ~= "" then layout.Link { link, tone = "quiet", size = "sm", on_click = c.on_open } else nil,
	}
end
M.check_row = check_row

function M.checks(list: { Check }): Node
	local rows: { Node } = {}
	for _, c in list do
		table.insert(rows, check_row(c))
	end
	return Settings.Group(rows :: any)
end

-- How the checks stand, in a few words: "2 of 3 passed", and what to
-- know without opening them: "Tests failed", or "Tests running".
-- With more than one failed (or running), the first named and how many
-- more: "core-tests +2 failed".
function M.checks_summary(list: { Check }): (string, string?, string)
	local passed = 0
	local failing: Check? = nil
	local running: Check? = nil
	local failed_n, running_n = 0, 0
	for _, c in list do
		if c.state == "passing" then
			passed += 1
		elseif c.state == "failing" then
			failing = failing or c
			failed_n += 1
		elseif c.state == "running" then
			running = running or c
			running_n += 1
		end
	end
	local called = if failing
		then (if failed_n > 1
			then locale.s("ds.git.check_failed_more", { name = (failing :: Check).name, n = tostring(failed_n - 1) })
			else locale.s("ds.git.check_failed", { name = (failing :: Check).name }))
		elseif running then (if running_n > 1
			then locale.s("ds.git.check_running_more", { name = (running :: Check).name, n = tostring(running_n - 1) })
			else locale.s("ds.git.check_running", { name = (running :: Check).name }))
		else nil
	local outcome = if failing then "danger" elseif running then "neutral" else "success"
	return locale.s("ds.git.passed_of", { passed = tostring(passed), total = tostring(#list) }), called, outcome
end

-- The checks as a block that folds: one line, the outcome's glyph, how
-- they stand and what failed, a chevron; open, the checks themselves and
-- the log of the one that failed (`log`). `open` is the caller's (the
-- gallery remembers it, as the app would). It opens and closes as the
-- design system's trees do (height, 200 ms, out quart). `summary` says
-- something else on its second line (there are no checks: "No checks
-- reported"); `empty`, what it says open with none.
export type ChecksBlock = { list: { Check }, open: State<boolean>, log: Node?, summary: string?, empty: string? }

function M.checks_block(o: ChecksBlock): Node
	local summary, called, outcome = M.checks_summary(o.list)
	if o.summary then
		summary, called, outcome = o.summary, nil, if #o.list == 0 then "none" else outcome
	end
	local open = o.open
	local lead: Node
	if outcome == "danger" then
		lead = M.glyph("close-circle", "danger")
	elseif outcome == "neutral" then
		lead = M.glyph(progress.ProgressRing { size = 16, tone = "neutral", label = locale.s("ds.git.running") }, nil)
	elseif outcome == "none" then
		lead = M.glyph("minus", nil)
	else
		lead = M.glyph("check-circle", "success")
	end
	local rows: { Node } = {}
	for _, c in o.list do
		table.insert(rows, ui.div { h = 1, w_full = true, shrink = 0, bg = "$zeron.settings.rule", aria_hidden = true })
		table.insert(rows, check_row(c))
	end
	if #o.list == 0 and o.empty then
		table.insert(rows, ui.div { h = 1, w_full = true, shrink = 0, bg = "$zeron.settings.rule", aria_hidden = true })
		table.insert(rows, ui.div { w_full = true, px = "$space.4", py = "$space.3", ui.text { o.empty, wrap = true, text_size = "$font.size.sm", color = "$text.secondary" } })
	end
	if o.log then
		table.insert(rows, ui.div { w_full = true, px = "$space.4", pb = "$space.4", o.log })
	end
	local header = ui.button {
		id = "checks-head",
		label = if called
			then locale.t("ds.git.checks_label_called", { summary = summary, called = called })
			else locale.t("ds.git.checks_label", { summary = summary }),
		expanded = open,
		w_full = true,
		h = 60,
		px = "$space.4",
		gap = "$space.3",
		items = "center",
		cursor = "pointer",
		bg = "#00000000",
		hover = { bg = "$hover" },
		active = { bg = "#00000000" },
		focus = { border = 0 },
		on_click = function()
			open:set(not open:peek())
		end,
		lead,
		ui.column {
			grow = 1,
			min_w = 0,
			items = "start",
			gap = "$zeron.space.hair",
			ui.text { locale.t("ds.git.checks"), text_size = "$font.size.md", line_height = "$font.line.md", weight = "$font.weight.medium", color = "$text.primary" },
			-- One line, how they stand and what to know, fading at its end
			-- where it runs long (never wrapping); the outcome is the glyph's
			-- to say, so the words stay quiet.
			FadeText {
				if called then `{summary} · {called}` else summary,
				literal = true,
				w_full = true,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = "$text.secondary",
			},
		},
		-- (The whole row opens it, its chevron says so: no words for it.)
		-- The chevron turns over as the block opens, in a slot of its own.
		M.chevron(function(): number
			return if open:get() then 180 else 0
		end, 16, 200),
	}
	-- Closed, the body holds nothing (a box of no height with children in
	-- it is a layout warning); the column's height eases between the two.
	local body = ui.column {
		id = "checks-body",
		w_full = true,
		overflow = "hidden",
		layout = "height",
		layout_clip = true,
		layout_duration = 200,
		layout_ease = "outQuart",
		ui.show(open, ui.column { w_full = true, table.unpack(rows) }),
	}
	return ui.column {
		w_full = true,
		rounded = M.RADIUS,
		bg = "$zeron.settings.card",
		border = M.EDGE,
		border_color = "$zeron.settings.edge",
		overflow = "hidden",
		header,
		body,
	}
end

-- A check's log, as a quiet panel of mono lines.
function M.log(lines: { string }): Node
	local rows: { Node } = {}
	for _, s in lines do
		table.insert(rows, ui.text { s, literal = true, font = "$font.mono", text_size = "$font.size.xs", line_height = "$font.line.sm", color = if string.match(s, "^FAIL") then "$zeron.tone.danger.ink" else "$text.secondary", wrap = true })
	end
	return ui.column {
		w_full = true,
		px = "$space.3",
		py = "$space.2",
		rounded = "$radius.md",
		bg = "$zeron.secondary.bg",
		table.unpack(rows),
	}
end

-- What happened to a pull request, in order: its commits (a run of them as
-- one event, so comments stand out), each comment, the checks' outcome,
-- each verdict, and the merge. Each with who and when (`at`, seconds since
-- the epoch). A comment's `where` is the line it's on, `on_where` a way
-- there (named `where_label`); `content` stands in for its words (its
-- markdown); `verb` says what someone did in other words than the kind's.
export type Event = {
	kind: "commits" | "comment" | "checks" | "review" | "merged",
	at: number,
	who: people.Who?,
	commits: { any }?,
	body: string?,
	content: Node?,
	where: string?,
	on_where: (() -> ())?,
	where_label: string?,
	checks: { Check }?,
	sha: string?,
	verdict: string?,
	verb: string?,
}

-- Where an activity's pull request goes: its branch, and its base.
export type ActivityOpts = { branch: string?, base: string? }

local LEAD = 24

-- An event's first line: who, what they did, and when.
local function line(e: Event, what: string | Message, extra: Node?): Node
	return ui.row {
		w_full = true,
		min_h = LEAD,
		gap = "$space.1",
		items = "center",
		flex_wrap = true,
		if e.who then people.name(e.who, "$font.size.sm") else nil,
		ui.text { what, literal = true, text_size = "$font.size.sm", color = "$text.secondary" },
		extra,
		people.Timestamp { at = e.at, size = "sm", tone = "tertiary" },
	}
end

-- A quiet card under an event's line (a comment's words, a run of commits).
local function note(...: Node): Node
	return ui.column {
		w_full = true,
		px = "$space.3",
		py = "$space.2",
		gap = "$space.1",
		rounded = "$radius.lg",
		bg = "$zeron.settings.card",
		border = 1,
		border_color = "$zeron.settings.edge",
		...,
	}
end

-- What someone said, in a note: their own content, or the words as text.
local function said(e: Event): Node
	return note(e.content or ui.text { e.body or "", literal = true, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.primary", wrap = true })
end

-- Where a comment is, as a mono chip: a link to it when it goes somewhere.
local function where_chip(e: Event): Node
	local chip = Badge.Badge { e.where or "", mono = true }
	if not e.on_where then
		return chip
	end
	return ui.row {
		role = "link",
		label = e.where_label or (e.where or ""),
		focusable = true,
		cursor = "pointer",
		shrink = 0,
		rounded = "$radius.sm",
		hover = { opacity = 0.8 },
		focus = control.ring,
		on_click = e.on_where,
		chip,
	}
end

local function event(e: Event, last: boolean, o: ActivityOpts): Node
	local lead: Node
	local body: Node
	if e.kind == "commits" then
		local commits = e.commits or {}
		lead = M.glyph("git-commit", nil, LEAD)
		local rows: { Node } = {}
		for _, c in commits do
			table.insert(rows, ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				ui.text { c.sha, literal = true, w = 60, shrink = 0, font = "$font.mono", text_size = "$font.size.xs", color = "$text.tertiary" },
				FadeText { c.subject, grow = 1, text_size = "$font.size.sm", color = "$text.primary" },
			})
		end
		body = ui.column {
			w_full = true,
			gap = "$space.2",
			line(
				e,
				e.verb
					or (if o.branch
						then locale.t("ds.git.pushed_to", { n = tostring(#commits), branch = o.branch })
						else locale.t("ds.git.pushed", { n = tostring(#commits) }))
			),
			note(table.unpack(rows)),
		}
	elseif e.kind == "comment" then
		local who = e.who or { id = "someone", name = locale.s("ds.git.someone") }
		lead = people.face(who, LEAD)
		local parts: { Node } = {
			if e.where
				then line(e, e.verb or locale.t("ds.git.commented_on"), where_chip(e))
				else line(e, e.verb or locale.t("ds.git.commented")),
		}
		if (e.body and e.body ~= "") or e.content then
			table.insert(parts, said(e))
		end
		body = ui.column { w_full = true, gap = "$space.2", table.unpack(parts) }
	elseif e.kind == "checks" then
		local summary, called, outcome = M.checks_summary(e.checks or {})
		lead = if outcome == "danger"
			then M.glyph("close-circle", "danger", LEAD)
			elseif outcome == "neutral" then M.glyph(progress.ProgressRing { size = 14, tone = "neutral", label = locale.s("ds.git.running") }, nil, LEAD)
			else M.glyph("check-circle", "success", LEAD)
		body = line(
			e,
			if called
				then locale.t("ds.git.checks_ran_called", { sha = e.sha or "", summary = summary, called = called })
				else locale.t("ds.git.checks_ran", { sha = e.sha or "", summary = summary }),
			nil
		)
	elseif e.kind == "review" then
		local who = e.who or { id = "someone", name = locale.s("ds.git.someone") }
		lead = people.face(who, LEAD)
		local verb = e.verb
			or (if e.verdict == "approved"
				then locale.t("ds.git.approved")
				elseif e.verdict == "changes" then locale.t("ds.git.asked_for_changes")
				else locale.t("ds.git.commented"))
		local first = line(e, verb, Badge.Review { state = e.verdict :: any, size = "sm" })
		if (e.body and e.body ~= "") or e.content then
			body = ui.column { w_full = true, gap = "$space.2", first, said(e) }
		else
			body = first
		end
	else
		lead = M.glyph("pull-request", "success", LEAD)
		body = line(e, e.verb or (if o.base then locale.t("ds.git.merged_into", { base = o.base }) else locale.t("ds.git.merged")))
	end
	return ui.row {
		class = "git-event",
		w_full = true,
		gap = "$space.3",
		items = "start",
		pb = if last then 0 else "$space.4",
		-- The lead on the window's own fill, so the trunk passes behind it.
		ui.div { w = LEAD, h = LEAD, shrink = 0, rounded = "$radius.full", bg = "$zeron.window", items = "center", justify = "center", lead },
		ui.column { grow = 1, min_w = 0, body },
	}
end

function M.activity(events: { Event }, opts: ActivityOpts?): Node
	local o = opts or {}
	local rows: { Node } = {}
	for i, e in events do
		table.insert(rows, event(e, i == #events, o))
	end
	return ui.column {
		w_full = true,
		relative = true,
		-- The trunk down the leads, from the first's middle to the last's.
		ui.div {
			absolute = true,
			left = LEAD / 2 - 0.5,
			top = LEAD / 2,
			bottom = LEAD / 2,
			w = 1,
			bg = "$zeron.settings.rule",
			pointer_events = "none",
			aria_hidden = true,
		},
		table.unpack(rows),
	}
end

return M
