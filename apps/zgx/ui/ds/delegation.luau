-- Work an agent handed to other chats, and what came of it, on the activity
-- tree (ds/activity): a quiet line ("Delegated to 2 subagents · 1 running"),
-- then a row a chat hanging off it. A subagent an agent spawned itself is a
-- row on that tree too, so the two read as one thing wherever they ran.
--
-- A row's mark is where a tool's glyph is, and says one thing at a time: how
-- its chat stands while that's worth knowing (the working ring, what it asks
-- you, that it failed), else whose it is (its agent's mark). It settles in
-- its place: nothing joins, leaves or moves as a chat works, asks and ends.
-- After its name, the word its state needs, then, fainter, what it runs on
-- or a line of what it said, cut where the row ends.
--
--   Delegation { key = id, said = …, note = …, rows = tasks, on_open = … }

local Icon = require("@self/icon")
local FadeText = require("@self/fade_text")
local Mark = require("@self/mark")
local control = require("@self/control")
local locale = require("@self/locale")
local motion = require("@self/motion")
local status = require("@self/status")
local tree_line = require("@self/tree_line")

local M = {}

-- How a row's chat stands. "stopped": ended early, by someone; "gone": no
-- longer listed, so there's nothing to say of it.
export type Standing = "working" | "connecting" | "waiting" | "input" | "approval" | "failed" | "stopped" | "done" | "gone"
export type Row = {
	-- The same row as it goes on.
	id: string,
	title: string,
	agent: Mark.Name?,
	state: Standing,
	-- What it runs on ("Codex · Build box"), or a line of what it said.
	detail: string?,
	-- Its state's word left out: the line over it says as much.
	wordless: boolean?,
	-- Its chat, once there's one to open.
	chat: string?,
}
export type Props = {
	-- Keeps whether the reader folded it, as it's made again.
	key: string,
	-- What it is, as a plugin's selector finds it.
	hook: string?,
	class: string?,
	said: () -> string,
	-- What's still to come of it, after `said` ("1 running"); nil, nothing.
	note: (() -> string?)?,
	rows: () -> { Row },
	on_open: ((chat: string, title: string) -> ())?,
}

local T = tree_line.TREE
local ROW, HEADER, GLYPH = T.row, T.header, T.glyph
-- Where a row's press starts: past the branch's tip, the glyph's air inside.
local PRESS = T.tip
local FOLDED = ui.state("delegation_folded", {} :: { [string]: boolean })
local FOLD = 200
local ENTER: any = { opacity = { 0, 1 }, y = { -6, 0 }, duration = 360, ease = "outExpo" }

-- The word a state needs after a name, and its ink. (Working, done: the mark
-- says it.)
local WORDS: { [string]: { key: string, ink: string } } = {
	approval = { key = "ds.transcript.delegated.needs_approval", ink = "$zeron.status.approval" },
	input = { key = "ds.transcript.delegated.asks", ink = "$zeron.status.input" },
	failed = { key = "ds.transcript.delegated.failed", ink = "$zeron.status.failed" },
	stopped = { key = "ds.transcript.delegated.stopped", ink = "$text.tertiary" },
}

-- What a row's mark shows: its state's glyph, or its agent.
local function marked(r: Row): string
	local s = r.state
	if s == "working" or s == "waiting" or s == "connecting" then
		return "working"
	elseif s == "input" or s == "approval" or s == "failed" then
		return s
	elseif s == "stopped" then
		return "stopped"
	end
	return `by:{r.agent or ""}`
end

-- One settles over the other, in one box: nothing beside it moves.
local function mark(now: () -> Row): Node
	return ui.div {
		relative = true,
		w = GLYPH,
		h = GLYPH,
		shrink = 0,
		ui.presence(function(): string
			return marked(now())
		end, function(kind: string): Node
			local inside: Node
			if kind == "stopped" then
				inside = ui.div { color = "$text.tertiary", Icon { name = "close-circle", size = GLYPH } }
			elseif string.sub(kind, 1, 3) == "by:" then
				local name = string.sub(kind, 4)
				inside = ui.div {
					color = "$text.secondary",
					if name ~= "" then Mark { name = name :: any, size = GLYPH, bare = true } else Icon { name = "side-chat", size = GLYPH },
				}
			else
				inside = status.StatusIcon { kind = kind :: any, size = GLYPH }
			end
			return ui.div { absolute = true, left = 0, top = 0, w = GLYPH, h = GLYPH, items = "center", justify = "center", inside }
		end, {
			enter = motion.transition { opacity = { 0, 1 }, scale = { 0.7, 1 }, duration = motion.BASE, ease = "outCubic" },
			exit = motion.transition { opacity = 0, scale = 0.7, duration = motion.FADE, ease = "outCubic" },
		}),
	}
end

local function row(now: () -> Row, below: () -> boolean, on_open: ((chat: string, title: string) -> ())?): Node
	local r = now()
	local opens = r.chat ~= nil and on_open ~= nil
	local inside: { [any]: any } = {
		ml = ui.raw(PRESS, "past the tree's branch"),
		grow = 1,
		min_w = 0,
		h = ROW,
		pl = ui.raw(T.glyph_left - PRESS, "its mark where a tool's glyph is"),
		pr = "$space.2",
		gap = "$space.2",
		items = "center",
		rounded = "$radius.md",
		bg = "#00000000",
		overflow = "hidden",
		selectable = false,
		mark(now),
		FadeText {
			function(): string
				return now().title
			end,
			literal = true,
			shrink = 1,
			min_w = 48,
			text_size = "$font.size.sm",
			color = function(): string
				local s = now().state
				return if s == "stopped" or s == "gone" then "$text.secondary" else "$text.primary"
			end,
		},
		ui.show(
			function(): boolean
				return WORDS[now().state] ~= nil and not now().wordless
			end,
			ui.text {
				function(): string
					local word = WORDS[now().state]
					return if word then locale.s(word.key) else ""
				end,
				shrink = 0,
				text_size = "$font.size.sm",
				color = function(): string
					local word = WORDS[now().state]
					return if word then word.ink else "$text.tertiary"
				end,
			}
		),
		-- (From nothing, not its own width: a long line of it never cuts the
		-- name. The room it keeps is its own all the same.)
		ui.show(
			function(): boolean
				return now().detail ~= nil
			end,
			FadeText {
				function(): string
					return now().detail or ""
				end,
				literal = true,
				grow = 1,
				basis = 0,
				min_w = ui.raw(64, "a word or two of it, beside the longest name"),
				text_size = "$font.size.sm",
				color = "$text.tertiary",
			}
		),
	}
	if opens then
		inside.label = function(): string
			return locale.s("ds.transcript.delegated.open_chat", { title = now().title })
		end
		inside.hover = { bg = "$hover" }
		inside.active = { bg = "$pressed" }
		inside.focus = control.ring
		inside.cursor = "pointer"
		inside.on_click = function()
			local n = now()
			if n.chat and on_open then
				on_open(n.chat, n.title)
			end
		end
	end
	return ui.row {
		w_full = true,
		relative = true,
		h = ROW,
		items = "center",
		ui.canvas {
			absolute = true,
			left = 0,
			top = 0,
			bottom = 0,
			w = T.tip + 1,
			pointer_events = "none",
			draw = function(c, _, h)
				c:fill(tree_line.branch { x = T.trunk, mid = ROW / 2, tip = T.tip, h = h, bend = T.bend, on = below() }, "$zeron.tree")
			end,
		},
		if opens then ui.button(inside) else ui.row(inside),
	}
end

M.Delegation = ui.component(function(props: Props): Node
	local key = props.key
	local rows = ui.computed(props.rows)
	local count = ui.computed(function(): number
		return #rows:get()
	end)
	local open = ui.computed(function(): boolean
		return FOLDED:get()[key] ~= true
	end)
	-- (Folded, its rows are let go once the fold has closed over them.)
	local mounted = ui.use_state(open:peek())
	local folding: (() -> ())? = nil
	ui.effect(function()
		local on = open:get()
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
	local function line(): string
		local note = if props.note then props.note() else nil
		return if note then `{props.said()} · {note}` else props.said()
	end
	return ui.column {
		hook = props.hook,
		class = props.class,
		w_full = true,
		ui.button {
			label = line,
			expanded = open,
			selectable = false,
			h = HEADER,
			p = 0,
			w_full = true,
			items = "center",
			bg = "#00000000",
			color = "$text.secondary",
			hover = { color = "$text.primary", bg = "#00000000" },
			active = { bg = "#00000000" },
			focus = { border = 0 },
			cursor = "pointer",
			on_click = function()
				local all = table.clone(FOLDED:peek())
				all[key] = if open:peek() then true else nil
				FOLDED:set(all)
			end,
			on_press = function() end,
			on_release = function() end,
			FadeText { line, roll = true, glide = false, grow = 1, text_size = "$font.size.sm" },
		},
		ui.column {
			w_full = true,
			overflow = "hidden",
			layout = "height",
			layout_clip = true,
			layout_duration = FOLD,
			layout_ease = "outQuart",
			max_h = function(): number
				return if open:get() then 1e6 else 0
			end,
			ui.show(
				mounted,
				ui.column {
					w_full = true,
					ui.each(rows, function(r: Row, i: number): Node
						local function now(): Row
							return rows:get()[i] or r
						end
						return row(now, function(): boolean
							return i < count:get()
						end, props.on_open)
					end, {
						-- (Made again as its chat comes: what opens it is a button.)
						key = function(r: Row): string
							return `{r.id}:{r.chat or ""}`
						end,
						enter = motion.transition(ENTER),
					}),
				}
			),
		},
	}
end)

return M
