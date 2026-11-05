-- The Composer page: every state the composer can be in, one at a time,
-- chosen from an index at the top, the composer itself at the foot of a
-- stage below (a chat behind it, so the trays read over something). Each
-- state is real: the chips can be removed, the queue dragged, the plan
-- opened, a question answered, the mentions typed at; "Everything at
-- once" is the worst case, built first.

local ds = require("@ds")
local Composer = require("@ds/composer")
local UserMessage = require("@ds/user_message")
local Markdown = require("@ds/markdown")
local control = require("@ds/control")
local data = require("@demos/composer_data")
local Cargo = require("@ds/composer_cargo")
local Dock = require("@ds/composer_dock")
local Ask = require("@ds/composer_ask")
local Dropdown = require("@ds/dropdown")
local EffortSlider = require("@ds/effort_slider")

-- A gallery page (as gallery.luau reads one).
type Page = { id: string, title: string, about: string, demo: () -> Node, foot: (() -> Node)?, fill: boolean? }

-- The composer's own state: all of it set by a state of the page.
local draft = ui.state("cx_draft", "")
local turn = ui.state("cx_turn", "idle")
local running = ui.computed(function(): boolean
	return turn:get() == "running"
end)
local access = ui.state("cx_access", "full" :: string?)
local plan = ui.state("cx_plan", false)
local model = ui.state("cx_model", "opus" :: string?)
local effort = ui.state("cx_effort", "high" :: string?)
local fast = ui.state("cx_fast", false)
local checkout = ui.state("cx_checkout", "worktree" :: string?)
local device = ui.state("cx_device", "mac" :: string?)
local branch = ui.state("cx_branch", "zeron/zgx-onboarding-design-system" :: string?)
local cargo = ui.state("cx_cargo", {} :: { Cargo.Item })
local queue = ui.state("cx_queue", {} :: { Dock.Queued })
local todos = ui.state("cx_todos", {} :: { Dock.Todo })
local tasks = ui.state("cx_tasks", {} :: { Dock.Task })
local tray = ui.state("cx_tray", nil :: Dock.Tray?)
local question = ui.state("cx_question", nil :: Ask.Question?)
local chosen = ui.state("cx_chosen", {} :: { [string]: boolean })
local approval = ui.state("cx_approval", nil :: Ask.Approval?)
local notice = ui.state("cx_notice", nil :: Composer.Notice?)
local dictating = ui.state("cx_dictating", false)
local disabled = ui.state("cx_disabled", false)
local narrow = ui.state("cx_narrow", false)
local expanded = ui.state("cx_expanded", false)
local dropping = ui.state("cx_dropping", false)
local focus = ui.state("cx_focus", 0)
local current = ui.state("cx_state", "idle")
-- The draft set aside while a queued row is edited.
local aside = ui.state("cx_aside", "")

local HARNESSES: { ds.Harness } = {
	{ id = "claude", label = "Claude", mark = "claude" },
	{ id = "codex", label = "Codex", mark = "openai" },
	{ id = "grok", label = "Grok", mark = "grok" },
}
local CLAUDE_EFFORT: { EffortSlider.Level } = {
	{ value = "low", label = "Low" },
	{ value = "medium", label = "Medium" },
	{ value = "high", label = "High" },
	{ value = "xhigh", label = "Extra high" },
	{ value = "max", label = "Max" },
}
local GPT_EFFORT: { EffortSlider.Level } = {
	{ value = "minimal", label = "Minimal" },
	{ value = "low", label = "Low" },
	{ value = "medium", label = "Medium" },
	{ value = "high", label = "High" },
	{ value = "xhigh", label = "Extra high" },
}
local MODELS: { ds.Model } = {
	{ value = "opus", label = "Opus 5.5", harness = "claude", efforts = CLAUDE_EFFORT, effort = "high", fast = true },
	{ value = "sonnet", label = "Sonnet 5.5", harness = "claude", efforts = CLAUDE_EFFORT, effort = "medium" },
	{ value = "fable", label = "Fable 5.1", harness = "claude", efforts = CLAUDE_EFFORT, effort = "high" },
	{ value = "haiku", label = "Haiku 5.5", harness = "claude" },
	{ value = "gpt", label = "GPT-5.5", harness = "codex", efforts = GPT_EFFORT, effort = "medium", fast = true },
	{ value = "gpt-codex", label = "GPT-5.5 Codex", harness = "codex", efforts = GPT_EFFORT, effort = "high" },
	{ value = "grok", label = "Grok 5", harness = "grok", efforts = { { value = "low", label = "Low" }, { value = "high", label = "High" } }, effort = "high" },
}
local DEVICES: { ds.ComposerDevice } = {
	{ value = "mac", label = "MacBook Pro", you = true },
	{ value = "devbox", label = "devbox" },
	{ value = "studio", label = "studio", offline = true },
	{ value = "cloud", label = "Cloud", cloud = true },
}
local BRANCHES: { Dropdown.Item } = {
	{ value = "main", label = "main" },
	{ value = "zeron/zgx-onboarding-design-system", label = "zeron/zgx-onboarding-design-system" },
	{ value = "zeron/composer", label = "zeron/composer" },
}

