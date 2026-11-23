--!strict
-- A card of the board (ds/orch_board): whose it is (its orchestrator's
-- dot) and its title; two quiet lines saying what it is doing now (its
-- thread's live note) or what it did (its last report), never written by
-- hand; and its foot: its status' glyph and words with a clock that counts
-- from when the status began, then how many threads it has and at most
-- one chip (its merge, its pull request, its report). A card the engine
-- moved since the person last opened it carries a dot. A click opens it
-- (its detail, beside the board).
--
-- Its status is the engine's (the card's row); nothing here decides one.
-- The pieces are shared with the list view's rows and the detail:
--
--   Card.Card { ctx, card = id, drawer? } the card
--   Card.Glyph { status = fn, size? }     a status' glyph in its ink
--   Card.words(entry)                     its status' words
--   Card.tail(ctx, entry)                 what follows them (a clock, a time)
--   Card.summary(ctx, entry)              its two lines
--   Card.Chip { chip = fn, on_link? }     its one chip
--   Card.Threads { count = fn }           how many threads

local Icon = require("@self/icon")
local Status = require("@self/status")
local hue = require("@self/orch_hue")
local locale = require("@self/locale")
local material = require("@self/material")
local Clocks = require("@self/orch_progress_clocks")
local ContextMenu = require("@self/context_menu")
local Ctx = require("@self/orch_board_ctx")
local Ref = require("@self/orch_ref")
local trace = require("@self/orch_perf_trace")

local M = {}

M.RADIUS = "$radius.lg"

type Look = { icon: Icon.Name?, ink: string }

-- Each status' glyph (none: the working ring) and ink.
local LOOK: { [string]: Look } = {
	asked = { icon = "danger-triangle", ink = "$zeron.tone.warning.ink" },
	failed = { icon = "status-failed", ink = "$zeron.status.failed" },
	review = { icon = "eye", ink = "$zeron.status.input" },
	stalled = { icon = "danger-triangle", ink = "$zeron.tone.warning.ink" },
	working = { ink = "$zeron.status.working" },
	waiting = { ink = "$zeron.status.working" },
	checking = { ink = "$zeron.status.working" },
	limited = { icon = "status-limited", ink = "$zeron.status.limited" },
	queued = { icon = "clock-circle", ink = "$text.tertiary" },
	next = { icon = "clock-circle", ink = "$text.tertiary" },
	after = { icon = "clock-circle", ink = "$text.tertiary" },
	held = { icon = "archive-minimalistic", ink = "$text.tertiary" },
	paused = { icon = "action-pause", ink = "$text.tertiary" },
	done = { icon = "status-done", ink = "$zeron.status.done" },
	dropped = { icon = "close-circle", ink = "$text.tertiary" },
}
local OTHER: Look = { icon = "clock-circle", ink = "$text.tertiary" }

function M.look(status: string): Look
	return LOOK[status] or OTHER
end

-- A status' glyph in its ink: the working ring while an agent is on it.
function M.Glyph(props: { status: () -> string, size: number? }): Node
	local size = props.size or 14
	local function drawn(): string
		return if M.look(props.status()).icon == nil then "ring" else "icon"
	end
	return ui.div {
		class = "orch-status-glyph",
		shrink = 0,
		w = size,
		h = size,
		items = "center",
		justify = "center",
		color = function(): string
			return M.look(props.status()).ink
		end,
		ui.switch(drawn, {
			ring = Status.Spinner { size = size },
			icon = Icon {
				name = function(): Icon.Name
					return M.look(props.status()).icon or "clock-circle"
				end,
				size = size,
			},
		}),
	}
end

-- Markdown's marks left out, its lines one run.
local function plain(md: string): string
	local s = string.gsub(md, "%*%*", "")
	s = string.gsub(s, "`", "")
	s = string.gsub(s, "%[([^%]]*)%]%b()", "%1")
	s = string.gsub(s, "%s+", " ")
	return s
end
M.plain = plain

-- Two lines need no more than this many characters: the rest is cut
-- before it's laid out.
local SUMMARY_CHARS = 240
local function clipped(s: string): string
	if #s <= SUMMARY_CHARS then
		return s
	end
	local cut = utf8.offset(s, SUMMARY_CHARS) or SUMMARY_CHARS
	return string.sub(s, 1, cut - 1) .. "…"
end

-- A time on a card: as every time the person reads (ds/locale `when`).
function M.time(at: number, now: number): string
	return locale.when(at, now)
end

