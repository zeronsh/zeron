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

export type Kind = "exec" | "read" | "edit" | "write" | "search" | "glob" | "web" | "todo" | "agent" | "thought"
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
}

local GLYPH: { [Kind]: Icon.Name } = {
	exec = "terminal",
	read = "document",
	edit = "pen",
	write = "document-add",
	search = "search",
	glob = "folder-with-files",
	web = "global",
	todo = "checklist",
	agent = "bot",
	thought = "chat-round-line",
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

local function thought_row(now: () -> Tool, below: () -> boolean): Node
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
		return now().call or now().verb
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
	local function lines(): number
		local gaps = 0
		for _ in string.gmatch(words(), "\n%s*\n") do
			gaps += 1
		end
		return math.max(1, math.floor(((body.height or 0) - gaps * THOUGHT_GAP) / line + 0.5))
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
							-- (Parsed incrementally while it can grow.)
							key = if now().live then `thought:{id}` else nil,
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

-- (`below`: a tool with a branch follows, so the trunk runs on under this
-- one; `running`: it is the one in flight, its glyph the working loader.)
local function row(now: () -> Tool, below: () -> boolean, running: () -> boolean, open_subagent: ((sub: any) -> ())?): Node
	if now().kind == "thought" then
		return thought_row(now, below)
	end
	local function tint(): string
		return if now().failed then "$danger" else "$text.secondary"
	end
	local id = now().id
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
	-- (A tool that may come to have a result opens: whether it has one
	-- isn't known as its row is made.)
	local OPENS: { [string]: boolean } = { exec = true, read = true, edit = true, write = true, search = true, glob = true, web = true }
	local opens = has_call() or lane() ~= nil or OPENS[now().kind] == true
	return ui.column {
		w_full = true,
		relative = true,
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
				c:fill(tree_line.branch { x = TRUNK, mid = mid, tip = BRANCH_END, h = h, bend = BEND, on = below() }, "$zeron.tree")
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
			label = function(): string
				local t = now()
				local sub = lane()
				if sub then
					return locale.s("ds.activity.open_subagent", { title = sub.title or locale.s("ds.activity.subagent") })
				end
				return if t.target then `{t.verb} {line_of(t)}` else t.verb
			end,
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
			ui.div {
				absolute = true,
				left = GLYPH_LEFT,
				top = (ROW - GLYPH_SIZE) / 2,
				color = tint,
				ui.show(running, status.StatusIcon { kind = "working", size = GLYPH_SIZE }, Icon {
					name = function(): Icon.Name
						return GLYPH[now().kind]
					end,
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
					function(): string
						return now().verb
					end,
					literal = true,
					shrink = 0,
					text_size = "$font.size.sm",
					color = "$text.secondary",
				},
				ui.show(
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
				ui.show(
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
				),
				ui.show(
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
				),
			},
		},
		ui.show(
			showing,
			ui.column {
				w_full = true,
				ui.show(has_call, call_block(now)),
				ui.show(
					has_detail,
					ui.div {
						ml = ui.raw(GUTTER, "under the line's words, past the tree's gutter"),
						mb = "$space.1",
						ToolDetail.ToolDetail { tool = now :: any },
					}
				),
			}
		),
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
	-- Each tool by its id: a row finds its own at once, not by a walk of
	-- the group.
	local by_id = ui.computed(function(): { [string]: Tool }
		local out: { [string]: Tool } = {}
		for _, x in tools:get() do
			out[x.id] = x
		end
		return out
	end)
	local count = ui.computed(function(): number
		return #tools:get()
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
	-- Each tool's place in the list, to know the last (the one in flight
	-- while the group works).
	local function last_of(t: Tool): () -> boolean
		return function(): boolean
			local list = tools:get()
			return list[#list] ~= nil and list[#list].id == t.id
		end
	end
	-- Whether the trunk runs on below a tool: one with a branch (any but a
	-- thought) comes after it. (A thought draws no branch, so the tree ends
	-- at the last tool call.)
	local function below_of(t: Tool): () -> boolean
		return function(): boolean
			local seen = false
			for _, x in tools:get() do
				if seen and x.kind ~= "thought" then
					return true
				end
				if x.id == t.id then
					seen = true
				end
			end
			return false
		end
	end
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
				FadeText { title, roll = true, grow = 1, text_size = "$font.size.sm" },
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
				ui.each(tools, function(t: Tool)
					local last = last_of(t)
					-- (An animated list's rows have no row state: the tool as
					-- it is now, found by its id.)
					local now = ui.computed(function(): Tool
						return by_id:get()[t.id] or t
					end)
					return row(function(): Tool
						return now:get()
					end, below_of(t), function(): boolean
						return live() and last()
					end, props.open_subagent)
				end, {
					key = function(t: Tool): string
						return t.id
					end,
					enter = { opacity = { 0, 1 }, y = { -6, 0 }, duration = 360, ease = "outExpo" },
				})
			),
		},
	}
end)
