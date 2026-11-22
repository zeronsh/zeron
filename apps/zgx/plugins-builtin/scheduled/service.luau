-- Scheduled Tasks' service: the headless half of Zeron's scheduled work,
-- run by the engine on each device with no window open.
--
-- A task is a prompt and a cadence. When its time comes the service sends
-- the prompt to the chat the task names, or opens a new chat for it, and
-- follows the turn it starts: running, waiting on a question, finished or
-- failed. What happened is kept on this device, with the next three runs
-- and the last handful of history, so the page can show it at once.
--
-- Times are this device's local time. Each task sleeps until exactly its
-- next run (an hourly tick is only a safety net); a device asleep at the
-- minute a task was due runs it when it wakes.

local zeron = require("./zeron")("scheduled")

local STORE = zeron.store("tasks", { tasks = {} })
local TIMEZONE = os.date("%Z") or "local time"
local ZONE = string.gsub(string.gsub(TIMEZONE, " Standard Time$", ""), " Daylight Time$", "")
-- `os.date` reads the device's clock, but `os.time` reads a date table as
-- UTC in the engine's service. Turn wall-clock parts into the epoch Zeron
-- uses, correcting for the device's offset (and a daylight change).
local function local_epoch(parts: any, near: number): number
	local wall = os.time(parts)
	local at = wall + (near - os.time(os.date("*t", near)))
	for _ = 1, 2 do
		at = wall + (at - os.time(os.date("*t", at)))
	end
	return at
end
local HISTORY = 12
local MAX_UPCOMING = 3
local MAX_TASKS = 100
-- A task due less than this long ago still runs (a window reopened, a brief
-- sleep); one due before it is kept as missed, with its own way forward.
local MISSED_GRACE = 120

local function now(): number
	return os.time()
end

local function trim(text: any): string
	if type(text) ~= "string" then
		return ""
	end
	return string.match(text, "^%s*(.-)%s*$") or ""
end

-- A tool's HH:MM (24-hour) as the hour and minute a task keeps.
local function parse_time(text: any): (number, number)
	local hour, minute = string.match(trim(text), "^(%d%d?):(%d%d)$")
	local h = tonumber(hour)
	local m = tonumber(minute)
	if not h or not m or h > 23 or m > 59 then
		error("Ask the person for a time of day, like 09:00 or 18:30 (24-hour, local time).", 0)
	end
	return h, m
end

-- A model as a task keeps it: the harness that runs it, the model id the
-- engine's agents API takes, and the name the list can show. The picker
-- sends a catalog value ("harness/model", the model id may hold a slash of
-- its own); an agent may send the same. An explicit `harness` says which
-- first segment is the harness.
local function parse_model(args: any): (string?, string?, string?)
	local picked = trim(args.model)
	local harness = trim(args.harness)
	local label = trim(args.model_label)
	if picked == "" then
		picked = nil
	end
	if harness == "" then
		harness = nil
	end
	if label == "" then
		label = nil
	end
	if picked then
		local first, rest = string.match(picked, "^([^/]+)/(.+)$")
		if first and rest and (not harness or first == harness) then
			harness = harness or first
			picked = rest
		end
	end
	if not picked then
		harness = nil
		label = nil
	end
	return harness, picked, label
end


-- Days a task may run, as Monday..Sunday (ISO: 1..7). `os.date` says
-- Sunday as 1; `iso_day` is the one the task keeps.
local DAY_NAMES = { "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday" }
local DAY_INPUT = {
	monday = 1, mon = 1,
	tuesday = 2, tue = 2, tues = 2,
	wednesday = 3, wed = 3,
	thursday = 4, thu = 4, thur = 4, thurs = 4,
	friday = 5, fri = 5,
	saturday = 6, sat = 6,
	sunday = 7, sun = 7,
}

local function iso_day(wday: number): number
	return if wday == 1 then 7 else wday - 1
end

-- The days an agent or the picker named: sorted ISO days, nil when none.
local function parse_days(days: any): { number }?
	if type(days) ~= "table" then
		return nil
	end
	local seen: { [number]: boolean } = {}
	local out: { number } = {}
	for _, given in days do
		local n = if type(given) == "number" then math.floor(given) elseif type(given) == "string" then DAY_INPUT[string.lower(trim(given))] else nil
		if n and n >= 1 and n <= 7 and not seen[n] then
			seen[n] = true
			table.insert(out, n)
		end
	end
	if #out == 0 then
		return nil
	end
	table.sort(out)
	return out
