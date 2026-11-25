-- The force Graph's hover card (ds/orch_force): what a node is, in a few
-- compact rows, from what the Map already holds (its world, a chat's row,
-- Progress's row of it, the board's items), never a transcript.
--
--   A thread: its whole title (two lines at most); its state and how long
--   ("Done 2h ago", "Working 14m", "last step 30s ago") with its steps or
--   todo; where it sits (orchestrator, its card and column, role, model,
--   machine); what it's doing (Progress's note while it works, its
--   card's outcome once done); stopped by its usage limit, that ("Waiting
--   for limit", "resets 14:05", or running on another account meanwhile);
--   what applies of unread, asking, failed, a
--   pull request, a branch; and Open chat, Open card, Pin, Mark read;
--   its card paused by the person, Resume (into Now) first.
--   An orchestrator: its crew working / idle / failed, its board's Now /
--   Review / Blocked, and Open page.
--   A machine (the machines pivot): online or not, its CPU, how many
--   threads work on it.
--   A fold (a hub's idle and finished threads as one): how many, the first
--   few of them, and Show them.
--
-- Facts on one line stand apart by spacing, never a typed separator.
--
--   ForceCard.Card(props) -> Node   (props below)

local HoverCard = require("@self/hover_card")
local Button = require("@self/button")
local Icon = require("@self/icon")
local Cells = require("@self/orch_cells")
local status = require("@self/status")
local locale = require("@self/locale")
local limit = require("@self/usage_limit")

local M = {}

export type Props = {
	-- The node under the pointer, as ds/orch_force_model has it, or nil.
	node: () -> any,
	world: () -> any,
	chat: (id: string) -> any,
	state_of: (id: string) -> string,
	machine_name: (device: string?) -> string,
	-- Progress's row of a chat (account/orchestration's `progress`), if any.
	progress: ((id: string) -> any)?,
	pinned: () -> boolean,
	on_pin: () -> (),
	on_open_chat: (id: string) -> (),
	on_open_orchestrator: (id: string) -> (),
	on_open_card: ((item: string) -> ())?,
	-- A paused card resumed (into Now).
	on_resume: ((item: string) -> ())?,
	on_mark_read: ((ids: { string }) -> ())?,
	-- A fold opened (its threads out, each alone), by the fold's id.
	on_open_fold: ((id: string) -> ())?,
}

local W = 300
local LIVE: { [string]: boolean } = { working = true, waiting = true }
local UNREAD: { [string]: boolean } = { done = true, failed = true }

-- How long ago `t` was, compact ("30s ago", "14m ago", "2h ago").
local function ago(t: number?): string
	if type(t) ~= "number" or t <= 0 then
		return ""
	end
	local d = math.max(0, os.time() - t)
	if d < 60 then
		return locale.s("ds.orch.map.card.seconds_ago", { n = math.floor(d) })
	end
	return locale.s("ds.orch.map.card.ago", { ago = locale.ago_short(d) })
end
-- How long, compact ("40s", "14m", "2h").
local function took(d: number): string
	if d < 60 then
		return locale.s("ds.orch.map.card.seconds", { n = math.floor(math.max(0, d)) })
	end
	return locale.ago_short(d)
end

-- A thread's crew row, its card, and its orchestrator, from the world.
local function crew_of(world: any, id: string): (any, any, any)
	local now = world.now or {}
	local member = nil
	for _, m in now.crew or {} do
		if m.chat_id == id and not m.released then
			member = m
		end
	end
	local item = nil
	local lead = member and member.orchestrator_id
	for _, it in now.items or {} do
		if (member and it.id == member.item_id) or (not member and it.threads and table.find(it.threads, id)) then
			item = it
			lead = lead or it.orchestrator_id
		end
	end
	local orch = nil
	for _, o in now.orchestrators or {} do
		if o.id == lead then
			orch = o
		end
	end
	return member, item, orch
end

local function open_ask(world: any, id: string): any
	for _, a in (world.now or {}).asks or {} do
		if a.state == "open" and a.from_chat == id then
			return a
		end
	end
	return nil
end

-- Whether a card is paused by the person: Blocked, `paused_at` set.
local function paused(it: any): boolean
	if not it or it.state ~= "blocked" then
		return false
	end
	local p = it.paused_at
	if p == nil then
		p = it.pausedAt
	end
	return type(p) == "number" or (type(p) == "string" and p ~= "")