-- A status' words, as its card says them.
function M.words(e: Ctx.Entry): string
	local s = e.status
	local note: string? = e.card and e.card.note
	if note == "" then
		note = nil
	end
	if s == "waiting" then
		return if note then locale.s("orch.board.status.waiting_on", { what = note }) else locale.s("orch.board.status.waiting")
	elseif s == "after" then
		return if note then locale.s("orch.board.status.after_named", { what = note }) else locale.s("orch.board.status.after")
	elseif s == "working" and note then
		return note
	elseif s == "queued" then
		return locale.s(if e.place == "running" then "orch.board.status.starting" else "orch.board.status.queued")
	elseif s == "done" then
		local from = e.item.created
		if e.closed_at and from and e.closed_at > from then
			return locale.s("orch.board.status.done_in", { time = Ctx.age(e.closed_at - from, locale) })
		end
		return locale.s("orch.board.status.done")
	elseif LOOK[s] then
		return locale.s(`orch.board.status.{s}`)
	end
	return ""
end

-- What follows the words, quieter: the clock of a status still under way,
-- when a wait ends, or the engine's note.
-- (The clock is read only where one shows: a time to the minute, a count
-- to the second while it shows seconds.)
function M.tail(ctx: Ctx.Ctx, e: Ctx.Entry): string
	local s = e.status
	if e.place == "snoozed" then
		return if e.until_at then locale.s("orch.board.back_at", { time = M.time(e.until_at, ctx.clock(0)) }) else ""
	elseif s == "limited" then
		return if e.until_at then locale.s("orch.board.until", { time = M.time(e.until_at, ctx.clock(0)) }) else ""
	elseif e.place == "needs_you" or e.place == "running" or s == "paused" then
		return Ctx.age(ctx.clock(e.status_since) - e.status_since, locale)
	elseif s == "next" then
		return (e.card and e.card.note) or e.item.waiting or ""
	end
	return ""
end

-- Its two lines: the question it waits on; what its thread is doing now;
-- else what it last reported, or how it ended; before any of that, what it
-- was asked to do.
function M.summary(ctx: Ctx.Ctx, e: Ctx.Entry): string
	if e.place == "needs_you" and e.ask and (e.ask.question or "") ~= "" then
		return clipped(plain(e.ask.question))
	end
	if e.place == "running" then
		local lead = (e.item.threads or {})[1]
		local row = ctx.progress(lead)
		local note = row and row.note
		if type(note) == "string" and note ~= "" then
			return clipped(plain(note))
		end
		local doing = Clocks.doing(row, ctx.thread(lead))
		if doing then
			return clipped(plain(doing))
		end
	end
	local word = e.card and e.card.word
	if word and (word.summary or "") ~= "" then
		return clipped(plain(word.summary))
	end
	local closed = e.card and e.card.closed
	if closed and (closed.note or "") ~= "" then
		return clipped(plain(closed.note))
	end
	local text = e.item.outcome
	if not text or text == "" then
		text = e.item.detail
	end
	return if text and text ~= "" then clipped(plain(text)) else ""
end

-- A chip's glyph, by its kind.
local CHIP_ICON: { [string]: Icon.Name } = {
	merged = "git-merge",
	pr = "pull-request",
	report = "document",
	accepted = "check",
	closed = "check",
	dropped = "close",
}

-- Its one chip: how it finished, or its pull request.
function M.Chip(props: { chip: () -> any, on_link: ((link: any) -> ())? }): Node
	local function chip(): any
		return props.chip()
	end
	return ui.show(function(): boolean
		return chip() ~= nil
	end, ui.row {
		class = "orch-card-chip",
		shrink = 0,
		max_w = 160,
		h = 18,
		px = "$space.1",
		gap = "$zeron.space.tight",
		items = "center",
		rounded = "$radius.sm",
		border = 1,
		border_color = "$border",
		color = "$text.tertiary",
		ui.div {
			shrink = 0,
			Icon {
				name = function(): Icon.Name
					local c = chip()
					return (c and CHIP_ICON[c.kind]) or "tag"
				end,
				size = 11,
			},
		},
		ui.text {
			function(): string
				local c = chip()
				return if c then c.label else ""
			end,
			min_w = 0,
			truncate = true,
			tabular = true,
			text_size = "$font.size.xs",
			color = "$text.secondary",
		},
	})
end

-- How many threads it has, where it has any.
function M.Threads(props: { count: () -> number }): Node
	return ui.show(function(): boolean
		return props.count() > 0
	end, ui.row {
		class = "orch-card-threads",
		shrink = 0,
		gap = "$zeron.space.tight",
		items = "center",
		color = "$text.tertiary",
		Icon { name = "chat-round-line", size = 12 },
		ui.text {
			function(): string
				return tostring(props.count())
			end,
			tabular = true,
			text_size = "$font.size.xs",
		},
	})
