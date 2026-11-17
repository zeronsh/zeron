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

local function project_options(): { any }
	local out: { any } = { { value = NONE, label = locale.s("orch.dialog.project_none") } }
	local devices: { [string]: string } = {}
	for _, d in sessions.now:get().devices or {} do
		devices[d.id] = d.name
	end
	for _, p in sessions.now:get().projects or {} do
		table.insert(out, { value = p.id, label = p.name, description = devices[p.device] })
	end
	return out
end

-- A request id for one opening's create: the engine answers the same id
-- again with the orchestrator the first made.
local requests = 0
local function request_id(): string
	requests += 1
	return string.format("orch-new-%d-%d-%d", os.time(), math.floor(os.clock() * 1e6), requests)
end

-- New orchestrator: a name (all it needs), its goal, the project and
-- device it works in, its model. Created, it opens.
--
-- Only Create (its click, or Enter) sends it, once an opening: leaving a
-- field never does (zgx's input commits on blur), and a second Enter or
-- click while, or after, the first is answered asks nothing more. Closed
-- unsent (Cancel, Escape, a click outside), what was typed is there again
-- as it reopens; sent, it starts empty.
local function NewDialog(): Node
	local name = ui.state("orch_new_name", "")
	local goal = ui.state("orch_new_goal", "")
	local project = ui.state("orch_new_project", NONE :: string)
	local device = ui.state("orch_new_device", NONE :: string)
	local model = ui.state("orch_new_model", NONE :: string)
	local busy = ui.state("orch_new_busy", false)
	-- What its flow starts from (screens/orchestrator/flows' starters), and
	-- a flow read from a file, when that's the one.
	local start = ui.state("orch_new_start", flows.BLANK :: string)
	local imported = ui.state("orch_new_imported", nil :: any)
	local import_error = ui.state("orch_new_import_error", "")
	-- This opening's request: set as it opens, cleared once it's sent.
	local request = ui.state("orch_new_request", "")
	-- Opened: on the project and device it was asked from, the draft kept.
	ui.effect(function()
		if actions.new_open:get() then
			local seed = actions.new_seed:peek()
			ui.batch(function()
				if seed then
					project:set(seed.project or NONE)
					device:set(seed.device or NONE)
				end
				busy:set(false)
				request:set(request_id())
			end)
		end
	end)
	-- A project fixes its device.
	ui.effect(function()
		local p = project:get()
		if p == NONE then
			return
		end
		for _, x in sessions.now:peek().projects or {} do
			if x.id == p and x.device and device:peek() ~= x.device then
				device:set(x.device)
			end
		end
	end)
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
		local chosen = if start:peek() == flows.IMPORTED then imported:peek() else flows.starter(start:peek())
		actions
			.create({
				name = string.sub(n, 1, 80),
				goal = trim(goal:peek()),
				project = if project:peek() ~= NONE then project:peek() else nil,
				device = if device:peek() ~= NONE then device:peek() else nil,
				harness = harness,
				model = m,
				request_id = req,
			})
			:after(function(reply: any)
				-- Its flow, kept for it by its id.
				local id = if type(reply) == "table" then reply.id elseif type(reply) == "string" then reply else nil
				if chosen and type(id) == "string" then
					flows.set(id, chosen)
					-- And its plan in its notes, to follow until the engine
					-- routes it: under any notes it has, never over them.
					local had: string? = nil
					for _, o in orchestration.now:peek().orchestrators or {} do
						if o.id == id then
							had = o.notes
						end
					end
					actions.update(id, { notes = Flow.with_plan(had, chosen) })
				end
				ui.batch(function()
					name:set("")
					goal:set("")
					project:set(NONE)
					device:set(NONE)
					model:set(NONE)
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
	-- As Zeron's own "New …" dialogs (a template, a plugin): a title and a
	-- line, its name, then what it's for; where it runs and its model as the
	-- composer's chips do, small, along one row under them.
	return ds.Dialog {
		open = actions.new_open,
		title = ui.t("orch.dialog.new_title") :: any,
		body = ui.t("orch.dialog.new_body") :: any,
		size = "md",
		confirm = {
			label = ui.t("orch.dialog.create") :: any,
			on_click = go,
			keep_open = true,
			busy = function(): boolean
				return busy:get()
			end,
		},
		ui.column {
			id = "orch-new-dialog",
			w_full = true,
			gap = "$space.3",
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
			ds.TextArea {
				value = goal,
				name = ui.t("orch.dialog.goal") :: any,
				placeholder = ui.t("orch.dialog.goal_placeholder") :: any,
				rows = 3,
				max_rows = 6,
				w_full = true,
			},
			ui.row {
				w_full = true,
				gap = "$space.2",
				ui.column { grow = 1, basis = 0, min_w = 0, ds.Select { value = project, name = ui.t("orch.dialog.project") :: any, options = project_options, size = "sm", w_full = true } },
				ui.column { grow = 1, basis = 0, min_w = 0, ds.Select { value = device, name = ui.t("orch.dialog.device") :: any, options = function(): { any }
					return device_options(true)
				end, size = "sm", w_full = true, disabled = function(): boolean
					return project:get() ~= NONE
				end } },
				ui.column { grow = 1, basis = 0, min_w = 0, ds.Select { value = model, name = ui.t("orch.dialog.model") :: any, options = model_options, size = "sm", w_full = true } },
			},
			-- What its flow starts from: nothing, Plan → Build → Review, a
			-- saved template, or a team's JSON file.
			ui.row {
				id = "orch-new-start",
				w_full = true,
				gap = "$space.2",
				items = "center",
				ui.column { grow = 1, min_w = 0, ds.Select {
					value = start,
					name = ui.t("orch.dialog.start_from") :: any,
					options = function(): { any }
						local out = flows.starters()
						local f = imported:get()
						if f then
							table.insert(out, { value = flows.IMPORTED, label = f.name })
						end
						return out
					end,
					size = "sm",
					w_full = true,
				} },
				ds.Button {
					ui.t("orch.dialog.import_flow") :: any,
					id = "orch-new-import",
					tone = "ghost",
					size = "sm",
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
		},
	}
end

-- A field of the orchestrator open in settings, as it is now.
local function of(id: string?): any
	if not id then
		return nil
	end
	for _, o in orchestration.now:peek().orchestrators or {} do
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
local function SettingsSheet(): Node
	local name = ui.state("orch_set_name", "")
	local goal = ui.state("orch_set_goal", "")
	local rules = ui.state("orch_set_rules", "")
	local crew_rules = ui.state("orch_set_crew_rules", "")
	local notes = ui.state("orch_set_notes", "")
	local devices = ui.state("orch_set_devices", {} :: { [string]: boolean })
	local limit = ui.state("orch_set_limit", 0)
	local model = ui.state("orch_set_model", NONE :: string)
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
		table.sort(chosen)
		local harness, m = split_model(model:peek())
		local crew = o.crew or {}
		local was_devices = table.clone(crew.devices or {})
		table.sort(was_devices)
		local n_limit = limit:peek()
		if
			table.concat(chosen, ",") ~= table.concat(was_devices, ",")
			or (if n_limit > 0 then n_limit else nil) ~= crew.max_running
			or m ~= crew.model
			or (m ~= nil and harness ~= crew.harness)
		then
			fields.crew = {
				devices = chosen,
				maxRunning = if n_limit > 0 then n_limit else nil,
				harness = if m then harness else nil,
				model = m,
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
			ds.TextArea { value = goal, label = ui.t("orch.dialog.goal") :: any, rows = 2, max_rows = 4, w_full = true },
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
				ui.each(ui.computed(function(): { any }
					return device_options(false)
				end), function(d: any): Node
					return ds.Checkbox {
						label = d.label,
						value = function(): boolean
							return devices:get()[d.value] == true
						end,
						on_change = function(on: boolean)
							local now = table.clone(devices:peek())
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
				ds.NumberStepper { value = limit, min = 0, max = 32, label = ui.t("orch.dialog.max_running") :: any },
				ui.column { grow = 1, min_w = 0, ds.Select { value = model, label = ui.t("orch.dialog.crew_model") :: any, options = model_options, w_full = true } },
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