end

local function has_day(task: any, iso: number): boolean
	for _, day in task.days or {} do
		if day == iso then
			return true
		end
	end
	return false
end

local function days_text(days: any): string
	local names = {}
	for _, day in days or {} do
		local name = DAY_NAMES[day]
		if name then
			table.insert(names, name)
		end
	end
	if #names == 0 then
		return "Monday"
	elseif #names == 1 then
		return names[1]
	end
	return `{table.concat(names, ", ", 1, #names - 1)} and {names[#names]}`
end

local function store_data(): any	local data = STORE:get()
	if type(data) ~= "table" then
		data = { tasks = {} }
	end
	if type(data.tasks) ~= "table" then
		data.tasks = {}
	end
	return data
end

local function save(data: any)
	STORE:set(data)
end

local function task_by_id(data: any, id: string): any?
	for _, task in data.tasks do
		if task.id == id then
			return task
		end
	end
	return nil
end

local function task_by_run(data: any, chat: string?): any?
	if not chat then
		return nil
	end
	for _, task in data.tasks do
		if task.run_chat == chat then
			return task
		end
	end
	return nil
end

local function clamp(n: any, lo: number, hi: number, fallback: number): number
	local v = tonumber(n)
	if not v or v ~= v then
		return fallback
	end
	return math.clamp(math.floor(v + 0.5), lo, hi)
end

local function clock12(hour24: number, minute: number): string
	local h = hour24 % 12
	if h == 0 then
		h = 12
	end
	return string.format("%02d:%02d %s", h, minute, if hour24 < 12 then "AM" else "PM")
end

local function clock24(hour24: number, minute: number): string
	return string.format("%02d:%02d", hour24, minute)
end

local function day_key(at: number): string
	return os.date("%Y-%m-%d", at)
end

local function day_label(at: number, at_now: number): string
	local key = day_key(at)
	if key == day_key(at_now) then
		return "Today"
	elseif key == day_key(at_now + 86400) then
		return "Tomorrow"
	elseif key == day_key(at_now - 86400) then
		return "Yesterday"
	end
	return os.date("%a %d %b", at)
end

local function when_text(at: number, at_now: number): string
	return `{day_label(at, at_now)} · {clock12(os.date("*t", at).hour, os.date("*t", at).min)}`
end

local function history_text(at: number, at_now: number): string
	local t = os.date("*t", at)
	local label = day_label(at, at_now)
	if label == "Today" or label == "Tomorrow" or label == "Yesterday" then
		return `{label}, {clock24(t.hour, t.min)}`
	end
	return `{os.date("%a %d %b", at)}, {clock24(t.hour, t.min)}`
end

local function duration_text(seconds: number): string
	if seconds < 60 then
		return `{math.max(0, math.floor(seconds + 0.5))}s`
	elseif seconds < 3600 then
		local m = math.floor(seconds / 60)
		local s = math.floor(seconds % 60)
		return if s > 0 then `{m}m {s}s` else `{m}m`
	end
	local h = math.floor(seconds / 3600)
	local m = math.floor((seconds % 3600) / 60)
	return if m > 0 then `{h}h {m}m` else `{h}h`
end

local function relative_text(at: number, at_now: number): string
	local diff = at - at_now
	if diff <= 30 then
		return "now"
	elseif diff < 3600 then
		return `in {math.max(1, math.floor(diff / 60))}m`
	elseif diff < 86400 then
		return `in {math.floor(diff / 3600)}h`
	end
	return `in {math.floor(diff / 86400)}d`
end

-- The cadence a task actually follows: a custom one names its own.
local function cadence_of(task: any): string
	if task.repeat_kind == "custom" then
		return task.custom_repeat or "daily"
	end
	return task.repeat_kind or "weekdays"
end

