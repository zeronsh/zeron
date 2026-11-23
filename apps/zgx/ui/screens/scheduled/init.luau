-- Scheduled Tasks.
--
-- A page in Zeron's own language, as the pull request board is: a title and
-- its count, a quiet line saying where the tasks run, the states as the
-- design system's tabs, and one card headed by its search, the tasks as
-- rows under it. Nothing here is a room of its own; the page is meant to
-- read as another page of the app.
--
-- The tasks are the engine's scheduled service (plugins-builtin/scheduled):
-- this page reads them, creates and edits them, and opens the chats their
-- runs report in. No demo tasks live here.

local ds = require("@ds")
local git = require("@ds/git")
local control = require("@ds/control")
local ListRow = require("@ds/list_row").ListRow
local store = require("@app/screens/scheduled/store")

local M = {}

-- The page's width at most: a column the gallery's git pages' width, so
-- rows hold together on a wide window instead of stretching.
local WIDTH = 960

-- The three states of a task, as the tabs name them.
local TABS = {
	{ value = "upcoming", label = "Upcoming" },
	{ value = "paused", label = "Paused" },
	{ value = "finished", label = "Finished" },
}

-- Each state's badge, ink and glyph. The glyph is the row's state mark: a
-- quiet clock for the ones that wait, the working ring while one runs, and
-- the design system's own input and done glyphs for the states that want
-- to be seen.
local STATUS = {
	active = { tone = "neutral", label = "Active", glyph = "clock-circle", ink = "$text.tertiary" },
	running = { tone = "accent", label = "Running", kind = "working", ink = "$accent" },
	needs_input = { tone = "warning", label = "Needs input", kind = "input", ink = "$warning" },
	missed = { tone = "warning", label = "Missed", glyph = "clock-circle", ink = "$warning" },
	paused = { tone = "neutral", label = "Paused", pause = true, ink = "$text.tertiary" },
	finished = { tone = "success", label = "Finished", kind = "done", ink = "$text.tertiary" },
}

-- The order the Upcoming tab reads in, and the name of each section.
local GROUPS = { "Needs you", "Today", "Tomorrow", "Later" }

-- The suggestions the page and the create dialog offer: a catalogue worth
-- scrolling, and a hand-picked few for the dialog.
local TEMPLATES = {
	{ icon = "checklist", label = "Morning brief", body = "What needs you, before the day starts.", title = "Morning brief", prompt = "Review the open pull requests and tell me what needs attention.", cadence = "weekdays" },
	{ icon = "action-test", label = "Nightly tests", body = "Run the suite and leave a short report.", title = "Nightly tests", prompt = "Run the suite, triage failures, and leave a short report.", cadence = "daily" },
	{ icon = "calendar", label = "Weekly review", body = "Merged work, risks and what comes next.", title = "Weekly review", prompt = "Summarize merged work and next week's priorities.", cadence = "weekly" },
	{ icon = "git-branch", label = "PR triage", body = "Sort the queue by risk and tell me where to start.", title = "PR triage", prompt = "Sort the open pull requests by risk and tell me where to start.", cadence = "weekdays" },
	{ icon = "document", label = "Release notes", body = "Draft them from the week's merged pull requests.", title = "Release notes", prompt = "Draft release notes from the week's merged pull requests.", cadence = "weekly" },
	{ icon = "shield-check", label = "Dependency watch", body = "Check for updates and security advisories.", title = "Dependency watch", prompt = "Check for outdated dependencies and security advisories.", cadence = "weekly" },
	{ icon = "file-code", label = "Docs drift", body = "Find docs that no longer match the code.", title = "Docs drift", prompt = "Find documentation that no longer matches the code, and open a pull request with the fixes.", cadence = "weekdays" },
	{ icon = "lock-keyhole-unlocked", label = "Security audit", body = "Look for exposed secrets and risky changes.", title = "Security audit", prompt = "Audit the repository for exposed secrets, risky dependencies and suspicious changes.", cadence = "weekly" },
	{ icon = "document", label = "Changelog", body = "Keep the changelog in step with what shipped.", title = "Changelog", prompt = "Update the changelog from the week's merged pull requests.", cadence = "weekly" },
	{ icon = "action-test", label = "Flaky tests", body = "Hunt down tests that fail for no reason.", title = "Flaky tests", prompt = "Find the flakiest tests from recent runs and suggest fixes.", cadence = "daily" },
	{ icon = "inbox", label = "Issue triage", body = "Sort new issues by severity and owner.", title = "Issue triage", prompt = "Triage new issues by severity, label them, and tell me which need a decision.", cadence = "weekdays" },
	{ icon = "worktree", label = "Cleanup worktrees", body = "Remove stale worktrees and branches.", title = "Cleanup worktrees", prompt = "Find worktrees and branches older than a week that are safe to remove, and clean them up.", cadence = "weekly" },
	{ icon = "git-branch", label = "Branch hygiene", body = "Find branches that have already landed.", title = "Branch hygiene", prompt = "Find branches already merged into main and tell me which can be deleted.", cadence = "weekly" },
	{ icon = "chat-round-line", label = "Customer feedback", body = "Turn this week's feedback into themes.", title = "Customer feedback", prompt = "Summarize this week's customer feedback into themes and suggested next steps.", cadence = "weekdays" },
	{ icon = "inbox", label = "Inbox sweep", body = "Leave only what needs a person.", title = "Inbox sweep", prompt = "Triage unread notifications and leave only the ones that need me.", cadence = "daily" },
	{ icon = "checklist", label = "Code review digest", body = "The reviews waiting on you, in one list.", title = "Code review digest", prompt = "List the pull requests waiting on my review, newest first, with the riskiest first.", cadence = "weekdays" },
}