-- Timers a state runs (an upload climbing, words arriving while
-- dictating), stopped as the next state is chosen.
local timers: { () -> () } = {}
local function stop_timers()
	for _, stop in timers do
		stop()
	end
	timers = {}
end

-- The cargo with one item changed.
local function patch_cargo(id: string, change: (item: Cargo.Item) -> ())
	local list = table.clone(cargo:peek())
	for i, it in list do
		if it.id == id then
			local copy = table.clone(it)
			change(copy)
			list[i] = copy
		end
	end
	cargo:set(list)
end
local function drop_cargo(id: string)
	local list: { Cargo.Item } = {}
	for _, it in cargo:peek() do
		if it.id ~= id then
			table.insert(list, it)
		end
	end
	cargo:set(list)
end

-- An upload climbing to 99% (the last step is the commit's: Zeron caps it
-- there), one item finishing on the way.
local function climb(id: string, from: number, done_at: number?)
	local p = from
	table.insert(
		timers,
		ui.every(140, function()
			p = math.min(0.99, p + 0.012 + (0.99 - p) * 0.03)
			if done_at and p >= done_at then
				patch_cargo(id, function(it)
					it.progress = nil
				end)
				return false
			end
			patch_cargo(id, function(it)
				it.progress = p
			end)
			return p < 0.99
		end)
	)
end

-- Words arriving while dictating, as a transcription lands them. What the
-- draft held before is kept: Cancel gives it back; Stop keeps the words.
local SPOKEN = "port the queue tray's reorder then fold the plan when every step is done and capture both themes before you stop"
local before_dictation = ui.state("cx_before_dictation", "")
local function transcribe()
	before_dictation:set(draft:peek())
	local words = {}
	for w in string.gmatch(SPOKEN, "%S+") do
		table.insert(words, w)
	end
	local i = 0
	table.insert(
		timers,
		ui.every(380, function()
			if not dictating:peek() then
				return false
			end
			i += 1
			if i > #words then
				return false
			end
			local now = draft:peek()
			draft:set(if now == "" then (string.upper(string.sub(words[i], 1, 1)) .. string.sub(words[i], 2)) else now .. " " .. words[i])
			return true
		end)
	)
end

-- Everything back to an idle composer.
local function reset()
	stop_timers()
	draft:set("")
	turn:set("idle")
	cargo:set({})
	queue:set({})
	todos:set({})
	tasks:set({})
	tray:set(nil)
	question:set(nil)
	chosen:set({})
	approval:set(nil)
	notice:set(nil)
	dictating:set(false)
	disabled:set(false)
	narrow:set(false)
	expanded:set(false)
	dropping:set(false)
	plan:set(false)
	access:set("full")
	model:set("opus")
	effort:set("high")
	fast:set(false)
	aside:set("")
end

type Spec = { id: string, label: string, apply: () -> () }
type Group = { title: string, states: { Spec } }