end

-- The dot of a card the engine moved since the person last opened it.
function M.Unseen(props: { shown: () -> boolean }): Node
	return ui.show(props.shown, ui.div {
		class = "orch-card-unseen",
		label = locale.t("orch.board.unseen"),
		shrink = 0,
		w = 7,
		h = 7,
		rounded = "$radius.full",
		bg = "$accent",
	})
end

-- A card as a reference (ds/orch_ref): its state, its first thread, its
-- orchestrator, the project it's in (its threads', else its
-- orchestrator's; left out where it's the orchestrator's own name).
function M.ref(ctx: Ctx.Ctx, e: Ctx.Entry): any
	local it = e.item
	local threads = it.threads or {}
	local of: any = ctx.props.project_of
	local function project_of(chat: string?): string?
		if not chat or not of then
			return nil
		end
		local name = of(chat)
		return if name and name ~= "" then name else nil
	end
	local project: string? = nil
	for _, chat in threads do
		project = project_of(chat)
		if project then
			break
		end
	end
	if not project then
		project = project_of(it.orchestrator_id)
	end
	local o = ctx.owner(it.orchestrator_id)
	if o and project and string.lower(project) == string.lower(o.name or "") then
		project = nil
	end
	return Ref.make("card", it.id, it.title, {
		state = it.state,
		thread = threads[1],
		orchestrator = it.orchestrator_id,
		project = project,
	})
end

-- Whose a card is, said: its orchestrator's dot, named.
function M.owner_name(ctx: Ctx.Ctx, e: Ctx.Entry?): string
	local o = e and ctx.owner(e.item.orchestrator_id)
	return if o then o.name or "" else ""
end

export type Props = {
	ctx: Ctx.Ctx,
	-- Its card's id (not `id`: a component's `id` names its element).
	card: string,
	-- In a drawer: quieter, its summary left out.
	drawer: boolean?,
	-- What stands under its foot (a paused card's Resume).
	extra: ((entry: () -> Ctx.Entry?) -> Node?)?,
	menu: ((entry: Ctx.Entry) -> { any }?)?,
}