local QUICK_TEMPLATES = {}
for i = 1, math.min(6, #TEMPLATES) do
	table.insert(QUICK_TEMPLATES, TEMPLATES[i])
end

local function status_of(task: any): any
	return STATUS[task.status] or STATUS.active
end

-- The row's state mark: a glyph rather than a dot, so it says which state
-- it is at a glance. Working is the design system's own animated ring.
local function status_mark(task: any): Node
	local status = status_of(task)
	local box: { [any]: any } = {
		w = 18,
		h = 18,
		shrink = 0,
		items = "center",
		justify = "center",
		tooltip = status.label,
		label = status.label,
	}
	if task.status == "running" then
		box[1] = ds.StatusIcon { kind = "working", size = 15 }
	elseif task.status == "needs_input" then
		box[1] = ds.StatusIcon { kind = "input", size = 15 }
	elseif task.status == "missed" then
		box.color = "$warning"
		box[1] = ds.Icon { name = "clock-circle", size = 14 }
	elseif task.status == "finished" then
		box[1] = ds.StatusIcon { kind = "done", size = 15 }
	elseif task.status == "paused" then
		box.color = "$text.tertiary"
		box[1] = ui.row {
			gap = ui.raw(2, "the pause mark's two bars"),
			items = "center",
			ui.div { w = 2, h = 7, rounded = "$radius.full", bg = "$text.tertiary" },
			ui.div { w = 2, h = 7, rounded = "$radius.full", bg = "$text.tertiary" },
		}
	else
		box.color = "$text.tertiary"
		box[1] = ds.Icon { name = status.glyph or "clock-circle", size = 14 }
	end
	return ui.div(box)
end

-- What stands at a row's end: how soon the task runs, or how it ended.
local function when_of(task: any): string
	if task.status == "running" then
		return if task.elapsed and task.elapsed ~= "" then `Running {task.elapsed}` else "Running now"
	elseif task.status == "needs_input" then
		return "Needs input"
	elseif task.status == "paused" then
		return "Paused"
	elseif task.status == "missed" then
		return "Missed"
	elseif task.status == "finished" then
		return "Finished"
	end
	return if task.next_in and task.next_in ~= "" then task.next_in else task.next
end

-- The ink of what stands at a row's end: only the two live states speak
-- above a whisper.
local function when_ink(task: any): string
	if task.status == "running" then
		return "$accent"
	elseif task.status == "needs_input" or task.status == "missed" then
		return "$warning"
	end
	return "$text.tertiary"
end

-- Whether every word of `query` is in what the task says (the board's
-- search): its title, prompt, schedule, where it sends.
local function matches(task: any, query: string): boolean
	local text = string.lower(
		`{task.title or ""} {task.prompt or ""} {task.schedule or ""} {task.target or ""} {task.project or ""}`
	)
	for word in string.gmatch(string.lower(query), "%S+") do
		if not string.find(text, word, 1, true) then
			return false
		end
	end
	return true
end

-- The rows the current tab and search show, in order.
local function shown(tab: string, query: string): { any }
	local out = {}
	for _, task in store.visible(tab) do
		if matches(task, query) then
			table.insert(out, task)
		end
	end
	return out
end

-- The last run, as a row's second line ends: its result and how long it
-- took, with a glyph that says whether it finished, ran or failed.
local function last_run(task: any): Node
	local ok = task.last_ok
	local name = if ok == true then "check-circle" elseif ok == nil then "clock-circle" else "close-circle"
	local ink = if ok == false then "$danger" else "$text.tertiary"
	return ui.row {
		gap = "$space.1",
		items = "center",
		shrink = 0,
		ui.div { color = ink, ds.Icon { name = name, size = 12 } },
		ds.Text {
			task.last_result or "Not run yet",
			style = "caption",
			tone = "tertiary",
			truncate = true,
			max_w = 190,
			tooltip = task.last or task.last_result,
		},
	}
end

-- The two lines of a task row, and optionally the quick action at its end.
-- The mark starts the title's column, so the state reads before the name.
local function row_content(task: any, trailing: Node?): Node
	local status = status_of(task)
	return ui.row {
		w_full = true,
		gap = "$space.3",
		items = "center",
		px = "$space.4",
		py = "$space.3",
		status_mark(task),
		ui.column {
			grow = 1,
			min_w = 0,
			gap = "$space.1",
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				ds.FadeText {
					task.title,
					grow = 1,
					min_w = 0,
					text_size = "$font.size.md",
					weight = "$font.weight.medium",
					color = "$text.primary",
				},
				ui.text {
					when_of(task),
					font = "$font.mono",
					text_size = "$font.size.xs",
					color = when_ink(task),
					tabular = true,
					tooltip = task.next or when_of(task),
				},
			},
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				ui.div {
					grow = 1,
					min_w = 0,
					ds.Text {
						`{task.schedule} · {task.target}`,
						style = "caption",
						tone = "tertiary",
						truncate = true,
						w_full = true,
						tooltip = `Sends to {task.target}`,
					},
				},
				last_run(task),
			},
		},
		trailing,
	}
end

-- A row's one quick action: run it, stop it, resume it, or answer it.
local function quick(task: any, hovered: State<boolean>): Node
	local icon, label, action
	if task.status == "running" then
		icon, label = "alt-arrow-right", "Open the run"
		action = function()
			store.open_task(task)
		end
	elseif task.status == "needs_input" then
		icon, label = "alt-arrow-right", "Answer in the chat"
		action = function()
			store.open_task(task)
		end
	elseif task.status == "paused" then
		icon, label = "action-play", "Resume task"
		action = function()
			store.toggle(task.id, function(ok: boolean, err: string?)
				if ok then
					ds.toast.show { title = `Resumed “{task.title}”`, tone = "success" }
				else
					ds.toast.show { title = "Couldn't resume", body = err or "", tone = "danger" }
				end
			end)
		end
	else
		icon = if task.status == "finished" then "restart" else "action-play"
		label = "Run now"
		action = function()
			store.run_now(task.id, function(ok: boolean, err: string?)
				if ok then
					ds.toast.show { title = `Running “{task.title}”`, tone = "info" }
				else
					ds.toast.show { title = "Couldn't run", body = err or "", tone = "danger" }
				end
			end)
		end
	end
	return ui.div {
		w = 24,
		shrink = 0,
		items = "center",
		opacity = ui.tween(ui.computed(function(): number
			return if hovered:get() then 1 else 0.55
		end), { duration = 90 }),
		ds.IconButton {
			icon = icon,
			label = label,
			size = "sm",
			tone = "quiet",
			tooltip_side = "left",
			on_click = action,
		},
	}
end

local TaskRow = ui.component(function(props: { task: any }): Node
	local task = props.task
	local hovered = ui.use_state(false)
	return ListRow {
		label = task.title,
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		selected = function(): boolean
			return store.selected:get() == task.id
		end,
		on_click = function()
			store.select(task.id)
		end,
		row_content(task, quick(task, hovered)),
	}
end)

-- A row that comes in as the list changes: a short fade, a few points of
-- travel, each row a beat after the one before it (never more than a
-- handful, so a long list still settles at once).
local function TaskItem(task: any, index: number): Node
	return ui.presence(function(): string
		return `{task.id}:{task.status}`
	end, function(): Node
		return TaskRow { task = task }
	end, {
		w_full = true,
		enter = {
			opacity = { 0, 1 },
			y = { 5, 0 },
			duration = 150,
			delay = math.min(index, 6) * 16,
		},
		exit = { opacity = 0, y = { 0, -3 }, duration = 70 },
	})
end

-- The section label over a day's rows: a quiet word and its count.
local function group_head(name: string, count: number): Node
	return ui.row {
		w_full = true,
		gap = "$space.2",
		items = "center",
		px = "$space.4",
		pt = "$space.3",
		pb = "$space.1",
		ui.text {
			name,
			text_size = "$font.size.xs",
			weight = "$font.weight.medium",
			color = "$text.tertiary",
		},
		ui.text {
			tostring(count),
			text_size = "$font.size.xs",
			color = "$text.tertiary",
			tabular = true,
		},
	}
end

-- The search heading the card, as the board's: a quiet field set in from
-- the card's edge, its corner derived so its focus ring sits inside it.
local function search(): Node
	return ui.div {
		w_full = true,
		shrink = 0,
		p = "$space.1",
		theme = { radius = { md = control.nest(git.RADIUS, git.EDGE, "$space.1") } },
		ds.SearchField {
			value = store.query,
			placeholder = "Search scheduled tasks",
			name = "Search scheduled tasks",
			size = "sm",
			quiet = true,
			w_full = true,
			results = function(): number
				return #shown(store.tab:get(), store.query:get())
			end,
		},
	}
end

-- The empty words for the tab, or for a search that found nothing.
local function empty_state(tab: string, query: string): Node
	if query ~= "" then
		return ds.EmptyState {
			icon = "search",
			size = "sm",
			title = `No matches for “{query}”`,
			body = "Try another word, or clear the search.",
			action = {
				label = "Clear search",
				on_click = function()
					store.query:set("")
				end,
			},
		}
	elseif tab == "paused" then
		return ds.EmptyState {
			icon = "clock-circle",
			size = "sm",
			title = "Nothing paused",
			body = "Paused tasks wait here until you resume them.",
		}
	elseif tab == "finished" then
		return ds.EmptyState {
			icon = "check-circle",
			size = "sm",
			title = "Nothing finished yet",
			body = "A task stays here after its last run, with what happened.",
		}
	end
	return ds.EmptyState {
		icon = "calendar",
		size = "sm",
		title = "No upcoming tasks",
		body = "Tasks you schedule run on their own, then report back in their chat.",
		action = {
			label = "New task",
			on_click = store.open_create,
		},
	}
end