local GROUPS: { Group } = {
	{
		title = "Draft",
		states = {
			{ id = "idle", label = "Idle", apply = function() end },
			{
				id = "focused",
				label = "Focused",
				apply = function()
					focus:add(1)
				end,
			},
			{
				id = "filled",
				label = "Filled",
				apply = function()
					draft:set(data.DRAFT_SHORT)
				end,
			},
			{
				id = "multi",
				label = "Multi-line",
				apply = function()
					draft:set(data.DRAFT_MULTI)
				end,
			},
			{
				id = "long",
				label = "Long draft",
				apply = function()
					draft:set(data.long_draft())
					focus:add(1)
				end,
			},
			{
				id = "expanded",
				label = "New chat",
				apply = function()
					expanded:set(true)
				end,
			},
			{
				id = "narrow",
				label = "Narrow",
				apply = function()
					narrow:set(true)
					draft:set(data.DRAFT_SHORT)
					cargo:set({ data.IMAGE_SHELL, data.FILE_DOCK })
					queue:set(data.queue())
					tray:set("queue")
				end,
			},
		},
	},
	{
		title = "Cargo",
		states = {
			{
				id = "one",
				label = "One file",
				apply = function()
					cargo:set({ data.FILE_DOCK })
					draft:set("Is the tuck still 18 here?")
				end,
			},
			{
				id = "mixed",
				label = "Mixed",
				apply = function()
					cargo:set({ data.IMAGE_SHELL, data.FILE_DOCK, data.PASTED, data.ANNOTATION_1, data.IMAGE_TRANSCRIPT })
					draft:set(data.DRAFT_SHORT)
				end,
			},
			{
				id = "images",
				label = "Images",
				apply = function()
					cargo:set({ data.IMAGE_SHELL, data.IMAGE_TRANSCRIPT, data.IMAGE_ART })
					draft:set("The sidebar in both themes, and the plate for the wallpaper.")
				end,
			},
			{
				id = "files",
				label = "Files",
				apply = function()
					cargo:set({ data.FILE_DOCK, data.FILE_QUEUE, data.FILE_THEME })
					draft:set("Compare these against the Luau port.")
				end,
			},
			{
				id = "pasted",
				label = "Pasted text",
				apply = function()
					cargo:set({ data.PASTED, data.PASTED_SHORT })
					draft:set("The first is the failing test's output, the second the panic.")
				end,
			},
			{
				id = "uploading",
				label = "Uploading",
				apply = function()
					local a = table.clone(data.IMAGE_SHELL)
					a.progress = 0.08
					local b = table.clone(data.FILE_DOCK)
					b.progress = 0.31
					local c = table.clone(data.IMAGE_TRANSCRIPT)
					c.progress = 0.72
					cargo:set({ a, b, c })
					climb(a.id, 0.08, nil)
					climb(b.id, 0.31, nil)
					climb(c.id, 0.72, 0.96)
				end,
			},
			{
				id = "failed",
				label = "Failed upload",
				apply = function()
					local a = table.clone(data.FILE_LOG)
					a.failed = "Couldn't upload"
					local b = table.clone(data.IMAGE_ART)
					b.failed = "Couldn't upload"
					cargo:set({ data.IMAGE_SHELL, a, b })
				end,
			},
			{
				id = "limits",
				label = "Too many, too large",
				apply = function()
					cargo:set(data.many())
					notice:set(data.NOTICES.too_many)
				end,
			},
			{
				id = "too_large",
				label = "Too large",
				apply = function()
					cargo:set({ data.FILE_DOCK })
					notice:set(data.NOTICES.too_large)
				end,
			},
			{
				id = "annotations",
				label = "Annotations",
				apply = function()
					cargo:set({ data.ANNOTATION_1, data.ANNOTATION_2, data.IMAGE_SHELL })
					draft:set("The send button is a pixel low in the queue tray's row. Line it up with the row's grip.")
				end,
			},
			{
				id = "drop",
				label = "Drop target",
				apply = function()
					dropping:set(true)
				end,
			},
		},
	},
	{
		title = "Mentions",
		states = {
			{
				id = "at_files",
				label = "@ files",
				apply = function()
					draft:set("Read @que")
					focus:add(1)
				end,
			},
			{
				id = "at_commands",
				label = "/ commands",
				apply = function()
					draft:set("/")
					focus:add(1)
				end,
			},
			{
				id = "at_skills",
				label = "$ skills",
				apply = function()
					draft:set("Then $re")
					focus:add(1)
				end,
			},
			{
				id = "tokens",
				label = "Chips in the text",
				apply = function()
					draft:set(data.DRAFT_MENTIONS)
				end,
			},
		},
	},
	{
		title = "Turn",
		states = {
			{
				id = "running",
				label = "Running",
				apply = function()
					turn:set("running")
				end,
			},
			{
				id = "steering",
				label = "Steering",
				apply = function()
					turn:set("running")
					draft:set(data.DRAFT_STEER)
					queue:set({ { id = "q0", text = "Then run the full test suite and paste any failures here" } })
					tray:set("queue")
				end,
			},
			{
				id = "queue",
				label = "Queued",
				apply = function()
					turn:set("running")
					queue:set(data.queue())
					tray:set("queue")
				end,
			},
			{
				id = "queue_edit",
				label = "Editing a queued row",
				apply = function()
					turn:set("running")
					local list = data.queue()
					list[2].editing = true
					list[3].locked = "Editing on devbox"
					queue:set(list)
					tray:set("queue")
					draft:set(list[2].text)
				end,
			},
			{
				id = "question",
				label = "Question",
				apply = function()
					question:set(data.QUESTION)
					queue:set(data.queue())
				end,
			},
			{
				id = "question_multi",
				label = "Question, page 2 of 3",
				apply = function()
					question:set(data.QUESTION_MULTI)
					chosen:set({ cargo = true, trays = true })
				end,
			},
			{
				id = "approval",
				label = "Approval",
				apply = function()
					approval:set(data.APPROVAL)
					turn:set("running")
				end,
			},
		},
	},
	{
		title = "Trays",
		states = {
			{
				id = "todos",
				label = "Plan, open",
				apply = function()
					turn:set("running")
					todos:set(data.todos())
					tray:set("todos")
				end,
			},
			{
				id = "todos_folded",
				label = "Plan, folded",
				apply = function()
					turn:set("running")
					todos:set(data.todos())
					tray:set(nil)
				end,
			},
			{
				id = "todos_done",
				label = "Plan, done",
				apply = function()
					todos:set(data.todos_done())
					tray:set(nil)
				end,
			},
			{
				id = "tasks",
				label = "Running tasks",
				apply = function()
					turn:set("running")
					tasks:set(data.tasks())
					tray:set("tasks")
				end,
			},
			{
				id = "everything",
				label = "Everything at once",
				apply = function()
					turn:set("running")
					draft:set(data.DRAFT_SHORT)
					cargo:set({ data.IMAGE_SHELL, data.IMAGE_TRANSCRIPT, data.FILE_DOCK, data.PASTED })
					queue:set(data.queue())
					todos:set(data.todos())
					tasks:set(data.tasks())
					tray:set("queue")
				end,
			},
		},
	},
	{
		title = "Modes",
		states = {
			{
				id = "plan",
				label = "Plan mode",
				apply = function()
					plan:set(true)
					access:set("supervised")
				end,
			},
			{
				id = "access",
				label = "Supervised",
				apply = function()
					access:set("supervised")
				end,
			},
			{
				id = "model",
				label = "Fast, extra high",
				apply = function()
					model:set("gpt")
					effort:set("xhigh")
					fast:set(true)
				end,
			},
			{
				id = "dictation",
				label = "Dictating",
				apply = function()
					dictating:set(true)
					transcribe()
				end,
			},
		},
	},
	{
		title = "Trouble",
		states = {
			{
				id = "offline",
				label = "Offline",
				apply = function()
					notice:set(data.NOTICES.offline)
					draft:set(data.DRAFT_SHORT)
					queue:set({ data.queue()[1] })
				end,
			},
			{
				id = "reconnecting",
				label = "Reconnecting",
				apply = function()
					notice:set(data.NOTICES.reconnecting)
					draft:set(data.DRAFT_SHORT)
				end,
			},
			{
				id = "sending",
				label = "Sending",
				apply = function()
					notice:set(data.NOTICES.sending)
				end,
			},
			{
				id = "error",
				label = "Send failed",
				apply = function()
					local n = table.clone(data.NOTICES.error)
					n.action = "Retry"
					n.dismissable = true
					notice:set(n)
					draft:set(data.DRAFT_SHORT)
				end,
			},
			{
				id = "restored",
				label = "Draft restored",
				apply = function()
					local n = table.clone(data.NOTICES.restored)
					n.action = "Discard"
					n.dismissable = true
					notice:set(n)
					draft:set(data.DRAFT_MULTI)
					cargo:set({ data.FILE_QUEUE })
				end,
			},
			{
				id = "disabled",
				label = "Disabled",
				apply = function()
					disabled:set(true)
					local n = table.clone(data.NOTICES.archived)
					n.action = "Unarchive"
					notice:set(n)
				end,
			},
		},
	},
}

