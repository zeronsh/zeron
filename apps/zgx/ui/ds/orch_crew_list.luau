--!strict
-- An orchestrator's crew as its side panel's Crew tab lists it: a thread a
-- row of two lines (how it stands as an icon, its name, how long it has
-- stood so; then its machine, its model and what it is doing now, each
-- after its own icon), in three groups: Working, Waiting on you (an ask
-- open, input or approval wanted, failed) and Idle. Each group folds (Idle
-- folded unless opened, its head saying how many and the latest of them)
-- and shows a few rows, then "n more". A row opens the thread's detail.
--
--   CrewList {
--     members = function() return { { member = CrewRow, chat = ChatRow? } } end,
--     asks = function() return { Ask } end,     -- open asks: who waits on you
--     device_name = function(device_id) return "desk" end,
--     now = function() return os.time() end,
--     open = function(group) return true end,   -- "working" | "waiting" | "idle"
--     on_toggle = function(group, open) end,
--     on_open = function(chat_id) end,
--   }

local Icon = require("@self/icon")
local Mark = require("@self/mark")
local status = require("@self/status")
local locale = require("@self/locale")
local Name = require("@ds/orch_name")
local Fold = require("@ds/orch_fold")
local data = require("@self/orch_data")

export type Member = { member: { [string]: any }, chat: { [string]: any }? }
export type Props = {
	members: () -> { Member },
	asks: (() -> { any })?,
	device_name: ((device_id: string) -> string?)?,
	now: (() -> number)?,
	open: (group: string) -> boolean,
	on_toggle: (group: string, open: boolean) -> (),
	on_open: (chat_id: string) -> (),
}

local M = {}

-- Rows a group shows before "n more".
M.CAP = 5

local UNDER_WAY: { [string]: boolean } = { working = true, connecting = true, waiting = true }
local WANTS: { [string]: boolean } = { input = true, approval = true, plan = true, failed = true, limited = true }

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

-- "claude-opus-5-5" as people say it: "Opus 5.5".
function M.model_name(m: string?): string?
	if not m or m == "" then
		return nil
	end
	local s = string.gsub(m, "^[%w%-]+/", "")
	if string.match(s, "^gpt") then
		return (string.gsub(s, "^gpt", "GPT"))
	end
	s = string.gsub(s, "^claude%-", "")
	s = string.gsub(s, "%-(%d+)%-(%d+)$", " %1.%2")
	s = string.gsub(s, "%-", " ")
	return (string.gsub(s, "^%l", string.upper))
end

-- Which group a thread stands in, given the open asks by thread.
function M.group_of(m: Member, asked: { [string]: any }): string
	local st = m.chat and m.chat.status
	if asked[m.member.chat_id] or (st and WANTS[st]) then
		return "waiting"
	elseif st and UNDER_WAY[st] then
		return "working"
	end
	return "idle"
end

-- The open asks by the thread that asked.
function M.asked_by(asks: { any }?): { [string]: any }
	local out: { [string]: any } = {}
	for _, a in asks or {} do
		if a.from_chat and (a.state == nil or a.state == "open") then
			out[a.from_chat] = out[a.from_chat] or a
		end
	end
	return out
end

-- Since when it has stood as it does: since it asked, while an ask of its
-- is open (`asked`: the open asks by thread); its activity's start while
-- under way; else its last change.
local function since_of(m: Member, now: number, asked: { [string]: any }?): number
	local ask = asked and asked[m.member.chat_id]
	if ask and type(ask.created) == "number" then
		return ask.created
	end
	local c: { [string]: any } = m.chat or {}
	local a = c.activity
	if UNDER_WAY[c.status or ""] and type(a) == "table" and type(a.since) == "number" then
		return a.since
	end
	return c.updated or m.member.joined or now
end
M.since_of = since_of

-- Its status drawn: the kind a status icon takes.
function M.kind_of(m: Member, asked: { [string]: any }): string
	local st = (m.chat and m.chat.status) or ""
	local ask = asked[m.member.chat_id]
	if ask then
		return if ask.kind == "approval" then "approval" else "input"
	elseif UNDER_WAY[st] then
		return "working"
	elseif status.KINDS[st :: any] then
		return st
	end
	return "idle"
end

