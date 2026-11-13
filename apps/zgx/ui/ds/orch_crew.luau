--!strict
-- An orchestrator's crew: a unit a thread, so where each one stands reads
-- at a glance. Its head: its agent's mark on a tile, its name, its role,
-- machine and model, and how far through its todo ("5/8") at the end.
-- Under it, in Zeron's own words for it: working (the roller and its
-- time), or how else it stands (needs you, failed, done); what it's doing
-- now (the activity's glyph and command); its todo as segmented cells with
-- the step under way; the last thing it reported (a badge for how that
-- stood, its one line, when). A status shows at least 300 ms
-- (ds/orch_hold). Those that need you come first, then those working, then
-- the rest, the latest moved first; a hairline between. A unit opens its
-- thread.
--
--   OrchCrew {
--     members = function() return { { member = CrewRow, chat = ChatRow? } } end,
--     hue = function() return 196 end,
--     device_name = function(device_id) return "desk" end,
--     on_open = function(chat_id) end,
--     now = function() return os.time() end,
--     columns = 2,             -- side by side (one by default)
--   }
--
-- A unit is made once a thread and follows it: its activity, todo, status
-- or report moving reaches only what shows them.

local Icon = require("@self/icon")
local Mark = require("@self/mark")
local Badge = require("@self/badge")
local Working = require("@self/working")
local status = require("@self/status")
local control = require("@self/control")
local locale = require("@self/locale")
local data = require("@self/orch_data")
local Cells = require("@self/orch_cells")
local hold = require("@self/orch_hold")

export type Member = { member: { [string]: any }, chat: { [string]: any }? }
export type Props = {
	members: control.Live<{ Member }>,
	hue: control.Live<number?>?,
	device_name: ((device_id: string) -> string?)?,
	on_open: ((chat_id: string) -> ())?,
	now: (() -> number)?,
	columns: number?,
}

-- The gap between units side by side.
local GAP = 8 -- ($space.2)
-- The tile's size, and where the lines under the head start (past it).
local TILE = 28
local INDENT = 36

-- How a thread stands: needs you (asks or approves), under way, or not.
local UNDER_WAY: { [string]: boolean } = { working = true, connecting = true, waiting = true }
local NEEDS: { [string]: boolean } = { input = true, approval = true }

local function rank(m: Member): number
	local st = m.chat and m.chat.status
	if st and NEEDS[st] then
		return 0
	elseif st and UNDER_WAY[st] then
		return 1
	end
	return 2
end

-- A report's badge: its tone.
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
		return math.max(1, props.columns or 1)
	end
	local function card_w(): number
		local n = columns()
		local w = width:get()
		return math.max(0, math.floor((w - GAP * (n - 1)) / n))
	end

	local function card(first: Member, _i: number, row: State<Member>): Node
		local id = first.member.chat_id
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
		-- How it stands, held 300 ms at least.
		local kind = hold.value(function(): string
			local st = chat().status
			if st and UNDER_WAY[st] then
				return "working"
			end
			return if st and status.KINDS[st] then st else "idle"
		end)
		local activity = hold.value(function(): { [string]: any }?
			local a = chat().activity
			local st = chat().status
			if type(a) ~= "table" or not (st and UNDER_WAY[st]) then
				return nil
			end
			return a
		end)
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
		local function since(): number
			local a = activity()
			return (a and type(a.since) == "number" and a.since) or chat().updated or now_s()
		end

		-- Its head: the agent's tile, the name over where it runs, its todo's
		-- count at the end.
		local head = ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			ui.div {
				w = TILE,
				h = TILE,
				shrink = 0,
				rounded = "$radius.md",
				bg = "$hover",
				items = "center",
				justify = "center",
				color = "$text.secondary",
				ui.presence(function(): string
					return MARKS[chat().agent or ""] or ""
				end, function(name: string): Node
					if name == "" then
						return Icon { name = "chat-round-line", size = 14 }
					end
					return Mark { name = name, size = 15, bare = true }
				end),
			},
			ui.column {
				grow = 1,
				min_w = 0,
				gap = "$zeron.space.hair",
				ui.row {
					w_full = true,
					gap = "$space.2",
					items = "center",
					ui.text {
						title,
						class = "orch-crew-title",
						min_w = 0,
						truncate = true,
						text_size = "$font.size.sm",
						weight = "$font.weight.medium",
						color = "$text.primary",
					},
				},
				ui.show(function(): boolean
					return where() ~= ""
				end, small(where, { class = "orch-crew-where" })),
			},
			ui.show(function(): boolean
				return todo() ~= nil
			end, ui.text {
				function(): string
					local t = todo()
					return if t then locale.s("ds.orch.crew.todo", { done = t.done or 0, total = t.total or 0 }) else ""
				end,
				class = "orch-crew-count",
				shrink = 0,
				tabular = true,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.primary",
			}),
		}

		-- How it stands: working, the roller and its time; else its state in
		-- its hue (nothing, idle).
		local state = ui.switch(function(): string
			local k = kind()
			if k == "working" then
				return "working"
			end
			return if k == "idle" then "idle" else "other"
		end, {
			working = Working { since = since, seed = #id, size = "sm" },
			other = ui.row {
				h = 20,
				gap = "$zeron.space.tight",
				items = "center",
				status.StatusIcon { kind = kind :: any, size = 14 },
				ui.text {
					function(): string
						local k = kind()
						local info = status.KINDS[k :: any]
						return if info then info.long else ""
					end,
					text_size = "$font.size.sm",
					color = function(): string
						return `$zeron.status.{kind()}`
					end,
				},
			},
			idle = ui.div {},
		})

		-- What it's doing now: the glyph and the command.
		local doing = ui.show(function(): boolean
			return activity() ~= nil
		end, ui.row {
			w_full = true,
			gap = "$space.1",
			items = "center",
			class = "orch-crew-activity",
			ui.div {
				color = "$text.tertiary",
				shrink = 0,
				Icon {
					name = function(): Icon.Name
						local a = activity()
						return ((a and (data.ACTIVITY :: { [string]: string })[a.kind]) or "terminal") :: any
					end,
					size = 12,
				},
			},
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
		})

		-- How far through its todo: a cell a step, and the one under way.
		local progress = ui.show(function(): boolean
			return todo() ~= nil
		end, ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			class = "orch-crew-todo",
			Cells {
				done = function(): number
					local t = todo()
					return if t then t.done or 0 else 0
				end,
				total = function(): number
					local t = todo()
					return if t then t.total or 0 else 0
				end,
				current = function(): boolean
					return kind() == "working"
				end,
				cell = 5,
				h = 10,
			},
			small(function(): string
				local t = todo()
				return (t and t.current) or ""
			end, { grow = 1 }),
		})

		-- The last thing it reported.
		local said = ui.show(function(): boolean
			return report() ~= nil
		end, ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			class = "orch-crew-report",
			ui.presence(function(): string
				local r = report()
				return (r and r.state) or ""
			end, function(st: string): Node
				return Badge.Badge {
					locale.s(`ds.orch.crew.report.{if CHIP[st] then st else "progress"}`),
					tone = CHIP[st] or "neutral",
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
		})

		return ui.button {
			class = "orch-crew-card",
			provide = { session = id },
			label = function(): string
				return locale.s("ds.orch.crew.open", { title = title() })
			end,
			w = card_w,
			shrink = 0,
			direction = "column",
			p = "$space.2",
			gap = "$space.2",
			items = "stretch",
			rounded = "$radius.lg",
			bg = "#00000000",
			hover = { bg = "$hover" },
			active = { bg = "$pressed" },
			on_click = function()
				if props.on_open then
					props.on_open(id)
				end
			end,
			head,
			ui.column {
				w_full = true,
				pl = ui.raw(INDENT, "under the name, past the tile"),
				gap = "$zeron.space.tight",
				state,
				doing,
				progress,
				said,
			},
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
		ui.each(shown, function(m: Member, i: number, row: State<Member>): Node
			return ui.column {
				w = card_w,
				shrink = 0,
				-- (A hairline over each but the first, one column.)
				ui.show(function(): boolean
					local top = shown:get()[1]
					return columns() == 1 and top ~= nil and top.member.chat_id ~= row:get().member.chat_id
				end, ui.div { w_full = true, h = 1, my = "$space.1", bg = "$border.subtle" }),
				card(m, i, row),
			}
		end, {
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