M.Card = ui.component(function(props: Props): Node
	local ctx = props.ctx
	local id = props.card
	trace.hit("board.card")
	local entry = ui.computed(function(): Ctx.Entry?
		trace.hit("board.card_entry")
		return ctx.entry(id)
	end)
	local function e(): Ctx.Entry?
		return entry:get()
	end
	local status = ui.computed(function(): string
		local x = e()
		return if x then x.status else "next"
	end)
	local title = ui.computed(function(): string
		local x = e()
		return if x then x.item.title or "" else ""
	end)
	local summary = ui.computed(function(): string
		local x = e()
		if not x or props.drawer then
			return ""
		end
		return M.summary(ctx, x)
	end)
	local words = ui.computed(function(): string
		local x = e()
		trace.hit("board.word")
		return if x then M.words(x) else ""
	end)
	local tail = ui.computed(function(): string
		local x = e()
		trace.hit("board.tail")
		return if x then M.tail(ctx, x) else ""
	end)
	local owner_hue = ui.computed(function(): number?
		local x = e()
		local o = x and ctx.owner(x.item.orchestrator_id)
		return o and o.hue
	end)
	local selected = ui.computed(function(): boolean
		return ctx.selected() == id
	end)
	local hovered = ui.use_state(false)
	local rest = material.pick("$zeron.settings.note", "$zeron.settings.card")
	-- How it comes in (ds/orch_board_motion), decided as it's built.
	local m = ctx.motion
	local built = m.build(id, props.drawer == true)
	local start, start_content = m.start(built)
	local content_ref = ui.ref()
	local mounted_el: any = nil
	ui.on_cleanup(function()
		if mounted_el then
			m.detach(id, mounted_el)
		end
	end)
	local quiet = props.drawer == true

	local head = ui.row {
		w_full = true,
		min_w = 0,
		gap = "$space.2",
		items = "start",
		if ctx.owners
			then ui.div {
				class = "orch-card-owner",
				role = "img",
				label = function(): string
					return M.owner_name(ctx, e())
				end,
				shrink = 0,
				h = (ui.theme.get("font.line.sm") :: number?) or 16,
				items = "center",
				justify = "center",
				hue.Dot {
					hue = function(): number?
						return owner_hue:get()
					end,
					size = 7,
				},
			}
			else nil,
		ui.text {
			function(): string
				return title:get()
			end,
			class = "orch-card-title",
			grow = 1,
			min_w = 0,
			wrap = true,
			lines = 2,
			truncate = true,
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			weight = "$font.weight.medium",
			color = if quiet then "$text.secondary" else "$text.primary",
		},
		ui.div {
			shrink = 0,
			h = (ui.theme.get("font.line.sm") :: number?) or 16,
			items = "center",
			justify = "center",
			M.Unseen {
				shown = function(): boolean
					local x = e()
					return x ~= nil and x.card ~= nil and x.card.unseen == true and x.place == "done"
				end,
			},
		},
	}
	local foot = ui.row {
		class = "orch-card-foot",
		w_full = true,
		min_w = 0,
		gap = "$space.1",
		items = "center",
		M.Glyph {
			status = function(): string
				return status:get()
			end,
			size = 13,
		},
		ui.text {
			function(): string
				return words:get()
			end,
			class = "orch-card-status",
			shrink = 1,
			min_w = 0,
			truncate = true,
			text_size = "$font.size.xs",
			weight = "$font.weight.medium",
			color = "$text.secondary",
		},
		ui.show(function(): boolean
			return tail:get() ~= ""
		end, ui.text {
			function(): string
				return tail:get()
			end,
			class = "orch-card-clock",
			shrink = 2,
			min_w = 0,
			truncate = true,
			tabular = true,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		}),
		ui.div { grow = 1, min_w = 0 },
		M.Threads {
			count = function(): number
				local x = e()
				return if x then #(x.item.threads or {}) else 0
			end,
		},
		M.Chip {
			chip = function(): any
				local x = e()
				return x and x.card and x.card.chip
			end,
		},
	}
	local extra = if props.extra then props.extra(e) else nil
	-- Dragged: a reference to it (ds/orch_ref; Capture's composer takes one).
	-- It moves nothing: where a card stands is the engine's.
	local box = ui.button(Ref.on({
		class = "orch-card",
		id = `orch-card-{id}`,
		selected = function(): boolean
			return selected:get()
		end,
		label = function(): string
			return title:get()
		end,
		w_full = true,
		p = 0,
		items = "start",
		rounded = M.RADIUS,
		bg = function(): string
			return if hovered:get() or selected:get() then "$hover" else rest()
		end,
		hover = {},
		active = { bg = "$pressed" },
		border = 1,
		border_color = function(): string
			return if selected:get() then "$focus.ring" else "#00000000"
		end,
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		on_click = function()
			ctx.open(id)
		end,
		ui.column {
			ref = content_ref,
			w_full = true,
			min_w = 0,
			opacity = start_content,
			p = if ctx.compact then "$space.2" else ui.raw(10, "a card's even inset"),
			ui.column {
			w_full = true,
			min_w = 0,
			gap = if ctx.compact then "$space.1" else "$zeron.space.snug",
			head,
			ui.show(function(): boolean
				return summary:get() ~= ""
			end, ui.text {
				function(): string
					return summary:get()
				end,
				class = "orch-card-summary",
				w_full = true,
				min_w = 0,
				wrap = true,
				lines = 2,
				truncate = true,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = "$text.secondary",
			}),
			foot,
			extra,
			},
			-- Where plugins decorate it (the github plugin: its checks).
			-- (Outside the parts' gaps: empty, it takes no room.)
			ui.div { hook = "orchestration.item", class = "orch-item-slot", provide = { item = id }, w_full = true },
		},
	}, function(): any
		local x = entry:peek()
		return if x then M.ref(ctx, x) else nil
	end) :: any)
	local whole: Node = box
	if props.menu then
		whole = (ContextMenu {
			label = locale.t("orch.board.card_actions"),
			ring = M.RADIUS,
			w_full = true,
			shrink = 0,
			items = function(): { any }?
				local x = entry:peek()
				trace.hit("board.menu")
				return if x then (props.menu :: any)(x) else nil
			end,
			box,
		}) :: Node
	end
	-- How it moves (ds/orch_board_motion): the node that dims and travels,
	-- inside the row its list slides (so a slide and a hand-over never
	-- add up on one node).
	return ui.div {
		w_full = true,
		min_w = 0,
		-- (Coming in by a move: over its new neighbours while it travels.)
		z = if built.how ~= nil then 1 else nil,
		ui.div {
			class = "orch-card-motion",
			layout_id = built.layout_id,
			layout_duration = built.duration,
			layout_ease = "outQuint",
			w_full = true,
			min_w = 0,
			opacity = start,
			on_mount = function(el: any)
				mounted_el = el
				m.attach(id, el, { content = content_ref })
			end,
			whole,
		},
	}
end)

return M
