-- Scheduled Tasks, as the engine's scheduled service has them.
--
-- The page is a view of the headless service in
-- `plugins-builtin/scheduled/service.luau`: that service keeps the tasks,
-- runs them at their exact time (sleeping until then, not polling), sends
-- their prompt into a chat, follows the turn, and keeps the history. Here
-- is only what the window reads and the calls it makes; no demo runs.

local sessions = require("@account/sessions")

local M = {}

-- The service's one action (plugins/mod.rs: `plugin.<id>.service_call`).
local ACTION = "plugin.scheduled.service_call"

M.tab = ui.state("scheduled_tab", "upcoming")
M.query = ui.state("scheduled_query", "")
M.selected = ui.state("scheduled_selected", nil :: string?)
M.tasks = ui.state("scheduled_live_tasks", {} :: { any })
M.timezone = ui.state("scheduled_timezone", "local time")
M.loading = ui.state("scheduled_loading", true)
M.error = ui.state("scheduled_error", nil :: string?)
M.saving = ui.state("scheduled_saving", false)

M.create_open = ui.state("scheduled_create_open", false)
M.create_step = ui.state("scheduled_create_step", "details")
M.editing = ui.state("scheduled_editing", nil :: string?)
M.delete_open = ui.state("scheduled_delete_open", false)
M.delete_target = ui.state("scheduled_delete_target", nil :: string?)

M.new_title = ui.state("scheduled_new_title", "")
M.new_prompt = ui.state("scheduled_new_prompt", "")
M.new_repeat = ui.state("scheduled_new_repeat", "weekdays")
M.new_target = ui.state("scheduled_new_target", "new")
M.new_target_name = ui.state("scheduled_new_target_name", "New chat")
M.new_custom_repeat = ui.state("scheduled_new_custom_repeat", "daily")
M.new_days = ui.state("scheduled_new_days", { 1 } :: { number })
M.new_hour = ui.state("scheduled_new_hour", 9)
M.new_minute = ui.state("scheduled_new_minute", 0)
M.new_period = ui.state("scheduled_new_period", "AM")
M.new_model = ui.state("scheduled_new_model", nil :: string?)
M.new_effort = ui.state("scheduled_new_effort", nil :: string?)

-- A dialog is a moment, not a place: it doesn't come back with the app.
ui.after(0, function()
	if M.create_open:peek() then
		M.create_open:set(false)
	end
	if M.delete_open:peek() then
		M.delete_open:set(false)
	end
end)

-- The app's model catalog, shared with the composer (src/chat.rs fills it):
-- each model's `value` is "<harness>/<model id>", its `harness` names the
-- agent, and `provider` says where it is served from.
M.catalog = ui.shared("chat.catalog", { state = "looking", harnesses = {}, models = {} } :: any)

function M.load_models()
	if (M.catalog:peek() or {}).state ~= "ready" then
		ui.invoke("chat.catalog")
	end
end

M.REPEATS = {
	{ value = "once", label = "Once" },
	{ value = "daily", label = "Every day" },
	{ value = "weekdays", label = "Weekdays" },
	{ value = "weekly", label = "Weekly" },
	{ value = "custom", label = "Custom" },
}

M.REPEAT_TEXT = {
	once = "Once, tomorrow at 09:00 AM",
	daily = "Every day at 09:00 AM",
	weekdays = "Every weekday at 09:00 AM",
	weekly = "Every Monday at 09:00 AM",
	custom = "Custom schedule",
}

M.DAYS = {
	{ value = 1, label = "Mon", name = "Monday" },
	{ value = 2, label = "Tue", name = "Tuesday" },
	{ value = 3, label = "Wed", name = "Wednesday" },
	{ value = 4, label = "Thu", name = "Thursday" },
	{ value = 5, label = "Fri", name = "Friday" },
	{ value = 6, label = "Sat", name = "Saturday" },
	{ value = 7, label = "Sun", name = "Sunday" },
}

local function day_at(days: { number }, day: number): number?
	for i, value in days do
		if value == day then
			return i
		end
	end
	return nil
end

function M.toggle_day(day: number)
	local days = table.clone(M.new_days:peek())
	local at = day_at(days, day)
	if at then
		table.remove(days, at)
	else
		table.insert(days, day)
		table.sort(days)
	end
	-- One day at least: a schedule with no day would never run.
	if #days == 0 then
		return
	end
	M.new_days:set(days)
end

function M.selected_days(): { number }
	return table.clone(M.new_days:peek())
end

function M.day_on(day: number): boolean
	return day_at(M.new_days:get(), day) ~= nil
end