-- The next occurrence of the task strictly after `from`, on this device's
-- local clock. (A calendar day, not 86,400 seconds: summer time changes
-- the length of a day, not the time a task runs.)
local function next_after(task: any, from: number): number?
	local cadence = cadence_of(task)
	local hour = task.hour or 9
	local minute = task.minute or 0
	local base = os.date("*t", from)
	for offset = 0, 21 do
		local at = local_epoch({
			year = base.year,
			month = base.month,
			day = base.day + offset,
			hour = hour,
			min = minute,
			sec = 0,
			isdst = nil,
		}, from)
		if at and at > from + 1 then
			local wday = os.date("*t", at).wday
			local fits = cadence == "once"
				or cadence == "daily"
				or (cadence == "weekdays" and wday >= 2 and wday <= 6)
				or (cadence == "weekly" and wday == 2)
				or (cadence == "days" and has_day(task, iso_day(wday)))
			if fits then
				return at
			end
		end
	end
	return nil
end

local function schedule_text(task: any): string
	local time = clock12(task.hour or 9, task.minute or 0)
	local cadence = cadence_of(task)
	if cadence == "once" then
		local at = task.next_at
		return if at then `Once · {day_label(at, now())} at {time}` else `Once at {time}`
	elseif cadence == "daily" then
		return `Every day at {time}`
	elseif cadence == "weekdays" then
		return `Every weekday at {time}`
	elseif cadence == "weekly" then
		return `Every Monday at {time}`
	elseif cadence == "days" then
		return `Every {days_text(task.days)} at {time}`
	end
	return `Every day at {time}`
end

local function notify(title: string, body: string, chat: string?)
	pcall(function()
		zeron.notify({ title = title, body = body, session = chat }):after(nil, function() end)
	end)
end

-- A run whose time passed while Zeron wasn't there: kept as missed, with
-- the time it was due, so the page can offer Run now or Reschedule.
local function mark_missed(task: any, at: number)
	task.status = "missed"
	task.missed_at = task.next_at or at
	task.last_result = "Missed while Zeron was closed"
	task.last_ok = nil
	task.updated_at = at
end

-- Arm one task's exact wake-up: a sleeping task in the service, not a poll.
-- (Forward-declared: `begin` and `finish` arm the next run after theirs.)
local arm: (string) -> ()

local function finish(task: any, data: any, started: number, ok: boolean, problem: string?, chat: string?)
	local at = now()
	local duration = math.max(0, at - started)
	local outcome = if ok then `Completed · {duration_text(duration)}` else `Failed · {problem or "the run stopped"}`
	task.history = task.history or {}
	table.insert(task.history, 1, {
		at = started,
		ok = ok,
		outcome = outcome,
		duration = duration,
		chat = chat,
	})
	while #task.history > HISTORY do
		table.remove(task.history)
	end
	task.last_at = started
	task.last_ok = ok
	task.last_result = outcome
	task.last_chat = chat
	task.run_chat = nil
	task.run_started = nil
	task.question = nil
	if task.repeat_kind == "once" then
		task.status = "finished"
		task.next_at = nil
	elseif task.pause_after then
		task.status = "paused"
		task.next_at = nil
	else
		task.status = "active"
		task.next_at = next_after(task, at)
	end
	task.pause_after = nil
	task.updated_at = at
	save(data)
	if not ok then
		notify("Scheduled task failed", `{task.title} · {problem or "the run stopped"}`, chat)
	end
	if task.status == "active" then
		arm(task.id)
	end
end

local function busy_in_chat(data: any, chat: string?, id: string): boolean
	if not chat or chat == "" then
		return false
	end
	for _, other in data.tasks do
		if other.id ~= id and other.status == "running" and (other.run_chat == chat or other.chat == chat) then
			return true
		end
	end
	return false
end

