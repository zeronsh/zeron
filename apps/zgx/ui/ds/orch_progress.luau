-- How a thread's work is going, read and never asked of it: the progress
-- the engine keeps for each chat (`orchestration.progress`, docs/design/
-- orchestrators.md "Progress") drawn as live telemetry. How long it has
-- been at it, how far through its steps (segmented cells, "3/5"), what it
-- is on now (or the command running, "cargo test · 9m"), how long since its
-- last step, the time left when the engine knows it, and a one-line note. Stalled, it says so in the warning's ink, not
-- an alarm's. "What's up?" asks the host for a fresh note (never the
-- agent); while it's written, a spinner (still after SPIN_MAX seconds). The clocks are plain words to the
-- minute ("<1m", "9m", "1h 12m"), in tabular figures, and move once a
-- minute, on the minute: nothing ticks by the second.
--
--   Progress {
--     variant = "full",            -- "full" (a widget's unit), "strip" (one
--                                  -- line, in a row or a card), "chip" (tiny)
--     row = fn -> Row?,            -- orchestration.progress[chat]
--     chat = fn -> Chat?,          -- its session row (title, agent, activity)
--     title = fn -> string?,       -- else the chat's
--     context = fn -> string?,     -- "Orchestrators · desk"
--     now = fn -> number?,         -- Unix seconds (a test's clock); else it
--                                  -- ticks itself
--     lean = true,                 -- strip: the clocks and the note alone
--                                  -- (where the row around already shows
--                                  -- the status, steps and activity)
--     on_check = fn(), checking = fn -> bool, cooling = fn -> bool,
--     on_open = fn(), pinned = fn -> bool, on_pin = fn(on: bool),
--   }
--
-- The clocks (the contract's): elapsed = now − started (stopped at
-- `updated` once the stretch is over), since the last step = now −
-- last_step, time left = eta_secs − (now − updated), shown only while
-- working with an ETA and some left. A status or activity shows 300 ms at
-- least (ds/orch_hold). A row with `asking` set (its thread asked the
-- person and waits on the answer) reads "Waiting on you". A thread its
-- usage limit stopped (its session's `limited`, ds/usage_limit) reads
-- "Waiting for limit · resets 14:05" in the limited state, its time left
-- "resets in 12m" (moving with the clocks, by the minute); one running on
-- another account meanwhile, "On me@home.dev · me@work.dev resets 14:05".

local Icon = require("@self/icon")
local Mark = require("@self/mark")
local Badge = require("@self/badge")
local Button = require("@self/button")
local IconButton = require("@self/icon_button").IconButton
local ContextMenu = require("@self/context_menu")
local status = require("@self/status")
local locale = require("@self/locale")
local Cells = require("@self/orch_cells")
local hold = require("@self/orch_hold")
local Tooltip = require("@self/tooltip")
local pure = require("@self/orch_progress_clocks")
local trace = require("@self/orch_perf_trace")
local limit = require("@self/usage_limit")

local M = {}

export type Row = { [string]: any }
export type Variant = "full" | "strip" | "chip"
export type Props = {
	variant: Variant?,
	row: () -> Row?,
	chat: (() -> { [string]: any }?)?,
	title: (() -> string?)?,
	context: (() -> string?)?,
	now: (() -> number)?,
	lean: boolean?,
	-- strip: as wide as what it says (a line on its own), not the width given
	hug: boolean?,
	-- Said quietly where its note would be, while it has none ("No
	-- details yet").
	placeholder: (() -> string?)?,
	on_check: (() -> ())?,
	checking: (() -> boolean)?,
	cooling: (() -> boolean)?,
	on_open: (() -> ())?,
	pinned: (() -> boolean)?,
	on_pin: ((on: boolean) -> ())?,
	id: string?,
	grow: number?,
	min_w: number?,
}

M.ACTIVE = pure.ACTIVE
M.kind = pure.kind
M.stalled = pure.stalled
M.clocks = pure.clocks
M.doing = pure.doing
M.step = pure.step
type Clocks = pure.Clocks

-- The tile, its size, and where the lines under a full unit's head start.
local TILE = 28
local INDENT = 40
-- How wide a lean strip must be to say its note (else its tooltip does).
M.LEAN_NOTE = 300
-- How long (seconds) a note being written rolls before its mark goes still.
M.SPIN_MAX = 20

-- The agent's mark ("codex" is OpenAI's).
local MARKS: { [string]: string } = {
	claude = "claude",
	openai = "openai",
	codex = "openai",
	cursor = "cursor",
	grok = "grok",
	devin = "devin",
	opencode = "opencode",
	antigravity = "antigravity",
	pi = "pi",
	hermes = "hermes",
	muse = "muse",
}

-- A duration to the minute, in Zeron's short words: "<1m", "9m", "1h 12m"
-- ("3h" on the hour, and from ten hours on).
function M.short(seconds: number): string
	local t = math.max(0, math.floor(seconds))
	if t < 60 then
		return locale.s("ds.orch.progress.under_minute")
	elseif t < 3600 then
		return locale.s("time.short.minutes", { n = t // 60 })
	end
	local m = t // 60
	if m % 60 == 0 or m >= 600 then
		return locale.s("time.short.hours", { n = m // 60 })
	end
	return locale.s("time.short.hours_minutes", { h = m // 60, m = m % 60 })
end

local function steps_of(r: Row?): { done: number, total: number }?
	local s = r and r.steps
	if type(s) ~= "table" or type(s.total) ~= "number" or s.total <= 0 then
		return nil
	end
	return s
end

local function small(text: any, extra: { [string]: any }?): Node
	local p: { [any]: any } = {
		text,
		tabular = true,
		shrink = 0,
		text_size = "$font.size.xs",
		color = "$text.tertiary",
	}
	for k, v in extra or {} do
		p[k] = v
	end
	return ui.text(p :: any)
end

M.Progress = ui.component(function(props: Props): Node
	local variant: Variant = props.variant or "full"
	-- The clocks move by the minute, on the minute: the moment they're read
	-- at is kept, and taken again only once a new minute has begun (one
	-- timer a unit, asleep till then; none while a test's clock drives it,
	-- whose changes within a minute are let pass).
	local at = ui.use_state(if props.now then props.now() else os.time())
	if props.now then
		local given: () -> number = props.now
		ui.effect(function()
			local t = given()
			if pure.minute(t) ~= pure.minute(at:peek()) then
				at:set(t)
			end
		end)
	else
		ui.effect(function()
			-- (Each turn of the minute sets the next: owned by this run, gone
			-- with the unit.)
			at:get()
			ui.after(pure.until_minute(os.time()) * 1000, function()
				at:set(math.max(os.time(), at:peek() + 1))
			end)
		end)
	end
	local function now(): number
		return at:get()
	end
	-- What it reads of the live sources, each through a selector of the
	-- unit's own: a write anywhere (another thread's row or session, the
	-- rows pushed whole) runs these again, and nothing it shows unless its
	-- own value changed.
	local the_row = ui.computed(function(): Row?
		trace.hit("progress.row")
		return props.row()
	end)
	local function row(): Row?
		return the_row:get()
	end
	-- (Of its session, what it shows: its name, its agent, its activity.)
	local the_chat = ui.computed(function(): any
		local c = if props.chat then props.chat() else nil
		if not c then
			return nil
		end
		return { title = c.title, agent = c.agent, activity = c.activity, limited = c.limited }
	end)
	local function chat(): { [string]: any }?
		return the_chat:get()
	end
	local function title(): string
		local t = if props.title then props.title() else nil
		if not t or t == "" then
			local c = chat()
			t = c and c.title
		end
		return if t and t ~= "" then t else locale.s("ds.orch.crew.thread")
	end
	local the_context = ui.computed(function(): string
		return (if props.context then props.context() else nil) or ""
	end)
	local function context(): string
		return the_context:get()
	end
	local function clocks(): Clocks
		trace.hit("progress.clocks")
		return M.clocks(row(), now())
	end
	-- Its usage limit, if one stopped it.
	local function lim(): limit.Limit?
		return limit.of(chat())
	end
	local function limit_words(): string?
		local l = lim()
		return if l then limit.text(l) else nil
	end
	-- How it stands, and what it's on, held 300 ms at least.
	local kind = hold.value(function(): string
		if limit.waiting(chat()) then
			return "limited"
		end
		return M.kind(row())
	end)
	local stalled = hold.value(function(): boolean
		return M.stalled(row())
	end)
	local doing = hold.value(function(): string?
		return M.doing(row(), chat())
	end)
	local function steps(): { done: number, total: number }?
		return steps_of(row())
	end
	-- It asked the person and waits on the answer (an open ask of its).
	local function waiting_on_you(): boolean
		local r = row()
		return r ~= nil and r.asking == true
	end
	local function note(): string
		local r = row()
		return (r and r.note) or ""
	end
	-- Its note, or what's said in its place.
	local function said_text(): string
		local n = note()
		if n ~= "" then
			return n
		end
		return (if props.placeholder then props.placeholder() else nil) or ""
	end
	local is_refreshing = ui.computed(function(): boolean
		local r = row()
		return (r ~= nil and r.refreshing == true) or (props.checking ~= nil and props.checking())
	end)
	local function refreshing(): boolean
		return is_refreshing:get()
	end
	-- A fresh note that takes longer than SPIN_MAX (a host that never
	-- writes it back): the roller stops, a still mark in its place.
	local spin_over = ui.use_state(false)
	ui.effect(function()
		if is_refreshing:get() then
			spin_over:set(false)
			-- (Made in this run: gone when it stops refreshing.)
			ui.after(M.SPIN_MAX * 1000, function()
				spin_over:set(true)
			end)
		end
	end)
	-- Its "being written" mark: the roller (which plays only while it
	-- shows), still once it has rolled SPIN_MAX.
	local function refresh_mark(): Node
		return ui.presence(function(): string
			return if spin_over:get() then "still" else "spin"
		end, function(k: string): Node
			if k == "still" then
				return ui.div { class = "orch-progress-refresh-still", shrink = 0, color = "$text.tertiary", Icon { name = "refresh", size = 10 } }
			end
			return status.Spinner { size = 10 }
		end, { shrink = 0 })
	end
	local function pinned(): boolean
		return props.pinned ~= nil and props.pinned()
	end

	-- What its time since the last step says, and in what ink (stalled: the
	-- warning's).
	local function since_text(): string
		local s = clocks().since_step
		if s == nil then
			return ""
		end
		return locale.s("ds.orch.progress.last_step", { ago = M.short(s) })
	end
	local function since_ink(): string
		return if stalled() then "$zeron.tone.warning.ink" else "$text.tertiary"
	end
	-- The steps' figure, and the time left after it.
	local function steps_text(): string
		local s = steps()
		return if s then locale.s("ds.orch.progress.steps", { done = s.done or 0, total = s.total }) else ""
	end
	local function left_text(): string
		local lm = lim()
		if lm then
			return limit.left(lm, now())
		end
		local l = clocks().left
		return if l then locale.s("ds.orch.progress.left", { t = M.short(math.max(60, l)) }) else ""
	end
	-- What it says it's on: the command or activity working, else its
	-- state's name.
	local function doing_text(): string
		local d = doing()
		local w = limit_words()
		if w then
			return w
		elseif d then
			return d
		elseif waiting_on_you() then
			return locale.s("ds.orch.progress.waiting_on_you")
		end
		local k = kind()
		if k == "working" then
			return M.step(row()) or locale.s("ds.orch.activity.other")
		end
		local info = status.KINDS[k :: any]
		return if info then info.long else ""
	end
	local function tool_text(): string
		local t = clocks().tool
		return if t and doing() and not lim() then M.short(t) else ""
	end

	-- Its menu: what's up, open, pin.
	local menu_open = ui.use_state(false)
	local function entries(): { any }
		local out: { any } = {}
		-- (A full unit has its own button for it.)
		if props.on_check and variant ~= "full" then
			table.insert(out, {
				label = locale.s("ds.orch.progress.whats_up"),
				icon = "refresh",
				disabled = props.cooling ~= nil and props.cooling(),
				on_select = props.on_check,
			})
		end
		if props.on_open then
			table.insert(out, { label = locale.s("ds.orch.progress.open"), icon = "chat-round-line", on_select = props.on_open })
		end
		if props.on_pin then
			if #out > 0 then
				table.insert(out, { separator = true })
			end
			local on = pinned()
			table.insert(out, {
				label = locale.s(if on then "ds.orch.progress.unpin" else "ds.orch.progress.pin"),
				icon = "pin",
				on_select = function()
					(props.on_pin :: any)(not on)
				end,
			})
		end
		return out
	end
	local function menu(): Node?
		if not (props.on_check or props.on_open or props.on_pin) then
			return nil
		end
		return ContextMenu {
			items = entries,
			open = menu_open,
			label = locale.t("ds.orch.progress.menu"),
			shrink = 0,
			IconButton {
				class = "orch-progress-menu",
				icon = "more-horizontal",
				label = locale.t("ds.orch.progress.menu"),
				size = "sm",
				tone = "quiet",
				lit = menu_open,
				on_click = function()
					menu_open:set(true)
				end,
			},
		}
	end

	-- Its state's glyph, or the warning's when stalled.
	local function glyph(size: number): Node
		return ui.switch(function(): string
			return if stalled() then "stalled" else "state"
		end, {
			stalled = ui.div {
				class = "orch-progress-stalled-glyph",
				shrink = 0,
				color = "$zeron.tone.warning.ink",
				Icon { name = "clock-circle", size = size },
			},
			state = ui.show(function(): boolean
				return kind() ~= "idle"
			end, status.StatusIcon { kind = kind :: any, size = size }),
		})
	end

	local function note_line(props2: { [string]: any }): Node
		local p: { [any]: any } = {
			class = "orch-progress-note",
			w_full = true,
			min_w = 0,
			gap = "$space.1",
			items = "start",
			-- A fresh one being written: the roller, the old one quieter.
			ui.show(refreshing, ui.div {
				class = "orch-progress-refreshing",
				h = props2.line or 16,
				items = "center",
				shrink = 0,
				label = locale.t("ds.orch.progress.refreshing"),
				refresh_mark(),
			}),
			ui.text {
				said_text,
				class = "orch-progress-note-text",
				literal = true,
				grow = 1,
				min_w = 0,
				wrap = props2.wrap,
				truncate = not props2.wrap,
				max_h = props2.max_h,
				overflow = if props2.wrap then "hidden" else nil,
				text_size = props2.text_size or "$font.size.sm",
				line_height = props2.line_height,
				color = function(): string
					return if refreshing() or note() == "" then "$text.tertiary" else "$text.secondary"
				end,
			},
		}
		return ui.show(function(): boolean
			return said_text() ~= "" or refreshing()
		end, ui.row(p :: any))
	end

	local label = function(): string
		return locale.s("ds.orch.progress.of", { title = title() })
	end

	if variant == "chip" then
		-- Tiny: its dot, its time, its steps.
		return ui.row {
			id = props.id,
			class = "orch-progress orch-progress-chip",
			label = label,
			shrink = 0,
			h = 18,
			gap = "$space.1",
			items = "center",
			ui.switch(function(): string
				return if stalled() then "stalled" elseif kind() == "idle" then "none" else "state"
			end, {
				stalled = Badge.Dot { color = "$zeron.tone.warning.ink", size = 6 },
				state = ui.presence(kind, function(k: string): Node
					return status.StatusDot { kind = k :: any, size = 6 }
				end, { shrink = 0 }),
				none = ui.div {},
			}),
			small(function(): string
				local e = clocks().elapsed
				return if e then M.short(e) else ""
			end, { class = "orch-progress-elapsed", color = "$text.secondary" }),
			ui.show(function(): boolean
				return steps() ~= nil
			end, small(steps_text, { class = "orch-progress-steps" })),
		}
	end

	if variant == "strip" then
		local lean = props.lean == true
		-- (Lean and narrow, its note is its tooltip alone: the clocks keep
		-- their room.)
		local width = ui.use_state(0)
		return ui.row {
			id = props.id,
			on_resize = if lean
				then function(size: { w: number, h: number })
					if math.abs(size.w - width:peek()) >= 1 then
						width:set(size.w)
					end
				end
				else nil,
			tooltip = if lean then Tooltip { note } else nil,
			class = if lean then "orch-progress orch-progress-strip orch-progress-lean" else "orch-progress orch-progress-strip",
			label = label,
			-- (Hugging: as wide as what it says, for a line on its own, e.g. a
			-- session's head; else the width it's given, its clocks at the end.)
			w_full = not props.hug,
			grow = props.grow,
			min_w = props.min_w or 0,
			h = 22,
			gap = "$space.2",
			items = "center",
			if lean
				then ui.show(stalled, ui.div {
					class = "orch-progress-stalled-glyph",
					shrink = 0,
					color = "$zeron.tone.warning.ink",
					Icon { name = "clock-circle", size = 12 },
				})
				else glyph(14),
			if lean then nil else ui.show(function(): boolean
				return steps() ~= nil
			end, ui.row {
				shrink = 0,
				gap = "$zeron.space.tight",
				items = "center",
				Cells {
					class = "orch-progress-cells",
					done = function(): number
						local s = steps()
						return if s then s.done or 0 else 0
					end,
					total = function(): number
						local s = steps()
						return if s then s.total else 0
					end,
					current = function(): boolean
						return kind() == "working"
					end,
					cell = 4,
					h = 8,
					max = 16,
				},
				small(steps_text, { class = "orch-progress-steps", color = "$text.secondary" }),
			}),
			-- What it's on (lean: its note, the row around says the rest).
			if lean
				then ui.row {
					grow = 1,
					min_w = 0,
					items = "center",
					ui.show(function(): boolean
						return width:get() >= M.LEAN_NOTE
					end, note_line { text_size = "$font.size.xs" }),
				}
				else ui.row {
					grow = if props.hug then 0 else 1,
					shrink = 1,
					min_w = 0,
					items = "center",
					ui.row {
					min_w = 0,
					shrink = 1,
					gap = "$space.1",
					items = "center",
					tooltip = Tooltip { note },
					ui.text {
						doing_text,
						class = "orch-progress-doing",
						literal = true,
						min_w = 0,
						truncate = true,
						font = function(): string
							return if doing() and not lim() then "$font.mono" else "$font.sans"
						end,
						text_size = "$font.size.xs",
						color = function(): string
							return if lim() then "$zeron.status.limited" else "$text.secondary"
						end,
					},
					ui.show(function(): boolean
						return tool_text() ~= ""
					end, small(function(): string
						return `· {tool_text()}`
					end, { class = "orch-progress-tool-time" })),
					ui.show(refreshing, ui.div {
						class = "orch-progress-refreshing",
						shrink = 0,
						label = locale.t("ds.orch.progress.refreshing"),
						refresh_mark(),
					}),
					},
				},
			ui.show(function(): boolean
				return left_text() ~= ""
			end, small(left_text, { class = "orch-progress-left" })),
			ui.show(function(): boolean
				return since_text() ~= ""
			end, small(since_text, {
				class = "orch-progress-since",
				color = since_ink,
			})),
			ui.show(function(): boolean
				return clocks().elapsed ~= nil
			end, small(function(): string
				local e = clocks().elapsed
				return if e then M.short(e) else ""
			end, {
				class = "orch-progress-elapsed",
				color = "$text.secondary",
			})),
			menu(),
		}
	end

	-- Full: the widget's unit.
	local tile = ui.div {
		w = TILE,
		h = TILE,
		shrink = 0,
		rounded = "$radius.md",
		bg = "$hover",
		items = "center",
		justify = "center",
		color = "$text.secondary",
		ui.presence(function(): string
			local c = chat()
			return MARKS[(c and c.agent) or ""] or ""
		end, function(name: string): Node
			if name == "" then
				return Icon { name = "chat-round-line", size = 14 }
			end
			return Mark { name = name, size = 15, bare = true }
		end),
	}
	local head = ui.row {
		class = "orch-progress-head",
		w_full = true,
		gap = "$space.3",
		items = "start",
		tile,
		ui.column {
			grow = 1,
			min_w = 0,
			gap = "$zeron.space.hair",
			ui.row {
				w_full = true,
				min_w = 0,
				gap = "$space.1",
				items = "center",
				ui.text {
					title,
					class = "orch-progress-title",
					literal = true,
					min_w = 0,
					truncate = true,
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
					color = "$text.primary",
				},
				-- Pinned: a quiet pin after its name.
				ui.show(pinned, ui.div {
					class = "orch-progress-pinned",
					shrink = 0,
					color = "$text.tertiary",
					label = locale.t("ds.orch.progress.pinned"),
					Icon { name = "pin", size = 12 },
				}),
			},
			ui.show(function(): boolean
				return context() ~= ""
			end, ui.text {
				context,
				class = "orch-progress-context",
				literal = true,
				min_w = 0,
				truncate = true,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			}),
		},
		-- Its live figures: the time it has taken; under it, the time left
		-- (or that it's stalled) and since its last step.
		ui.column {
			shrink = 0,
			items = "end",
			gap = "$zeron.space.hair",
			ui.show(function(): boolean
				return clocks().elapsed ~= nil
			end, ui.text {
				function(): string
					local e = clocks().elapsed
					return if e then M.short(e) else ""
				end,
				id = if props.id then `{props.id}-elapsed` else nil,
				class = "orch-progress-elapsed",
				tabular = true,
				shrink = 0,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = function(): string
					local r: any = row() or {}
					return if M.ACTIVE[r.state or ""] then "$text.primary" else "$text.secondary"
				end,
			}),
			ui.row {
				gap = "$space.1",
				items = "center",
				ui.show(stalled, Badge.Badge {
					locale.t("ds.orch.progress.stalled"),
					class = "orch-progress-stalled",
					tone = "warning",
					size = "sm",
				}),
				ui.show(function(): boolean
					return not stalled() and left_text() ~= ""
				end, small(function(): string
					return if since_text() ~= "" then `{left_text()} ·` else left_text()
				end, { class = "orch-progress-left" })),
				ui.show(function(): boolean
					return since_text() ~= ""
				end, small(since_text, { class = "orch-progress-since", color = since_ink })),
			},
		},
	}
	-- What it's on: its state's glyph, its steps as cells, the command (or
	-- the step, or its state's name), how long since its last step.
	local function on_text(): string
		local d = doing()
		local w = limit_words()
		if w then
			return w
		elseif d then
			return d
		elseif waiting_on_you() then
			return locale.s("ds.orch.progress.waiting_on_you")
		end
		if kind() == "working" or kind() == "idle" then
			return M.step(row()) or (if kind() == "working" then locale.s("ds.orch.activity.other") else "")
		end
		local info = status.KINDS[kind() :: any]
		return if info then info.long else ""
	end
	local telemetry = ui.row {
		class = "orch-progress-activity",
		w_full = true,
		h = 24,
		gap = "$space.2",
		items = "center",
		glyph(14),
		ui.show(function(): boolean
			return steps() ~= nil
		end, Cells {
			class = "orch-progress-cells",
			done = function(): number
				local s = steps()
				return if s then s.done or 0 else 0
			end,
			total = function(): number
				local s = steps()
				return if s then s.total else 0
			end,
			current = function(): boolean
				return kind() == "working"
			end,
			cell = 7,
			gap = 2,
			h = 10,
			max = 12,
		}),
		ui.show(function(): boolean
			return steps() ~= nil
		end, small(steps_text, { class = "orch-progress-steps", color = "$text.secondary" })),
		ui.row {
			grow = 1,
			min_w = 0,
			gap = "$space.1",
			items = "center",
			ui.text {
				on_text,
				class = "orch-progress-doing",
				literal = true,
				min_w = 0,
				truncate = true,
				font = function(): string
					return if doing() and not lim() then "$font.mono" else "$font.sans"
				end,
				text_size = "$font.size.xs",
				color = function(): string
					if lim() then
						return "$zeron.status.limited"
					elseif doing() or kind() == "working" or kind() == "idle" then
						return "$text.secondary"
					end
					local k = kind()
					return `$zeron.status.{if k == "waiting" then "working" else k}`
				end,
			},
			ui.show(function(): boolean
				return tool_text() ~= ""
			end, small(function(): string
				return `· {tool_text()}`
			end, { class = "orch-progress-tool-time" })),
		},
		-- What's up? and the menu (open, pin), at its end.
		ui.row {
			class = "orch-progress-actions",
			shrink = 0,
			gap = "$space.1",
			items = "center",
			if props.on_check
				then Button {
					class = "orch-progress-check",
					tone = "ghost",
					size = "xs",
					-- (A fresh note on its way: the note's roller says so.)
					disabled = function(): boolean
						return refreshing() or (props.cooling ~= nil and props.cooling())
					end,
					label = locale.t("ds.orch.progress.whats_up"),
					on_click = props.on_check,
					locale.t("ds.orch.progress.whats_up"),
				}
				else nil,
			menu(),
		},
	}
	local line = (ui.theme.get("font.line.sm") :: number?) or 18
	-- Its note, across.
	local said = note_line { wrap = true, max_h = line * 2, line_height = "$font.line.sm", line = line }
	return ui.column {
		id = props.id,
		class = "orch-progress orch-progress-full",
		label = label,
		w_full = true,
		gap = "$space.2",
		head,
		ui.column {
			class = "orch-progress-body",
			w_full = true,
			pl = ui.raw(INDENT, "under the title, past the tile"),
			gap = "$space.1",
			telemetry,
			said,
		},
	}
end)

return setmetatable(M, {
	__call = function(_: any, props: Props): Node
		return M.Progress(props)
	end,
}) :: any