local function apply(id: string)
	for _, g in GROUPS do
		for _, s in g.states do
			if s.id == id then
				ui.batch(function()
					reset()
					current:set(id)
					s.apply()
				end)
				return
			end
		end
	end
end

-- A state's chip in the index: quiet, ringed while it is the one shown.
local function chip(s: Spec): Node
	local function on(): boolean
		return current:get() == s.id
	end
	return ui.button {
		id = `cx-state-{s.id}`,
		h = 24,
		px = "$space.2",
		shrink = 0,
		rounded = "$radius.md",
		bg = "#00000000",
		border = 1,
		border_over = true,
		border_color = function()
			return if on() then "$zeron.composer.option.ring" else "#00000000"
		end,
		color = function()
			return if on() then "$text.primary" else "$text.secondary"
		end,
		text_size = "$font.size.sm",
		weight = "$font.weight.medium",
		cursor = "pointer",
		hover = { bg = "$hover", color = "$text.primary" },
		active = { bg = "$pressed", color = "$text.primary" },
		focus = control.ring,
		role = "radio",
		checked = on,
		label = s.label,
		on_click = function()
			apply(s.id)
		end,
		ui.text { s.label },
	}
end

local function index(): Node
	local rows: { Node } = {}
	for _, g in GROUPS do
		local chips: { Node } = {}
		for _, s in g.states do
			table.insert(chips, chip(s))
		end
		table.insert(
			rows,
			ui.row {
				gap = "$space.1",
				items = "center",
				flex_wrap = true,
				ds.Text { string.upper(g.title), style = "label", tone = "tertiary", w = 72, shrink = 0 },
				table.unpack(chips),
			}
		)
	end
	return ui.column { gap = "$space.1", role = "radiogroup", label = "Composer state", table.unpack(rows) }