-- Begin one task: mark it running, send its prompt, and keep the chat its
-- turn runs in. The turn's end comes back through `zeron.on`.
local function begin(id: string)
	local data = store_data()
	local task = task_by_id(data, id)
	if not task or task.status == "running" then
		return
	end
	if busy_in_chat(data, task.chat, task.id) then
		task.next_at = next_after(task, now())
		task.last_result = "Skipped · that chat is running something else"
		task.last_ok = nil
		task.updated_at = now()
		save(data)
		arm(task.id)
		return
	end
	local at = now()
	task.status = "running"
	task.run_started = at
	task.run_chat = nil
	task.question = nil
	task.missed_at = nil
	task.last_result = "Running now"
	task.last_ok = nil
	task.pause_after = false
	task.updated_at = at
	task.next_at = next_after(task, at)
	save(data)
	arm(task.id)

	local ok, reply = pcall(function()
		if type(task.chat) == "string" and task.chat ~= "" then
			-- A task that named a model runs the chat on it. The chat keeps
			-- its own agent: a model of another agent is an honest refusal,
			-- not a silent change.
			if task.model then
				local seen_ok, seen = pcall(function()
					return zeron.chat_model(task.chat):await()
				end)
				if seen_ok and type(seen) == "table" then
					local provider = seen.provider
					if type(provider) == "string" and provider ~= "" and task.harness and provider ~= task.harness then
						error(`That chat runs {provider}, so it can't run {task.harness}.`, 0)
					end
					local same = seen.model == task.model and (task.effort == nil or seen.effort == task.effort)
					if not same then
						zeron.set_model({ chat = task.chat, model = task.model, effort = task.effort }):await()
					end
				else
					zeron.set_model({ chat = task.chat, model = task.model, effort = task.effort }):await()
				end
			end
			return zeron.send_message({ chat = task.chat, text = task.prompt }):await()
		end
		-- A task with no chat opens one here, on the device whose service
		-- runs it (the engine needs a device or a space to make a chat),
		-- with its model when it has one.
		return zeron.new_chat({
			prompt = task.prompt,
			device = zeron.device.id,
			harness = task.harness,
			model = task.model,
			effort = task.effort,
		}):await()
	end)

	data = store_data()
	task = task_by_id(data, id)
	if not task then
		return
	end
	if not ok or type(reply) ~= "table" or type(reply.chat) ~= "string" or reply.chat == "" then
		local why = if ok
			then (reply and (reply.message or reply.status)) or "the chat didn't open"
			else tostring(reply)
		finish(task, data, at, false, `Couldn't start · {why}`, nil)
		return
	end
	task.run_chat = reply.chat
	task.last_chat = reply.chat
	task.updated_at = now()
	save(data)
end

-- One sleeping task per scheduled task, for the exact next time it runs.
-- Nothing polls: with a run days away the service sleeps in hour-sized
-- pieces and the engine's clock is the only thing that moves.
local armed: { [string]: number } = {}
local MAX_SLEEP = 3600

arm = function(id: string)
	local data = store_data()
	local task = task_by_id(data, id)
	if not task or task.status ~= "active" or not task.next_at then
		armed[id] = nil
		return
	end
	local due = task.next_at
	if armed[id] == due then
		return
	end
	armed[id] = due
	zeron.task(function()
		while true do
			local left = due - now()
			if left <= 0 then
				break
			end
			zeron.sleep(math.min(left, MAX_SLEEP)):await()
		end
		local latest = store_data()
		local task_now = task_by_id(latest, id)
		if armed[id] ~= due then
			return
		end
		armed[id] = nil
		if not task_now or task_now.status ~= "active" or task_now.next_at ~= due then
			return
		end
		if due <= now() then
			if due < now() - MISSED_GRACE then
				mark_missed(task_now, now())
				save(latest)
				notify("Scheduled task missed", `{task_now.title} was due while Zeron was closed.`, task_now.chat)
			else
				begin(id)
			end
		else
			arm(id)
		end
	end)
end

local function tick()
	local data = store_data()
	local at = now()
	local due = {}
	local missed = {}
	local changed = false
	for _, task in data.tasks do
		if task.status == "active" then
			if not task.next_at then
				task.next_at = next_after(task, at)
				changed = true
			end
			if task.next_at and task.next_at <= at then
				if task.next_at < at - MISSED_GRACE then
					mark_missed(task, at)
					table.insert(missed, { title = task.title, chat = task.chat })
					changed = true
				else
					table.insert(due, { id = task.id, at = task.next_at })
				end
			elseif task.next_at then
				arm(task.id)
			end
		end
	end
	if changed then
		save(data)
	end
	for _, row in missed do
		notify("Scheduled task missed", `{row.title} was due while Zeron was closed.`, row.chat)
	end
	table.sort(due, function(a, b)
		return a.at < b.at
	end)
	for _, row in due do
		begin(row.id)
	end
end