-- What it's doing now, in a few plain words: its activity, the question
-- it asked, how it stands.
function M.doing(m: Member, asked: { [string]: any }): string
	local c: { [string]: any } = m.chat or {}
	local a = c.activity
	if UNDER_WAY[c.status or ""] and type(a) == "table" then
		if a.label and a.label ~= "" then
			return a.label
		end
		return locale.s(`ds.orch.activity.{a.kind}`)
	end
	local ask = asked[m.member.chat_id]
	if ask and type(ask.question) == "string" then
		return (string.gsub(ask.question, "%*%*", ""))
	end
	local todo = c.todo
	if type(todo) == "table" and todo.current and todo.current ~= "" and UNDER_WAY[c.status or ""] then
		return todo.current
	end
	local r = m.member.last_report
	if type(r) == "table" and r.summary and r.summary ~= "" then
		return r.summary
	end
	local info = status.KINDS[(c.status or "") :: any]
	return if info then info.long else locale.s("ds.orch.crew_list.idle_word")
end

local function sorted(list: { Member }, now: number, asked: { [string]: any }): { Member }
	table.sort(list, function(a: Member, b: Member): boolean
		local sa, sb = since_of(a, now, asked), since_of(b, now, asked)
		if sa ~= sb then
			return sa > sb
		end
		return a.member.chat_id < b.member.chat_id
	end)
	return list
end

-- A part of the second line: its icon, then its words.
local function part(icon: Node, text: () -> string, props: { [string]: any }?): Node
	local p: { [any]: any } = {
		gap = "$space.1",
		items = "center",
		min_w = 0,
		shrink = 0,
		hidden = function(): boolean
			return text() == ""
		end,
		ui.div { color = "$text.tertiary", shrink = 0, icon },
		ui.text {
			text,
			min_w = 0,
			truncate = true,
			text_size = "$font.size.xs",
			color = "$text.secondary",
		},
	}
	for k, v in props or {} do
		p[k] = v
	end
	return ui.row(p :: any)
end

