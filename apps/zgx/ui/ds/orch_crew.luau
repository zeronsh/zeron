--!strict
-- An orchestrator's crew: a card a thread, so where each one stands reads
-- at a glance. Each says how it stands (its status dot), its name and
-- role, the machine it runs on and its agent, what it's doing now (the
-- activity's glyph and words, and for how long), how far through its todo
-- (a thin bar in the orchestrator's hue, "5/8" and the step under way),
-- and the last thing it reported (a chip for how that stood, its one line,
-- when). Those that need you come first, then those working, then the rest,
-- the latest moved first. A card opens its thread.
--
--   OrchCrew {
--     members = function() return { { member = CrewRow, chat = ChatRow? } } end,
--     hue = function() return 196 end,
--     device_name = function(device_id) return "desk" end,
--     on_open = function(chat_id) end,
--     now = function() return os.time() end,
--     columns = 2,             -- else one under 520 wide, two above
--   }
--
-- A card is made once a thread and follows it: its activity, todo, status
-- or report moving reaches only what shows them.

local Icon = require("@self/icon")
local Mark = require("@self/mark")
local Badge = require("@self/badge")
local status = require("@self/status")
local control = require("@self/control")
local locale = require("@self/locale")
local motion = require("@self/motion")
local material = require("@self/material")
local data = require("@self/orch_data")
local hue = require("@self/orch_hue")

export type Member = { member: { [string]: any }, chat: { [string]: any }? }
export type Props = {
	members: control.Live<{ Member }>,
	hue: control.Live<number?>?,
	device_name: ((device_id: string) -> string?)?,
	on_open: ((chat_id: string) -> ())?,
	now: (() -> number)?,
	columns: number?,
}

-- Two cards a row from this wide.
local TWO_AT = 520
-- A card's corner and inset (one value: rule 6), and the gap between cards.
local RADIUS = "$radius.lg"
local INSET = "$space.3"
local GAP = 8 -- ($space.2)

-- How a thread stands, as its dot says it: needs you (asks or approves),
-- under way, failed, done, or idle.
local UNDER_WAY: { [string]: boolean } = { working = true, connecting = true, waiting = true }
local NEEDS: { [string]: boolean } = { input = true, approval = true }
local DOT: { [string]: boolean } = {
	working = true,
	connecting = true,
	waiting = true,
	input = true,
	approval = true,
	done = true,
	failed = true,
	limited = true,
}

local function rank(m: Member): number
	local st = m.chat and m.chat.status
	if st and NEEDS[st] then
		return 0
	elseif st and UNDER_WAY[st] then
		return 1
	end
	return 2
end

-- A report's chip: its words and tone.
local CHIP: { [string]: Badge.Tone } = {
	done = "success",
	blocked = "warning",
	failed = "danger",
	progress = "neutral",
	message = "neutral",
}

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

-- A model's id said short: "claude-opus-5-5" → "opus 5.5", "gpt-5.5" kept.
local function short_model(model: string?): string
	if not model or model == "" then
		return ""
	end
	local fam, ver = string.match(model, "^claude%-(%a+)%-([%d%-]+)$")
	if fam then
		return `{fam} {(string.gsub(ver :: string, "%-", "."))}`
	end
	return model
end

local function idle_dot(size: number): Node
	return ui.div { w = size, h = size, shrink = 0, rounded = "$radius.full", border = 1, border_color = "$text.tertiary" }
end

-- A thin bar of how far along, in the hue, easing as it moves.
local function bar(value: () -> number?, ink: () -> any): Node
	local track_w = ui.use_state(0)
	return ui.div {
		w_full = true,
		h = 3,
		relative = true,
		rounded = "$radius.full",
		bg = "$zeron.slider.track",
		overflow = "hidden",
		on_resize = function(size: { w: number, h: number })
			if math.abs(size.w - track_w:peek()) >= 0.5 then
				track_w:set(size.w)
			end
		end,
		ui.div {
			absolute = true,
			left = 0,
			top = 0,
			h = 3,
			rounded = "$radius.full",
			bg = ink,
			w = motion.tween(ui.computed(function(): number
				return math.clamp(value() or 0, 0, 1) * track_w:get()
			end), { duration = 320, ease = "out(3)" }),
		},
	}
end

local function small(text: () -> string, props: { [string]: any }?): Node
	local p: { [any]: any } = {
		text,
		truncate = true,
		min_w = 0,
		text_size = "$font.size.xs",
		color = "$text.tertiary",
	}
	for k, v in props or {} do
		p[k] = v
	end
	return ui.text(p :: any)
end

return ui.component(function(props: Props): Node
	local width = ui.use_state(0)
	local function now_s(): number
		return if props.now then props.now() else os.time()
	end
	local function ink(): any
		local h = if props.hue then control.value(props.hue) :: number? else nil
		return if h then hue.paint(h) else "$text.secondary"
	end
	local shown = ui.computed(function(): { Member }
		local out: { Member } = {}
		for _, m in (control.value(props.members) :: { Member }?) or {} do
			if not m.member.released then
				table.insert(out, m)
			end
		end
		table.sort(out, function(a: Member, b: Member): boolean
			local ra, rb = rank(a), rank(b)
			if ra ~= rb then
				return ra < rb
			end
			local ua = (a.chat and a.chat.updated) or a.member.joined or 0
			local ub = (b.chat and b.chat.updated) or b.member.joined or 0
			if ua ~= ub then
				return ua > ub
			end
			return a.member.chat_id < b.member.chat_id
		end)
		return out
	end)
	local function columns(): number
		if props.columns then
			return props.columns
		end
		return if width:get() >= TWO_AT then 2 else 1
	end
	local function card_w(): number
		local n = columns()
		local w = width:get()
		return math.max(0, math.floor((w - GAP * (n - 1)) / n))
	end

	local function card(first: Member, _i: number, row: State<Member>): Node
		local id = first.member.chat_id
		local hovered = ui.use_state(false)
		local function chat(): { [string]: any }
			return row:get().chat or {}
		end
		local function member(): { [string]: any }
			return row:get().member
		end
		local function title(): string
			local t = chat().title
			return if t and t ~= "" then t else locale.s("ds.orch.crew.thread")
		end
		local function dot_kind(): string
			local st = chat().status
			return if st and DOT[st] then st else "idle"
		end
		local function activity(): { [string]: any }?
			local a = chat().activity
			local st = chat().status
			if type(a) ~= "table" or not (st and UNDER_WAY[st]) then
				return nil
			end
			return a
		end
		local function todo(): { [string]: any }?
			local t = chat().todo
			return if data.todo_fraction(t) ~= nil then t else nil
		end
		local function report(): { [string]: any }?
			return member().last_report
		end
		local function where(): string
			local parts = {}
			local role = member().role
			if role and role ~= "" then
				table.insert(parts, role)
			end
			local dev = chat().device
			local name = if dev and props.device_name then props.device_name(dev) else nil
			if name and name ~= "" then
				table.insert(parts, name)
			end
			local model = short_model(chat().model)
			if model ~= "" then
				table.insert(parts, model)
			end
			return table.concat(parts, " · ")
		end
		local dots: { [string]: Node } = { idle = idle_dot(8) }
		for k in DOT do
			dots[k] = status.StatusDot { kind = k :: any, size = 8 }
		end

		-- Its head: the dot, the name, its agent's mark.
		local head = ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			ui.div { w = 8, h = 8, shrink = 0, ui.switch(dot_kind, dots :: any) },
			ui.text {
				title,
				class = "orch-crew-title",
				grow = 1,
				min_w = 0,
				truncate = true,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.primary",
			},
			ui.presence(function(): string
				return MARKS[chat().agent or ""] or ""
			end, function(name: string): Node
				if name == "" then
					return ui.div {}
				end
				return ui.div { shrink = 0, color = "$text.secondary", Mark { name = name, size = 12, bare = true } }
			end, { shrink = 0 }),
		}
		-- Its role, machine and model, quiet.
		local sub = ui.show(function(): boolean
			return where() ~= ""
		end, small(where, { class = "orch-crew-where", pl = ui.raw(16, "under the name, past the dot") }))
		-- What it's doing now: the glyph, the words, for how long.
		local doing = ui.show(
			function(): boolean
				return activity() ~= nil
			end,
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				class = "orch-crew-activity",
				ui.presence(function(): string
					local a = activity()
					return (a and (data.ACTIVITY :: { [string]: string })[a.kind]) or "terminal"
				end, function(glyph: string): Node
					return ui.div { color = "$text.secondary", Icon { name = glyph :: any, size = 12 } }
				end, { shrink = 0 }),
				ui.text {
					function(): string
						local a = activity()
						if not a then
							return ""
						end
						if a.label and a.label ~= "" then
							return a.label
						end
						return locale.s(`ds.orch.activity.{a.kind}`)
					end,
					class = "orch-crew-activity-label",
					grow = 1,
					min_w = 0,
					truncate = true,
					font = "$font.mono",
					text_size = "$font.size.xs",
					color = "$text.secondary",
				},
				small(function(): string
					local a = activity()
					if not a or type(a.since) ~= "number" then
						return ""
					end
					return status.elapsed(math.max(0, now_s() - a.since))
				end, { shrink = 0, tabular = true, truncate = false }),
			}
		)
		-- How far through its todo.
		local progress = ui.show(
			function(): boolean
				return todo() ~= nil
			end,
			ui.column {
				w_full = true,
				gap = "$space.1",
				class = "orch-crew-todo",
				bar(function(): number?
					return data.todo_fraction(todo())
				end, ink),
				ui.row {
					w_full = true,
					gap = "$space.2",
					items = "center",
					small(function(): string
						local t = todo()
						return if t then locale.s("ds.orch.crew.todo", { done = t.done or 0, total = t.total or 0 }) else ""
					end, { shrink = 0, tabular = true, truncate = false, color = "$text.secondary" }),
					small(function(): string
						local t = todo()
						return (t and t.current) or ""
					end, { grow = 1 }),
				},
			}
		)
		-- The last thing it reported.
		local said = ui.show(
			function(): boolean
				return report() ~= nil
			end,
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				class = "orch-crew-report",
				ui.presence(function(): string
					local r = report()
					return (r and r.state) or ""
				end, function(state: string): Node
					return Badge.Badge {
						locale.s(`ds.orch.crew.report.{if CHIP[state] then state else "progress"}`),
						tone = CHIP[state] or "neutral",
						size = "sm",
						class = "orch-crew-report-state",
					}
				end, { shrink = 0 }),
				ui.text {
					function(): string
						local r = report()
						return (r and r.summary) or ""
					end,
					grow = 1,
					min_w = 0,
					truncate = true,
					text_size = "$font.size.xs",
					color = "$text.secondary",
				},
				small(function(): string
					local r = report()
					return if r and type(r.at) == "number" then status.ago(math.max(0, now_s() - r.at)) else ""
				end, { shrink = 0, tabular = true, truncate = false }),
			}
		)
		return ui.button {
			class = "orch-crew-card",
			provide = { session = id },
			label = function(): string
				return locale.s("ds.orch.crew.open", { title = title() })
			end,
			w = card_w,
			shrink = 0,
			direction = "column",
			p = INSET,
			gap = "$space.2",
			items = "stretch",
			rounded = RADIUS,
			bg = function(): string
				if hovered:get() then
					return "$hover"
				end
				return material.pick("$zeron.settings.note", "$zeron.settings.card")()
			end,
			hover = {},
			active = { bg = "$pressed" },
			on_hover = function(inside: boolean)
				hovered:set(inside)
			end,
			on_click = function()
				if props.on_open then
					props.on_open(id)
				end
			end,
			ui.column { w_full = true, gap = "$space.1", head, sub },
			doing,
			progress,
			said,
		}
	end

	return ui.column {
		class = "orch-crew",
		w_full = true,
		on_resize = function(size: { w: number, h: number })
			if math.abs(size.w - width:peek()) >= 0.5 then
				width:set(size.w)
			end
		end,
		ui.show(
			function(): boolean
				return #shown:get() == 0
			end,
			ui.text {
				locale.t("ds.orch.crew.empty"),
				class = "orch-crew-empty",
				text_size = "$font.size.sm",
				color = "$text.tertiary",
			}
		),
		ui.each(shown, card, {
			key = function(m: Member): string
				return m.member.chat_id
			end,
			direction = "row",
			flex_wrap = true,
			w_full = true,
			gap = "$space.2",
			items = "start",
		}),
	}
end)
