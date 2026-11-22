-- An orchestrator's page (`/orchestrator/:id`): its head (its hue, name and
-- goal; how many of its crew are working; its model; paused; its
-- settings), then its conversation beside its side panel. The conversation
-- is its chat, transcript and composer, as a side chat's lane draws one
-- (screens/chat/lane_tab), where its crew's reports come in as Crew updates
-- cards. The side panel (screens/orchestrator/panel) holds its tasks and its
-- crew. A switch in the head shows the conversation, both, or the panel
-- alone, at every width; the seam between the two sizes the panel. Width
-- and choice are kept for each orchestrator (panel_state). A new one, with
-- nothing for the panel yet, shows its conversation alone and one line on
-- what to type first; the panel offers itself once there's a task, an ask
-- or a crew thread.
--
--   Page { id = function() return orchestrator_id end }

local ds = require("@ds")
local locale = require("@ds/locale")
local hue = require("@ds/orch_hue")
local data = require("@ds/orch_data")
local rows = require("@account/rows")
local decide = require("@ds/orch_decide")
local Links = require("@ds/links")
local seam = require("@ds/seam")
local orchestration = require("@account/orchestration")
local sessions = require("@account/sessions")
local route = require("@app/screens/shell/route")
local lanes = require("@app/screens/chat/lanes")
local LaneTab = require("@app/screens/chat/lane_tab")
local actions = require("@app/screens/orchestrator/actions")
local NeedsYou = require("@app/screens/control/needs")
local Panel = require("@app/screens/orchestrator/panel")
local panel_state = require("@app/screens/orchestrator/panel_state")
local Flow = require("@ds/orch_flow")
local FlowEditor = require("@ds/orch_flow_editor")
local flows = require("@app/screens/orchestrator/flows")
local store = require("@app/screens/chat/store")
local toast = require("@ds/toast")

local M = {}

-- What the page shows: the conversation ("chat"), both ("split") or the
-- side panel alone ("live"); the shown orchestrator's, kept for it
-- (panel_state) once the person chooses.
M.mode = ui.state("orch_page_mode", "chat")

-- What the page shows: its work (the conversation and live state) or its
-- flow, to edit (ds/orch_flow_editor).
M.view = ui.state("orch_page_view", "work")

-- The models the composer's device offers, as `harness/model`, grouped by
-- agent: a flow block's choices.
local function model_options(): { any }
	local cat = store.catalog:get()
	local labels: { [string]: string } = {}
	for _, h in cat.harnesses or {} do
		labels[h.id] = h.label
	end
	local out: { any } = {}
	for _, m in cat.models or {} do
		table.insert(out, { value = m.value, label = m.label, group = labels[m.harness] or m.harness })
	end
	return out
end

-- Where its Tasks tab is scrolled: kept as the page is made again (a
-- thread opened from it and back, the orchestrator a moment unknown while
-- the host publishes) and as the board and crew change under it.
M.place = ui.state("orch_page_scroll", 0)