end

-- Something to send to: the two turns before, faded behind the composer.
local function behind(): Node
	return ui.column {
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		px = "$space.6",
		pt = "$space.5",
		items = "center",
		opacity = 0.7,
		pointer_events = "none",
		ui.column {
			w_full = true,
			max_w = "$zeron.prose.measure",
			gap = "$space.4",
			UserMessage { text = "Port the composer dock to zgx: the queue tray first, then the plan. Keep Zeron's tuck." },
			Markdown {
				text = "Read `composer_dock.rs` and `queue.rs`. The dock is one element that travels between the new chat and a session; the trays tuck 18 points under the pill with their top corners at 16. I'll port the queue's rows first, then the plan's folding, and leave the handoff fade for last.",
			},
		},
	}
end

local page: Page = {
	id = "composer",
	title = "Composer",
	fill = true,
	about = "Where a message is written. One pill: what the message carries sits in it, what waits on the agent sits behind it in trays that tuck under it, one open at a time. Pick a state; everything in it works.",
	demo = function(): Node
		return ui.column {
			w_full = true,
			h_full = true,
			gap = "$space.4",
			index(),
			ui.column {
				id = "composer_stage",
				w_full = true,
				grow = 1,
				min_h = 0,
				relative = true,
				overflow = "hidden",
				rounded = "$radius.xl",
				border = 1,
				border_color = "$border.subtle",
				behind(),
				ui.row {
					absolute = true,
					left = 0,
					right = 0,
					bottom = 16,
					px = "$space.6",
					justify = "center",
					ui.div {
						w_full = true,
						max_w = function(): number
							return if narrow:get() then 360 else 640
						end,
						Composer {
							value = draft,
							placeholder = "Ask anything, @ to mention…",
							expanded = function()
								return expanded:get()
							end,
							running = running,
							access = access,
							plan = plan,
							model = model,
							effort = effort,
							fast = fast,
							harnesses = HARNESSES,
							models = MODELS,
							device = device,
							devices = DEVICES,
							checkout = checkout,
							branch = branch,
							branches = BRANCHES,
							context = 0.66,
							menus_below = false,
							focus = focus,
							narrow = function()
								return narrow:get()
							end,
							disabled = function()
								return disabled:get()
							end,
							notice = notice,
							-- What a notice's action does, by the state it came with.
							on_notice_action = function()
								local id = current:peek()
								if id == "error" then
									notice:set(data.NOTICES.sending)
								elseif id == "restored" then
									draft:set("")
									cargo:set({})
									notice:set(nil)
								elseif id == "disabled" then
									disabled:set(false)
									notice:set(nil)
								end
							end,
							on_dismiss_notice = function()
								notice:set(nil)
							end,
							dropping = dropping,
							mentions = data.MENTIONS,
							cargo = cargo,
							on_remove_cargo = drop_cargo,
							on_retry_cargo = function(id: string)
								patch_cargo(id, function(it)
									it.failed = nil
									it.progress = 0.05
								end)
								climb(id, 0.05, 0.97)
							end,
							on_paste_text = function(text: string): boolean
								local lines = select(2, string.gsub(text, "\n", "\n")) + 1
								if lines < 8 and #text < 600 then
									return false
								end
								local list = table.clone(cargo:peek())
								local pasted: Cargo.Item = {
									id = `c-paste-{os.clock()}`,
									kind = "text",
									name = "Pasted text",
									meta = `{lines} lines · {math.max(1, math.floor(#text / 1024 + 0.5))} KB`,
								}
								table.insert(list, pasted)
								cargo:set(list)
								return true
							end,
							on_files = function(files: { any })
								local list = table.clone(cargo:peek())
								for _, f in files do
									local image = string.match(f.type or "", "^image/") ~= nil
									local file: Cargo.Item = {
										id = `c-file-{f.name}-{os.clock()}`,
										kind = if image then "image" else "file",
										name = f.name,
										src = if image then f.path else nil,
										meta = `{math.max(1, math.floor((f.size or 0) / 1024 + 0.5))} KB`,
									}
									table.insert(list, file)
								end
								cargo:set(list)
							end,
							queue = queue,
							can_steer = true,
							tray = tray,
							on_queue_send = function(id: string)
								local list = {}
								for _, q in queue:peek() do
									if q.id ~= id then
										table.insert(list, q)
									end
								end
								queue:set(list)
								turn:set("running")
							end,
							on_queue_remove = function(id: string)
								local list = {}
								for _, q in queue:peek() do
									if q.id ~= id then
										table.insert(list, q)
									end
								end
								queue:set(list)
							end,
							on_queue_reorder = function(from: number, to: number)
								local list = table.clone(queue:peek())
								local moved = table.remove(list, from)
								if moved then
									table.insert(list, to, moved)
								end
								queue:set(list)
							end,
							on_queue_edit = function(id: string)
								local list = table.clone(queue:peek())
								for i, q in list do
									if q.id == id then
										local copy = table.clone(q)
										copy.editing = true
										list[i] = copy
										aside:set(draft:peek())
										draft:set(q.text)
									end
								end
								queue:set(list)
								focus:add(1)
							end,
							on_queue_save = function(id: string)
								local list = table.clone(queue:peek())
								for i, q in list do
									if q.id == id then
										local copy = table.clone(q)
										copy.editing = nil
										copy.text = draft:peek()
										list[i] = copy
									end
								end
								queue:set(list)
								draft:set(aside:peek())
							end,
							on_queue_cancel = function(id: string)
								local list = table.clone(queue:peek())
								for i, q in list do
									if q.id == id then
										local copy = table.clone(q)
										copy.editing = nil
										list[i] = copy
									end
								end
								queue:set(list)
								draft:set(aside:peek())
							end,
							todos = todos,
							on_dismiss_todos = function()
								todos:set({})
							end,
							tasks = tasks,
							on_stop_task = function(id: string)
								local list = table.clone(tasks:peek())
								for i, t in list do
									if t.id == id then
										local copy = table.clone(t)
										copy.ended = "stopped"
										list[i] = copy
									end
								end
								tasks:set(list)
								table.insert(
									timers,
									ui.after(1400, function()
										local rest = {}
										for _, t in tasks:peek() do
											if t.id ~= id then
												table.insert(rest, t)
											end
										end
										tasks:set(rest)
									end)
								)
							end,
							on_stop_all = function()
								tasks:set({})
							end,
							question = question,
							chosen = chosen,
							on_pick = function(v: string)
								local q = question:peek()
								if q and q.multi then
									local now = table.clone(chosen:peek())
									now[v] = if now[v] then nil else true
									chosen:set(now)
								else
									chosen:set({ [v] = true })
								end
							end,
							on_answer = function()
								question:set(nil)
								chosen:set({})
								turn:set("running")
							end,
							on_back = function()
								local q = question:peek()
								if q and q.page and q.page > 1 then
									local copy = table.clone(q)
									copy.page = q.page - 1
									question:set(copy)
								end
							end,
							on_close_question = function()
								question:set(nil)
								chosen:set({})
							end,
							approval = approval,
							on_approve = function()
								approval:set(nil)
								turn:set("running")
							end,
							on_deny = function()
								approval:set(nil)
								turn:set("idle")
							end,
							dictating = dictating,
							on_dictate = function()
								if dictating:peek() then
									dictating:set(false)
									stop_timers()
								else
									dictating:set(true)
									transcribe()
								end
							end,
							on_cancel_dictation = function()
								dictating:set(false)
								stop_timers()
								draft:set(before_dictation:peek())
							end,
							on_stop = function()
								turn:set("idle")
							end,
							on_send = function()
								turn:set("running")
								cargo:set({})
							end,
						},
					},
				},
			},
		}
	end,
}

return page