-- The rows of the card, in the current tab and search. Keyed, so a tab
-- change moves rows rather than remounting the card (which used to look
-- like it opened again).
local function row_entries(tab: string, query: string): { any }
	local tasks = shown(tab, query)
	local entries: { any } = {}
	if tab == "upcoming" then
		for _, group in GROUPS do
			local members = {}
			for _, task in tasks do
				if (task.group or "Later") == group then
					table.insert(members, task)
				end
			end
			if #members > 0 then
				if #entries > 0 then
					table.insert(entries, { kind = "rule", key = `rule:{group}` })
				end
				table.insert(entries, { kind = "group", key = `group:{group}`, name = group, count = #members })
				for i, task in members do
					if i > 1 then
						table.insert(entries, { kind = "rule", key = `rule:{group}:{i}` })
					end
					table.insert(entries, { kind = "task", key = task.id, task = task, index = i - 1 })
				end
			end
		end
	else
		for i, task in tasks do
			if i > 1 then
				table.insert(entries, { kind = "rule", key = `rule:{tab}:{i}` })
			end
			table.insert(entries, { kind = "task", key = task.id, task = task, index = i - 1 })
		end
	end
	return entries
end

-- The rows of the card, in the current tab and search. The card itself
-- stays; only its rows come again as the data does.
local function rows(): Node
	return ui.presence(function(): string
		local tab = store.tab:get()
		local query = store.query:get()
		local parts = {}
		for _, task in shown(tab, query) do
			table.insert(parts, `{task.id}:{task.status}:{task.group or ""}:{task.next_in or ""}:{task.last_result or ""}`)
		end
		return `{tab}:{query}:{#store.tasks:get()}:{tostring(store.loading:get())}:{store.error:get() or ""}:{table.concat(parts, "|")}`
	end, function(_key: string): Node
		local tab = store.tab:peek()
		local query = store.query:peek()
		local tasks = shown(tab, query)
		if #store.tasks:peek() == 0 then
			if store.loading:peek() then
				return ui.column {
					w_full = true,
					py = "$space.3",
					px = "$space.4",
					gap = "$space.1",
					ds.SkeletonRow { title = 220, line = 150, trail = 64 },
					ds.SkeletonRow { title = 190, line = 130, trail = 64 },
					ds.SkeletonRow { title = 240, line = 170, trail = 64 },
				}
			end
			local err = store.error:peek()
			if err then
				return ui.column {
					w_full = true,
					py = "$space.5",
					px = "$space.4",
					ds.EmptyState {
						icon = "clock-circle",
						size = "sm",
						title = "Scheduled tasks are starting",
						body = err,
						action = {
							label = "Try now",
							on_click = function()
								store.refresh()
							end,
						},
					},
				}
			end
		end
		if #tasks == 0 then
			return ui.column {
				w_full = true,
				py = "$space.5",
				px = "$space.4",
				empty_state(tab, query),
			}
		end
		local entries = row_entries(tab, query)
		local list: { Node } = {}
		for _, entry in entries do
			if entry.kind == "task" then
				table.insert(list, TaskItem(entry.task, entry.index))
			elseif entry.kind == "group" then
				table.insert(list, group_head(entry.name, entry.count))
			else
				table.insert(list, git.rule())
			end
		end
		return ui.column { w_full = true, gap = 0, table.unpack(list) }
	end, {
		w_full = true,
		enter = { opacity = { 0.35, 1 }, duration = 130 },
	})
end

-- The page's head: what it is and how many there are, and the one action
-- that matters. Under it, what is happening now and what comes next.
local function summary_text(): string
	if store.loading:get() and #store.tasks:get() == 0 then
		return "Loading scheduled tasks…"
	end
	if store.error:get() and #store.tasks:get() == 0 then
		return "The scheduled service is starting."
	end
	local c = store.counts()
	local bits = {}
	if c.active > 0 then
		table.insert(bits, `{c.active} active`)
	end
	if c.running > 0 then
		table.insert(bits, `{c.running} running`)
	end
	if c.needs > 0 then
		table.insert(bits, if c.needs == 1 then "1 needs you" else `{c.needs} need you`)
	end
	if c.missed > 0 then
		table.insert(bits, if c.missed == 1 then "1 missed" else `{c.missed} missed`)
	end
	local soon = store.next_up()
	if soon and soon.next_in and soon.next_in ~= "" then
		table.insert(bits, `next {soon.next_in}`)
	end
	if #bits == 0 then
		return "Nothing scheduled yet."
	end
	return table.concat(bits, " · ")
end

local function head(): Node
	return ui.column {
		w_full = true,
		gap = "$space.2",
		ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			ds.Text { "Scheduled tasks", style = "title", heading = 1 },
			ui.text {
				function(): string
					return tostring(#store.tasks:get())
				end,
				text_size = "$font.size.md",
				color = "$text.tertiary",
				tabular = true,
			},
			ui.div { grow = 1 },
			ds.Button {
				tone = "primary",
				size = "sm",
				on_click = store.open_create,
				ui.div { y = -0.5, ds.Icon { name = "plus", size = 14 } },
				"New task",
			},
		},
		ui.text {
			summary_text,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
			wrap = true,
		},
	}
end

-- The one task stopped on a question or a missed run: a quiet callout above
-- the tabs, not a banner over the page.
local function attention(): Node?
	local missed = store.missed_task()
	if missed then
		return ui.row {
			w_full = true,
			gap = "$space.3",
			items = "center",
			p = "$space.3",
			rounded = "$radius.lg",
			bg = "$surface.sunken",
			border = 1,
			border_color = "$border.subtle",
			ui.div { color = "$warning", ds.Icon { name = "clock-circle", size = 18 } },
			ui.column {
				grow = 1,
				min_w = 0,
				gap = 0,
				ui.text {
					`{missed.title} was missed while Zeron was closed`,
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
					color = "$text.primary",
				},
				ui.text {
					`It was due {missed.missed_when or missed.next}. Run it now, or put it back on its schedule.`,
					text_size = "$font.size.xs",
					color = "$text.secondary",
					truncate = true,
				},
			},
			ds.Button {
				"Run now",
				tone = "secondary",
				size = "xs",
				on_click = function()
					store.run_now(missed.id, function(ok: boolean, err: string?)
						if ok then
							ds.toast.show { title = `Running "{missed.title}"`, tone = "info" }
						else
							ds.toast.show { title = "Couldn't run", body = err or "", tone = "danger" }
						end
					end)
				end,
			},
			ds.Button {
				"Reschedule",
				tone = "ghost",
				size = "xs",
				on_click = function()
					store.reschedule(missed.id, function(ok: boolean, err: string?)
						if ok then
							ds.toast.show { title = `Rescheduled "{missed.title}"`, tone = "success" }
						else
							ds.toast.show { title = "Couldn't reschedule", body = err or "", tone = "danger" }
						end
					end)
				end,
			},
		}
	end
	local task = store.needs_attention()
	if not task then
		return nil
	end
	return ui.row {
		w_full = true,
		gap = "$space.3",
		items = "center",
		p = "$space.3",
		rounded = "$radius.lg",
		bg = "$surface.sunken",
		border = 1,
		border_color = "$border.subtle",
		ds.StatusIcon { kind = "input", size = 18 },
		ui.column {
			grow = 1,
			min_w = 0,
			gap = 0,
			ui.text {
				`{task.title} needs your input`,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.primary",
			},
			ui.text {
				task.question or "The run is waiting for your answer in the chat.",
				text_size = "$font.size.xs",
				color = "$text.secondary",
				truncate = true,
			},
		},
		ds.Button {
			"Answer",
			tone = "secondary",
			size = "xs",
			on_click = function()
				store.open_task(task)
			end,
		},
	}
end

-- The callout's slot: it arrives and leaves softly as the task it names
-- comes or goes.
local function attention_slot(): Node
	return ui.presence(function(): string
		local missed = store.missed_task()
		if missed then
			return `missed:{missed.id}`
		end
		local task = store.needs_attention()
		return if task then `needs:{task.id}` else ""
	end, function(key: string): Node
		if key == "" then
			return ui.div {}
		end
		return attention() or ui.div {}
	end, {
		w_full = true,
		enter = { opacity = { 0, 1 }, y = { -4, 0 }, duration = 160 },
		exit = { opacity = 0, y = { 0, -3 }, duration = 80 },
	})
end

-- The states as underlined tabs, each with its count. The bar is laid
-- out here rather than measured: it keeps its place as rows come and go.
local TAB_AT = { upcoming = 0, paused = 98, finished = 180 }
local TAB_W = { upcoming = 82, paused = 66, finished = 72 }

local function filters(): Node
	local function current(): string
		return store.tab:get()
	end
	local x = ui.tween(ui.computed(function(): number
		return TAB_AT[current()] or 0
	end), { duration = 180, ease = "out(3)" })
	local w = ui.tween(ui.computed(function(): number
		return TAB_W[current()] or 82
	end), { duration = 150, ease = "out(3)" })
	local tabs: { Node } = {}
	for _, tab in TABS do
		local function on(): boolean
			return current() == tab.value
		end
		table.insert(tabs, ui.row {
			role = "tab",
			selected = on,
			label = tab.label,
			focusable = true,
			w = TAB_W[tab.value] or 82,
			h = 36,
			px = "$space.1",
			gap = "$zeron.space.tight",
			items = "center",
			shrink = 0,
			rounded = "$radius.sm",
			cursor = "pointer",
			color = function(): string
				return if on() then "$text.primary" else "$text.secondary"
			end,
			hover = { color = "$text.primary" },
			on_click = function()
				store.tab:set(tab.value)
			end,
			ui.text {
				tab.label,
				text_size = "$font.size.md",
				weight = "$font.weight.medium",
			},
			ui.text {
				function(): string
					return tostring(#store.visible(tab.value))
				end,
				font = "$font.mono",
				text_size = "$font.size.xs",
				color = function(): string
					return if on() then "$text.secondary" else "$text.tertiary"
				end,
				tabular = true,
			},
		})
	end
	return ui.div {
		id = "scheduled-filters",
		w_full = true,
		ui.column {
			relative = true,
			w_full = true,
			ui.div { absolute = true, left = 0, right = 0, bottom = 0, h = 1, bg = "$border.subtle" },
			ui.row {
				role = "tablist",
				label = "Task state",
				relative = true,
				gap = "$space.4",
				items = "center",
				ui.div {
					absolute = true,
					left = 0,
					bottom = 0,
					h = 2,
					rounded = "$radius.full",
					bg = "$text.primary",
					x = x,
					w = w,
					pointer_events = "none",
				},
				table.unpack(tabs),
			},
		},
	}
end

-- What the list says of itself: how many, and the clock the times are in.
local function footer(): Node
	local function count(): string
		local tab, query = store.tab:get(), store.query:get()
		local n = #shown(tab, query)
		local all = #store.tasks:get()
		if query ~= "" then
			return `{n} of {all} scheduled tasks`
		end
		return if n == 1 then "1 scheduled task" else `{n} scheduled tasks`
	end
	return ui.row {
		w_full = true,
		gap = "$space.3",
		items = "center",
		ds.Text { count, style = "caption", tone = "tertiary", grow = 1 },
	}
end

local CADENCE = { once = "Once", daily = "Every day", weekdays = "Weekdays", weekly = "Weekly", custom = "Custom" }

-- A template: a small flat card, the same border and corner as the list's
-- card, that fills the create dialog with a job worth keeping.
local TemplateCard = ui.component(function(props: { data: any }): Node
	local s = props.data
	local hovered = ui.use_state(false)
	return ds.Surface {
		kind = "plain",
		interactive = true,
		w = 262,
		h = 112,
		shrink = 0,
		rounded = "$radius.lg",
		bg = "$surface.base",
		border = 1,
		border_color = "$border.subtle",
		hover = { border_color = "$border.strong" },
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		on_click = function()
			store.prefill(s.title, s.prompt, s.cadence)
		end,
		ui.column {
			w_full = true,
			h_full = true,
			p = "$space.3",
			gap = "$space.2",
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				ui.div {
					w = 24,
					h = 24,
					shrink = 0,
					items = "center",
					justify = "center",
					rounded = "$radius.md",
					bg = "$surface.sunken",
					color = "$text.secondary",
					ds.Icon { name = s.icon, size = 14 },
				},
				ui.text {
					s.label,
					grow = 1,
					min_w = 0,
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
					color = "$text.primary",
				},
				ui.div {
					color = "$text.tertiary",
					opacity = ui.tween(ui.computed(function(): number
						return if hovered:get() then 1 else 0.45
					end), { duration = 120 }),
					x = ui.tween(ui.computed(function(): number
						return if hovered:get() then 0 else -2
					end), { duration = 120 }),
					ds.Icon { name = "arrow-up-right", size = 12 },
				},
			},
			ds.Text {
				s.body,
				style = "caption",
				tone = "tertiary",
				wrap = true,
				grow = 1,
			},
			ui.text {
				CADENCE[s.cadence] or s.cadence,
				font = "$font.mono",
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
		},
	}
end)

-- The rail's ends: a short breath of the page's own colour and blur, so a
-- card leaving is dimmed rather than sliced at the viewport's edge.
local function rail_veil(): string
	local c = ui.theme.get("background")
	return if type(c) == "string" then c else "#00000000"
end

local function veil(alpha: string): string
	local c = rail_veil()
	return if string.sub(c, 1, 1) == "#" and #c == 7 then c .. alpha else c
end

-- The tape: one row of every template, then the same row again, moving
-- slowly and without a seam. Under the pointer its speed eases to zero
-- (not a hard stop) and eases back on the way out.
local CARD_W, CARD_GAP, LOOP_MS = 262, 12, 200000
-- The rail's visible width inside the page's column, for the edge fade.
local RAIL_W = 934
local TRAVEL = #TEMPLATES * (CARD_W + CARD_GAP)
local TRACK_W = 2 * #TEMPLATES * CARD_W + (2 * #TEMPLATES - 1) * CARD_GAP

local TemplateTape = ui.component(function(): Node
	local offset = ui.state("scheduled_template_offset_3", -72)
	local speed = ui.state("scheduled_template_speed_3", 1)
	local hovered = ui.use_state(false)
	local visible = ui.use_state(true)
	-- Thirty frames a second, while it is on screen: the tape advances by
	-- exactly what one loop's share of the time is, and its speed eases
	-- toward zero under the pointer and back after it leaves.
	ui.every(33, function()
		if not visible:get() then
			return
		end
		local want = if hovered:get() then 0 else 1
		local now = speed:peek()
		now += (want - now) * 0.16
		if math.abs(want - now) < 0.004 then
			now = want
		end
		if now ~= speed:peek() then
			speed:set(now)
		end
		if now <= 0.001 then
			return
		end
		local step = TRAVEL * 33 / LOOP_MS * now
		local at = offset:peek() - step
		if at <= -TRAVEL then
			at += TRAVEL
		end
		offset:set(at)
	end)
	local all: { Node } = {}
	for _ = 1, 2 do
		for _, s in TEMPLATES do
			table.insert(all, TemplateCard { data = s })
		end
	end
	return ui.div {
		w_full = true,
		h = 116,
		relative = true,
		ui.scroll_x {
			w_full = true,
			h = 116,
			on_hover = function(inside: boolean)
				hovered:set(inside)
			end,
			on_visible = function(shown: boolean)
				visible:set(shown)
			end,
			ui.row {
				w = TRACK_W,
				shrink = 0,
				gap = "$space.3",
				items = "start",
				x = offset,
				table.unpack(all),
			},
		},
		ui.overlay {
			absolute = true,
			left = 0,
			top = 2,
			bottom = 2,
			w = 8,
			pointer_events = "none",
			z = 2,
			gradient = {
				angle = 90,
				stops = {
					{ 0, veil("d8") },
					{ 1, veil("00") },
				},
			},
		},
		ui.overlay {
			absolute = true,
			right = 0,
			top = 2,
			bottom = 2,
			w = 8,
			pointer_events = "none",
			z = 2,
			gradient = {
				angle = 90,
				stops = {
					{ 0, veil("00") },
					{ 1, veil("e0") },
				},
			},
		},
	}
end)

local function templates(): Node
	local header = ui.row {
		w_full = true,
		gap = "$space.2",
		items = "center",
		ui.text {
			"Start from a template",
			text_size = "$font.size.sm",
			weight = "$font.weight.medium",
			color = "$text.primary",
		},
		ui.div { grow = 1 },
		ui.text {
			`{tostring(#TEMPLATES)} templates`,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		},
	}
	return ui.column {
		w_full = true,
		gap = "$space.3",
		pt = "$space.1",
		header,
		TemplateTape {},
	}
end

local function list_page(): Node
	return ui.scroll {
		w_full = true,
		h_full = true,
		fade = { 16, 16 },
		fade_overflow = true,
		ui.column {
			w_full = true,
			min_h = "100%",
			items = "center",
			px = "$space.5",
			pt = "$space.6",
			pb = "$space.6",
			ui.column {
				w_full = true,
				max_w = ui.raw(WIDTH, "the scheduled page's column"),
				gap = "$space.5",
				head(),
				attention_slot(),
				filters(),
				ui.column {
					w_full = true,
					gap = "$space.3",
					git.card(search(), { rows() }),
					footer(),
				},
				templates(),
			},
		},
	}
end

-- A run that was due while Zeron was closed: kept, with the two ways
-- forward, in its own page.
local function missed_card(task: any): Node?
	if task.status ~= "missed" then
		return nil
	end
	local function run_now()
		store.run_now(task.id, function(ok: boolean, err: string?)
			if ok then
				ds.toast.show { title = `Running "{task.title}"`, tone = "info" }
			else
				ds.toast.show { title = "Couldn't run", body = err or "", tone = "danger" }
			end
		end)
	end
	local function reschedule()
		store.reschedule(task.id, function(ok: boolean, err: string?)
			if ok then
				ds.toast.show { title = `Rescheduled "{task.title}"`, tone = "success" }
			else
				ds.toast.show { title = "Couldn't reschedule", body = err or "", tone = "danger" }
			end
		end)
	end
	return ui.row {
		w_full = true,
		gap = "$space.3",
		items = "center",
		p = "$space.3",
		rounded = "$radius.lg",
		bg = "$surface.sunken",
		border = 1,
		border_color = "$border.subtle",
		ui.div { color = "$warning", ds.Icon { name = "clock-circle", size = 18 } },
		ui.column {
			grow = 1,
			min_w = 0,
			gap = 0,
			ui.text {
				"This run was missed",
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.primary",
			},
			ui.text {
				`It was due {task.missed_when or task.next}. Run it now, or put it back on its schedule.`,
				text_size = "$font.size.xs",
				color = "$text.secondary",
				wrap = true,
			},
		},
		ds.Button {
			"Run now",
			tone = "secondary",
			size = "xs",
			on_click = run_now,
		},
		ds.Button {
			"Reschedule",
			tone = "ghost",
			size = "xs",
			on_click = reschedule,
		},
	}
end

-- The question a task stopped on, in its own page: the same quiet callout
-- as the page's, with the way into the chat that holds it.
local function question_card(task: any): Node?
	if task.status ~= "needs_input" then
		return nil
	end
	return ui.row {
		w_full = true,
		gap = "$space.3",
		items = "center",
		p = "$space.3",
		rounded = "$radius.lg",
		bg = "$surface.sunken",
		border = 1,
		border_color = "$border.subtle",
		ds.StatusIcon { kind = "input", size = 18 },
		ui.column {
			grow = 1,
			min_w = 0,
			gap = 0,
			ui.text {
				"Waiting for your answer",
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.primary",
			},
			ui.text {
				task.question or "The run is waiting for your answer in the chat.",
				text_size = "$font.size.xs",
				color = "$text.secondary",
				wrap = true,
			},
		},
		ds.Button {
			"Open chat",
			tone = "secondary",
			size = "xs",
			on_click = function()
				store.open_task(task)
			end,
		},
	}
end

-- A task's model as the detail's card shows it: its agent's mark, its name,
-- and where it is served from (or the chat's own model when none is set).
local function model_row(task: any): Node
	local value = store.task_model_value(task)
	local mark = store.task_model_mark(task)
	return ui.row {
		w_full = true,
		gap = "$space.2",
		items = "center",
		if mark
			then ds.Mark { name = mark, size = 18 }
			else ds.Icon { name = "magic-stick", size = 15 },
		ui.column {
			gap = 0,
			min_w = 0,
			ui.text {
				function(): string
					return store.task_model_name(task)
				end,
				text_size = "$font.size.sm",
				color = "$text.primary",
				shrink = 1,
			},
			ui.text {
				function(): string
					return if value then store.model_hint(value) else "The chat's own model"
				end,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
				shrink = 1,
			},
		},
	}
end

-- The model chosen in the dialog, as the review shows it.
local function chosen_model_row(): Node
	local value = store.new_model:get()
	local mark = store.value_mark(value)
	local name = store.model_name(value)
	if name == "" then
		name = "Default model"
	end
	local hint = store.model_hint(value)
	if hint == "" then
		hint = "The chat's own model"
	end
	return ui.row {
		w_full = true,
		gap = "$space.2",
		items = "center",
		if mark
			then ds.Mark { name = mark, size = 18 }
			else ds.Icon { name = "magic-stick", size = 15 },
		ui.column {
			gap = 0,
			min_w = 0,
			ui.text { name, text_size = "$font.size.sm", color = "$text.primary", shrink = 1 },
			ui.text { hint, text_size = "$font.size.xs", color = "$text.tertiary", shrink = 1 },
		},
	}
end

-- A label and its value, as the detail's card reads them.
local function info_row(label: string, value: any): Node
	return ui.row {
		w_full = true,
		gap = "$space.3",
		items = "start",
		px = "$space.4",
		py = "$space.3",
		ui.text {
			label,
			w = 96,
			shrink = 0,
			text_size = "$font.size.sm",
			color = "$text.tertiary",
		},
		if type(value) == "string"
			then ui.text {
				value,
				grow = 1,
				min_w = 0,
				text_size = "$font.size.sm",
				color = "$text.primary",
				wrap = true,
			}
			else value,
	}
end

local function history_row(run: any): Node
	local ink = if run.ok == true then "$success" elseif run.ok == false then "$danger" else "$text.tertiary"
	return ui.row {
		w_full = true,
		gap = "$space.2",
		items = "center",
		px = "$space.4",
		py = "$space.2",
		ui.div {
			color = ink,
			ds.Icon {
				name = if run.ok == true then "check-circle" elseif run.ok == nil then "clock-circle" else "close-circle",
				size = 14,
			},
		},
		ui.text {
			run.when,
			font = "$font.mono",
			text_size = "$font.size.sm",
			color = "$text.secondary",
			grow = 1,
		},
		ui.text { run.outcome, text_size = "$font.size.xs", color = "$text.tertiary" },
	}
end

-- The next runs, when the task is still going to have them.
local function upcoming_card(task: any): Node?
	local runs = task.upcoming
	if not runs or #runs == 0 then
		return nil
	end
	local rows: { Node } = {}
	for _, run in runs do
		table.insert(rows, ui.row {
			w_full = true,
			gap = "$space.3",
			items = "center",
			px = "$space.4",
			py = "$space.2",
			ui.text {
				run.at,
				font = "$font.mono",
				text_size = "$font.size.sm",
				color = "$text.secondary",
				grow = 1,
			},
			ui.text { run.eta, text_size = "$font.size.xs", color = "$text.tertiary", tabular = true },
		})
	end
	return git.card(ui.row {
		w_full = true,
		items = "center",
		px = "$space.4",
		py = "$space.3",
		ui.text {
			"Coming up",
			text_size = "$font.size.sm",
			weight = "$font.weight.medium",
			color = "$text.primary",
			grow = 1,
		},
		ui.text {
			`{tostring(#runs)} runs`,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
			tabular = true,
		},
	}, rows)
end

local function detail_page(task: any): Node
	local status = status_of(task)
	local run_count = #(task.history or {})
	local meta = `{task.schedule} · {task.target}`
	if task.project and task.project ~= "" then
		meta = `{meta} · {task.project}`
	end
	local history: { Node } = {}
	for _, run in task.history or {} do
		table.insert(history, history_row(run))
	end
	if #history == 0 then
		table.insert(history, ui.column {
			w_full = true,
			px = "$space.4",
			py = "$space.4",
			ui.text { "No runs yet.", text_size = "$font.size.sm", color = "$text.tertiary" },
		})
	end
	local primary_label = if task.status == "running" or task.status == "needs_input"
		then "Open chat"
		elseif task.status == "finished" then "Run again"
		else "Run now"
	local function primary_action()
		if task.status == "running" or task.status == "needs_input" then
			store.open_task(task)
			return
		end
		store.run_now(task.id, function(ok: boolean, err: string?)
			if ok then
				ds.toast.show { title = `Running “{task.title}”`, tone = "info" }
			else
				ds.toast.show { title = "Couldn't run", body = err or "", tone = "danger" }
			end
		end)
	end
	return ui.scroll {
		w_full = true,
		h_full = true,
		fade = { 16, 16 },
		fade_overflow = true,
		ui.column {
			w_full = true,
			min_h = "100%",
			items = "center",
			px = "$space.5",
			pt = "$space.5",
			pb = "$space.6",
			ui.column {
				w_full = true,
				max_w = ui.raw(780, "the scheduled detail's column"),
				gap = "$space.5",
				ds.Breadcrumbs {
					items = {
						{ label = "Scheduled tasks", icon = "calendar", on_click = store.clear },
						{ label = task.title },
					},
					size = "md",
				},
				ui.row {
					w_full = true,
					gap = "$space.4",
					items = "end",
					flex_wrap = true,
					ui.column {
						grow = 1,
						min_w = 380,
						gap = "$space.2",
						ui.row {
							w_full = true,
							gap = "$space.2",
							items = "center",
							ds.Text { task.title, style = "title", heading = 1, shrink = 1, min_w = 0, wrap = true },
							ds.Badge {
								status.label,
								tone = status.tone,
								size = "md",
							},
							ui.div { grow = 1 },
						},
						ui.text {
							meta,
							text_size = "$font.size.sm",
							color = "$text.tertiary",
							wrap = true,
						},
						ui.row {
							w_full = true,
							gap = "$space.2",
							items = "center",
							flex_wrap = true,
							ds.Button {
								primary_label,
								tone = "primary",
								size = "sm",
								on_click = primary_action,
							},
							if task.status == "running" then
								ds.Button {
									if task.pause_after then "Pausing after this run" else "Pause after this run",
									tone = "secondary",
									size = "sm",
									disabled = task.pause_after == true,
									on_click = function()
										store.toggle(task.id, function(ok: boolean, err: string?)
											if ok then
												ds.toast.show { title = "It pauses when this run ends", tone = "info" }
											else
												ds.toast.show { title = "Couldn't pause", body = err or "", tone = "danger" }
											end
										end)
									end,
								}
							elseif task.status == "paused" then
								ds.Button {
									"Resume",
									tone = "secondary",
									size = "sm",
									on_click = function()
										store.toggle(task.id, function(ok: boolean, err: string?)
											if ok then
												ds.toast.show { title = `Resumed “{task.title}”`, tone = "success" }
											else
												ds.toast.show { title = "Couldn't resume", body = err or "", tone = "danger" }
											end
										end)
									end,
								}
							elseif task.status == "active" then
								ds.Button {
									"Pause",
									tone = "secondary",
									size = "sm",
									on_click = function()
										store.toggle(task.id, function(ok: boolean, err: string?)
											if ok then
												ds.toast.show { title = `Paused "{task.title}"`, tone = "info" }
											else
												ds.toast.show { title = "Couldn't pause", body = err or "", tone = "danger" }
											end
										end)
									end,
								}
							elseif task.status == "missed" then
								ds.Button {
									"Reschedule",
									tone = "secondary",
									size = "sm",
									on_click = function()
										store.reschedule(task.id, function(ok: boolean, err: string?)
											if ok then
												ds.toast.show { title = `Rescheduled "{task.title}"`, tone = "success" }
											else
												ds.toast.show { title = "Couldn't reschedule", body = err or "", tone = "danger" }
											end
										end)
									end,
								}
							else
								nil,
							ds.Button {
								"Edit",
								tone = "ghost",
								size = "sm",
								on_click = function()
									store.edit(task.id)
								end,
								ds.Icon { name = "pen", size = 12 },
							},
							ds.IconButton {
								icon = "trash-bin-minimalistic",
								label = "Delete task",
								tone = "danger",
								size = "sm",
								on_click = function()
									store.ask_delete(task.id)
								end,
							},
						},
					},
					ui.column {
						shrink = 0,
						items = "end",
						gap = 0,
						ui.text {
							"NEXT RUN",
							text_size = "$font.size.xs",
							weight = "$font.weight.medium",
							color = "$text.tertiary",
						},
						ui.text {
							task.next,
							font = "$font.mono",
							text_size = "$font.size.xl",
							color = "$text.primary",
							tabular = true,
						},
						ui.text {
							if task.next_in and task.next_in ~= task.next and task.next_in ~= "Running" then task.next_in else "",
							text_size = "$font.size.xs",
							color = "$text.tertiary",
							tabular = true,
						},
					},
				},
				question_card(task),
				missed_card(task),
				git.card(nil, {
					info_row("Repeats", ui.column {
						gap = "$space.1",
						ui.text { task.schedule, text_size = "$font.size.sm", color = "$text.primary" },
						ui.text { task.timezone, text_size = "$font.size.xs", color = "$text.tertiary" },
					}),
					info_row("Sends to", ui.column {
						gap = "$space.1",
						ui.row {
							gap = "$space.1",
							items = "center",
							ui.text { task.target, text_size = "$font.size.sm", color = "$accent" },
							ds.Icon { name = "arrow-up-right", size = 12 },
						},
						ui.text {
							if task.target_new
								then "A new chat for every run"
								else (if task.project and task.project ~= "" then `This chat · {task.project}` else "This chat"),
							text_size = "$font.size.xs",
							color = "$text.tertiary",
						},
					}),
					info_row("Runs with", model_row(task)),
					info_row("Last run", task.last),
				}),
				upcoming_card(task),
				git.card(nil, {
					ui.column {
						w_full = true,
						px = "$space.4",
						py = "$space.3",
						gap = "$space.1",
						ui.text {
							"What it does",
							text_size = "$font.size.xs",
							weight = "$font.weight.medium",
							color = "$text.tertiary",
						},
						ui.text {
							task.prompt,
							text_size = "$font.size.md",
							color = "$text.primary",
							wrap = true,
						},
					},
				}),
				git.card(ui.row {
					w_full = true,
					items = "center",
					px = "$space.4",
					py = "$space.3",
					ui.text {
						"History",
						text_size = "$font.size.sm",
						weight = "$font.weight.medium",
						color = "$text.primary",
						grow = 1,
					},
					ui.text {
						if run_count == 0 then "No runs" elseif run_count == 1 then "1 run" else `{run_count} runs`,
						text_size = "$font.size.xs",
						color = "$text.tertiary",
						tabular = true,
					},
				}, history),
			},
		},
	}
end

local function suggestion_chip(s: any): Node
	return ds.Button {
		tone = "ghost",
		size = "xs",
		on_click = function()
			store.use_template(s.title, s.prompt, s.cadence)
		end,
		ui.div { y = -0.5, ds.Icon { name = s.icon, size = 12 } },
		s.label,
	}
end

-- A line under the form, not a box: one quiet fact about what a run may ask
-- of the person.
local function quiet_note(text: string): Node
	return ui.row {
		w_full = true,
		gap = "$space.2",
		items = "center",
		ds.Icon { name = "shield-check", size = 13 },
		ui.text {
			text,
			grow = 1,
			min_w = 0,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
			wrap = true,
		},
	}
end

-- The repeat choices, each saying the whole schedule, so the one field says
-- when the task runs without a second row of hours beside it.
local WHEN_OPTIONS = {
	{ value = "once", label = store.REPEAT_TEXT.once },
	{ value = "daily", label = store.REPEAT_TEXT.daily },
	{ value = "weekdays", label = store.REPEAT_TEXT.weekdays },
	{ value = "weekly", label = store.REPEAT_TEXT.weekly },
	{ value = "custom", label = store.REPEAT_TEXT.custom },
}

-- The custom schedule's cadence, and its clock: hour and minute steppers
-- with the half of the day beside them, the time they make shown in mono.
local CUSTOM_REPEATS = {
	{ value = "once", label = "Once" },
	{ value = "daily", label = "Daily" },
	{ value = "weekdays", label = "Weekdays" },
	{ value = "weekly", label = "Weekly" },
	{ value = "days", label = "Days" },
}

-- The model a task starts on: a card that opens the app's model catalog,
-- the agent's mark and the model's name on it (the provider under it).
local function model_picker(): Node
	return ui.column {
		w_full = true,
		gap = "$space.2",
		ui.text { "Model", text_size = "$font.size.xs", color = "$text.tertiary" },
		ds.Dropdown {
			value = store.new_model,
			label = "Model",
			width = 420,
			search = "Search models",
			loading = function(): boolean
				return (store.catalog:get() or {}).state ~= "ready"
			end,
			items = function(): { any }
				return store.model_items()
			end,
			empty = function(_query: string): any
				return {
					title = "No models",
					hint = "Connect an agent in Settings to choose one.",
				}
			end,
			on_choose = function(value: string)
				local model = store.model_by_value(value)
				store.new_effort:set(if model then model.effort else nil)
			end,
			actions = {
				{
					label = "Use the default model",
					icon = "refresh",
					on_click = function()
						store.new_model:set(nil)
						store.new_effort:set(nil)
					end,
				},
			},
			trigger = function(menu: any): Node
				local value = store.new_model:get()
				local model = store.model_by_value(value)
				local mark = store.value_mark(value)
				local name = if model then model.label else "Default model"
				local hint = if model then store.model_hint(value) else "The chat's own model"
				return ui.button {
					w_full = true,
					on_click = menu.toggle,
					px = "$space.3",
					py = "$space.2",
					rounded = "$radius.lg",
					bg = "$surface.raised",
					border = 1,
					border_color = "$border.subtle",
					hover = { bg = "$hover" },
					active = { bg = "$pressed" },
					cursor = "pointer",
					ui.row {
						w_full = true,
						gap = "$space.3",
						items = "center",
						ui.div {
							w = 34,
							h = 34,
							shrink = 0,
							items = "center",
							justify = "center",
							rounded = "$radius.md",
							bg = "$surface.sunken",
							if mark
								then ds.Mark { name = mark, size = 18 }
								else ds.Icon { name = "magic-stick", size = 15 },
						},
						ui.column {
							grow = 1,
							min_w = 0,
							gap = 0,
							items = "start",
							ui.text {
								name,
								text_size = "$font.size.sm",
								weight = "$font.weight.medium",
								color = "$text.primary",
								shrink = 1,
							},
							ui.text {
								hint,
								text_size = "$font.size.xs",
								color = "$text.tertiary",
								shrink = 1,
							},
						},
						ds.Icon {
							name = function(): string
								return if menu.open:get() then "alt-arrow-up" else "alt-arrow-down"
							end,
							size = 14,
							color = "$text.tertiary",
						},
					},
				}
			end,
		},
		ui.text {
			function(): string
				local only = store.target_harness()
				if only then
					return `Only {only} models can run in this chat.`
				end
				return "Used for every run. Default follows the agent's own."
			end,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		},
	}
end

local function day_chip(day: any): Node
	local function on(): boolean
		return store.day_on(day.value)
	end
	return ui.button {
		label = day.name,
		min_w = 44,
		h = 28,
		px = "$space.2",
		justify = "center",
		rounded = "$radius.full",
		border = 1,
		border_color = function(): string
			return if on() then "$accent" else "$border.subtle"
		end,
		bg = function(): string
			return if on() then "$accent.hover" else "$surface.sunken"
		end,
		color = function(): string
			return if on() then "$text.primary" else "$text.secondary"
		end,
		hover = { border_color = "$border.strong" },
		cursor = "pointer",
		on_click = function()
			store.toggle_day(day.value)
		end,
		ui.text { day.label, text_size = "$font.size.xs", weight = "$font.weight.medium" },
	}
end

local function days_picker(): Node
	local chips: { Node } = {}
	for _, day in store.DAYS do
		table.insert(chips, day_chip(day))
	end
	return ui.column {
		w_full = true,
		gap = "$space.2",
		pt = "$space.1",
		ui.text { "On these days", text_size = "$font.size.xs", color = "$text.tertiary" },
		ui.row {
			gap = "$space.1",
			items = "center",
			flex_wrap = true,
			table.unpack(chips),
		},
	}
end

local function custom_schedule(): Node
	return ui.column {
		w_full = true,
		gap = "$space.3",
		ui.column {
			w_full = true,
			gap = "$space.3",
			p = "$space.3",
			rounded = "$radius.lg",
			bg = "$surface.raised",
			border = 1,
			border_color = "$border.subtle",
			ui.row {
				w_full = true,
				gap = "$space.3",
				items = "center",
				ui.column {
					gap = 0,
					items = "start",
					ui.text { "At", text_size = "$font.size.xs", color = "$text.tertiary" },
					ui.row {
						gap = "$space.1",
						items = "baseline",
						ui.text {
							function(): string
								return string.format("%02d", math.floor(store.new_hour:get() + 0.5))
							end,
							font = "$font.mono",
							text_size = "$font.size.xxl",
							color = "$text.primary",
							tabular = true,
						},
						ui.text {
							":",
							font = "$font.mono",
							text_size = "$font.size.xxl",
							color = "$accent",
						},
						ui.text {
							function(): string
								return string.format("%02d", math.floor(store.new_minute:get() + 0.5))
							end,
							font = "$font.mono",
							text_size = "$font.size.xxl",
							color = "$text.primary",
							tabular = true,
						},
						ui.text {
							function(): string
								return store.new_period:get()
							end,
							font = "$font.mono",
							text_size = "$font.size.xs",
							color = "$text.tertiary",
						},
					},
					ui.text {
						function(): string
							return store.timezone:get()
						end,
						text_size = "$font.size.xs",
						color = "$text.tertiary",
					},
				},
				ui.div { grow = 1 },
				ui.column {
					gap = "$space.2",
					items = "end",
					ui.row {
						gap = "$space.2",
						items = "end",
						ui.column {
							gap = "$space.1",
							ui.text { "Hour", text_size = "$font.size.xs", color = "$text.tertiary" },
							ds.NumberStepper {
								value = store.new_hour,
								min = 1,
								max = 12,
								name = "Hour",
								size = "sm",
								w = 86,
							},
						},
						ui.column {
							gap = "$space.1",
							ui.text { "Minute", text_size = "$font.size.xs", color = "$text.tertiary" },
							ds.NumberStepper {
								value = store.new_minute,
								min = 0,
								max = 59,
								step = 1,
								name = "Minute",
								size = "sm",
								w = 86,
							},
						},
					},
					ds.Segmented {
						value = store.new_period,
						options = {
							{ value = "AM", label = "AM" },
							{ value = "PM", label = "PM" },
						},
						segment = 44,
					},
				},
			},
			ui.div { h = 1, w_full = true, bg = "$border.subtle" },
			ui.text { "Repeat", text_size = "$font.size.xs", color = "$text.tertiary" },
			ds.Segmented {
				value = store.new_custom_repeat,
				options = CUSTOM_REPEATS,
				fill = true,
			},
			ui.presence(function(): string
				return if store.new_custom_repeat:get() == "days" then "days" else ""
			end, function(key: string): Node
				return if key == "days" then days_picker() else ui.div {}
			end, {
				w_full = true,
				enter = { opacity = { 0, 1 }, y = { -3, 0 }, duration = 140 },
				exit = { opacity = 0, duration = 70 },
			}),
		},
	}
end

local function create_details(): Node
	local chips: { Node } = {}
	for _, s in QUICK_TEMPLATES do
		table.insert(chips, suggestion_chip(s))
	end
	return ui.column {
		w_full = true,
		gap = "$space.3",
		ds.TextArea {
			value = store.new_prompt,
			label = "What should run?",
			placeholder = "Review the open pull requests and summarize what needs my attention.",
			rows = 4,
			max_rows = 6,
			w_full = true,
			autofocus = true,
		},
		ui.column {
			w_full = true,
			gap = "$space.2",
			ui.text { "Start from", text_size = "$font.size.xs", color = "$text.tertiary" },
			ui.row {
				gap = "$space.1",
				items = "center",
				flex_wrap = true,
				table.unpack(chips),
			},
		},
		ds.TextField {
			value = store.new_title,
			label = "Name",
			placeholder = "Morning brief",
			w_full = true,
		},
		ds.Select {
			value = store.new_target,
			label = "Sends to",
			options = function(): { any }
				return store.chat_options()
			end,
			on_change = function(id: string)
				store.choose_chat(id)
			end,
			w_full = true,
			height = 480,
		},
		ui.row {
			w_full = true,
			gap = "$space.1",
			items = "center",
			ds.Icon {
				name = function(): string
					return if store.new_target:get() == "new" then "chat-add" else "chat-round-line"
				end,
				size = 12,
			},
			ui.text {
				function(): string
					return if store.new_target:get() == "new"
						then "Each run opens a chat of its own"
						else `Runs in {store.new_target_name:get()}`
				end,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
		},
		model_picker(),
		ds.Select {
			value = store.new_repeat,
			label = "When",
			options = WHEN_OPTIONS,
			w_full = true,
		},
		ui.presence(function(): string
			return if store.new_repeat:get() == "custom" then "custom" else "preset"
		end, function(which: string): Node
			return if which == "custom" then custom_schedule() else ui.div {}
		end, {
			w_full = true,
			enter = { opacity = { 0, 1 }, duration = 160 },
			exit = { opacity = 0, duration = 80 },
		}),
		quiet_note("Supervised access · a run that needs approval waits for you."),
	}
end

local function create_review(): Node
	local title = store.new_title:get()
	local prompt = store.new_prompt:get()
	if title == "" then
		title = if prompt ~= "" then string.sub(prompt, 1, 56) else "New scheduled task"
	end
	if prompt == "" then
		prompt = "Do the task and report what changed."
	end
	local schedule = store.schedule_text()
	local preview = {
		id = "preview",
		title = title,
		prompt = prompt,
		schedule = schedule,
		target = if store.new_target:get() ~= "new" then store.new_target_name:get() else "New chat",
		project = "",
		status = "active",
		next_in = "After saving",
		last_result = "Not run yet",
		last_ok = nil,
	}
	return ui.column {
		w_full = true,
		gap = "$space.4",
		ds.Button {
			"Back",
			tone = "ghost",
			size = "xs",
			on_click = function()
				store.create_step:set("details")
			end,
			ds.Icon { name = "alt-arrow-left", size = 11 },
		},
		git.card(nil, {
			row_content(preview),
			ui.column {
				w_full = true,
				px = "$space.4",
				py = "$space.3",
				gap = "$space.1",
				ui.text {
					"What it does",
					text_size = "$font.size.xs",
					weight = "$font.weight.medium",
					color = "$text.tertiary",
				},
				ui.text { prompt, text_size = "$font.size.sm", color = "$text.primary", wrap = true },
			},
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				px = "$space.4",
				py = "$space.3",
				ds.Icon { name = "chat-round-line", size = 13 },
				ui.text {
					function(): string
						return if store.new_target:get() ~= "new"
							then `Sends to this chat · {store.new_target_name:get()}`
							else "Sends to a new chat of its own"
					end,
					grow = 1,
					min_w = 0,
					text_size = "$font.size.sm",
					color = "$text.secondary",
				},
			},
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				px = "$space.4",
				py = "$space.3",
				ds.Icon { name = "magic-stick", size = 13 },
				ui.column {
					grow = 1,
					min_w = 0,
					chosen_model_row(),
				},
			},
		}),
		quiet_note("Nothing runs until its time. A run that needs approval waits for you."),
	}
end

local function create_dialog(): Node
	local function step(): string
		return store.create_step:get()
	end
	local function editing(): boolean
		return store.editing:get() ~= nil
	end
	return ds.Dialog {
		open = store.create_open,
		title = function(): string
			return if editing() then "Edit scheduled task" else "New scheduled task"
		end,
		body = function(): string
			if step() == "review" then
				return if editing() then "A last look before saving." else "A last look before it's scheduled."
			end
			if editing() then
				return "Change what it does, or when it runs."
			end
			return if store.new_target:get() ~= "new"
				then "Zeron runs it with this chat's context, then reports back here."
				else "Zeron opens a chat of its own for every run."
		end,
		size = "lg",
		width = 720,
		confirm = {
			label = function(): string
				if step() == "details" then
					return if editing() then "Review changes" else "Review task"
				end
				return if editing() then "Save changes" else "Create task"
			end,
			disabled = function(): boolean
				return store.new_prompt:get() == ""
			end,
			busy = function(): boolean
				return store.saving:get()
			end,
			keep_open = true,
			on_click = function()
				if store.create_step:peek() == "details" then
					store.create_step:set("review")
				else
					local was_editing = store.editing:peek() ~= nil
					store.submit(function(ok: boolean, err: string?)
						ds.toast.show {
							title = if not ok
								then "Couldn't save"
								elseif was_editing then "Task updated"
								else "Scheduled task created",
							body = if ok then nil else (err or ""),
							tone = if ok then "success" else "danger",
						}
					end)
				end
			end,
		},
		ui.scroll {
			w_full = true,
			h = 480,
			ui.presence(function(): string
				return step()
			end, function(which: string): Node
				return if which == "details" then create_details() else create_review()
			end, {
				w_full = true,
				pb = "$space.2",
				enter = { opacity = { 0, 1 }, duration = 140 },
				exit = { opacity = 0, duration = 70 },
				mode = "wait",
			}),
		},
	}
end

local function delete_dialog(): Node
	return ds.Dialog {
		open = store.delete_open,
		title = function(): string
			local task = store.by_id(store.delete_target:get())
			return if task then `Delete “{task.title}”?` else "Delete this task?"
		end,
		body = "The schedule stops. Chats and runs it already made stay where they are.",
		tone = "danger",
		confirm = {
			label = "Delete task",
			on_click = function()
				store.delete(function(ok: boolean, err: string?)
					ds.toast.show {
						title = if ok then "Task deleted" else "Couldn't delete",
						body = if ok then nil else (err or ""),
						tone = if ok then "success" else "danger",
					}
				end)
			end,
		},
		cancel = "Keep task",
	}
end

function M.Page(): Node
	return ui.column {
		w_full = true,
		h_full = true,
		min_h = 0,
		ui.presence(function(): string
			local id = store.selected:get()
			if not id then
				return "list"
			end
			local task = store.by_id(id)
			if not task then
				return "list"
			end
			-- Its state is part of the key: Pause, Resume, Run now and a
			-- finished run redraw the page in place, buttons and badge with it.
			return `task|{id}|{task.status}|{tostring(task.pause_after)}|{tostring(task.updated_at)}`
		end, function(key: string): Node
			if key == "list" then
				return list_page()
			end
			local id = string.match(key, "^task|([^|]+)")
			local task = if id then store.by_id(id) else nil
			return if task then detail_page(task) else list_page()
		end, {
			w_full = true,
			h_full = true,
			enter = { opacity = { 0.6, 1 }, duration = 120 },
		}),
		create_dialog(),
		delete_dialog(),
	}
end

return M
