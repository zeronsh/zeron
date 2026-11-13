-- How a thread's work is going, read and never asked of it: the progress
-- the engine keeps for each chat (`orchestration.progress`, docs/design/
-- orchestrators.md "Progress") drawn as live telemetry. How long it has
-- been at it (rolling digits), how far through its steps (segmented
-- cells, "3/5"), what it is on now (or the command running, "cargo test ·
-- 9m"), how long since its last step, the time left when the engine knows
-- it, and a one-line note. Stalled, it says so in the warning's ink, not
-- an alarm's. "What's up?" asks the host for a fresh note (never the
-- agent); while it's written, a spinner. The clocks tick here, once a
-- second, in tabular figures that keep their width.
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
-- least (ds/orch_hold).

local Icon = require("@self/icon")
local Mark = require("@self/mark")
local Badge = require("@self/badge")
local Button = require("@self/button")
local IconButton = require("@self/icon_button").IconButton
local ContextMenu = require("@self/context_menu")
local status = require("@self/status")
local locale = require("@self/locale")
local Cells = require("@self/orch_cells")
local Counter = require("@self/orch_counter")
local hold = require("@self/orch_hold")
local Tooltip = require("@self/tooltip")
local clocks = require("@self/orch_progress_clocks")

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

M.ACTIVE = clocks.ACTIVE
M.kind = clocks.kind
M.stalled = clocks.stalled
M.clocks = clocks.clocks
M.clock = clocks.clock
M.doing = clocks.doing
M.step = clocks.step
type Clocks = clocks.Clocks

-- The tile, its size, and where the lines under a full unit's head start.
local TILE = 28
local INDENT = 40
-- How wide a lean strip must be to say its note (else its tooltip does).
M.LEAN_NOTE = 300

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