function M.days_text(): string
	local names = {}
	for _, day in M.DAYS do
		if M.day_on(day.value) then
			table.insert(names, day.name)
		end
	end
	if #names == 0 then
		return "Monday"
	elseif #names == 1 then
		return names[1]
	end
	return `{table.concat(names, ", ", 1, #names - 1)} and {names[#names]}`
end

local function trim(text: any): string
	if type(text) ~= "string" then
		return ""
	end
	return string.match(text, "^%s*(.-)%s*$") or ""
end

-- The custom schedule's time, as the dialog and the review say it: a
-- 12-hour clock, minutes padded.
function M.custom_time(): string
	local h = math.clamp(math.floor(M.new_hour:get() + 0.5), 1, 12)
	local m = math.clamp(math.floor(M.new_minute:get() + 0.5), 0, 59)
	return string.format("%02d:%02d %s", h, m, M.new_period:get())
end

-- The hour a task is stored with (0–23), from the dialog's 12-hour clock.
function M.hour24(): number
	local h = math.clamp(math.floor(M.new_hour:get() + 0.5), 1, 12) % 12
	return h + (if M.new_period:get() == "PM" then 12 else 0)
end

-- The whole schedule for the repeat chosen: presets' words, or the custom
-- cadence with the chosen time.
function M.schedule_text(): string
	local kind = M.new_repeat:get()
	if kind ~= "custom" then
		return M.REPEAT_TEXT[kind] or M.REPEAT_TEXT.weekdays
	end
	local at = M.custom_time()
	local cadence = M.new_custom_repeat:get()
	if cadence == "once" then
		return `Once, tomorrow at {at}`
	elseif cadence == "weekdays" then
		return `Every weekday at {at}`
	elseif cadence == "weekly" then
		return `Every Monday at {at}`
	elseif cadence == "days" then
		return `Every {M.days_text()} at {at}`
	end
	return `Every day at {at}`
end

-- ── The service ─────────────────────────────────────────────────────────

local function apply(result: any, focus: any, select: boolean?)
	if type(result) ~= "table" or type(result.tasks) ~= "table" then
		M.error:set("The scheduled service answered something unexpected.")
		M.loading:set(false)
		return
	end
	M.tasks:set(result.tasks)
	M.timezone:set(if type(result.timezone) == "string" and result.timezone ~= "" then result.timezone else "local time")
	M.error:set(nil)
	M.loading:set(false)
	if select ~= false then
		if type(result.task) == "string" and result.task ~= "" then
			M.selected:set(result.task)
		elseif type(focus) == "string" and focus ~= "" then
			M.selected:set(focus)
		end
	end
	local now = M.selected:peek()
	if now and now ~= focus and not M.by_id(now) then
		M.selected:set(nil)
	end
end

local function service(method: string, args: any, done: ((any) -> ())?, failed: ((string) -> ())?)
	local ok, promise = pcall(function()
		return ui.call(ACTION, {
			method = method,
			args = if args == nil then ui.null else args,
			timeout = 30,
		})
	end)
	if not ok then
		if failed then
			failed(tostring(promise))
		end
		return
	end
	promise:after(done, function(err: any)
		if failed then
			failed(tostring(err))
		end
	end)
end

local refreshing = false

