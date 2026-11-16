-- An orchestrator's page (`/orchestrator/:id`): its head (its hue, name and
-- goal; how many of its crew are working; its model; paused; its
-- settings), then its conversation beside its live state. The conversation
-- is its chat, transcript and composer, as a side chat's lane draws one
-- (screens/chat/lane_tab), where its crew's reports come in as Crew updates
-- cards. The live state is what needs you from it, its board and its crew.
-- Narrower than both need, one shows at a time, a switch in the head
-- choosing.
--
--   Page { id = function() return orchestrator_id end }

local ds = require("@ds")
local locale = require("@ds/locale")
local hue = require("@ds/orch_hue")
local data = require("@ds/orch_data")
local OrchBoard = require("@ds/orch_board")
local OrchCrew = require("@ds/orch_crew")
local stage = require("@ds/stage")
local rows = require("@account/rows")
local Asks = require("@ds/orch_asks")
local decide = require("@ds/orch_decide")
local Links = require("@ds/links")
local Block = require("@ds/orch_block")
local Counter = require("@ds/orch_counter")
local orchestration = require("@account/orchestration")
local sessions = require("@account/sessions")
local route = require("@app/screens/shell/route")
local lanes = require("@app/screens/chat/lanes")
local LaneTab = require("@app/screens/chat/lane_tab")
local actions = require("@app/screens/orchestrator/actions")
local Progress = require("@app/screens/control/progress")
local KeptScroll = require("@ds/kept_scroll")

local M = {}

-- Narrow: which half shows, the conversation or the live state (one for
-- the window, kept across a reload).
local half = ui.state("orch_page_half", "chat")

-- Where its live state is scrolled: kept as the page is made again (a
-- thread opened from it and back, the orchestrator a moment unknown while
-- the host publishes) and as the board and crew change under it.
M.place = ui.state("orch_page_scroll", 0)

