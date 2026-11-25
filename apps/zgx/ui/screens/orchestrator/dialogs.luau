-- The orchestrator's dialogs, one of each for the window (placed by the
-- shell): New orchestrator (a name, and where it runs if you like), and an
-- orchestrator's settings (a sheet: its goal and rules, its crew's rules,
-- its notes, how its crew runs, when it wakes). Opened through
-- screens/orchestrator/actions (`actions.new()`, `actions.settings(id)`).
--
--   Dialogs()    both, placed once

local ds = require("@ds")
local locale = require("@ds/locale")
local orchestration = require("@account/orchestration")
local sessions = require("@account/sessions")
local store = require("@app/screens/chat/store")
local actions = require("@app/screens/orchestrator/actions")
local flows = require("@app/screens/orchestrator/flows")
local Flow = require("@ds/orch_flow")
local OrchChips = require("@ds/orch_chips")
local data = require("@ds/orch_data")

local M = {}

local NONE = "none"

local function trim(s: string): string
	return string.match(s, "^%s*(.-)%s*$") or ""
end

-- The models the composer's device offers, grouped by agent: `harness/model`
-- values, as the catalog has them.
local function model_options(): { any }
	local cat = store.catalog:get()
	local labels: { [string]: string } = {}
	for _, h in cat.harnesses or {} do
		labels[h.id] = h.label
	end
	local out: { any } = { { value = NONE, label = locale.s("orch.dialog.model_default") } }
	for _, m in cat.models or {} do
		table.insert(out, { value = m.value, label = m.label, group = labels[m.harness] or m.harness })
	end
	return out
end

-- `harness/model` as its two halves.
local function split_model(v: string?): (string?, string?)
	if not v or v == NONE then
		return nil, nil
	end
	local h, m = string.match(v, "^([^/]+)/(.+)$")
	if h then
		return h, m
	end
	return nil, v
end

local function device_options(with_none: boolean): { any }
	local out: { any } = {}
	if with_none then
		table.insert(out, { value = NONE, label = locale.s("orch.dialog.device_here") })
	end
	for _, d in sessions.now:get().devices or {} do
		if not d.cloud then
			table.insert(out, { value = d.id, label = d.name })
		end
	end
	return out
end