-- Ask the service for the tasks. The service is the truth (it runs in the
-- engine with no window); this only mirrors it for the page and sidebar.
function M.refresh(quiet: boolean?)
	if refreshing then
		return
	end
	refreshing = true
	if not quiet then
		M.loading:set(#M.tasks:get() == 0)
	end
	service("list", nil, function(result)
		refreshing = false
		apply(result)
	end, function(err)
		refreshing = false
		M.loading:set(false)
		M.error:set(if #M.tasks:get() == 0
			then "The scheduled service is starting. It retries by itself."
			else err)
	end)
end

-- While the window is open, read faster while a run is live and slowly
-- when everything is quiet: the service does the waking, not this poll.
local polling = false
local function poll()
	if polling then
		return
	end
	polling = true
	local function again()
		local live = false
		for _, task in M.tasks:peek() do
			if task.status == "running" or task.status == "needs_input" then
				live = true
				break
			end
		end
		local wait = if live then 3000 elseif M.error:peek() then 5000 else 60000
		ui.after(wait, function()
			M.refresh(true)
			again()
		end)
	end
	again()
end

ui.after(350, function()
	M.refresh()
	poll()
end)

-- ── Reading the list ────────────────────────────────────────────────────

local function is_upcoming(task: any): boolean
	return task.status == "active" or task.status == "running" or task.status == "needs_input" or task.status == "missed"
end

function M.visible(tab: string): { any }
	local out = {}
	for _, task in M.tasks:get() do
		if (tab == "upcoming" and is_upcoming(task))
			or (tab == "paused" and task.status == "paused")
			or (tab == "finished" and task.status == "finished")
		then
			table.insert(out, task)
		end
	end
	return out
end

function M.by_id(id: string?): any?
	if not id then
		return nil
	end
	for _, task in M.tasks:get() do
		if task.id == id then
			return task
		end
	end
	return nil
end

function M.next_up(): any?
	local best = nil
	for _, task in M.tasks:get() do
		if task.status == "active" and task.next_at then
			if not best or task.next_at < best.next_at then
				best = task
			end
		end
	end
	return best
end

function M.needs_attention(): any?
	for _, task in M.tasks:get() do
		if task.status == "needs_input" then
			return task
		end
	end
	return nil
end

function M.missed_task(): any?
	for _, task in M.tasks:get() do
		if task.status == "missed" then
			return task
		end
	end
	return nil
end

function M.counts(): { active: number, running: number, needs: number, missed: number, paused: number, finished: number }
	local out = { active = 0, running = 0, needs = 0, missed = 0, paused = 0, finished = 0 }
	for _, task in M.tasks:get() do
		if task.status == "active" then
			out.active += 1
		elseif task.status == "running" then
			out.running += 1
		elseif task.status == "needs_input" then
			out.needs += 1
		elseif task.status == "missed" then
			out.missed += 1
		elseif task.status == "paused" then
			out.paused += 1
		elseif task.status == "finished" then
			out.finished += 1
		end
	end
	return out
end

function M.summary(): { count: number, attention: boolean }
	local c = M.counts()
	return { count = c.active + c.running + c.missed, attention = c.needs > 0 or c.missed > 0 }
end

function M.select(id: string)
	M.selected:set(id)
end

function M.clear()
	M.selected:set(nil)
end

-- ── A chat, where a run reports -------------------------------------------------

function M.open_chat(chat: string?)
	if type(chat) == "string" and chat ~= "" then
		ui.invoke("chat.open", chat)
	end
end

function M.open_task(task: any)
	local chat = task and (task.run_chat or task.chat)
	if type(chat) == "string" and chat ~= "" then
		M.open_chat(chat)
	elseif task then
		M.select(task.id)
	end
end

local function project_name(id: string?): string
	if not id then
		return ""
	end
	for _, project in sessions.now:get().projects or {} do
		if project.id == id then
			return project.name
		end
	end
	return ""
end

local function chat_by_id(id: string?): any?
	if not id or id == "new" then
		return nil
	end
	for _, chat in sessions.now:get().chats or {} do
		if chat.id == id then
			return chat
		end
	end
	return nil
end

-- The agent a chosen chat runs: a task reporting there can only start it on
-- that agent's own models (the engine keeps a chat on its agent).
function M.target_harness(): string?
	local chat = chat_by_id(M.new_target:peek())
	if not chat then
		return nil
	end
	return chat.harness or chat.agent
end

local function harness_map(): { [string]: any }
	local out: { [string]: any } = {}
	for _, h in M.catalog:get().harnesses or {} do
		out[h.id] = h
	end
	return out
end

-- The catalog's models as the dialog's dropdown takes them: the agent's
-- mark, the model's name, and where it is served from. Reporting in an
-- existing chat narrows them to that chat's agent.
function M.model_items(): { any }
	local agents = harness_map()
	local only = M.target_harness()
	local out: { any } = {}
	for _, model in M.catalog:get().models or {} do
		if not only or model.harness == only then
			local agent = agents[model.harness] or {}
			local provider = if model.provider and model.provider.name and model.provider.name ~= ""
				then model.provider.name
				else (agent.label or model.harness)
			table.insert(out, {
				value = model.value,
				label = model.label,
				mark = agent.mark,
				hint = provider,
			})
		end
	end
	return out
end

function M.model_by_value(value: string?): any?
	if not value then
		return nil
	end
	for _, model in M.catalog:get().models or {} do
		if model.value == value then
			return model
		end
	end
	return nil
end

function M.model_name(value: string?): string
	local model = M.model_by_value(value)
	return if model then model.label else ""
end

function M.model_hint(value: string?): string
	local model = M.model_by_value(value)
	if not model then
		return ""
	end
	local agent = harness_map()[model.harness]
	return if model.provider and model.provider.name and model.provider.name ~= ""
		then model.provider.name
		else (agent and agent.label or model.harness)
end

-- The agent's mark for a catalog value, even before its model is listed
-- (the first segment names the agent).
function M.value_mark(value: string?): string?
	local model = M.model_by_value(value)
	local id = if model then model.harness else (if type(value) == "string" then string.match(value, "^([^/]+)/") else nil)
	local agent = if id then harness_map()[id] else nil
	return if agent then agent.mark else nil
end

-- A task's model as a catalog value (its harness and raw id), if it named
-- one.
function M.task_model_value(task: any): string?
	if not task then
		return nil
	end
	if type(task.harness) == "string" and task.harness ~= "" and type(task.model) == "string" and task.model ~= "" then
		return `{task.harness}/{task.model}`
	end
	return if type(task.model) == "string" and task.model ~= "" then task.model else nil
end

function M.task_model_name(task: any): string
	local value = M.task_model_value(task)
	local model = M.model_by_value(value)
	if model then
		return model.label
	end
	if type(task.model_label) == "string" and task.model_label ~= "" then
		return task.model_label
	end
	if value then
		return value
	end
	return "Default model"
end

function M.task_model_mark(task: any): string?
	return M.value_mark(M.task_model_value(task))
end

-- The chats a task may report into: this device's, newest first, and a new
-- chat of its own. A chat on another device is not one this engine can
-- send into.
function M.chat_options(): { any }
	local out = {
		{
			value = "new",
			label = "A new chat",
			description = "A chat of its own for every run",
			icon = "chat-add",
		},
	}
	local now = sessions.now:get()
	local here = now.here
	for _, chat in now.chats or {} do
		if not here or not chat.device or chat.device == here then
			local project = project_name(chat.project)
			table.insert(out, {
				value = chat.id,
				label = if chat.title and chat.title ~= "" then chat.title else "Untitled chat",
				description = if project ~= "" then project else "This device",
				icon = "chat-round-line",
			})
		end
	end
	return out
end

function M.choose_chat(id: string)
	M.new_target:set(id)
	if id == "new" then
		M.new_target_name:set("New chat")
	else
		local chat = chat_by_id(id)
		M.new_target_name:set(if chat and chat.title and chat.title ~= "" then chat.title else "Chat")
	end
	-- A chat keeps its agent: a model of another agent can't run there.
	local only = M.target_harness()
	local model = M.model_by_value(M.new_model:peek())
	if only and model and model.harness ~= only then
		M.new_model:set(nil)
	end
end

-- ── Creating and editing ────────────────────────────────────────────────

function M.open_create()
	M.editing:set(nil)
	M.new_title:set("")
	M.new_prompt:set("")
	M.new_repeat:set("weekdays")
	M.new_target:set("new")
	M.new_target_name:set("New chat")
	M.new_custom_repeat:set("daily")
	M.new_days:set({ 1 })
	M.new_model:set(nil)
	M.new_effort:set(nil)
	M.create_step:set("details")
	M.create_open:set(true)
	M.load_models()
end

function M.prefill(title: string, prompt: string, cadence: string?)
	M.editing:set(nil)
	M.new_title:set(title)
	M.new_prompt:set(prompt)
	M.new_repeat:set(cadence or "weekdays")
	M.new_target:set("new")
	M.new_target_name:set("New chat")
	M.new_custom_repeat:set("daily")
	M.new_days:set({ 1 })
	M.new_model:set(nil)
	M.new_effort:set(nil)
	M.create_step:set("details")
	M.create_open:set(true)
	M.load_models()
end

-- A suggestion chosen inside the create dialog: it fills the fields, the
-- dialog already open.
function M.use_template(title: string, prompt: string, cadence: string?)
	M.new_title:set(title)
	M.new_prompt:set(prompt)
	M.new_repeat:set(cadence or "weekdays")
	M.create_step:set("details")
end

function M.edit(id: string)
	local task = M.by_id(id)
	if not task then
		return
	end
	M.editing:set(id)
	M.new_title:set(task.title or "")
	M.new_prompt:set(task.prompt or "")
	M.new_target:set(if task.target_new then "new" else (task.chat or "new"))
	M.new_target_name:set(task.target or "New chat")
	M.new_model:set(M.task_model_value(task))
	M.new_effort:set(task.effort)
	local days: { number } = {}
	for _, day in task.days or { 1 } do
		table.insert(days, day)
	end
	M.new_days:set(days)
	local repeat_kind = task.repeat_kind or "weekdays"
	local hour = math.clamp(math.floor((task.hour or 9) + 0.5), 0, 23)
	local minute = math.clamp(math.floor((task.minute or 0) + 0.5), 0, 59)
	local function set_clock()
		local h12 = hour % 12
		if h12 == 0 then
			h12 = 12
		end
		M.new_hour:set(h12)
		M.new_minute:set(minute)
		M.new_period:set(if hour >= 12 then "PM" else "AM")
	end
	if repeat_kind == "custom" then
		M.new_repeat:set("custom")
		M.new_custom_repeat:set(task.custom_repeat or "daily")
		set_clock()
	elseif repeat_kind == "days" then
		M.new_repeat:set("custom")
		M.new_custom_repeat:set("days")
		set_clock()
	elseif repeat_kind == "once" and hour == 9 and minute == 0 then
		M.new_repeat:set("once")
	elseif hour ~= 9 or minute ~= 0 then
		M.new_repeat:set("custom")
		M.new_custom_repeat:set(repeat_kind)
		set_clock()
	else
		M.new_repeat:set(repeat_kind)
	end
	M.create_step:set("details")
	M.create_open:set(true)
	M.load_models()
end

local function payload(): any
	local kind = M.new_repeat:peek()
	local custom = kind == "custom"
	local target = M.new_target:peek() or "new"
	local is_new = target == "new"
	local picked = M.new_model:peek()
	local model = M.model_by_value(picked)
	local harness = if model then model.harness else (if type(picked) == "string" then string.match(picked, "^([^/]+)/") else nil)
	return {
		id = M.editing:peek(),
		title = trim(M.new_title:peek()),
		prompt = trim(M.new_prompt:peek()),
		repeat_kind = kind,
		custom_repeat = M.new_custom_repeat:peek(),
		days = M.selected_days(),
		hour = if custom then M.hour24() else 9,
		minute = if custom then math.clamp(math.floor(M.new_minute:peek() + 0.5), 0, 59) else 0,
		chat = if is_new then nil else target,
		chat_name = if is_new then nil else M.new_target_name:peek(),
		project = if is_new then "" else project_name((chat_by_id(target) or {}).project),
		harness = harness,
		model = picked,
		model_label = if model then model.label else nil,
		effort = M.new_effort:peek(),
	}
end

local function reset_form()
	M.new_title:set("")
	M.new_prompt:set("")
	M.new_repeat:set("weekdays")
	M.new_target:set("new")
	M.new_target_name:set("New chat")
	M.new_custom_repeat:set("daily")
	M.new_days:set({ 1 })
	M.new_model:set(nil)
	M.new_effort:set(nil)
	M.create_step:set("details")
end

-- Save the dialog's task. The service validates and owns the record; the
-- dialog closes only when it has answered.
function M.submit(done: ((boolean, string?) -> ())?)
	if M.saving:get() then
		return
	end
	M.saving:set(true)
	M.error:set(nil)
	local editing = M.editing:peek()
	local method = if editing then "update" else "create"
	service(method, payload(), function(result)
		M.saving:set(false)
		apply(result)
		M.create_open:set(false)
		M.editing:set(nil)
		reset_form()
		if done then
			done(true, nil)
		end
	end, function(err)
		M.saving:set(false)
		M.error:set(err)
		if done then
			done(false, err)
		end
	end)
end

-- ── Running, pausing, deleting ──────────────────────────────────────────

function M.run_now(id: string, done: ((boolean, string?) -> ())?)
	service("run", { id = id }, function(result)
		apply(result, id, false)
		if done then
			done(true, nil)
		end
	end, function(err)
		M.error:set(err)
		if done then
			done(false, err)
		end
	end)
end

function M.reschedule(id: string, done: ((boolean, string?) -> ())?)
	service("reschedule", { id = id }, function(result)
		apply(result, id, false)
		if done then
			done(true, nil)
		end
	end, function(err)
		M.error:set(err)
		if done then
			done(false, err)
		end
	end)
end

function M.toggle(id: string, done: ((boolean, string?) -> ())?)
	local task = M.by_id(id)
	if not task then
		return
	end
	local method = if task.status == "paused" then "resume" else "pause"
	service(method, { id = id }, function(result)
		apply(result, id, false)
		if done then
			done(true, nil)
		end
	end, function(err)
		M.error:set(err)
		if done then
			done(false, err)
		end
	end)
end

function M.ask_delete(id: string)
	M.delete_target:set(id)
	M.delete_open:set(true)
end

function M.delete(done: ((boolean, string?) -> ())?)
	local id = M.delete_target:peek()
	if not id then
		return
	end
	service("delete", { id = id }, function(result)
		apply(result)
		M.delete_target:set(nil)
		M.delete_open:set(false)
		if M.selected:peek() == id then
			M.selected:set(nil)
		end
		if done then
			done(true, nil)
		end
	end, function(err)
		M.error:set(err)
		if done then
			done(false, err)
		end
	end)
end

return M