local function task_view(task: any, at: number): any
	local cadence = cadence_of(task)
	local history = {}
	local last = nil
	for i, run in task.history or {} do
		if i <= 6 then
			table.insert(history, {
				when = history_text(run.at, at),
				outcome = run.outcome,
				ok = run.ok,
			})
		end
		if i == 1 then
			last = run
		end
	end
	local status = task.status or "active"
	local next_at = task.next_at
	local next_text = "Finished"
	local next_in = "Finished"
	if status == "running" then
		next_text = "Running now"
		next_in = "Running"
	elseif status == "needs_input" then
		next_text = "Waiting for you"
		next_in = "Needs input"
	elseif status == "paused" then
		next_text = "Paused"
		next_in = "Paused"
	elseif status == "missed" then
		next_text = if task.missed_at then `Missed · {when_text(task.missed_at, at)}` else "Missed while Zeron was closed"
		next_in = "Missed"
	elseif status == "active" and next_at then
		next_text = when_text(next_at, at)
		next_in = relative_text(next_at, at)
	end
	local group = "Later"
	if status == "paused" then
		group = "Paused"
	elseif status == "finished" then
		group = "Finished"
	elseif status == "missed" or status == "needs_input" then
		group = "Needs you"
	elseif next_at then
		local label = day_label(next_at, at)
		group = if label == "Today" or label == "Tomorrow" then label else "Later"
	elseif status == "running" then
		group = "Today"
	end
	local upcoming = {}
	if status == "active" and next_at then
		local cursor = next_at
		for _ = 1, MAX_UPCOMING do
			if not cursor then
				break
			end
			table.insert(upcoming, { at = when_text(cursor, at), eta = relative_text(cursor, at) })
			cursor = next_after(task, cursor)
		end
	end
	local target_new = type(task.chat) ~= "string" or task.chat == ""
	local target = if target_new then "New chat" else (task.chat_name or "Chat")
	local last_result = if last then last.outcome else "Not run yet"
	local last_ok = if last then last.ok else nil
	if status == "missed" then
		last_result = "Missed while Zeron was closed"
		last_ok = nil
	end
	local last_text = if last then `{history_text(last.at, at)} · {last.outcome}` else "Not run yet"
	if status == "missed" and task.missed_at then
		last_text = `Missed · {history_text(task.missed_at, at)}`
	end
	local run_chat = task.run_chat or task.last_chat or (last and last.chat) or nil
	return {
		id = task.id,
		title = task.title,
		prompt = task.prompt,
		status = status,
		repeat_kind = task.repeat_kind or cadence,
		custom_repeat = task.custom_repeat,
		days = task.days,
		hour = task.hour or 9,
		minute = task.minute or 0,
		schedule = schedule_text(task),
		timezone = ZONE,
		target = target,
		target_new = target_new,
		chat = task.chat,
		run_chat = run_chat,
		project = task.project or "",
		harness = task.harness,
		model = task.model,
		model_label = task.model_label,
		effort = task.effort,
		next = next_text,
		next_in = next_in,
		next_at = next_at,
		missed_at = task.missed_at,
		missed_when = if status == "missed" and task.missed_at then when_text(task.missed_at, at) else nil,
		group = group,
		elapsed = if status == "running" and task.run_started then duration_text(at - task.run_started) else nil,
		pause_after = task.pause_after == true,
		last = last_text,
		last_result = last_result,
		last_ok = last_ok,
		question = task.question,
		upcoming = upcoming,
		history = history,
		created_at = task.created_at,
		updated_at = task.updated_at,
	}
end

local function view(data: any, focus: string?): any
	local at = now()
	local tasks = {}
	for _, task in data.tasks do
		table.insert(tasks, task_view(task, at))
	end
	local rank = { running = 0, needs_input = 1, active = 2, paused = 3, finished = 4 }
	table.sort(tasks, function(a, b)
		local ra, rb = rank[a.status] or 2, rank[b.status] or 2
		if ra ~= rb then
			return ra < rb
		end
		local na, nb = a.next_at or math.huge, b.next_at or math.huge
		if na ~= nb then
			return na < nb
		end
		return tostring(a.id) < tostring(b.id)
	end)
	return { tasks = tasks, now = at, timezone = ZONE, task = focus }
end