-- The narrowest the conversation and the panel go side by side; narrower
-- than both, Split shows the conversation.
local CHAT_MIN, PANEL_MIN = 360, 300
M.CHAT_MIN, M.PANEL_MIN = CHAT_MIN, PANEL_MIN
-- The panel's share of a split page until it's dragged, and its bounds then.
local SIDE_SHARE, SIDE_LEAST, SIDE_MOST = 0.4, 360, 560
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
-- chat or a crew thread waiting on an answer (screens/control/needs).
local function needs_of(id: string?): { any }
	return NeedsYou.of(orchestration.now:get(), sessions.now:get(), id)
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
	local function name_of(id: string?): string?
		return data.name_of(orchestration.now:get(), chats:get(), id)
	end
	local function hue_of(id: string?): number?
		return data.hue_of(orchestration.now:get(), chats:get(), id)
	end
	-- A link of a card's or an ask's: a chat opened here, a web one outside.
	local function open_link(link: any)
		if link.kind == "chat" then
			NeedsYou.open_chat(link.target)
		elseif string.match(link.target or "", "^https?://") then
			Links.open(link.target)
		end
	end
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
	local function paused(): boolean
		local o = orchestrator()
		return o ~= nil and o.paused == true
	end
	local function device_name(device: string): string?
		for _, d in sessions.now:get().devices or {} do
			if d.id == device then
				return d.name
			end
		end
		return nil
	end
	-- Whether the panel has anything to show: a task, an ask, a crew thread.
	local offered_now = ui.computed(function(): boolean
		local id = props.of()
		if #crew:get() > 0 or needs:get() > 0 then
			return true
		end
		for _, it in orchestration.now:get().items or {} do
			if it.orchestrator_id == id then
				return true
			end
		end
		return false
	end)
	local function offered(): boolean
		return offered_now:get()
	end
	-- Its mode as kept, else Split once there's something to show.
	local function kept_mode(kept: any, has: boolean): string
		local k = kept and kept.mode
		if k == "chat" or k == "split" or k == "live" then
			return k
		end
		return if has then "split" else "chat"
	end
	ui.effect(function()
		local m = kept_mode(panel_state.get(props.of()), offered_now:get())
		if M.mode:peek() ~= m then
			M.mode:set(m)
		end
	end)
	-- Chosen in the head: kept for this orchestrator.
	ui.effect(function()
		local m = M.mode:get()
		local id = props.of()
		if id and m ~= kept_mode(panel_state.peek(id), offered_now:peek()) then
			panel_state.set(id, "mode", m)
		end
	end)
	local function fits(): boolean
		return width:get() >= CHAT_MIN + PANEL_MIN + 1
	end
	-- What shows: nothing for the panel, the conversation; Split too
	-- narrow for both, the conversation.
	local function shows(): string
		if not offered() then
			return "chat"
		end
		local m = M.mode:get()
		if m == "split" and not fits() then
			return "chat"
		end
		return m
	end
	local function panel_most(): number
		return width:get() - CHAT_MIN - 1
	end
	-- The panel's width split: as dragged, else its share; within bounds.
	local function panel_w(): number
		local kept = panel_state.get(props.of()).w
		local want = kept or math.clamp(math.floor(width:get() * SIDE_SHARE), SIDE_LEAST, SIDE_MOST)
		return math.floor(math.clamp(want, PANEL_MIN, math.max(PANEL_MIN, panel_most())))
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
					return working:get() > 0 and not paused()
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
					ui.show(paused, ui.div { id = "orch-paused", shrink = 0, ds.Badge { ui.t("orch.page.paused") :: any, size = "sm", icon = "action-pause", tone = "warning" } }),
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
				hidden = function(): boolean
					return #crew:get() == 0
				end,
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
			-- Its work or its flow.
			ui.div {
				shrink = 0,
				ds.Segmented {
					id = "orch-view",
					value = M.view,
					options = {
						{ value = "work", label = ui.t("orch.page.work") :: any },
						{ value = "flow", label = ui.t("orch.page.flow") :: any },
					},
				},
			},
			-- The conversation, both, or the panel alone: at every width,
			-- once there's something for the panel to show.
			ui.div {
				id = "orch-mode",
				shrink = 0,
				hidden = function(): boolean
					return M.view:get() == "flow" or not offered()
				end,
				ds.Segmented {
					value = M.mode,
					label = ui.t("orch.page.mode") :: any,
					options = {
						{ value = "chat", label = ui.t("orch.page.chat") :: any },
						{ value = "split", label = ui.t("orch.page.split") :: any },
						{ value = "live", label = ui.t("orch.page.live") :: any },
					},
				},
			},
			-- Paused: a word to resume it by; else its pause, a glyph.
			ui.presence(function(): string
				return if paused() then "paused" else "running"
			end, function(kind: string): Node
				local function toggle()
					local o = orchestrator()
					if o then
						actions.set_paused(o.id, not o.paused)
					end
				end
				if kind == "paused" then
					return ds.Button {
						ds.Icon { name = "action-play", size = 14 },
						ui.t("orch.page.resume") :: any,
						id = "orch-pause",
						tone = "secondary",
						size = "xs",
						tooltip = locale.s("orch.page.resume_tip"),
						on_click = toggle,
					}
				end
				return ds.IconButton {
					id = "orch-pause",
					icon = "action-pause",
					label = ui.t("orch.page.pause") :: any,
					tooltip = locale.s("orch.page.pause_tip"),
					tooltip_side = "bottom",
					size = "sm",
					tone = "quiet",
					on_click = toggle,
				}
			end, { shrink = 0 }),
			ds.IconButton {
				id = "orch-settings",
				icon = "settings",
				label = ui.t("orch.page.settings") :: any,
				tooltip = locale.s("orch.page.settings_tip"),
				tooltip_side = "bottom",
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

	-- The conversation: its chat in a lane of its own; empty and with
	-- nothing for the panel yet, one line on what to type first.
	local function conversation(): Node
		return ui.presence(function(): string
			return props.of() or ""
		end, function(id: string): Node
			if id == "" then
				return ui.div {}
			end
			local o = orchestrator()
			local view = lanes.view(slot_of(id))
			return ui.column {
				w_full = true,
				h_full = true,
				LaneTab.Tab {
					tab = { id = slot_of(id), kind = "side_chat", chat = id, title = if o then o.name else "", data = { own = true } },
				},
				ui.show(function(): boolean
					return not offered() and #(view.messages:get() or {}) == 0
				end, ui.row {
					id = "orch-first-hint",
					absolute = true,
					left = 0,
					right = 0,
					top = "40%",
					justify = "center",
					pointer_events = "none",
					px = "$space.6",
					ui.text {
						locale.t("orch.page.first_hint"),
						text_size = "$font.size.md",
						color = "$text.tertiary",
						wrap = true,
						align = "center",
					},
				}),
			}
		end, { w_full = true, h_full = true })
	end

	-- The seam between the two: a hairline, dragged to size the panel
	-- (a double click puts it back).
	local seam_lit = ui.use_state(false)
	local seam_held = ui.use_state(false)
	local seam_drag = seam.drag()
	local function seam_node(): Node
		local start: any = nil
		return ui.div {
			id = "orch-seam",
			w = 1,
			h_full = true,
			shrink = 0,
			bg = "$border",
			hidden = function(): boolean
				return shows() ~= "split"
			end,
			ui.div {
				id = "orch-seam-handle",
				absolute = true,
				top = 0,
				bottom = 0,
				left = ui.raw(-seam.REACH, "the seam's reach each side of the hairline"),
				w = ui.raw(2 * seam.REACH + 1, "the seam's reach each side of the hairline"),
				cursor = "col_resize",
				pointer_events = "auto",
				label = ui.t("orch.page.resize") :: any,
				on_hover = function(inside: boolean)
					seam_lit:set(inside)
				end,
				on_press = function(e: any)
					if e.clicks == 2 then
						panel_state.set(props.of(), "w", nil)
						return
					end
					start = { x = e.x, w = panel_w() }
					seam_held:set(true)
				end,
				on_pointer_move = function(e: any)
					if not start then
						return
					end
					M.resize_to(start.w - (e.x - start.x))
				end,
				on_release = function()
					start = nil
					seam_held:set(false)
					seam_drag.release()
				end,
				ui.div {
					absolute = true,
					top = 0,
					bottom = 0,
					left = ui.raw(seam.REACH - 1, "the lit gap at the hairline"),
					w = 3,
					rounded = "$radius.full",
					pointer_events = "none",
					bg = "$border.strong",
					opacity = seam.lit(function(): boolean
						return seam_lit:get() or seam_held:get()
					end, seam_drag),
				},
			},
		}
	end
	-- (A width asked for, kept within its bounds: the drag's, and tests'.)
	M.resize_to = function(want: number)
		local lo, hi = PANEL_MIN, panel_most()
		seam_drag.at(want, lo, hi)
		panel_state.set(props.of(), "w", math.floor(math.clamp(want, lo, math.max(lo, hi))))
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
				min_w = CHAT_MIN,
				h_full = true,
				hidden = function(): boolean
					return shows() == "live"
				end,
				conversation(),
			},
			seam_node(),
			ui.column {
				id = "orch-side",
				h_full = true,
				min_w = 0,
				shrink = 0,
				grow = function(): number
					return if shows() == "live" then 1 else 0
				end,
				w = function(): number | string
					if shows() ~= "split" then
						return "auto"
					end
					return panel_w()
				end,
				hidden = function(): boolean
					return shows() == "chat"
				end,
				Panel.Panel {
					of = props.of,
					crew = function(): { any }
						return crew:get()
					end,
					needs = function(): number
						return needs:get()
					end,
					name_of = name_of,
					device_name = device_name,
					open_link = open_link,
					place = M.place,
				},
			},
		}
	end

	-- Its flow's run as the editor shows it: its state, the step it's at
	-- and its name, and the steps whose cards are done.
	local function flow_run(): any
		local o = orchestrator()
		local r = o and o.flow_run
		if type(r) ~= "table" then
			return nil
		end
		local done: { [string]: boolean } = {}
		for node, card in r.cards or {} do
			local it = rows.item(card)
			if it and it.state == "done" then
				done[node] = true
			end
		end
		local step: string? = nil
		if r.at and type(r.flow) == "table" then
			for _, n in r.flow.nodes or {} do
				if n.id == r.at then
					step = n.name
				end
			end
		end
		return { state = r.state, at = r.at, step = step, done = done }
	end

	-- Its flow, to edit: the engine's (screens/orchestrator/flows), else as
	-- kept on this device from before.
	local function flow_view(): Node
		local function set(f: any)
			local id = props.of()
			if id then
				flows.set(id, f)
			end
		end
		local function said(key: string, tone: string)
			toast.show { title = locale.s(key), tone = tone :: any }
		end
		return ui.column {
			id = "orch-flow-view",
			w_full = true,
			grow = 1,
			min_h = 0,
			FlowEditor.FlowEditor {
				flow = function(): any
					return flows.of(props.of())
				end,
				on_change = set,
				hue = hue_now,
				scope = props.of,
				models = model_options,
				on_start = function(kind: string)
					set(if kind == "default" then Flow.initial() else Flow.blank())
				end,
				run = flow_run,
				on_run = function(task: string)
					local id = props.of()
					if not id then
						return
					end
					-- (Its last edit there first: the run follows the flow as shown.)
					flows.flush(id)
					orchestration.op({ op = "startFlow", orchestratorId = id, task = task }):after(function() end, function()
						said("orch.flow.run.failed", "danger")
					end)
				end,
				on_stop = function()
					local id = props.of()
					if id then
						orchestration.op({ op = "stopFlow", orchestratorId = id }):after(function() end, function()
							said("orch.flow.run.stop_failed", "danger")
						end)
					end
				end,
				on_import = function()
					flows.import_file(function(f: any, err: string?)
						if f then
							set(f)
							said("orch.flow.imported", "success")
						elseif err then
							said(err, "danger")
						end
					end)
				end,
				on_export = function()
					local f = flows.of(props.of())
					if f then
						flows.export_file(f, function(saved: boolean, err: string?)
							if saved then
								said("orch.flow.exported", "success")
							elseif err then
								said(err, "danger")
							end
						end)
					end
				end,
				on_save_template = function()
					local f = flows.of(props.of())
					if f then
						flows.save_template(f)
						said("orch.flow.template_saved", "success")
					end
				end,
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
			return ui.column {
				w_full = true,
				h_full = true,
				head(),
				ui.div { w_full = true, h = 1, shrink = 0, bg = "$border", hidden = function(): boolean
					return M.view:get() ~= "flow"
				end },
				ui.column {
					w_full = true,
					grow = 1,
					min_h = 0,
					hidden = function(): boolean
						return M.view:get() == "flow"
					end,
					body(),
				},
				ui.show(function(): boolean
					return M.view:get() == "flow"
				end, flow_view()),
			}
		end, { w_full = true, h_full = true }),
		-- An ask expanded from Needs you, whole (as on the Control room).
		NeedsYou.Decision { name_of = name_of, hue_of = hue_of, on_link = open_link },
	}
end)

-- (Its prop is `of`: a component's `id` is its node's.)
function M.Page(props: { id: () -> string? }): Node
	return Page { of = props.id }
end

return M