-- Where it works: a project (its device with it), or no project on one of
-- the computers. Grouped under each device's name, a folder for a project
-- and a computer for none (no device to say twice in a row's words).
-- NONE is no project here; `device:<id>`, no project on that device.
local DEVICE = "device:"
local function project_options(): { any }
	local now = sessions.now:get()
	local here = now.here
	local out: { any } = {}
	local named: { [string]: string } = {}
	local order: { any } = {}
	for _, d in now.devices or {} do
		if not d.cloud then
			named[d.id] = d.name
			-- (This computer first.)
			if d.id == here then
				table.insert(order, 1, d)
			else
				table.insert(order, d)
			end
		end
	end
	for _, d in order do
		table.insert(out, {
			value = if d.id == here then NONE else DEVICE .. d.id,
			label = locale.s("orch.dialog.project_none"),
			icon = "monitor",
			group = d.name,
		})
		for _, p in now.projects or {} do
			if p.device == d.id then
				table.insert(out, { value = p.id, label = p.name, icon = "folder", group = d.name })
			end
		end
	end
	-- (No devices known yet: the one choice there always is.)
	if #out == 0 then
		table.insert(out, { value = NONE, label = locale.s("orch.dialog.project_none"), icon = "monitor" })
	end
	return out
end

-- The device a project value puts it on (nil: this one).
local function device_of(value: string): string?
	if value == NONE then
		return sessions.now:peek().here
	end
	if string.sub(value, 1, #DEVICE) == DEVICE then
		return string.sub(value, #DEVICE + 1)
	end
	for _, p in sessions.now:peek().projects or {} do
		if p.id == value then
			return p.device
		end
	end
	return sessions.now:peek().here
end

-- A device's name.
local function device_name(id: string?): string
	for _, d in sessions.now:get().devices or {} do
		if d.id == id then
			return d.name
		end
	end
	return id or ""
end

-- The crew's choices: the orchestrator's own model, or another.
local SAME = "same"
local function crew_model_options(): { any }
	local out = model_options()
	out[1] = { value = SAME, label = locale.s("orch.dialog.crew_model_same") }
	return out
end

-- A model value's name, as its picker says it.
local function model_label(v: string): string
	for _, o in model_options() do
		if o.value == v then
			return o.label
		end
	end
	return v
end

-- The window's height in points (800 where a host gives none: a headless
-- test), as ds/orch_decision reads it.
local function window_height(): number
	local window = (ui :: any).window
	local state = if type(window) == "table" or type(window) == "userdata" then window.size else nil
	local ok, v = pcall(function(): any
		return if state then state:get() else nil
	end)
	local h = if ok and type(v) == "table" and type(v.height) == "number" then v.height else 0
	return if h > 1 then h else 800
end

-- What the dialog's title, its line, its foot and its insets take: the
-- fields scroll in the rest of the window.
local DIALOG_CHROME = 320

-- Crew threads at once, to start with.
local AT_ONCE = 3

-- A request id for one opening's create: the engine answers the same id
-- again with the orchestrator the first made.
local requests = 0
local function request_id(): string
	requests += 1
	return string.format("orch-new-%d-%d-%d", os.time(), math.floor(os.clock() * 1e6), requests)
end

-- New orchestrator, in the order it's thought of: its name, what it's for,
-- where it works and its model, where its crew runs and how many at once.
-- Its rules, its crew's, its crew's own model and the flow it starts from
-- are folded under "optional". Created, its page opens with its composer
-- taking the keys.
--
-- Create waits for a name, and says so. Only Create (its click, or Enter)
-- sends it, once an opening: leaving a field never does (zgx's input
-- commits on blur), and a second Enter or click while, or after, the first
-- is answered asks nothing more. Closed unsent (Cancel, Escape, a click
-- outside), what was typed is there again as it reopens; sent, it starts
-- empty.
local function NewDialog(): Node
	local name = ui.state("orch_new_name", "")
	local goal = ui.state("orch_new_goal", "")
	local project = ui.state("orch_new_project", NONE :: string)
	local model = ui.state("orch_new_model", NONE :: string)
	-- Where its crew runs, as chosen (nil: untouched, its own device).
	local devices = ui.state("orch_new_devices", nil :: { [string]: boolean }?)
	local limit = ui.state("orch_new_limit", AT_ONCE)
	local more = ui.state("orch_new_more", false)
	local rules = ui.state("orch_new_rules", "")
	local crew_rules = ui.state("orch_new_crew_rules", "")
	local crew_model = ui.state("orch_new_crew_model", SAME :: string)
	local busy = ui.state("orch_new_busy", false)
	-- How tall its fields stand (the dialog scrolls them past the window).
	local fields_h = ui.state("orch_new_fields_h", 480)
	-- What its flow starts from (screens/orchestrator/flows' starters), and
	-- a flow read from a file, when that's the one.
	local start = ui.state("orch_new_start", flows.BLANK :: string)
	local imported = ui.state("orch_new_imported", nil :: any)
	local import_error = ui.state("orch_new_import_error", "")
	-- This opening's request: set as it opens, cleared once it's sent.
	local request = ui.state("orch_new_request", "")
	-- Opened: on the project or device it was asked from, the draft kept.
	ui.effect(function()
		if actions.new_open:get() then
			local seed = actions.new_seed:peek()
			ui.batch(function()
				if seed then
					if seed.project then
						project:set(seed.project)
					elseif seed.device and seed.device ~= sessions.now:peek().here then
						project:set(DEVICE .. seed.device)
					else
						project:set(NONE)
					end
				end
				busy:set(false)
				request:set(request_id())
			end)
		end
	end)
	local function named(): boolean
		return trim(name:get()) ~= ""
	end
	-- Where it runs: the crew's devices shown chosen. None chosen is its
	-- own device (the engine's empty list), so that one shows ticked.
	local function own(): string?
		return device_of(project:get())
	end
	local function shown(): { [string]: boolean }
		local d = devices:get()
		if d and next(d) ~= nil then
			return d
		end
		local mine = own()
		return if mine then { [mine] = true } else {}
	end
	local function go()
		local n = trim(name:peek())
		local req = request:peek()
		if n == "" or busy:peek() or req == "" or not actions.new_open:peek() then
			return
		end
		ui.batch(function()
			busy:set(true)
			-- (Spent: nothing more is sent from this opening.)
			request:set("")
		end)
		local harness, m = split_model(model:peek())
		local p = project:peek()
		local space, device = nil, nil
		if string.sub(p, 1, #DEVICE) == DEVICE then
			device = string.sub(p, #DEVICE + 1)
		elseif p ~= NONE then
			space = p
		end
		-- Its crew: where (only when it isn't just its own device), how many
		-- at once (0: no limit), and its model (the same: none sent).
		local chosen: { string } = {}
		local mine = device_of(p)
		for d, on in devices:peek() or {} do
			if on then
				table.insert(chosen, d)
			end
		end
		table.sort(chosen)
		if #chosen == 1 and chosen[1] == mine then
			chosen = {}
		end
		local crew_harness, crew_m = nil, nil
		if crew_model:peek() ~= SAME then
			crew_harness, crew_m = split_model(crew_model:peek())
		end
		local n_limit = limit:peek()
		local chosen_flow = if start:peek() == flows.IMPORTED then imported:peek() else flows.starter(start:peek())
		actions
			.create({
				name = string.sub(n, 1, 80),
				goal = trim(goal:peek()),
				project = space,
				device = device,
				harness = harness,
				model = m,
				rules = trim(rules:peek()),
				crew_rules = trim(crew_rules:peek()),
				crew = {
					devices = chosen,
					max_running = if n_limit > 0 then n_limit else nil,
					harness = crew_harness,
					model = crew_m,
				},
				request_id = req,
			})
			:after(function(reply: any)
				-- Its flow, kept for it by its id.
				local id = if type(reply) == "table" then reply.id elseif type(reply) == "string" then reply else nil
				if chosen_flow and type(id) == "string" then
					flows.set(id, chosen_flow)
					-- And its plan in its notes, to follow until the engine
					-- routes it: under any notes it has, never over them.
					local had: string? = nil
					for _, o in (orchestration.now:peek() or {}).orchestrators or {} do
						if o.id == id then
							had = o.notes
						end
					end
					actions.update(id, { notes = Flow.with_plan(had, chosen_flow) })
				end
				ui.batch(function()
					name:set("")
					goal:set("")
					project:set(NONE)
					model:set(NONE)
					devices:set(nil)
					limit:set(AT_ONCE)
					more:set(false)
					rules:set("")
					crew_rules:set("")
					crew_model:set(SAME)
					start:set(flows.BLANK)
					imported:set(nil)
					import_error:set("")
					busy:set(false)
					actions.new_open:set(false)
				end)
			end, function()
				-- (Not made: the same request may go again, answered once.)
				ui.batch(function()
					busy:set(false)
					request:set(req)
				end)
			end)
	end
	local function half(node: Node): Node
		return ui.column { grow = 1, basis = 0, min_w = 0, node }
	end
	-- The optional part, folded: its rules, its crew's, its crew's model,
	-- and the flow it starts from.
	local optional = ui.column {
		id = "orch-new-optional",
		w_full = true,
		gap = "$space.4",
		pt = "$space.2",
		ds.TextArea {
			value = rules,
			label = ui.t("orch.dialog.rules") :: any,
			placeholder = ui.t("orch.dialog.rules_placeholder"),
			hint = ui.t("orch.dialog.rules_hint") :: any,
			rows = 2,
			max_rows = 6,
			w_full = true,
		},
		ds.TextArea {
			value = crew_rules,
			label = ui.t("orch.dialog.crew_rules") :: any,
			placeholder = ui.t("orch.dialog.crew_rules_placeholder"),
			hint = ui.t("orch.dialog.crew_rules_hint") :: any,
			rows = 2,
			max_rows = 6,
			w_full = true,
		},
		ui.row {
			w_full = true,
			gap = "$space.3",
			items = "end",
			half(ds.Select { value = crew_model, label = ui.t("orch.dialog.crew_model") :: any, options = crew_model_options, w_full = true }),
			-- What its flow starts from: nothing, Plan, Build, Review, a saved
			-- template, or a team's JSON file.
			half(ui.row {
				id = "orch-new-start",
				w_full = true,
				gap = "$space.2",
				items = "end",
				ui.column { grow = 1, min_w = 0, ds.Select {
					value = start,
					label = ui.t("orch.dialog.start_from") :: any,
					options = function(): { any }
						local out = flows.starters()
						local f = imported:get()
						if f then
							table.insert(out, { value = flows.IMPORTED, label = f.name })
						end
						return out
					end,
					w_full = true,
				} },
				ds.Button {
					ui.t("orch.dialog.import_flow") :: any,
					id = "orch-new-import",
					tone = "ghost",
					on_click = function()
						import_error:set("")
						flows.import_file(function(f: any, err: string?)
							ui.batch(function()
								if f then
									imported:set(f)
									start:set(flows.IMPORTED)
								elseif err then
									import_error:set(err)
								end
							end)
						end)
					end,
				},
			}),
		},
		ui.show(function(): boolean
			return import_error:get() ~= ""
		end, ds.Text {
			function(): string
				local key = import_error:get()
				return if key ~= "" then locale.s(key) else ""
			end,
			style = "caption",
			tone = "danger",
			wrap = true,
		}),
	}
	return ds.Dialog {
		open = actions.new_open,
		title = ui.t("orch.dialog.new_title") :: any,
		body = ui.t("orch.dialog.new_body") :: any,
		size = "lg",
		confirm = {
			label = ui.t("orch.dialog.create_open") :: any,
			on_click = go,
			keep_open = true,
			busy = function(): boolean
				return busy:get()
			end,
			-- (Nothing to call it yet: the name's line says so.)
			disabled = function(): boolean
				return not named()
			end,
		},
		-- (Taller than the window, with its optional part open: its fields
		-- scroll, its title and buttons staying put.)
		-- (A scroll is 240 high unless sized: as tall as its fields, up to
		-- the room the window leaves.)
		ui.scroll {
			w_full = true,
			h = function(): number
				return math.min(fields_h:get(), math.max(240, window_height() - DIALOG_CHROME))
			end,
			fade = { 12, 12 },
			fade_overflow = true,
			ui.column {
				on_resize = function(size: any)
					if size.h and size.h > 0 then
						fields_h:set(size.h)
					end
				end,
				id = "orch-new-dialog",
				w_full = true,
				gap = "$space.4",
				-- Its name, and while there's none, why Create waits. (Its own
				-- line, gone once it's named: a field's hint keeps its room.)
				ui.column {
					w_full = true,
					gap = "$zeron.space.tight",
					ds.TextField {
						value = name,
						label = ui.t("orch.dialog.name"),
						placeholder = function(): string
							return locale.s("orch.dialog.name_placeholder")
						end,
						autofocus = true,
						w_full = true,
						-- (Enter is the dialog's own: its Create.)
					},
					ui.show(function(): boolean
						return not named()
					end, ui.div { id = "orch-new-name-needed", ds.Hint { ui.t("orch.dialog.name_needed") :: any } }),
				},
				ds.TextArea {
					value = goal,
					label = ui.t("orch.dialog.what_for") :: any,
					placeholder = ui.t("orch.dialog.what_for_placeholder") :: any,
					hint = ui.t("orch.dialog.what_for_hint") :: any,
					rows = 2,
					max_rows = 6,
					w_full = true,
				},
				ui.row {
					w_full = true,
					gap = "$space.3",
					items = "start",
					half(ds.Select {
						value = project,
						label = ui.t("orch.dialog.project") :: any,
						options = project_options,
						hint = function(): string
							return locale.s("orch.dialog.on_device", { device = device_name(own()) })
						end,
						w_full = true,
					}),
					half(ds.Select {
						value = model,
						label = ui.t("orch.dialog.model") :: any,
						options = model_options,
						hint = function(): string
							local c = crew_model:get()
							return if c == SAME
								then locale.s("orch.dialog.crew_same")
								else locale.s("orch.dialog.crew_other", { model = model_label(c) })
						end,
						w_full = true,
					}),
				},
				ui.row {
					w_full = true,
					gap = "$space.4",
					items = "end",
					ui.column {
						grow = 1,
						min_w = 0,
						OrchChips {
							ids = "orch-new-devices",
							label = ui.t("orch.dialog.crew_runs_on") :: any,
							chosen = shown,
							on_change = function(set: { [string]: boolean })
								-- (None left is its own again.)
								devices:set(if next(set) ~= nil then set else nil)
							end,
							options = function(): { any }
								return device_options(false)
							end,
						},
					},
					ds.NumberStepper {
						value = limit,
						min = 0,
						max = 32,
						w = 132,
						label = ui.t("orch.dialog.max_running") :: any,
						zero_label = function(): string
							return locale.s("orch.dialog.no_limit")
						end,
					},
				},
				-- The rest, folded until asked for.
				ui.column {
					w_full = true,
					border_sides = { 1, 0, 0, 0 },
					border_color = "$border.subtle",
					pt = "$space.3",
					ui.button {
						id = "orch-new-more",
						role = "button",
						expanded = function(): boolean
							return more:get()
						end,
						label = ui.t("orch.dialog.more") :: any,
						w_full = true,
						h = 28,
						gap = "$space.2",
						items = "center",
						rounded = "$radius.md",
						bg = "#00000000",
						cursor = "pointer",
						color = "$text.secondary",
						hover = { color = "$text.primary" },
						on_click = function()
							more:set(not more:peek())
						end,
						ds.Icon {
							name = function(): any
								return if more:get() then "alt-arrow-down" else "alt-arrow-right"
							end,
							size = 14,
						},
						ui.text { ui.t("orch.dialog.more") :: any, grow = 1, text_size = "$font.size.md" },
						ui.text { ui.t("orch.dialog.optional") :: any, text_size = "$font.size.sm", color = "$text.tertiary" },
					},
					ui.show(function(): boolean
						return more:get()
					end, optional),
				},
			},
		},
	}
end

-- A field of the orchestrator open in settings, as it is now.
local function of(id: string?): any
	if not id then
		return nil
	end
	for _, o in (orchestration.now:peek() or {}).orchestrators or {} do
		if o.id == id then
			return o
		end
	end
	return nil
end

-- "09:00, 14:30" as times; nil where one doesn't read as HH:MM.
local function times(text: string): { string }?
	local out: { string } = {}
	for part in string.gmatch(text, "[^,%s]+") do
		local h, m = string.match(part, "^(%d%d?):(%d%d)$")
		local hn, mn = tonumber(h), tonumber(m)
		if not hn or not mn or hn > 23 or mn > 59 then
			return nil
		end
		table.insert(out, string.format("%02d:%02d", hn, mn))
	end
	return out
end

-- Its settings: what it is for and its rules, its crew's rules, its notes
-- (its plan, which it keeps too), where and how its crew runs, and when it
-- wakes on its own. Saved together; only what changed is sent.
-- The device an orchestrator runs on (its chat's).
local function own_device(id: string?): string?
	if not id then
		return nil
	end
	local c = data.chats(sessions.now:get())[id]
	return c and c.device or nil
end

-- A crew's devices as the engine reads them: just its own device is none
-- chosen (both run it there).
local function crew_devices(list: { string }, own: string?): { string }
	local out = table.clone(list)
	table.sort(out)
	if #out == 1 and out[1] == own then
		return {}
	end
	return out
end

local function SettingsSheet(): Node
	local name = ui.state("orch_set_name", "")
	local goal = ui.state("orch_set_goal", "")
	local rules = ui.state("orch_set_rules", "")
	local crew_rules = ui.state("orch_set_crew_rules", "")
	local notes = ui.state("orch_set_notes", "")
	local devices = ui.state("orch_set_devices", {} :: { [string]: boolean })
	local limit = ui.state("orch_set_limit", 0)
	local model = ui.state("orch_set_model", NONE :: string)
	-- When its account hits its usage limit: continue on another account
	-- (and, with none, another harness or model); off, wait for the reset.
	local on_account = ui.state("orch_set_on_limit_account", false)
	local on_harness = ui.state("orch_set_on_limit_harness", false)
	local wake_on = ui.state("orch_set_wake_on", false)
	local every = ui.state("orch_set_every", 0)
	local at = ui.state("orch_set_at", "")
	local weekdays = ui.state("orch_set_weekdays", false)
	local prompt = ui.state("orch_set_prompt", "")
	local at_error = ui.state("orch_set_at_error", false)
	-- Opened: the orchestrator's fields as they are.
	ui.effect(function()
		if not actions.settings_open:get() then
			return
		end
		local o = of(actions.settings_of:peek())
		if not o then
			return
		end
		local crew = o.crew or {}
		local chosen: { [string]: boolean } = {}
		for _, d in crew.devices or {} do
			chosen[d] = true
		end
		local w = o.wake
		ui.batch(function()
			name:set(o.name or "")
			goal:set(o.goal or "")
			rules:set(o.rules or "")
			crew_rules:set(o.crew_rules or "")
			notes:set(o.notes or "")
			devices:set(chosen)
			limit:set(crew.max_running or 0)
			model:set(if crew.model then (if crew.harness then `{crew.harness}/{crew.model}` else crew.model) else NONE)
			local policy = crew.on_limit or {}
			on_account:set(policy.switch_account == true)
			on_harness:set(policy.switch_harness == true)
			wake_on:set(w ~= nil)
			every:set((w and w.every_minutes) or 0)
			at:set(if w and w.at then table.concat(w.at, ", ") else "")
			weekdays:set(w ~= nil and w.weekdays_only == true)
			prompt:set((w and w.prompt) or "")
			at_error:set(false)
		end)
	end)
	local function save()
		local id = actions.settings_of:peek()
		local o = of(id)
		if not id or not o then
			actions.settings_open:set(false)
			return
		end
		local fields: { [string]: any } = {}
		local function put(key: string, now: string, was: string?)
			if now ~= (was or "") then
				fields[key] = now
			end
		end
		local n = trim(name:peek())
		if n ~= "" then
			put("name", string.sub(n, 1, 80), o.name)
		end
		put("goal", trim(goal:peek()), o.goal)
		put("rules", rules:peek(), o.rules)
		put("crewRules", crew_rules:peek(), o.crew_rules)
		put("notes", notes:peek(), o.notes)
		-- The crew's policy, whole, when any of it moved.
		local chosen: { string } = {}
		for d, on in devices:peek() do
			if on then
				table.insert(chosen, d)
			end
		end
		local mine = own_device(id)
		chosen = crew_devices(chosen, mine)
		local harness, m = split_model(model:peek())
		local crew = o.crew or {}
		local was_devices = crew_devices(crew.devices or {}, mine)
		local n_limit = limit:peek()
		local policy = crew.on_limit or {}
		local switch_account = on_account:peek()
		-- (Another harness only as a step past another account.)
		local switch_harness = switch_account and on_harness:peek()
		if
			table.concat(chosen, ",") ~= table.concat(was_devices, ",")
			or (if n_limit > 0 then n_limit else nil) ~= crew.max_running
			or m ~= crew.model
			or (m ~= nil and harness ~= crew.harness)
			or switch_account ~= (policy.switch_account == true)
			or switch_harness ~= (policy.switch_harness == true)
		then
			-- (Whole: what's left out is reset, so its limit policy goes too.)
			fields.crew = {
				devices = chosen,
				maxRunning = if n_limit > 0 then n_limit else nil,
				harness = if m then harness else nil,
				model = m,
				onLimit = { switchAccount = switch_account, switchHarness = switch_harness },
			}
		end
		-- When it wakes: off clears it.
		if wake_on:peek() then
			local list = times(at:peek())
			if not list then
				at_error:set(true)
				return
			end
			local mins = every:peek()
			fields.wake = {
				everyMinutes = if mins >= 5 then mins else nil,
				at = list,
				weekdaysOnly = weekdays:peek(),
				prompt = trim(prompt:peek()),
			}
		elseif o.wake ~= nil then
			fields.wake = ui.null
		end
		actions.settings_open:set(false)
		if next(fields) ~= nil then
			actions.update(id, fields)
		end
	end
	local function heading(key: string): Node
		return ds.SectionTitle { ui.t(key) :: any }
	end
	return ds.Sheet {
		open = actions.settings_open,
		title = function(): string
			local o = of(actions.settings_of:get())
			return if o then o.name else locale.s("orch.dialog.settings_title")
		end,
		foot = ui.row {
			w_full = true,
			justify = "end",
			gap = "$space.2",
			ds.Button { ui.t("common.cancel") :: any, tone = "ghost", on_click = function()
				actions.settings_open:set(false)
			end },
			ds.Button { ui.t("orch.dialog.save") :: any, id = "orch-settings-save", tone = "primary", on_click = save },
		},
		ui.column {
			id = "orch-settings",
			w_full = true,
			gap = "$space.4",
			ds.TextField { value = name, label = ui.t("orch.dialog.name"), w_full = true },
			ds.TextArea { value = goal, label = ui.t("orch.dialog.what_for") :: any, rows = 2, max_rows = 4, w_full = true },
			-- Who reads each, said under it: the rules only the orchestrator
			-- (its instructions), the crew rules every crew thread (with the
			-- first message, and again when they change), the notes its plan.
			ds.TextArea {
				value = rules,
				label = ui.t("orch.dialog.rules") :: any,
				placeholder = ui.t("orch.dialog.rules_placeholder"),
				hint = ui.t("orch.dialog.rules_hint") :: any,
				rows = 3,
				max_rows = 10,
				w_full = true,
			},
			ds.TextArea {
				value = crew_rules,
				label = ui.t("orch.dialog.crew_rules") :: any,
				placeholder = ui.t("orch.dialog.crew_rules_placeholder"),
				hint = ui.t("orch.dialog.crew_rules_hint") :: any,
				rows = 3,
				max_rows = 10,
				w_full = true,
			},
			ds.TextArea {
				value = notes,
				label = ui.t("orch.dialog.notes") :: any,
				placeholder = ui.t("orch.dialog.notes_placeholder"),
				hint = ui.t("orch.dialog.notes_hint") :: any,
				rows = 4,
				max_rows = 14,
				w_full = true,
			},
			heading("orch.dialog.crew"),
			ui.column {
				w_full = true,
				gap = "$space.2",
				ds.Text { ui.t("orch.dialog.devices") :: any, style = "caption", tone = "secondary" },
				-- (None chosen is its own device, the engine's empty list: that
				-- one shows ticked, never "runs nowhere".)
				ui.each(ui.computed(function(): { any }
					return device_options(false)
				end), function(d: any): Node
					local function shown(): { [string]: boolean }
						local set = devices:get()
						if next(set) ~= nil then
							return set
						end
						local mine = own_device(actions.settings_of:get())
						return if mine then { [mine] = true } else {}
					end
					return ds.Checkbox {
						label = d.label,
						value = function(): boolean
							return shown()[d.value] == true
						end,
						on_change = function(on: boolean)
							local now = table.clone(shown())
							now[d.value] = if on then true else nil
							devices:set(now)
						end,
					}
				end, { key = function(d: any): string
					return d.value
				end }),
			},
			ui.row {
				w_full = true,
				gap = "$space.3",
				items = "end",
				ds.NumberStepper {
					value = limit,
					min = 0,
					max = 32,
					w = 132,
					label = ui.t("orch.dialog.max_running") :: any,
					-- (0 is none: the engine's no limit.)
					zero_label = function(): string
						return locale.s("orch.dialog.no_limit")
					end,
				},
				ui.column { grow = 1, min_w = 0, ds.Select { value = model, label = ui.t("orch.dialog.crew_model") :: any, options = model_options, w_full = true } },
			},
			-- When limited: another account, and past that (only with it on)
			-- another harness or model.
			ui.column {
				id = "orch-settings-on-limit",
				w_full = true,
				gap = "$space.2",
				ds.Checkbox { value = on_account, label = ui.t("orch.dialog.on_limit_account") :: any },
				ui.presence(function(): string
					return if on_account:get() then "on" else "off"
				end, function(k: string): Node
					return ui.div {
						id = "orch-settings-on-limit-harness",
						pl = "$space.5",
						ds.Checkbox {
							value = function(): boolean
								return k == "on" and on_harness:get()
							end,
							on_change = function(v: boolean)
								on_harness:set(v)
							end,
							disabled = k ~= "on",
							label = ui.t("orch.dialog.on_limit_harness") :: any,
						},
					}
				end),
			},
			heading("orch.dialog.wake"),
			ds.Switch { value = wake_on, label = ui.t("orch.dialog.wake_on") },
			ui.show(function(): boolean
				return wake_on:get()
			end, ui.column {
				w_full = true,
				gap = "$space.3",
				ui.row {
					w_full = true,
					gap = "$space.3",
					items = "start",
					ui.column { grow = 1, min_w = 0, ds.TextField {
						value = at,
						label = ui.t("orch.dialog.wake_at"),
						placeholder = function(): string
							return "09:00, 14:00"
						end,
						error = function(): string?
							return if at_error:get() then locale.s("orch.dialog.wake_at_error") else nil
						end,
						w_full = true,
					} },
					ds.NumberStepper { value = every, min = 0, max = 1440, step = 5, unit = ui.t("orch.dialog.minutes") :: any, label = ui.t("orch.dialog.wake_every") :: any },
				},
				ds.Checkbox { value = weekdays, label = ui.t("orch.dialog.weekdays") :: any },
				ds.TextField { value = prompt, label = ui.t("orch.dialog.wake_prompt"), w_full = true },
			}),
		},
	}
end

function M.Dialogs(): Node
	return ui.div {
		id = "orch-dialogs",
		absolute = true,
		left = 0,
		top = 0,
		NewDialog(),
		SettingsSheet(),
	}
end

return M