local function make_task(args: any): any
	if type(args) ~= "table" then
		error("A scheduled task needs its words and its time.", 0)
	end
	local title = trim(args.title)
	local prompt = trim(args.prompt)
	if prompt == "" then
		error("A scheduled task needs something to run.", 0)
	end
	if title == "" then
		title = string.sub(prompt, 1, 56)
	end
	local data = store_data()
	if #data.tasks >= MAX_TASKS then
		error(`That's {MAX_TASKS} scheduled tasks: remove one before adding another.`, 0)
	end
	local id = `t_{os.time()}_{math.random(100000, 999999)}`
	while task_by_id(data, id) do
		id = `t_{os.time()}_{math.random(100000, 999999)}`
	end
	local repeat_kind = args.repeat_kind or "weekdays"
	if repeat_kind ~= "once" and repeat_kind ~= "daily" and repeat_kind ~= "weekdays" and repeat_kind ~= "weekly" and repeat_kind ~= "days" and repeat_kind ~= "custom" then
		repeat_kind = "weekdays"
	end
	local custom_repeat = args.custom_repeat or "daily"
	if custom_repeat ~= "once" and custom_repeat ~= "daily" and custom_repeat ~= "weekdays" and custom_repeat ~= "weekly" and custom_repeat ~= "days" then
		custom_repeat = "daily"
	end
	local harness, model, model_label = parse_model(args)
	local days = parse_days(args.days)
	if (repeat_kind == "days" or (repeat_kind == "custom" and custom_repeat == "days")) and not days then
		days = { 1 }
	end
	local task = {
		id = id,
		title = title,
		prompt = prompt,
		chat = if type(args.chat) == "string" and args.chat ~= "" then args.chat else nil,
		chat_name = trim(args.chat_name) ~= "" and trim(args.chat_name) or nil,
		project = trim(args.project),
		repeat_kind = repeat_kind,
		custom_repeat = custom_repeat,
		days = if repeat_kind == "days" or (repeat_kind == "custom" and custom_repeat == "days") then days else nil,
		harness = harness,
		model = model,
		model_label = model_label,
		effort = trim(args.effort) ~= "" and trim(args.effort) or nil,
		hour = clamp(args.hour, 0, 23, 9),
		minute = clamp(args.minute, 0, 59, 0),
		status = "active",
		created_at = now(),
		updated_at = now(),
		history = {},
	}
	task.next_at = next_after(task, now())
	table.insert(data.tasks, task)
	save(data)
	arm(task.id)
	return data, task.id
end

local function edit_task(args: any): any
	if type(args) ~= "table" or type(args.id) ~= "string" then
		error("Which scheduled task?", 0)
	end
	local data = store_data()
	local task = task_by_id(data, args.id)
	if not task then
		error("That scheduled task is gone.", 0)
	end
	local title = trim(args.title)
	local prompt = trim(args.prompt)
	if prompt == "" then
		error("A scheduled task needs something to run.", 0)
	end
	task.title = if title ~= "" then title else string.sub(prompt, 1, 56)
	task.prompt = prompt
	task.chat = if type(args.chat) == "string" and args.chat ~= "" then args.chat else nil
	task.chat_name = trim(args.chat_name) ~= "" and trim(args.chat_name) or nil
	task.project = trim(args.project)
	local repeat_kind = args.repeat_kind or task.repeat_kind or "weekdays"
	if repeat_kind ~= "once" and repeat_kind ~= "daily" and repeat_kind ~= "weekdays" and repeat_kind ~= "weekly" and repeat_kind ~= "days" and repeat_kind ~= "custom" then
		repeat_kind = "weekdays"
	end
	local custom_repeat = args.custom_repeat or task.custom_repeat or "daily"
	if custom_repeat ~= "once" and custom_repeat ~= "daily" and custom_repeat ~= "weekdays" and custom_repeat ~= "weekly" and custom_repeat ~= "days" then
		custom_repeat = "daily"
	end
	task.repeat_kind = repeat_kind
	task.custom_repeat = custom_repeat
	local days = parse_days(args.days) or task.days
	if (repeat_kind == "days" or (repeat_kind == "custom" and custom_repeat == "days")) and not days then
		days = { 1 }
	end
	task.days = if repeat_kind == "days" or (repeat_kind == "custom" and custom_repeat == "days") then days else nil
	local harness, model, model_label = parse_model(args)
	task.harness = harness
	task.model = model
	task.model_label = model_label
	task.effort = trim(args.effort) ~= "" and trim(args.effort) or nil
	task.hour = clamp(args.hour, 0, 23, task.hour or 9)
	task.minute = clamp(args.minute, 0, 59, task.minute or 0)
	task.updated_at = now()
	if task.status == "finished" or task.status == "missed" then
		task.status = "active"
		task.missed_at = nil
	end
	if task.status == "active" or task.status == "running" then
		task.next_at = next_after(task, now())
	else
		task.next_at = nil
	end
	armed[task.id] = nil
	save(data)
	arm(task.id)
	return data, task.id