-- A duration in a few characters: "42s", "9m", "1h 5m".
function M.short(seconds: number): string
	local t = math.max(0, math.floor(seconds))
	if t < 60 then
		return locale.s("time.short.seconds", { n = t })
	elseif t < 3600 then
		return locale.s("time.short.minutes", { n = t // 60 })
	end
	local m = t // 60
	if m % 60 == 0 then
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
	local tick = ui.use_state(os.time())
	if not props.now then
		-- Once a second: the clocks show whole seconds.
		ui.every(1000, function()
			tick:set(os.time())
		end)
	end
	local function now(): number
		if props.now then
			return props.now()
		end
		return tick:get()
	end
	local function row(): Row?
		return props.row()
	end
	local function chat(): { [string]: any }?
		return if props.chat then props.chat() else nil
	end
	local function title(): string
		local t = if props.title then props.title() else nil
		if not t or t == "" then
			local c = chat()
			t = c and c.title
		end
		return if t and t ~= "" then t else locale.s("ds.orch.crew.thread")
	end
	local function context(): string
		return (if props.context then props.context() else nil) or ""
	end
	local function clocks(): Clocks
		return M.clocks(row(), now())
	end
	-- How it stands, and what it's on, held 300 ms at least.
	local kind = hold.value(function(): string
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
	local function note(): string
		local r = row()
		return (r and r.note) or ""
	end
	local function refreshing(): boolean
		local r = row()
		return (r ~= nil and r.refreshing == true) or (props.checking ~= nil and props.checking())
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
		local l = clocks().left
		return if l then locale.s("ds.orch.progress.left", { t = M.short(l) }) else ""
	end
	-- What it says it's on: the command or activity working, else its
	-- state's name.
	local function doing_text(): string
		local d = doing()
		if d then
			return d
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
		return if t and doing() then M.short(t) else ""
	end

	-- Its menu: what's up, open, pin.
	local menu_open = ui.use_state(false)
	local function entries(): { any }
		local out: { any } = {}
		if props.on_check then
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
				status.Spinner { size = 10 },
			}),
			ui.text {
				note,
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
					return if refreshing() then "$text.tertiary" else "$text.secondary"
				end,
			},
		}
		return ui.show(function(): boolean
			return note() ~= ""
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
			w_full = true,
			grow = props.grow,
			min_w = props.min_w or 0,
			h = 22,
			gap = "$space.2",
			items = "center",
			if lean then nil else glyph(14),
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
					grow = 1,
					min_w = 0,
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
							return if doing() then "$font.mono" else "$font.sans"
						end,
						text_size = "$font.size.xs",
						color = "$text.secondary",
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
						status.Spinner { size = 10 },
					}),
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
			end, Counter {
				value = function(): number?
					return clocks().elapsed
				end,
				format = M.clock,
				max = 3599,
				text_size = "$font.size.xs",
				color = "$text.secondary",
				label = locale.t("ds.orch.progress.elapsed") :: any,
			}),
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
			ui.button {
				class = "orch-progress-title",
				label = function(): string
					return locale.s("ds.orch.crew.open", { title = title() })
				end,
				min_w = 0,
				p = 0,
				h = "auto",
				justify = "start",
				bg = "#00000000",
				hover = { bg = "#00000000" },
				active = { bg = "#00000000" },
				on_click = props.on_open,
				ui.text {
					title,
					literal = true,
					min_w = 0,
					truncate = true,
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
					color = "$text.primary",
				},
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
		-- Its live figures: the time it has taken; under it, its steps and
		-- the time left, or that it's stalled.
		ui.column {
			shrink = 0,
			items = "end",
			gap = "$zeron.space.hair",
			ui.show(function(): boolean
				return clocks().elapsed ~= nil
			end, Counter {
				id = if props.id then `{props.id}-elapsed` else nil,
				value = function(): number?
					return clocks().elapsed
				end,
				format = M.clock,
				max = 3599,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = function(): string
					local r: any = row() or {}
					return if M.ACTIVE[r.state or ""] then "$text.primary" else "$text.secondary"
				end :: any,
				label = locale.t("ds.orch.progress.elapsed") :: any,
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
					return not stalled() and steps() ~= nil
				end, small(steps_text, { class = "orch-progress-steps", color = "$text.secondary" })),
				ui.show(function(): boolean
					return not stalled() and left_text() ~= ""
				end, small(function(): string
					return `· {left_text()}`
				end, { class = "orch-progress-left" })),
			},
		},
	}
	local cells = ui.show(function(): boolean
		return steps() ~= nil
	end, ui.row {
		class = "orch-progress-todo",
		w_full = true,
		gap = "$space.2",
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
			cell = 6,
			h = 10,
			max = 24,
		},
		ui.text {
			function(): string
				return M.step(row()) or ""
			end,
			class = "orch-progress-step",
			literal = true,
			grow = 1,
			min_w = 0,
			truncate = true,
			text_size = "$font.size.xs",
			color = "$text.secondary",
		},
	})
	local activity = ui.row {
		class = "orch-progress-activity",
		w_full = true,
		h = 20,
		gap = "$space.1",
		items = "center",
		glyph(14),
		ui.text {
			doing_text,
			class = "orch-progress-doing",
			literal = true,
			min_w = 0,
			truncate = true,
			font = function(): string
				return if doing() then "$font.mono" else "$font.sans"
			end,
			text_size = function(): string
				return if doing() then "$font.size.xs" else "$font.size.sm"
			end,
			color = function(): string
				if doing() then
					return "$text.secondary"
				end
				local k = kind()
				return if k == "idle" then "$text.secondary" else `$zeron.status.{if k == "waiting" then "working" else k}`
			end,
		},
		ui.show(function(): boolean
			return tool_text() ~= ""
		end, small(function(): string
			return `· {tool_text()}`
		end, { class = "orch-progress-tool-time" })),
		ui.div { grow = 1, min_w = "$space.2" },
		ui.show(function(): boolean
			return since_text() ~= ""
		end, small(since_text, { class = "orch-progress-since", color = since_ink })),
	}
	local line = (ui.theme.get("font.line.sm") :: number?) or 18
	local actions = ui.row {
		class = "orch-progress-actions",
		w_full = true,
		gap = "$space.1",
		items = "center",
		if props.on_check
			then Button {
				class = "orch-progress-check",
				tone = "ghost",
				size = "xs",
				busy = refreshing,
				disabled = function(): boolean
					return props.cooling ~= nil and props.cooling()
				end,
				label = locale.t("ds.orch.progress.whats_up"),
				on_click = props.on_check,
				Icon { name = "refresh", size = 12 },
				locale.t("ds.orch.progress.whats_up"),
			}
			else nil,
		if props.on_open
			then Button {
				class = "orch-progress-open",
				tone = "ghost",
				size = "xs",
				label = locale.t("ds.orch.progress.open"),
				on_click = props.on_open,
				locale.t("ds.orch.progress.open"),
			}
			else nil,
		ui.div { grow = 1 },
		if props.on_pin
			then IconButton {
				class = "orch-progress-pin",
				icon = "pin",
				label = function(): string
					return locale.s(if pinned() then "ds.orch.progress.unpin" else "ds.orch.progress.pin")
				end,
				size = "sm",
				tone = "quiet",
				lit = pinned,
				on_click = function()
					(props.on_pin :: any)(not pinned())
				end,
			}
			else nil,
	}
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
			gap = "$zeron.space.snug",
			cells,
			activity,
			note_line { wrap = true, max_h = line * 2, line_height = "$font.line.sm", line = line },
			actions,
		},
	}
end)

return setmetatable(M, {
	__call = function(_: any, props: Props): Node
		return M.Progress(props)
	end,
}) :: any