end

-- Facts on a line: each its own words, spaced apart (never a typed
-- separator between them); a line too long for the card wraps.
local function facts(list: () -> { string }, class: string?, color: string?): Node
	return ui.row {
		class = class,
		w_full = true,
		min_w = 0,
		flex_wrap = true,
		gap = "$space.2",
		ui.each(ui.computed(list), function(t: string): Node
			return ui.text { t, text_size = "$font.size.xs", color = color or "$text.tertiary", truncate = true, min_w = 0 }
		end),
	}
end

-- A signal chip: its glyph and words.
local function chip(glyph: string, words: () -> string, color: string, class: string): Node
	return ui.row {
		class = `orch-force-card-chip {class}`,
		gap = "$zeron.space.tight",
		items = "center",
		px = "$space.1",
		h = 18,
		rounded = "$radius.sm",
		shrink = 0,
		color = color,
		Icon { name = glyph :: any, size = 11 },
		ui.text { words, text_size = "$font.size.xs", weight = "$font.weight.medium" },
	}
end

function M.Card(props: Props): Node
	local function n(): any
		return props.node()
	end
	local function id(): string
		local x = n()
		return if x then x.id else ""
	end
	local function kind(): string
		local x = n()
		return if x then x.kind else ""
	end
	local function c(): any
		return props.chat(id()) or {}
	end
	local function st(): string
		return props.state_of(id())
	end
	local function prog(): any
		return if props.progress then props.progress(id()) else nil
	end
	local function crew(): (any, any, any)
		return crew_of(props.world(), id())
	end

	-- A thread.
	local function lim(): limit.Limit?
		return limit.of(c())
	end
	local function state_line(): { string }
		local s0 = st()
		local k = status.KINDS[s0 :: any]
		local word = if k then k.label else locale.s("ds.orch.map.card.idle")
		local l = lim()
		if s0 == "limited" and l then
			-- (Waiting for the reset: how long till it, where that's known.)
			local left = limit.left(l, os.time())
			return if left ~= "" then { word, left } else { word }
		end
		local p = prog()
		if LIVE[s0] then
			local since = (p and p.started) or c().turn_started
			local parts = { if since then `{word} {took(os.time() - since)}` else word }
			local step = p and (p.last_step or p.note_at)
			if step then
				table.insert(parts, locale.s("ds.orch.map.card.last_step", { ago = ago(step) }))
			end
			if p and type(p.eta_secs) == "number" then
				table.insert(parts, locale.s("ds.orch.map.card.eta", { eta = took(p.eta_secs) }))
			end
			return parts
		end
		local at = c().updated
		return { if at then locale.s("ds.orch.map.card.state_ago", { state = word, ago = ago(at) }) else word }
	end
	local function steps(): (number, number)
		local p = prog()
		if p and p.steps and (p.steps.total or 0) > 0 then
			return p.steps.done or 0, p.steps.total
		end
		local x = n()
		return (x and x.done) or 0, (x and x.total) or 0
	end
	local function context(): { string }
		local _, item, orch = crew()
		local member = crew()
		local parts = {}
		if orch then
			table.insert(parts, orch.name or "")
		end
		if item then
			table.insert(parts, item.title or "")
		end
		if member and member.role and member.role ~= "" then
			table.insert(parts, member.role)
		end
		local model = c().model
		if model and model ~= "" then
			table.insert(parts, model)
		end
		local x = n()
		local mname = props.machine_name(x and x.device)
		if mname ~= "" then
			table.insert(parts, mname)
		end
		local out = {}
		for _, t in parts do
			if t ~= "" then
				table.insert(out, t)
			end
		end
		return out
	end
	local function column(): string
		local _, item = crew()
		return if item and item.state then locale.s(`ds.orch.map.card.column.{item.state}`) else ""
	end
	local function doing(): string
		local s0 = st()
		local p = prog()
		if LIVE[s0] then
			local note = p and p.note
			if note and note ~= "" then
				return note
			end
			local a = c().activity
			return if a then (a.label or locale.s(`ds.orch.activity.{a.kind}`)) else ""
		end
		local _, item = crew()
		local o = item and item.outcome
		if o and o ~= "" then
			-- (Its first line, its markdown's marks dropped.)
			local first = string.match(o, "[^\n]+") or o
			return (string.gsub(first, "[*_`#>]", ""))
		end
		return ""
	end
	local function pr(): any
		local _, item = crew()
		for _, l in (item and item.links) or {} do
			if l.kind == "pr" then
				return l
			end
		end
		return nil
	end

	local thread = ui.column {
		class = "orch-force-card-thread",
		w_full = true,
		gap = "$space.1",
		ui.row {
			gap = "$space.2",
			items = "start",
			w_full = true,
			ui.show(function(): boolean
				return st() ~= "idle"
			end, ui.div { pt = "$zeron.space.hair", shrink = 0, status.StatusIcon { kind = function(): any
				return st()
			end, size = 14 } }),
			ui.text {
				function(): string
					local x = n()
					return if x then x.title else ""
				end,
				class = "orch-force-card-title",
				lines = 2,
				grow = 1,
				min_w = 0,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.primary",
			},
		},
		-- Stopped by its usage limit: what it waits for, or where it runs
		-- meanwhile.
		ui.show(function(): boolean
			return lim() ~= nil
		end, ui.row {
			class = "orch-force-card-limit",
			w_full = true,
			min_w = 0,
			gap = "$zeron.space.tight",
			items = "center",
			color = "$zeron.status.limited",
			-- (Waiting, its title's glyph says so already.)
			ui.show(function(): boolean
				return limit.switched(lim())
			end, Icon { name = "status-limited", size = 11 }),
			-- (Its words as facts apart, spaced.)
			ui.row {
				class = "orch-force-card-limit-text",
				grow = 1,
				min_w = 0,
				flex_wrap = true,
				gap = "$space.2",
				ui.each(ui.computed(function(): { string }
					return limit.parts(lim())
				end), function(t: string): Node
					return ui.text { t, text_size = "$font.size.xs", weight = "$font.weight.medium", truncate = true, min_w = 0 }
				end),
			},
		}),
		facts(state_line, "orch-force-card-state", "$text.secondary"),
		ui.show(function(): boolean
			local _, total = steps()
			return total > 0
		end, Cells {
			done = function(): number
				return (steps())
			end,
			total = function(): number
				local _, total = steps()
				return total
			end,
			current = function(): boolean
				return LIVE[st()] == true
			end,
			fill = true,
			w = W - 24,
			h = 6,
		}),
		ui.show(function(): boolean
			return doing() ~= ""
		end, ui.text { doing, class = "orch-force-card-doing", text_size = "$font.size.xs", color = "$text.primary", lines = 2, min_w = 0, w_full = true }),
		ui.row {
			w_full = true,
			gap = "$space.1",
			items = "center",
			min_w = 0,
			facts(context, "orch-force-card-context"),
			ui.show(function(): boolean
				return column() ~= ""
			end, ui.text { column, class = "orch-force-card-column", shrink = 0, px = "$space.1", rounded = "$radius.sm", bg = "$zeron.chip.bg", text_size = "$font.size.xs", color = "$text.secondary" }),
		},
		-- Signals: only those that apply.
		ui.row {
			class = "orch-force-card-signals",
			gap = "$space.1",
			wrap = true,
			w_full = true,
			ui.show(function(): boolean
				return UNREAD[st()] == true
			end, chip("eye", function(): string
				return locale.s("ds.orch.map.card.unread")
			end, "$text.secondary", "unread")),
			ui.show(function(): boolean
				return open_ask(props.world(), id()) ~= nil
			end, chip("status-input", function(): string
				return locale.s("ds.orch.map.card.needs_you")
			end, "$zeron.status.input", "asking")),
			ui.show(function(): boolean
				return st() == "failed"
			end, chip("status-failed", function(): string
				return locale.s("ds.orch.map.card.failed")
			end, "$zeron.status.failed", "failed")),
			ui.show(function(): boolean
				return pr() ~= nil
			end, chip("pull-request", function(): string
				local l = pr()
				return if l then l.label else ""
			end, "$zeron.pr.open", "pr")),
			ui.show(function(): boolean
				local b = c().branch
				return b ~= nil and b ~= "" and pr() == nil
			end, chip("git-branch", function(): string
				return c().branch or ""
			end, "$text.secondary", "branch")),
		},
		ui.row {
			class = "orch-force-card-actions",
			-- (Five at most, Resume first: a second line where they don't fit.)
			w_full = true,
			flex_wrap = true,
			gap = "$space.1",
			pt = "$space.1",
			ui.show(function(): boolean
				local _, item = crew()
				return paused(item) and props.on_resume ~= nil
			end, Button {
				id = "orch-force-card-resume",
				tone = "secondary",
				size = "xs",
				on_click = function()
					local _, item = crew()
					if item and props.on_resume then
						props.on_resume(item.id)
					end
				end,
				locale.t("ds.orch.map.card.resume"),
			}),
			Button {
				id = "orch-force-card-open",
				tone = "secondary",
				size = "xs",
				on_click = function()
					props.on_open_chat(id())
				end,
				locale.t("ds.orch.map.card.open_chat"),
			},
			ui.show(function(): boolean
				local _, item = crew()
				return item ~= nil and props.on_open_card ~= nil
			end, Button {
				id = "orch-force-card-open-card",
				tone = "ghost",
				size = "xs",
				on_click = function()
					local _, item = crew()
					if item and props.on_open_card then
						props.on_open_card(item.id)
					end
				end,
				locale.t("ds.orch.map.card.open_card"),
			}),
			Button {
				id = "orch-force-card-pin",
				tone = "ghost",
				size = "xs",
				on_click = function()
					props.on_pin()
				end,
				function(): string
					return if props.pinned() then locale.s("ds.orch.map.card.unpin") else locale.s("ds.orch.map.card.pin")
				end,
			},
			ui.show(function(): boolean
				return UNREAD[st()] == true and props.on_mark_read ~= nil
			end, Button {
				id = "orch-force-card-read",
				tone = "ghost",
				size = "xs",
				on_click = function()
					if props.on_mark_read then
						props.on_mark_read({ id() })
					end
				end,
				locale.t("ds.orch.map.card.mark_read"),
			}),
		},
	}

	-- An orchestrator: its crew and its board.
	local function tallies(): { [string]: number }
		local world = props.world()
		local now = world.now or {}
		local out = { working = 0, idle = 0, failed = 0, active = 0, review = 0, blocked = 0 }
		for _, m in now.crew or {} do
			if m.orchestrator_id == id() and not m.released then
				local s0 = props.state_of(m.chat_id)
				if LIVE[s0] then
					out.working += 1
				elseif s0 == "failed" then
					out.failed += 1
				else
					out.idle += 1
				end
			end
		end
		for _, it in now.items or {} do
			if it.orchestrator_id == id() and not it.removed and out[it.state] ~= nil and (it.state == "active" or it.state == "review" or it.state == "blocked") then
				out[it.state] += 1
			end
		end
		return out
	end
	local orchestrator = ui.column {
		class = "orch-force-card-orchestrator",
		w_full = true,
		gap = "$space.1",
		ui.text {
			function(): string
				local x = n()
				return if x then x.title else ""
			end,
			lines = 2,
			text_size = "$font.size.sm",
			weight = "$font.weight.medium",
			color = "$text.primary",
			w_full = true,
		},
		facts(function(): { string }
			local t = tallies()
			return {
				locale.s("ds.orch.map.card.crew_label"),
				locale.s("ds.orch.map.card.n_working", { n = t.working }),
				locale.s("ds.orch.map.card.n_idle", { n = t.idle }),
				locale.s("ds.orch.map.card.n_failed", { n = t.failed }),
			}
		end, "orch-force-card-crew", "$text.secondary"),
		facts(function(): { string }
			local t = tallies()
			return {
				locale.s("ds.orch.map.card.board_label"),
				locale.s("ds.orch.map.card.n_now", { n = t.active }),
				locale.s("ds.orch.map.card.n_review", { n = t.review }),
				locale.s("ds.orch.map.card.n_blocked", { n = t.blocked }),
			}
		end, "orch-force-card-board", "$text.secondary"),
		facts(function(): { string }
			local x = n()
			local m = props.machine_name(x and x.device)
			return if m ~= "" then { m } else {}
		end),
		ui.row {
			class = "orch-force-card-actions",
			gap = "$space.1",
			pt = "$space.1",
			Button {
				id = "orch-force-card-open-page",
				tone = "secondary",
				size = "xs",
				on_click = function()
					props.on_open_orchestrator(id())
				end,
				locale.t("ds.orch.map.card.open_page"),
			},
			Button {
				id = "orch-force-card-pin",
				tone = "ghost",
				size = "xs",
				on_click = function()
					props.on_pin()
				end,
				function(): string
					return if props.pinned() then locale.s("ds.orch.map.card.unpin") else locale.s("ds.orch.map.card.pin")
				end,
			},
		},
	}

	-- A machine (the machines pivot).
	local function device(): any
		local x = n()
		for _, d in props.world().devices or {} do
			if x and d.id == x.device then
				return d
			end
		end
		return nil
	end
	local machine = ui.column {
		class = "orch-force-card-machine",
		w_full = true,
		gap = "$space.1",
		ui.text {
			function(): string
				local x = n()
				return if x then x.title else ""
			end,
			text_size = "$font.size.sm",
			weight = "$font.weight.medium",
			color = "$text.primary",
		},
		facts(function(): { string }
				local d = device()
				local parts = { if d == nil or d.online ~= false then locale.s("ds.orch.map.card.online") else locale.s("ds.orch.map.card.offline") }
				local load = (props.world().loads or {})[(n() or {}).device or ""]
				if load and type(load.cpu) == "number" then
					table.insert(parts, locale.s("ds.orch.map.card.cpu", { cpu = math.floor(load.cpu * 100 + 0.5) }))
				end
				local working = 0
				for cid, ch in props.world().chats or {} do
					if ch.device == (n() or {}).device and LIVE[props.state_of(cid)] then
						working += 1
					end
				end
				table.insert(parts, locale.s("ds.orch.map.card.running", { count = working }))
				return parts
			end, "orch-force-card-machine-line", "$text.secondary"),
	}

	-- A fold: how many, the first few, and Show them.
	local FOLD_SHOWN = 5
	local function folded(): { string }
		local x = n()
		return (x and x.folded) or {}
	end
	local fold = ui.column {
		class = "orch-force-card-fold",
		w_full = true,
		gap = "$space.1",
		ui.text {
			function(): string
				return locale.s("ds.orch.map.card.fold_title", { count = #folded() })
			end,
			class = "orch-force-card-fold-title",
			text_size = "$font.size.sm",
			weight = "$font.weight.medium",
			color = "$text.primary",
		},
		ui.column {
			w_full = true,
			gap = "$zeron.space.hair",
			ui.each(ui.computed(function(): { string }
				local out = {}
				for k, cid in folded() do
					if k > FOLD_SHOWN then
						break
					end
					table.insert(out, cid)
				end
				return out
			end), function(cid: string): Node
				return ui.row {
					class = "orch-force-card-fold-row",
					gap = "$space.2",
					items = "center",
					w_full = true,
					min_w = 0,
					-- (Its state's glyph; an idle one none, its room kept.)
					ui.div {
						w = 12,
						h = 12,
						shrink = 0,
						ui.show(function(): boolean
							return status.KINDS[props.state_of(cid) :: any] ~= nil
						end, status.StatusIcon { kind = function(): any
							-- (Built while hidden too: a kind it knows.)
							local k = props.state_of(cid)
							return if status.KINDS[k :: any] then k else "done"
						end, size = 12 }),
					},
					ui.text {
						function(): string
							local ch = props.chat(cid)
							return (ch and ch.title) or ""
						end,
						text_size = "$font.size.xs",
						color = "$text.secondary",
						truncate = true,
						min_w = 0,
						grow = 1,
					},
				}
			end),
		},
		ui.row {
			class = "orch-force-card-actions",
			gap = "$space.1",
			pt = "$space.1",
			Button {
				id = "orch-force-card-fold-open",
				tone = "secondary",
				size = "xs",
				on_click = function()
					local x = n()
					if x and props.on_open_fold then
						props.on_open_fold(x.id)
					end
				end,
				locale.t("ds.orch.map.card.fold_open"),
			},
		},
	}

	return HoverCard.Card {
		class = "orch-force-card",
		w = W,
		p = "$space.3",
		gap = "$space.1",
		ui.switch(function(): string
			local k = kind()
			local x = n()
			return if x and x.folded then "fold" elseif k == "chat" then "thread" elseif k == "machine" then "machine" else "orchestrator"
		end, {
			thread = thread,
			orchestrator = orchestrator,
			machine = machine,
			fold = fold,
		}),
	}
end

return M