end

local function remove_task(args: any): any
	local id = if type(args) == "table" then args.id else args
	if type(id) ~= "string" then
		error("Which scheduled task?", 0)
	end
	local data = store_data()
	local kept = {}
	for _, task in data.tasks do
		if task.id ~= id then
			table.insert(kept, task)
		end
	end
	data.tasks = kept
	armed[id] = nil
	save(data)
	return data
end

local function pause_task(args: any): any
	local id = if type(args) == "table" then args.id else args
	local data = store_data()
	local task = task_by_id(data, id)
	if not task then
		error("That scheduled task is gone.", 0)
	end
	if task.status == "running" then
		task.pause_after = true
	else
		task.status = "paused"
		task.next_at = nil
		task.missed_at = nil
		task.pause_after = nil
	end
	task.updated_at = now()
	armed[id] = nil
	save(data)
	return data, id
end

local function resume_task(args: any): any
	local id = if type(args) == "table" then args.id else args
	local data = store_data()
	local task = task_by_id(data, id)
	if not task then
		error("That scheduled task is gone.", 0)
	end
	task.pause_after = nil
	task.status = "active"
	task.missed_at = nil
	task.next_at = next_after(task, now())
	task.updated_at = now()
	save(data)
	arm(id)
	return data, id
end

-- A missed run put back on the calendar: its next future time, on its own
-- cadence. (Run now is the other way forward, in `begin`.)
local function reschedule_task(args: any): any
	local id = if type(args) == "table" then args.id else args
	if type(id) ~= "string" then
		error("Which scheduled task?", 0)
	end
	local data = store_data()
	local task = task_by_id(data, id)
	if not task then
		error("That scheduled task is gone.", 0)
	end
	task.status = "active"
	task.missed_at = nil
	task.question = nil
	task.pause_after = nil
	task.next_at = next_after(task, now())
	if not task.next_at then
		task.status = "finished"
	end
	task.last_result = "Rescheduled"
	task.updated_at = now()
	save(data)
	arm(id)
	return data, id
end

local function run_task(args: any): any
	local id = if type(args) == "table" then args.id else args
	if type(id) ~= "string" then
		error("Which scheduled task?", 0)
	end
	begin(id)
	return store_data()
end

zeron.on("turn.finished", function(event: any)
	local by = event and event.by
	if type(by) == "table" and type(by.plugin) == "string" and by.plugin ~= "scheduled" then
		return
	end
	local data = store_data()
	local task = task_by_run(data, event and event.chat)
	if not task then
		return
	end
	local outcome = event.outcome or "completed"
	local ok = outcome == "completed"
	finish(task, data, task.run_started or now(), ok, if ok then nil else outcome, event.chat)
end)

zeron.on("turn.errored", function(event: any)
	local by = event and event.by
	if type(by) == "table" and type(by.plugin) == "string" and by.plugin ~= "scheduled" then
		return
	end
	local data = store_data()
	local task = task_by_run(data, event and event.chat)
	if not task then
		return
	end
	finish(task, data, task.run_started or now(), false, event.problem or event.outcome or "the run failed", event.chat)
end)

zeron.on("chat.awaiting_input", function(event: any)
	local data = store_data()
	local task = task_by_run(data, event and event.chat)
	if not task then
		return
	end
	task.status = "needs_input"
	task.question = "The run is waiting for your answer in the chat."
	task.last_result = "Waiting on a question"
	task.last_ok = nil
	task.updated_at = now()
	save(data)
	notify("Scheduled task needs you", task.title, task.run_chat)
end)

