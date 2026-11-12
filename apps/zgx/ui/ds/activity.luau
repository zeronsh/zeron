-- What an agent did on its way to a reply, as Zeron's transcript draws its
-- tools (transcript.rs, `activity_rail`): a quiet summary ("Ran 3 commands ·
-- read 2 files"), the working loader before it while it works, and under it,
-- open (a click on the summary), a
-- row a tool: a hairline trunk down the gutter, a rounded elbow off it to
-- each tool's glyph, then what it did. The sidebar's groups draw the same
-- tree (`$zeron.tree`, 1 point, 6-point elbows).
--
-- While the agent works the tree is open and its rows arrive one by one,
-- each sliding in under the last as the trunk reaches down to it; the
-- summary rolls as it grows. Settled, it folds to its summary unless opened.
--
-- A thought is a row with no glyph, no branch and no line of its own: its
-- words themselves sit in the tree, rendered as the reply's markdown is,
-- folded past four lines under "Show all N lines" (`thought_row`). The
-- trunk runs past a thought only to a tool below it: the tree ends at the
-- last tool call.
--
-- `Activity { tools = function() return tools end, live = function() … end }`.

local tree_line = require("@self/tree_line")
local control = require("@self/control")
local Icon = require("@self/icon")
local FadeText = require("@self/fade_text")
local status = require("@self/status")
local ToolDetail = require("@self/tool_detail")
local Markdown = require("@self/markdown")
local locale = require("@self/locale")
local FoldButton = require("@self/fold_button")

-- (`cloud`, `git`: a Cloud machine's own setup steps, the machine and its
-- checkout.)
export type Kind = "exec" | "read" | "edit" | "write" | "search" | "glob" | "web" | "todo" | "agent" | "thought" | "cloud" | "git"
-- One tool: what kind, what it did ("Read", "transcript.rs"), and for an
-- edit the lines it added and removed; `failed` tints its glyph.
export type Tool = {
	id: string,
	kind: Kind,
	verb: string,
	-- A subagent's spawn: the doc it runs in, how it stands, its title.
	subagent: { ref: string, title: string?, status: string? }?,
	target: string?,
	-- The whole call, when the line says less (a command of many lines, a
	-- tool's input): the row opens to it.
	call: string?,
	-- Its call is prose (a thought's words), not code.
	prose: boolean?,
	-- Being written now (a thought's words still coming): its fold shows
	-- the latest lines, as Zeron's streaming thought keeps its tail.
	live: boolean?,
	added: number?,
	removed: number?,
	failed: boolean?,
	-- What its row opens to beyond its call (ds/tool_detail): its output
	-- (as the harness gave it, escapes and all, growing while it runs), a
	-- command's exit code, its files' counts, an older doc's diff, and the
	-- refs of its whole output and diff.
	output: string?,
	exit_code: number?,
	output_ref: string?,
	output_bytes: number?,
	diff_ref: string?,
	stats: { ToolDetail.Stat }?,
	diff: ToolDetail.Diff?,
}

export type Writing = { words: (id: string) -> string?, seeded: () -> boolean }

export type Props = {
	tools: () -> { Tool },
	live: (() -> boolean)?,
	-- How long it worked, in seconds, once it is done ("Worked for 42s").
	worked: (() -> number?)?,
	-- Its line, as the caller words it (Zeron's grouping, the host's); by
	-- default, counted from its tools here.
	summary: (() -> string?)?,
	-- A name to keep whether the reader opened or folded it under, across
	-- it being made again (a reload, a list row scrolled back).
	key: string?,
	-- What it folds, made only while open or folding, in place of its
	-- tools' rows: a compact transcript's work (its words and groups).
	body: (() -> Node)?,
	-- A subagent's spawn row (its tool's `subagent`): what opens it (its
	-- lane). Without it, the row opens its call as any does.
	open_subagent: ((sub: any) -> ())?,
	-- A thought being written: `words`, a thought's words as they arrive, by
	-- its id (nil for one not being written: the reply's draft holds them,
	-- so a tick of them doesn't build its rows again); `seeded`, whether what
	-- a row finds as it's made stays as it is rather than fading in (a chat
	-- opened mid-thought).
	thought: Writing?,
	-- Where it shows (a transcript's list and its scroll): its rows built
	-- only near the view (`Window`).
	window: Window?,
}

-- A transcript's view, for a group windowed in it: `key` names the group
-- (its runs' heights are kept by it), `view` is the scroll it shows in,
-- `offset` its scroll; `at_end`, whether the view rests at its end; `row`,
-- the list's row holding the group (ds/markdown's `window`, the same).
export type Window = { key: string, view: Ref, offset: State<number>, at_end: State<boolean>?, row: Ref? }

local GLYPH: { [Kind]: Icon.Name } = {
	exec = "terminal",
	read = "document",
	edit = "pen",
	write = "document-add",
	search = "search",
	glob = "folder-with-files",
	web = "global",
	todo = "checklist",
	agent = "side-chat",
	thought = "chat-round-line",
	cloud = "cloud",
	git = "git-branch",
}

-- The tree, in points: the trunk's x (under the working loader's middle, so
-- the trunk runs down from it), the elbow's radius (Zeron's), where the
-- branch ends, the glyph's place and size, and the gutter they take before
-- the words. Zeron hangs its tree off a chevron 12 points in; with no
-- chevron the tree starts at the reply's own left edge. Rows are a little
-- tighter than Zeron's 32: a long run of tools reads as one list, not a
-- column of chips.
local LOADER = 16
local TRUNK, BEND, BRANCH_END = LOADER / 2 + 0.5, 6, 20
local GLYPH_LEFT, GLYPH_SIZE = 24, 16
local GUTTER = 48
local ROW, HEADER = 28, 26
-- How long the tree takes to open and fold.
local FOLD = 200
-- How a row joining the tree arrives.
local ENTER: any = { opacity = { 0, 1 }, y = { -6, 0 }, duration = 360, ease = "outExpo" }

-- Each row's height as last laid out, by its group and its tool: a group
-- made again (its row scrolled back) holds each row as it was. A plain
-- table, a key at a time; it starts again as it fills.
local KEPT_MOST = 8192
local MEASURED: { [string]: number } = {}
local kept_count: { [any]: number } = {}
local function kept_set(t: { [string]: any }, k: string, value: any)
	if t[k] == nil then
		local n = (kept_count[t] or 0) + 1
		if n > KEPT_MOST then
			table.clear(t)
			n = 1
		end
		kept_count[t] = n
	end
	t[k] = value
end

-- The summary: each kind counted, in the order an agent tends to work.
-- (Each kind's phrase a message, `{n, plural, …}`: its key.)
local PHRASES: { { kind: Kind, key: string } } = {
	{ kind = "thought", key = "ds.activity.phrase.thought" },
	{ kind = "search", key = "ds.activity.phrase.search" },
	{ kind = "glob", key = "ds.activity.phrase.glob" },
	{ kind = "read", key = "ds.activity.phrase.read" },
	{ kind = "exec", key = "ds.activity.phrase.exec" },
	{ kind = "edit", key = "ds.activity.phrase.edit" },
	{ kind = "write", key = "ds.activity.phrase.write" },
	{ kind = "web", key = "ds.activity.phrase.web" },
	{ kind = "todo", key = "ds.activity.phrase.todo" },
	{ kind = "agent", key = "ds.activity.phrase.agent" },
}

local function summary(tools: { Tool }): string
	local counts: { [string]: number } = {}
	local failed = 0
	for _, t in tools do
		counts[t.kind] = (counts[t.kind] or 0) + 1
		if t.failed then
			failed += 1
		end
	end
	local parts: { string } = {}
	for _, p in PHRASES do
		local n = counts[p.kind]
		if n then
			table.insert(parts, locale.s(p.key, { n = n }))
		end
	end
	if failed > 0 then
		table.insert(parts, locale.s("ds.activity.failed", { n = tostring(failed) }))
	end
	local s = table.concat(parts, " · ")
	return string.upper(string.sub(s, 1, 1)) .. string.sub(s, 2)
end

-- What a row's line says of its tool, on one line whatever it was given
-- (a command's newlines, a pattern's tabs, as single spaces).
local function line_of(t: Tool): string
	return (string.gsub(t.target or "", "%s+", " "))
end

-- A row opened: the whole call, in its own block under the line, wrapping,
-- as tall as it is.
local function call_block(now: () -> Tool): Node
	return ui.div {
		ml = ui.raw(GUTTER, "under the line's words, past the tree's gutter"),
		mb = "$space.1",
		px = "$space.3",
		py = "$space.2",
		rounded = "$radius.md",
		bg = "$zeron.code.wash",
		ui.text {
			function(): string
				return now().call or ""
			end,
			literal = true,
			wrap = true,
			font = if now().prose then nil else "$font.mono",
			text_size = if now().prose then "$font.size.sm" else "$font.size.xs",
			line_height = if now().prose then "$font.line.sm" else "$font.line.xs",
			color = "$text.secondary",
		},
	}
end

-- One tool's row: its branch off the trunk (the trunk going on below unless
-- it is the last), its glyph, what it did, on one line that clips whatever
-- is in it, so no tool's words draw over the next. It follows its tool
-- (`now`) as it goes on: finished, failed, its lines counted. A tool whose
-- line says less than its call opens to it (a press on the line), kept open
-- by its id.
-- (`running`: it is the one in flight, its glyph the working loader.)
-- What the reader opened, kept apart from the rows (a reload, a row
-- scrolled back): each tool's call by its id, each group by its key. One
-- map each, holding only what's open (or, for a group, chosen), not a
-- state per tool.
-- A row's call and what it did, as it opens (`row`): `now`, its tool.
local Opened = ui.component(function(props: { now: () -> Tool }): Node
	local now = props.now
	return ui.column {
		w_full = true,
		ui.show(function(): boolean
			return now().call ~= nil
		end, call_block(now)),
		ui.show(
			function(): boolean
				return ToolDetail.has(now() :: any)
			end,
			ui.div {
				ml = ui.raw(GUTTER, "under the line's words, past the tree's gutter"),
				mb = "$space.1",
				ToolDetail.ToolDetail { tool = now :: any },
			}
		),
	}
end)

-- A row's line, as FadeText makes it (ds/fade_text: a box that clips the
-- text at its own width, fading its end while it overflows), built here
-- directly: a long group makes thousands.
local function target_line(now: () -> Tool): Node
	local clip, text = ui.ref(), ui.ref()
	return ui.row {
		ref = clip,
		shrink = 1,
		min_w = 0,
		overflow = "hidden",
		items = "center",
		fade_x = function()
			local over = (text.width or 0) > (clip.width or 0) + 0.5
			return { 0, if over then 24 else 0 }
		end,
		ui.text {
			function(): string
				return line_of(now())
			end,
			ref = text,
			shrink = 0,
			literal = true,
			font = "$font.mono",
			text_size = "$font.size.sm",
			color = "$text.primary",
		},
	}
end

-- A row's branch (`tree_line.branch`), made once for each height and
-- whether the trunk goes on: rows of a height share it.
local BRANCHES: { [string]: any } = {}
local function branch_of(h: number, on: boolean): any
	local k = `{h}:{on}`
	local path = BRANCHES[k]
	if path == nil then
		path = tree_line.branch { x = TRUNK, mid = ROW / 2, tip = BRANCH_END, h = h, bend = BEND, on = on }
		kept_set(BRANCHES, k, path)
	end
	return path
end

local CALLS_OPEN = ui.state("activity_calls_open", {} :: { [string]: boolean })
local GROUPS_CHOSEN = ui.state("activity_groups_chosen", {} :: { [string]: boolean })

-- A thought's row: no glyph, no branch, no header, its words themselves
-- where a tool's line sits, rendered as the reply's markdown is
-- (ds/markdown, a step smaller, in the secondary ink); the trunk passes it
-- only on its way to a tool below (`below`). Past four lines it folds: the
-- words fade out over the last line and a half, and under them, once they
-- have faded away, a small quiet "Show all N lines"; opened, the whole
-- thought, "Show fewer lines" under it to fold it back.
-- Open is kept by the thought's id, as a call's is. A thought that fits
-- shows no button. Whether it folds is measured, as a long message's is
-- (ds/user_message): the words are laid out whole behind the clip, so
-- nothing rewraps as it opens.
--
-- Being written or settled, the fold is the same: its first lines, fading
-- out at the foot, "Show all N lines" under them. (Zeron's streaming thought
-- keeps its tail, the fade at the top; the user wanted one fold, faded at
-- the foot, Nov 2 2026.)
local THOUGHT_LINES = 4
local THOUGHT_FADE = 1.5
-- The button, and the room it takes under the words (folded, under their
-- faded foot; open, under their last line).
local THOUGHT_BUTTON = FoldButton.HEIGHT
local THOUGHT_ROOM = THOUGHT_BUTTON + 6
-- The gap ds/markdown leaves between a small text's blocks (`$space.3`):
-- taken off the words' height to count their lines.
local THOUGHT_GAP = 12

-- How many gaps between blocks (`\n`, blank space, `\n`) a thought's words
-- hold, counted as they grow: words that only gained an end are counted
-- from the last gap that could still change, not from their start, so a
-- long thought being written costs each tick its new words alone.
local GAP = "\n%s*\n"
-- (How much of the words' start and end is looked at to know they only grew.)
local EDGE = 64
local function gap_counter(): (text: string) -> number
	local seen, seen_len = "", 0
	-- Gaps wholly before `from` (none of them reaching the end then), and
	-- where to look on from.
	local settled, from = 0, 1
	local head, tail = "", ""
	local counted = 0
	return function(text: string): number
		if text == seen then
			return counted
		end
		if not (seen_len > 0
			and #text > seen_len
			and string.sub(text, 1, #head) == head
			and string.sub(text, seen_len - #tail + 1, seen_len) == tail)
		then
			-- (Not grown from what was counted: counted afresh.)
			settled, from = 0, 1
		end
		local n = settled
		local at = from
		while true do
			local s, e = string.find(text, GAP, at)
			if not s or not e then
				break
			end
			n += 1
			-- (One that runs to the end may run on as words come.)
			if e < #text then
				settled, from = n, e + 1
			end
			at = e + 1
		end
		-- No gap starts before the blank space the words end in (one there
		-- would end before it, and is counted): the next look starts there.
		local q = #text
		while q >= from do
			local b = string.byte(text, q)
			if b ~= 32 and b ~= 9 and b ~= 10 and b ~= 13 and b ~= 11 and b ~= 12 then
				break
			end
			q -= 1
		end
		from = math.max(from, q + 1)
		seen, seen_len, counted = text, #text, n
		head = string.sub(text, 1, EDGE)
		tail = string.sub(text, math.max(1, #text - EDGE + 1))
		return n
	end
end

local function thought_row(
	now: () -> Tool,
	below: () -> boolean,
	writing: Writing?,
	group: string?,
	resized: ((size: { w: number, h: number }) -> ())?
): Node
	local id = now().id
	local line = ui.theme.get("zeron.prose.small.line") :: number
	local folded_h = THOUGHT_LINES * line
	local body = ui.ref()
	local function is_open(): boolean
		return CALLS_OPEN:get()[id] == true
	end
	local function set_open(on: boolean)
		local all = table.clone(CALLS_OPEN:peek())
		all[id] = on
		CALLS_OPEN:set(all)
	end
	local function live(): boolean
		return now().live == true
	end
	-- (A thought named only, without its words: its name.)
	local function words(): string
		local arriving = if writing then writing.words(id) else nil
		return arriving or now().call or now().verb
	end
	-- Folds once the words run past four lines (half a point over for
	-- rounding); a ref reads 0 before its first layout, so a thought stands
	-- open until measured, which for one that fits changes nothing.
	local long = ui.computed(function(): boolean
		return (body.height or 0) > folded_h + 0.5
	end)
	local function folded(): boolean
		return long:get() and not is_open()
	end
	-- About how many lines the words run to: their height in lines of the
	-- prose, less the gaps between their blocks.
	local gaps = gap_counter()
	local function lines(): number
		return math.max(1, math.floor(((body.height or 0) - gaps(words()) * THOUGHT_GAP) / line + 0.5))
	end
	-- The clip: four lines while folded; open, as tall as the words. Not
	-- measured yet (just mounted: a tool group opening), four lines too, a
	-- ceiling a short thought never meets: a long one starts folded rather
	-- than standing whole for a frame and shrinking back, which made a group
	-- opening overshoot and settle.
	local function window(): number
		if (body.height or 0) <= 0 and not is_open() then
			return folded_h
		end
		return if folded() then folded_h else 1e6
	end
	-- Written and folded: the words shifted up so their latest line sits
	-- at the window's foot.
	local function rise(): number
		return 0
	end
	local function fade(): { number }
		if not folded() then
			return { 0, 0 }
		end
		local px = line * THOUGHT_FADE
		return { 0, px }
	end
	local function label(): string
		return if is_open() then locale.s("ds.activity.show_fewer_lines") else locale.s("ds.activity.show_all_lines", { n = tostring(lines()) })
	end
	return ui.column {
		w_full = true,
		relative = true,
		class = "thought",
		on_resize = resized,
		-- No branch: the trunk alone, past the words to a tool below, when
		-- one follows; nothing when the thought ends the group.
		ui.canvas {
			absolute = true,
			left = 0,
			top = 0,
			bottom = 0,
			w = TRUNK + 1,
			pointer_events = "none",
			draw = function(c, _, h)
				if below() then
					c:rect(TRUNK - 0.5, 0, 1, h, "$zeron.tree")
				end
			end,
		},
		ui.div {
			w_full = true,
			min_w = 0,
			pl = ui.raw(GUTTER, "where a tool's words start, past the tree's gutter"),
			py = ui.raw((ROW - line) / 2, "the first line's middle on the branch, as a tool's line is"),
			ui.column {
				relative = true,
				w_full = true,
				min_w = 0,
				ui.column {
					class = "thought-words",
					w_full = true,
					min_w = 0,
					overflow = "hidden",
					max_h = window,
					layout = "height",
					layout_clip = true,
					layout_duration = 260,
					layout_ease = "outQuart",
					fade = fade,
					ui.column {
						ref = body,
						class = "thought-body",
						w_full = true,
						min_w = 0,
						shrink = 0,
						y = rise,
						Markdown {
							text = words,
							-- (Parsed incrementally while it can grow, its words
							-- fading in as a reply's do. Named by its group too:
							-- a part's id is only its own chat's.)
							key = if now().live then `thought:{group or ""}:{id}` else nil,
							veil = now().live == true,
							veil_seed = if writing then writing.seeded() else nil,
							small = true,
							ink = "$text.secondary",
						},
					},
				},
				-- The button's room under the clip, once there is a button:
				-- the words have faded away before it shows.
				ui.div {
					w_full = true,
					shrink = 0,
					h = function(): number
						return if long:get() then THOUGHT_ROOM else 0
					end,
				},
				-- The way to open and fold it: a small, quiet button on the
				-- fill, at the words' left edge, in the room under them.
				ui.show(
					long,
					FoldButton.FoldButton {
						label = label,
						open = is_open,
						on_click = function()
							set_open(not is_open())
						end,
						place = { absolute = true, left = 0, bottom = 0 },
					}
				),
			},
		},
	}
end

-- (A tool that may come to have a result opens: whether it has one isn't
-- known as its row is made.)
local OPENS: { [string]: boolean } = { exec = true, read = true, edit = true, write = true, search = true, glob = true, web = true }
-- Kinds whose row counts lines added and removed (an edit's).
local COUNTS: { [string]: boolean } = { edit = true, write = true }

-- (`below`: a tool with a branch follows, so the trunk runs on under this
-- one; `running`: it is the one in flight, its glyph the working loader;
-- `resized`: told the row's size as it's laid out; `settled`: it is before
-- the group's last, so never the one in flight.)
-- A row is built often (each comes into view, a long group's thousands), so
-- what can't change once a tool has started is read once as it's made (its
-- kind, its verb, its glyph); only what a tool's result changes follows it.
local function row(
	now: () -> Tool,
	below: () -> boolean,
	running: () -> boolean,
	open_subagent: ((sub: any) -> ())?,
	writing: Writing?,
	group: string?,
	resized: ((size: { w: number, h: number }) -> ())?,
	settled: boolean?
): Node
	local t = now()
	if t.kind == "thought" then
		return thought_row(now, below, writing, group, resized)
	end
	local function tint(): string
		return if now().failed then "$danger" else "$text.secondary"
	end
	local id = t.id
	local function has_call(): boolean
		return now().call ~= nil
	end
	-- Opens to what it did, too: an edit's diff, a command's output, a
	-- read's or a search's result (ds/tool_detail), once it has some.
	local function has_detail(): boolean
		return ToolDetail.has(now() :: any)
	end
	-- Open as the reader left it (opened or closed), else closed.
	local function showing(): boolean
		if not (has_call() or has_detail()) then
			return false
		end
		return CALLS_OPEN:get()[id] == true
	end
	-- A subagent's spawn, with somewhere to open it: the row opens its lane.
	local function lane(): any
		return if open_subagent then (now() :: any).subagent else nil
	end
	local opens = t.call ~= nil or lane() ~= nil or OPENS[t.kind] == true
	-- (Only a subagent's spawn says more than its line: one that may open
	-- its lane follows it.)
	local label: string | () -> string = if t.target then `{t.verb} {line_of(t)}` else t.verb
	-- A Cloud machine's step says how far it is as it goes ("Waking its Cloud
	-- machine — cloning the repository…"): its words follow it.
	local verb: string | () -> string = t.verb
	if t.kind == "cloud" or t.kind == "git" then
		verb = function(): string
			return now().verb
		end
		label = verb
	end
	if open_subagent and t.kind == "agent" then
		label = function(): string
			local sub = lane()
			if sub then
				return locale.s("ds.activity.open_subagent", { title = sub.title or locale.s("ds.activity.subagent") })
			end
			local n = now()
			return if n.target then `{n.verb} {line_of(n)}` else n.verb
		end
	end
	return ui.column {
		w_full = true,
		relative = true,
		on_resize = resized,
		-- The branch, and the trunk on down past an opened call.
		ui.canvas {
			absolute = true,
			left = 0,
			top = 0,
			bottom = 0,
			w = BRANCH_END + 1,
			pointer_events = "none",
			draw = function(c, _, h)
				local mid = ROW / 2
				c:fill(branch_of(h, below()), "$zeron.tree")
			end,
		},
		ui.button {
			w_full = true,
			h = ROW,
			p = 0,
			items = "center",
			relative = true,
			overflow = "hidden",
			bg = "#00000000",
			hover = { bg = "#00000000" },
			active = { bg = "#00000000" },
			-- Only a row that opens is a stop for the keys (a long run of
			-- tools isn't a long run of stops), its ring showing there.
			focusable = opens,
			focus = if opens then control.ring else { border = 0 },
			selectable = false,
			label = label,
			-- (A tool's call comes with it: whether it opens is known as its
			-- row is made.)
			expanded = if opens then showing else nil,
			cursor = if opens then "pointer" else nil,
			on_click = function()
				local sub = lane()
				if sub and open_subagent then
					open_subagent(sub)
					return
				end
				if has_call() or has_detail() then
					local all = table.clone(CALLS_OPEN:peek())
					all[id] = not showing()
					CALLS_OPEN:set(all)
				end
			end,
			-- (A row before the group's last never runs: its glyph alone, placed
			-- and coloured itself. The last may be the one in flight.)
			if settled
				then Icon {
					name = GLYPH[t.kind],
					size = GLYPH_SIZE,
					absolute = true,
					left = GLYPH_LEFT,
					top = (ROW - GLYPH_SIZE) / 2,
					color = tint,
				}
				else ui.div {
					absolute = true,
					left = GLYPH_LEFT,
					top = (ROW - GLYPH_SIZE) / 2,
					color = tint,
					ui.show(running, status.StatusIcon { kind = "working", size = GLYPH_SIZE }, Icon {
						name = GLYPH[t.kind],
						size = GLYPH_SIZE,
					}),
				},
			ui.row {
				pl = ui.raw(GUTTER, "past the tree's gutter"),
				grow = 1,
				min_w = 0,
				gap = "$space.1",
				items = "center",
				ui.text {
					verb,
					literal = true,
					shrink = 0,
					text_size = "$font.size.sm",
					color = "$text.secondary",
				},
				-- (Its line as the tool goes on: a command's input may still
				-- be arriving as it starts.)
				if t.target
					then target_line(now)
					else ui.show(
					function(): boolean
						return now().target ~= nil
					end,
					FadeText {
						function(): string
							return line_of(now())
						end,
						literal = true,
						shrink = 1,
						font = "$font.mono",
						text_size = "$font.size.sm",
						color = "$text.primary",
					}
				),
				-- An edit's lines, added and removed, in their hues.
				if COUNTS[t.kind] or t.added ~= nil
					then ui.show(
					function(): boolean
						return now().added ~= nil
					end,
					ui.text {
						function(): string
							return `+{now().added or 0}`
						end,
						shrink = 0,
						tabular = true,
						font = "$font.mono",
						text_size = "$font.size.xs",
						color = "$success",
					}
				)
					else nil,
				if COUNTS[t.kind] or t.removed ~= nil
					then ui.show(
					function(): boolean
						local r = now().removed
						return r ~= nil and r > 0
					end,
					ui.text {
						function(): string
							return `−{now().removed or 0}`
						end,
						shrink = 0,
						tabular = true,
						font = "$font.mono",
						text_size = "$font.size.xs",
						color = "$danger",
					}
				)
					else nil,
			},
		},
		-- What it opens to, made only once it's opened (a component's body
		-- runs as it mounts: closed, nothing of it is made).
		ui.show(showing, Opened { now = now }),
	}
end

return ui.component(function(props: Props): Node
	local live = props.live or function()
		return false
	end
	-- What the reader chose (nil until they do): open while it works, folded
	-- once it's done, unless they said otherwise.
	-- Kept under `key` when given, so one opened stays open as it is made
	-- again (a reload, its row scrolled back).
	local own = ui.use_state(nil :: boolean?)
	local key = props.key
	local function choice(): boolean?
		return if key then GROUPS_CHOSEN:get()[key] else own:get()
	end
	local function choose(on: boolean)
		if key then
			local all = table.clone(GROUPS_CHOSEN:peek())
			all[key] = on
			GROUPS_CHOSEN:set(all)
		else
			own:set(on)
		end
	end
	local function open(): boolean
		local c = choice()
		if c ~= nil then
			return c
		end
		return live()
	end
	local opened = ui.computed(open)
	local tools = ui.computed(props.tools)
	local count = ui.computed(function(): number
		return #tools:get()
	end)
	-- Where the last tool with a branch (any but a thought) is: the trunk
	-- runs on below every row before it. (Looked for from the end: it is
	-- the last tool, or a thought or two before it.)
	local last_branch = ui.computed(function(): number
		local list = tools:get()
		for i = #list, 1, -1 do
			if list[i].kind ~= "thought" then
				return i
			end
		end
		return 0
	end)
	local title = ui.computed(function(): string
		local given = if props.summary then props.summary() else nil
		local s = given or summary(tools:get())
		local secs = if props.worked then props.worked() else nil
		if secs and not live() then
			return locale.s("ds.activity.worked_for", { secs = tostring(secs), summary = s })
		end
		return s
	end)
	-- The rows exist while the tree is open or folding: folded away, they'd
	-- be built for nothing as a list scrolls, and a selection across the
	-- reply would take text no one can see.
	local mounted = ui.use_state(opened:peek())
	local folding: (() -> ())? = nil
	ui.effect(function()
		local on = opened:get()
		if folding then
			folding()
			folding = nil
		end
		if on then
			mounted:set(true)
		elseif mounted:peek() then
			folding = ui.after(FOLD, function()
				folding = nil
				mounted:set(false)
			end)
		end
	end)
	-- The rows: a list virtualized by the view around it (the transcript's),
	-- each call a row of its own, built only as the view shows it, the rows
	-- about to come into it built ahead in the time frames leave over
	-- (zgx's `ahead`), so a scroll or the tree opening builds what shows
	-- alone. A row follows its tool in place (its item as a state), and
	-- holds its height in `heights`: a call's line is ROW, one opened or a
	-- thought its own, as it was last laid out.
	local own_key = key or tostring(own)
	local heights = ui.use_state({} :: { number })
	local listed: { string } = {}
	-- Calls arriving while the rows are made: each plays `ENTER` as it is
	-- first built, as a row joining a `ui.each` does.
	local arriving: { [string]: boolean } = {}
	local was_mounted = false
	-- A row's height as it is first laid out: a call's line, ROW, or as it
	-- was last laid out if opened; a thought, as it was if it's short or
	-- opened, else its four lines folded before it knows it's long (its
	-- button's room comes a frame on, as the words are measured).
	local line = ui.theme.get("zeron.prose.small.line") :: number
	local first_folded = THOUGHT_LINES * line + (ROW - line)
	local function height_of(t: Tool): number
		local h = MEASURED[`{own_key}:{t.id}`]
		local open = CALLS_OPEN:peek()[t.id] == true
		if t.kind ~= "thought" then
			return if open and h then h else ROW
		end
		if h and (open or h <= first_folded) then
			return h
		end
		return first_folded
	end
	ui.effect(function()
		local list = tools:get()
		local m = mounted:get()
		local n = #list
		-- (Rows that were there before: the same calls in the same places,
		-- looked at at the ends; anything else is written whole.)
		local same = #listed <= n
			and (#listed == 0 or (list[1].id == listed[1] and list[#listed].id == listed[#listed]))
		ui.batch(function()
			if not same then
				heights:set({})
				table.clear(listed)
			end
			for i = #listed + 1, n do
				local t = list[i]
				listed[i] = t.id
				if m and was_mounted then
					arriving[t.id] = true
				end
				heights:push(height_of(t))
			end
		end)
		was_mounted = m
	end)
	local function row_view(t: Tool, index: State<number>, item: State<Tool>): Node
		local id = t.id
		local function place(): number
			return index:get()
		end
		-- Its height, as laid out, for the rows after it.
		local function resized(size: { w: number, h: number })
			local k = `{own_key}:{id}`
			if MEASURED[k] ~= size.h then
				kept_set(MEASURED, k, size.h)
			end
			local i = index:peek()
			local held = heights:peek()[i]
			if held ~= nil and held ~= size.h then
				heights:set_key(i, size.h)
			end
		end
		local function made(): Node
			return row(function(): Tool
				return item:get()
			end, function(): boolean
				return place() < last_branch:get()
			end, function(): boolean
				return live() and place() == count:get()
			end, props.open_subagent, props.thought, props.key, resized, place() < count:peek())
		end
		-- (A call arriving joins as a row joining a `ui.each` does; the rest
		-- are made as they come into view, as they were.)
		if arriving[id] then
			arriving[id] = nil
			return ui.presence(function(): string
				return "row"
			end, made, { enter = ENTER, initial = true, w_full = true })
		end
		return made()
	end

	return ui.column {
		w_full = true,
		ui.show(
			function()
				return count:get() > 0
			end,
			ui.button {
				label = title,
				expanded = opened,
				-- (A control, not what was said: out of a selection across the reply.)
				selectable = false,
				h = HEADER,
				p = 0,
				bg = "#00000000",
				active = { bg = "#00000000" },
				w_full = true,
				items = "center",
				gap = "$space.2",
				cursor = "pointer",
				color = "$text.secondary",
				hover = { color = "$text.primary", bg = "#00000000" },
				focus = { border = 0 },
				on_click = function()
					choose(not open())
				end,
				-- The press is the toggle's from press to release (zgx keeps
				-- the pointer for a node with both), so the reply's
				-- selection around it never starts, extends or picks a word
				-- from it, however the fold moves the words under the
				-- pointer.
				on_press = function() end,
				on_release = function() end,
				-- While it works, the working loader before the summary, its
				-- ring over the trunk below; done, the summary alone, on the
				-- reply's left edge.
				ui.show(live, status.StatusIcon { kind = "working", size = LOADER }),
				-- (Its box fills the header: its width easing would show nothing,
				-- and lay the header out every frame of a roll.)
				FadeText { title, roll = true, glide = false, grow = 1, text_size = "$font.size.sm" },
			}
		),
		-- The tree: its rows, each arriving under the last.
		ui.column {
			w_full = true,
			overflow = "hidden",
			layout = "height",
			layout_clip = true,
			layout_duration = FOLD,
			layout_ease = "outQuart",
			max_h = function(): number
				return if opened:get() then 1e6 else 0
			end,
			if props.body
				then ui.presence(function(): string?
					return if mounted:get() then "body" else nil
				end, function(): Node
					return (props.body :: () -> Node)()
				end, { w_full = true })
				else ui.show(
				mounted,
				ui.list {
					w_full = true,
					virtualize = "ancestor",
					data = tools,
					key = "id",
					heights = heights,
					row = row_view,
				}
			),
		},
	}
end)