function M.CrewList(props: Props): Node
	local function now_s(): number
		return if props.now then props.now() else os.time()
	end
	local asked = ui.computed(function(): { [string]: any }
		return M.asked_by(if props.asks then props.asks() else nil)
	end)
	local groups = ui.computed(function(): { [string]: { Member } }
		local out: { [string]: { Member } } = { working = {}, waiting = {}, idle = {} }
		local by = asked:get()
		local now = os.time()
		for _, m in props.members() do
			if not m.member.released then
				table.insert(out[M.group_of(m, by)], m)
			end
		end
		for _, list in out do
			sorted(list, now, by)
		end
		return out
	end)

	local function row(first: Member, _i: number, cur: State<Member>): Node
		local id = first.member.chat_id
		local function m(): Member
			return cur:get()
		end
		local function chat(): { [string]: any }
			return m().chat or {}
		end
		local function title(): string
			return Name.crew(m().chat, Name.fallback(m().member, nil), id)
		end
		local line1 = ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			ui.div {
				w = 16,
				shrink = 0,
				items = "center",
				justify = "center",
				ui.presence(function(): string
					return M.kind_of(m(), asked:get())
				end, function(kind: string): Node
					-- (Its kind as a class: what tests read.)
					if kind == "idle" then
						return ui.div { class = "orch-status-idle", color = "$text.tertiary", Icon { name = "clock-circle", size = 14 } }
					end
					return ui.div { class = `orch-status-{kind}`, status.StatusIcon { kind = kind :: any, size = 14 } }
				end),
			},
			ui.text {
				title,
				class = "orch-crew-row-name",
				grow = 1,
				min_w = 0,
				truncate = true,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.primary",
			},
			ui.text {
				function(): string
					return status.ago(math.max(0, now_s() - since_of(m(), now_s(), asked:get())))
				end,
				class = "orch-crew-row-time",
				shrink = 0,
				tabular = true,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
		}
		local line2 = ui.row {
			w_full = true,
			pl = ui.raw(24, "under the name, past the status icon"),
			gap = "$space.3",
			items = "center",
			class = "orch-crew-row-line",
			part(Icon { name = "monitor", size = 12 }, function(): string
				local dev = chat().device
				return (dev and props.device_name and props.device_name(dev)) or ""
			end, { class = "orch-crew-row-machine" }),
			ui.row {
				gap = "$space.1",
				items = "center",
				shrink = 0,
				class = "orch-crew-row-model",
				hidden = function(): boolean
					return M.model_name(chat().model) == nil
				end,
				ui.presence(function(): string
					return MARKS[chat().agent or ""] or ""
				end, function(name: string): Node
					if name == "" then
						return ui.div { color = "$text.tertiary", Icon { name = "cpu", size = 12 } }
					end
					return Mark { name = name, size = 12, bare = true }
				end),
				ui.text {
					function(): string
						return M.model_name(chat().model) or ""
					end,
					text_size = "$font.size.xs",
					color = "$text.secondary",
				},
			},
			part(
				Icon {
					name = function(): Icon.Name
						local c = chat()
						local a = c.activity
						if UNDER_WAY[c.status or ""] and type(a) == "table" then
							return ((data.ACTIVITY :: { [string]: string })[a.kind] or "terminal") :: any
						end
						return "chat-round-line"
					end,
					size = 12,
				},
				function(): string
					return M.doing(m(), asked:get())
				end,
				{ shrink = 1, grow = 1, class = "orch-crew-row-doing" }
			),
		}
		return ui.button {
			class = "orch-crew-row",
			provide = { session = id },
			label = function(): string
				return locale.s("ds.orch.crew.open", { title = title() })
			end,
			w_full = true,
			direction = "column",
			px = "$space.2",
			py = "$space.2",
			gap = "$space.1",
			items = "stretch",
			rounded = "$radius.md",
			bg = "#00000000",
			hover = { bg = "$hover" },
			active = { bg = "$pressed" },
			on_click = function()
				props.on_open(id)
			end,
			line1,
			line2,
		}
	end

	local function group(key: string, title: any, icon: Node?): Node
		local all = ui.computed(function(): { Member }
			return groups:get()[key] or {}
		end)
		local more = ui.use_state(false)
		local shown = ui.computed(function(): { Member }
			local list = all:get()
			if more:get() or #list <= M.CAP then
				return list
			end
			return table.move(list, 1, M.CAP, 1, {})
		end)
		-- Folded Idle's head: the latest of it, and when.
		local aside = if key == "idle"
			then ui.row {
				gap = "$space.2",
				items = "center",
				min_w = 0,
				class = "orch-crew-latest",
				hidden = function(): boolean
					return props.open(key) or #all:get() == 0
				end,
				ui.text {
					function(): string
						local top = all:get()[1]
						return if top then Name.crew(top.chat, Name.fallback(top.member, nil), top.member.chat_id) else ""
					end,
					min_w = 0,
					truncate = true,
					text_size = "$font.size.xs",
					color = "$text.secondary",
				},
				ui.text {
					function(): string
						local top = all:get()[1]
						return if top then status.ago(math.max(0, now_s() - since_of(top, now_s(), asked:get()))) else ""
					end,
					shrink = 0,
					tabular = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			}
			else nil
		return ui.div {
			w_full = true,
			hidden = function(): boolean
				return #all:get() == 0
			end,
			Fold {
				id = `orch-crew-{key}`,
				title = title,
				icon = icon,
				count = function(): number
					return #all:get()
				end,
				aside = aside,
				open = function(): boolean
					return props.open(key)
				end,
				on_toggle = function(open: boolean)
					props.on_toggle(key, open)
				end,
				ui.column {
					w_full = true,
					ui.each(shown, row, {
						key = function(m: Member): string
							return m.member.chat_id
						end,
						w_full = true,
					}),
					ui.show(function(): boolean
						return #all:get() > M.CAP
					end, ui.button {
						class = "orch-crew-more",
						self = "start",
						ml = ui.raw(32, "under the names"),
						mt = "$space.1",
						px = "$space.2",
						h = 24,
						rounded = "$radius.md",
						items = "center",
						bg = "#00000000",
						hover = { bg = "$hover" },
						label = function(): string
							return if more:get() then locale.s("ds.orch.less") else locale.s("ds.orch.more", { n = #all:get() - M.CAP })
						end,
						on_click = function()
							more:set(not more:peek())
						end,
						ui.text {
							function(): string
								return if more:get() then locale.s("ds.orch.less") else locale.s("ds.orch.more", { n = #all:get() - M.CAP })
							end,
							text_size = "$font.size.xs",
							color = "$text.secondary",
						},
					}),
				},
			},
		}
	end

	return ui.column {
		id = "orch-crew-list",
		w_full = true,
		gap = "$space.3",
		ui.show(function(): boolean
			local g = groups:get()
			return #g.working + #g.waiting + #g.idle == 0
		end, ui.text {
			locale.t("ds.orch.crew.empty"),
			class = "orch-crew-empty",
			px = "$space.2",
			text_size = "$font.size.sm",
			color = "$text.tertiary",
		}),
		group("working", locale.t("ds.orch.crew_list.working"), nil),
		group("waiting", locale.t("ds.orch.crew_list.waiting"), nil),
		group("idle", locale.t("ds.orch.crew_list.idle"), nil),
	}
end

return M