-- Both side by side from this wide (the page's own width); narrower, one.
local SPLIT_AT = 880
-- The live state's share of a split page, and its bounds.
local SIDE_SHARE, SIDE_MIN, SIDE_MAX = 0.44, 380, 620
-- Under way: the crew counts these as working.
local WORKING: { [string]: boolean } = { working = true, connecting = true, waiting = true }

-- "claude-opus-5-5" as people say it: "Opus 5.5".
local function model_name(m: string?): string?
	if not m or m == "" then
		return nil
	end
	local s = string.gsub(m, "^[%w%-]+/", "")
	s = string.gsub(s, "^claude%-", "")
	s = string.gsub(s, "%-(%d+)%-(%d+)$", " %1.%2")
	s = string.gsub(s, "%-", " ")
	return (string.gsub(s, "^%l", string.upper))
end
M.model_name = model_name

-- What needs you from orchestrator `id` (tracked): its asks, and its own
-- chat or a crew thread waiting on an answer.
local function needs_of(id: string?): { any }
	local now = orchestration.now:get()
	local crew = data.crew_by_chat(now)
	local out = {}
	-- (A chat waiting on its own ask is that ask, once: data.needs.)
	for _, n in data.needs(now, sessions.now:get()) do
		local ask = n.ask
		if ask then
			if ask.orchestrator_id == id then
				table.insert(out, n)
			end
		elseif (n.from == id or (crew[n.from or ""] and crew[n.from or ""].orchestrator_id == id)) then
			table.insert(out, n)
		end
	end
	return out
end

-- A section of the live state, a block of its own: its title and count
-- over what it holds.
local function Section(title: any, count: (() -> number)?, body: Node, id: string): Node
	return Block.Block {
		id = id,
		title = title,
		figures = if count
			then ui.show(function(): boolean
				return count() > 0
			end, Counter { value = count, text_size = "$font.size.xs", color = "$text.secondary" })
			else nil,
		body,
	}
end

local Page = ui.component(function(props: { of: () -> string? }): Node
	local function orchestrator(): any
		local id = props.of()
		for _, o in orchestration.now:get().orchestrators or {} do
			if o.id == id then
				return o
			end
		end
		return nil
	end
	local chats = ui.computed(function(): { [string]: any }
		return data.chats(sessions.now:get())
	end)
	-- Its crew, each with its thread's row.
	local crew = ui.computed(function(): { any }
		local id = props.of()
		local all = chats:get()
		local out = {}
		for _, m in orchestration.now:get().crew or {} do
			if m.orchestrator_id == id and not m.released then
				table.insert(out, { member = m, chat = all[m.chat_id] })
			end
		end
		return out
	end)
	local working = ui.computed(function(): number
		local n = 0
		for _, c in crew:get() do
			if c.chat and WORKING[c.chat.status or ""] then
				n += 1
			end
		end
		return n
	end)
	-- (Not an ask answered and on its way: ds/orch_decide.)
	local needs = ui.computed(function(): number
		local n = 0
		for _, need in needs_of(props.of()) do
			if not need.ask or decide.counted(need.ask.id) then
				n += 1
			end
		end
		return n
	end)
	local function own_chat(): any
		local id = props.of()
		return if id then chats:get()[id] else nil
	end
	local function hue_now(): number?
		local o = orchestrator()
		return if o then o.hue else nil
	end
	local width = ui.use_state(1200)
	local function split(): boolean
		return width:get() >= SPLIT_AT
	end
	-- The conversation's lane, let go as the page goes or moves on.
	local function slot_of(id: string): string
		return `orch-{id}`
	end
	local watched: string? = nil
	ui.effect(function()
		local id = props.of()
		local slot = if id then slot_of(id) else nil
		if watched and watched ~= slot then
			lanes.watch(watched, nil)
		end
		watched = slot
	end)
	ui.on_cleanup(function()
		if watched then
			lanes.watch(watched, nil)
		end
	end)

	local function head(): Node
		return ui.row {
			id = "orch-head",
			w_full = true,
			shrink = 0,
			items = "center",
			gap = "$space.3",
			px = "$space.5",
			py = "$space.3",
			hue.Dot {
				hue = hue_now,
				size = 10,
				pulse = function(): boolean
					return working:get() > 0
				end,
			},
			ui.column {
				grow = 1,
				min_w = 0,
				ui.row {
					w_full = true,
					gap = "$space.2",
					items = "center",
					ds.FadeText {
						function(): string
							local o = orchestrator()
							return if o then o.name else ""
						end,
						id = "orch-name",
						text_size = "$font.size.lg",
						weight = "$font.weight.semibold",
						color = "$text.primary",
						shrink = 1,
					},
					ui.show(function(): boolean
						local o = orchestrator()
						return o ~= nil and o.paused == true
					end, ds.Badge { ui.t("orch.page.paused") :: any, size = "sm", icon = "action-pause" }),
				},
				ds.FadeText {
					function(): string
						local o = orchestrator()
						return if o then o.goal or "" else ""
					end,
					id = "orch-goal",
					text_size = "$font.size.sm",
					color = "$text.secondary",
					w_full = true,
				},
			},
			-- How many of its crew work now, of all of it.
			ui.row {
				id = "orch-crew-count",
				gap = "$space.1",
				items = "center",
				shrink = 0,
				ds.Icon { name = "users-group-rounded", size = 14, color = "$text.tertiary" },
				ui.text {
					function(): string
						return locale.s("orch.page.working_of", { working = working:get(), total = #crew:get() })
					end,
					text_size = "$font.size.sm",
					color = "$text.secondary",
					tabular = true,
				},
			},
			-- Its model.
			ui.row {
				id = "orch-model",
				gap = "$space.1",
				items = "center",
				shrink = 0,
				hidden = function(): boolean
					local c = own_chat()
					return c == nil or c.model == nil
				end,
				ui.presence(function(): string
					local c = own_chat()
					return (c and c.agent) or ""
				end, function(agent: string): Node
					return if agent ~= "" then ds.Mark { name = agent, size = 14, bare = true } else ui.div {}
				end),
				ui.text {
					function(): string
						local c = own_chat()
						return model_name(c and c.model) or ""
					end,
					text_size = "$font.size.sm",
					color = "$text.secondary",
				},
			},
			-- Narrow: the conversation or the live state.
			ui.div {
				shrink = 0,
				hidden = split,
				ds.Segmented {
					value = half,
					options = {
						{ value = "chat", label = ui.t("orch.page.chat") :: any },
						{ value = "live", label = ui.t("orch.page.live") :: any },
					},
				},
			},
			ds.IconButton {
				id = "orch-pause",
				icon = function(): any
					local o = orchestrator()
					return if o and o.paused then "action-play" else "action-pause"
				end,
				label = function(): string
					local o = orchestrator()
					return locale.s(if o and o.paused then "orch.page.resume" else "orch.page.pause")
				end,
				size = "sm",
				tone = "quiet",
				on_click = function()
					local o = orchestrator()
					if o then
						actions.set_paused(o.id, not o.paused)
					end
				end,
			},
			ds.IconButton {
				id = "orch-settings",
				icon = "settings",
				label = ui.t("orch.page.settings") :: any,
				size = "sm",
				tone = "quiet",
				on_click = function()
					local id = props.of()
					if id then
						actions.settings(id)
					end
				end,
			},
		}
	end

	-- The conversation: its chat in a lane of its own.
	local function conversation(): Node
		return ui.presence(function(): string
			return props.of() or ""
		end, function(id: string): Node
			if id == "" then
				return ui.div {}
			end
			local o = orchestrator()
			return LaneTab.Tab {
				tab = { id = slot_of(id), kind = "side_chat", chat = id, title = if o then o.name else "", data = { own = true } },
			}
		end, { w_full = true, h_full = true })
	end

	-- The live state: what needs you, its board, its crew.
	local kept = KeptScroll.use(M.place)
	local function live(): Node
		return ui.scroll {
			id = "orch-live",
			w_full = true,
			grow = 1,
			min_h = 0,
			fade = { 12, 12 },
			fade_overflow = true,
			offset = kept.offset,
			on_wheel = kept.on_wheel,
			on_press = kept.on_press,
			on_key = kept.on_key,
			ui.column {
				w_full = true,
				on_resize = kept.on_resize,
				gap = "$space.4",
				px = "$space.4",
				pt = "$space.2",
				pb = "$space.6",
				Section(ui.t("orch.page.needs_you"), function(): number
					return needs:get()
				end, Asks {
					needs = function(): { any }
						return needs_of(props.of())
					end,
					name_of = function(id: string?): string?
						return data.name_of(orchestration.now:get(), chats:get(), id)
					end,
					-- (A failure is said on the ask, with Retry: ds/orch_decide.)
					on_answer = function(ask: any, choice: any): any
						return orchestration.answer(ask.id, choice, ask)
					end,
					on_dismiss = function(ask: any)
						orchestration.dismiss(ask.id)
					end,
					on_open = function(chat: string, message: string?)
						route.open_session(chat, if message then { message = message } else nil)
					end,
				}, "orch-section-needs"),
				-- (Needs you with the page; the Board, then the crew, each in a
				-- frame of its own after: ds/stage.)
				Section(ui.t("orch.page.board"), nil, stage.Later(1, function(): Node return OrchBoard {
					id = "orch-page-board",
					items = function(): { any }
						return orchestration.now:get().items or {}
					end,
					orchestrators = function(): { any }
						return orchestration.now:get().orchestrators or {}
					end,
					filter = props.of,
					compact = true,
					-- (Each card's thread and ask read by themselves,
					-- account/rows: a push wakes the cards it changed.)
					orchestrator_of = rows.orchestrator,
					todo_of = function(chat: string): any
						local c = rows.chat(chat)
						return c and c.todo
					end,
					thread_of = function(chat: string): any
						return rows.chat(chat)
					end,
					ask_of = function(chat: string): any
						return rows.ask(chat)
					end,
					on_move = function(id: string, state: string, order: number)
						orchestration.move_item(id, state, order)
					end,
					on_approve = function(id: string)
						orchestration.approve_item(id)
					end,
					on_request_changes = function(id: string, text: string)
						orchestration.request_changes(id, text)
					end,
					on_priority = function(id: string, p: string)
						orchestration.set_priority(id, p)
					end,
					on_open_thread = function(chat: string)
						route.open_session(chat)
					end,
					-- (A card's dump, shown in the Control room's capture.)
					on_source = function(src: any)
						orchestration.reveal_dump:set(src.dump_id)
						route.control()
					end,
					on_remove = orchestration.remove_item,
					on_restore = orchestration.restore_item,
					on_start = orchestration.start_item,
					space_name = function(id: string): string?
						for _, p in sessions.projects:get() or {} do
							if p.id == id then
								return p.name
							end
						end
						return nil
					end,
					device_name = function(id: string): string?
						for _, d in sessions.devices:get() or {} do
							if d.id == id then
								return d.name
							end
						end
						return nil
					end,
					progress_of = Progress.row,
					on_link = function(link: any)
						if link.kind == "chat" then
							route.open_session(link.target)
						elseif string.match(link.target or "", "^https?://") then
							Links.open(link.target)
						end
					end,
				} end), "orch-section-board"),
				Section(ui.t("orch.page.crew"), function(): number
					return #crew:get()
				end, stage.Later(2, function(): Node return OrchCrew {
					members = crew,
					hue = hue_now,
					device_name = function(device: string): string?
						for _, d in sessions.now:get().devices or {} do
							if d.id == device then
								return d.name
							end
						end
						return nil
					end,
					on_open = function(chat: string)
						route.open_session(chat)
					end,
					progress = function(chat: string): Node
						return Progress.strip(chat, { lean = true })
					end,
				} end), "orch-section-crew"),
			},
		}
	end

	local function body(): Node
		return ui.row {
			id = "orch-body",
			w_full = true,
			grow = 1,
			min_h = 0,
			ui.column {
				id = "orch-conversation",
				grow = 1,
				min_w = 0,
				h_full = true,
				hidden = function(): boolean
					return not split() and half:get() ~= "chat"
				end,
				conversation(),
			},
			-- The seam between the two: a hairline.
			ui.div {
				w = 1,
				h_full = true,
				shrink = 0,
				bg = "$border",
				hidden = function(): boolean
					return not split()
				end,
			},
			ui.column {
				id = "orch-side",
				h_full = true,
				min_w = 0,
				shrink = 0,
				grow = function(): number
					return if split() then 0 else 1
				end,
				w = function(): number | string
					if not split() then
						return "auto"
					end
					return math.clamp(math.floor(width:get() * SIDE_SHARE), SIDE_MIN, SIDE_MAX)
				end,
				hidden = function(): boolean
					return not split() and half:get() ~= "live"
				end,
				live(),
			},
		}
	end

	return ui.column {
		id = "orchestrator-page",
		w_full = true,
		h_full = true,
		on_resize = function(size: { w: number, h: number })
			if math.abs(size.w - width:peek()) >= 1 then
				width:set(size.w)
			end
		end,
		ui.presence(function(): string
			if orchestrator() then
				return "found"
			end
			return if orchestration.now:get().state == "ready" then "missing" else "loading"
		end, function(kind: string): Node
			if kind == "loading" then
				return ui.div {}
			elseif kind == "missing" then
				return ds.EmptyState {
					icon = "users-group-rounded",
					size = "sm",
					fill = true,
					title = ui.t("orch.page.missing") :: any,
					action = {
						label = ui.t("orch.page.to_control") :: any,
						on_click = function()
							route.control()
						end,
					},
				}
			end
			return ui.column { w_full = true, h_full = true, head(), body() }
		end, { w_full = true, h_full = true }),
	}
end)

-- (Its prop is `of`: a component's `id` is its node's.)
function M.Page(props: { id: () -> string? }): Node
	return Page { of = props.id }
end

return M