zeron.on("chat.input_answered", function(event: any)
	local data = store_data()
	local task = task_by_run(data, event and event.chat)
	if not task or task.status ~= "needs_input" then
		return
	end
	task.status = "running"
	task.question = nil
	task.last_result = "Running again"
	task.updated_at = now()
	save(data)
end)

zeron.serve("list", function(): any
	return view(store_data())
end, { safe = true })

zeron.serve("create", function(args: any): any
	local data, id = make_task(args)
	return view(data, id)
end)

zeron.serve("update", function(args: any): any
	local data, id = edit_task(args)
	return view(data, id)
end)

zeron.serve("delete", function(args: any): any
	return view(remove_task(args))
end)

zeron.serve("run", function(args: any): any
	local id = if type(args) == "table" then args.id else args
	local data = run_task(args)
	return view(data, id)
end)

zeron.serve("pause", function(args: any): any
	local data, id = pause_task(args)
	return view(data, id)
end)

zeron.serve("resume", function(args: any): any
	local data, id = resume_task(args)
	return view(data, id)
end)

zeron.serve("reschedule", function(args: any): any
	local data, id = reschedule_task(args)
	return view(data, id)
end)

-- Agents ask for this in an ordinary chat. They must ask for what's
-- missing (the prompt, how often, the time); the tool never invents a
-- time. By default the task reports in the chat it was asked from.
zeron.tool {
	name = "schedule_task",
	run = function(args: any, call: any): any
		if type(args) ~= "table" then
			error("Say what the task should do, how often, and at what time.", 0)
		end
		local prompt = trim(args.prompt)
		if prompt == "" then
			error("Ask the person what the scheduled task should do.", 0)
		end
		local repeat_kind = args.repeat_kind
		if repeat_kind ~= "once" and repeat_kind ~= "daily" and repeat_kind ~= "weekdays" and repeat_kind ~= "weekly" and repeat_kind ~= "days" then
			error("Ask how often it should run: once, daily, weekdays, weekly, or specific days.", 0)
		end
		local hour, minute = parse_time(args.time)
		local days = parse_days(args.days)
		if repeat_kind == "days" and not days then
			error("Ask which days it should run (for example Monday and Thursday).", 0)
		end
		local harness, model, model_label = parse_model(args)
		if model and harness then
			local seen_ok, models = pcall(function()
				return zeron.models(harness):await()
			end)
			if seen_ok and type(models) == "table" then
				local found = nil
				for _, m in models do
					if m.id == model then
						found = m
						break
					end
				end
				if not found then
					error(`{harness} has no model "{model}". Ask which one to use.`, 0)
				end
				if not model_label then
					model_label = found.label
				end
			end
		end
		local chat_id = if type(args.chat_id) == "string" and args.chat_id ~= "" then args.chat_id else nil
		local from_chat = call and call.chat
		local chat = chat_id
			or (if args.new_chat == true then nil else from_chat)
		if chat and model and harness then
			local seen_ok, seen = pcall(function()
				return zeron.chat_model(chat):await()
			end)
			if seen_ok and type(seen) == "table" and type(seen.provider) == "string" and seen.provider ~= "" and seen.provider ~= harness then
				error(`That chat runs {seen.provider}. Pick one of its models, or set new_chat = true to run {harness}.`, 0)
			end
		end
		local chat_name = if chat
			then (if trim(args.chat_name) ~= "" then trim(args.chat_name) else (if chat_id then "Chat" else "This chat"))
			else nil
		local data, id = make_task({
			title = trim(args.title),
			prompt = prompt,
			repeat_kind = repeat_kind,
			hour = hour,
			minute = minute,
			chat = chat,
			chat_name = chat_name,
			project = "",
			days = days,
			harness = harness,
			model = model,
			model_label = model_label,
			effort = trim(args.effort),
		})
		local task = task_by_id(data, id)
		return {
			id = id,
			title = task.title,
			schedule = schedule_text(task),
			next = when_text(task.next_at, now()),
			target = if chat then task.chat_name else "New chat",
			chat = chat,
			model = if task.model then (task.model_label or task.model) else nil,
		}
	end,
}

-- A safety net only: each task sleeps until its own next time (arm()).
zeron.every(3600, function()
	tick()
end)

-- Once it has loaded, what was due while the engine was away runs now, and
-- every future task gets its exact wake-up.
zeron.task(function()
	tick()
end)
