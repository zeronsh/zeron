-- A session's lanes (screens/chat/lanes, ds/session_lanes) with its panel
-- (screens/panel, its side chat and subagent tabs: screens/chat/lane_tab),
-- for tests/lanes_in_place.test.luau. No engine: the sessions (a parent
-- with two side chats), the open chat's transcript (two spawn chips), and
-- what's asked (create, fork, send, watch) are made up here, each call
-- noted in `calls`.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local _stub = require("@app/files_stub")
local sessions = require("@account/sessions")
local store = require("@app/screens/chat/store")
local route = require("@app/screens/shell/route")
local lanes = require("@app/screens/chat/lanes")
local Panel = require("@app/screens/panel")
local Activity = require("@ds/activity")

local said = ui.state("calls", "")
local function note(what: string)
	said:set(if said:peek() == "" then what else `{said:peek()},{what}`)
end
-- (The composer's accounts: `agents.on_device` set by a test, what's asked of
-- a device noted.)
do
	local stub_agents = require("@account/agents") :: any
	stub_agents.watch_device = function(key: string)
		note(`watch_device:{key}`)
	end
	stub_agents.refresh_device = function(key: string)
		note(`refresh_device:{key}`)
	end
	stub_agents.switch_on = function(key: string, id: string, account: string): any
		note(`switch_on:{key}:{id}:{account}`)
		return { after = function(_: any, done: (any) -> ()) done(nil) end }
	end
end

-- A promise settled at once, as a host's does later.
local function settled(ok: boolean, value: any): any
	return {
		after = function(_self: any, done: (any) -> (), failed: ((any) -> ())?)
			if ok then
				done(value)
			elseif failed then
				failed(value)
			end
		end,
	}
end

local NOW = os.time()
local children = {
	{ id = "side-a", parent = "chat-1", title = "Looking at the tests", device = "d", status = "working", updated = NOW - 60, created = NOW - 120, agent = "openai" },
	{ id = "side-b", parent = "chat-1", title = "New side chat", device = "d", updated = NOW - 600, created = NOW - 700 },
}
local function publish()
	sessions.now:set({
		state = "ready",
		here = "d",
		devices = { { id = "d", name = "Mac", platform = "macos", here = true }, { id = "box", name = "Box", platform = "linux", here = false } },
		projects = {
			{ id = "p1", name = "atlas", device = "d", path = "/proj", git = true },
			{ id = "p2", name = "zephyr", device = "d", path = "/zephyr", git = true },
		},
		chats = {
			{ id = "chat-1", title = "Parent", project = "p1", device = "d", branch = "main", cwd = "/proj", updated = NOW, created = NOW - 3600, running_subagents = 1, side_chats = #children, working_side_chats = 1 },
		},
		archived = {},
		children = { ["chat-1"] = table.clone(children) },
		places = { pinned = {}, sections = {} },
		places_ready = true,
	} :: any)
end
publish()

-- The open chat: the parent, on Claude Code's Opus 5.5 at high effort (a
-- new side chat starts as it runs); the agents and models to pick from.
store.session:set({ id = "chat-1", state = "ready", title = "Parent", harness = "claude-code", model = "claude-code/opus-5.5", effort = "high", access = "supervised", project = "p1", device = "d", branch = "main", cwd = "/proj" } :: any)
store.catalog:set({
	state = "ready",
	harnesses = {
		{ id = "claude-code", label = "Claude Code", mark = "claude" },
		{ id = "codex", label = "Codex", mark = "openai" },
	},
	models = {
		{ value = "claude-code/opus-5.5", label = "Opus 5.5", harness = "claude-code" },
		{ value = "claude-code/sonnet-5", label = "Sonnet 5", harness = "claude-code" },
		{ value = "codex/gpt-6.1", label = "GPT-6.1", harness = "codex" },
		{ value = "codex/gpt-6.1-mini", label = "GPT-6.1 mini", harness = "codex" },
	},
} :: any)
store.messages:set({
	{ id = "m1", role = "user", text = "Scan it" },
	{
		id = "m2",
		role = "assistant",
		text = "",
		started = NOW - 300,
		tools = {
			{ id = "t1", kind = "agent", verb = "Ran a subagent", subagent = { ref = "chat-1--sub--1", title = "scan the repo", status = "running" } },
			{ id = "t2", kind = "agent", verb = "Ran a subagent", subagent = { ref = "chat-1--sub--2", title = "read the docs", status = "done" } },
			{ id = "t3", kind = "read", verb = "Read", target = "a.rs" },
		},
	},
} :: any)

local stub_sessions = sessions :: any
stub_sessions.create = function(req: any): any
	note(`create:{req.parent}:{req.id}`)
	-- (Where it's made, when its composer moved it.)
	if req.project or req.device or req.no_project then
		note(`place:{req.id}:{req.project or ""}:{req.device or ""}:{req.branch or ""}:{req.cwd or ""}:{if req.no_project then "none" else ""}`)
	end
	table.insert(children, 1, { id = req.id, parent = req.parent, title = "New side chat", device = "d", updated = os.time(), created = os.time() })
	publish()
	return settled(true, req.id)
end
-- A fork: settled at once, held until `fork_settle`, or failing.
local fork_fails = ui.state("test_fork_fails", false)
local fork_hold = ui.state("test_fork_hold", false)
local held: { () -> () } = {}
-- What the sidebar's session menu asks (for a side chat, from the panel).
stub_sessions.archive = function(id: string): any
	note(`archive:{id}`)
	return settled(true, nil)
end
stub_sessions.link = function(id: string): any
	note(`link:{id}`)
	return settled(true, `https://zeron.sh/c/{id}`)
end
stub_sessions.fork = function(source: string, _parent: string?, id: string?): any
	note(`fork:{source}:{id}`)
	if fork_fails:peek() then
		return settled(false, { message = "Wait for a completed response before starting a side chat" })
	end
	if fork_hold:peek() then
		return {
			after = function(_self: any, done: (any) -> (), _failed: any)
				table.insert(held, function()
					done(id)
				end)
			end,
		}
	end
	return settled(true, id)
end
-- A side chat on an agent from the ACP registry (named by its harness
-- alone, its icon among `agent_icons`).
ui.command { id = "side_on_registry", title = "A side chat on Factory Droid", run = function()
	ui.shared("agent_icons", {} :: { [string]: string }):set({
		["acp.factory-droid"] = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="currentColor" d="M8 1l2 5 5 2-5 2-2 5-2-5-5-2 5-2z"/></svg>',
	})
	children[1].agent = "acp.factory-droid"
	publish()
end }
ui.command { id = "fork_hold", title = "Forks wait", run = function()
	fork_hold:set(true)
end }
ui.command { id = "fork_settle", title = "Held forks finish", run = function()
	local now = held
	held = {}
	for _, f in now do
		f()
	end
end }
ui.command { id = "fork_ok", title = "Forks work", run = function()
	fork_fails:set(false)
end }
local stub_store = store :: any
-- A send, and what it went with (`pick:<session>:<model>:<agent>:<effort>`);
-- failing (as an engine that refuses it) after `send_fails`.
local send_fails = ui.state("test_send_fails", false)
stub_store.send = function(req: any): any
	note(`send:{req.session}:{req.text}{if req.fresh then ":fresh" else ""}`)
	note(`pick:{req.session}:{req.model or ""}:{req.harness or ""}:{req.effort or ""}:{req.access or ""}{if req.worktree then ":worktree" else ""}`)
	note(`mode:{req.session}:{if req.fast then "fast" else ""}:{if req.plan then "plan" else ""}`)
	-- (What it carries, each by its name, `@` if it's on the chat's computer
	-- already.)
	for _, a in req.attachments or {} do
		note(`attach:{req.session}:{a.kind}:{a.name}{if a.on_host then "@" else ""}`)
	end
	if send_fails:peek() then
		-- (Refused a beat later, as the host answers.)
		return {
			after = function(_self: any, _done: any, failed: ((any) -> ())?)
				ui.after(0, function()
					if failed then
						failed("failed")
					end
				end)
			end,
		}
	end
	return settled(true, nil)
end
ui.command { id = "send_fails", title = "Sends fail", run = function()
	send_fails:set(true)
end }
-- A project's branches, as git where it is would say.
stub_store.refs = function(_path: string, _device: string?): any
	return settled(true, { { name = "main", current = true }, { name = "feature", current = false } })
end
stub_store.interrupt = function(id: string)
	note(`stop:{id}`)
end
-- The host's lane views: a watched chat says its id in its transcript.
local stub_lanes = lanes :: any
stub_lanes.watch = function(slot: string, id: string?, frozen: boolean?, _chat: string?)
	note(`watch:{id or "none"}{if frozen then ":frozen" else ""}`)
	if id then
		lanes.view(slot).messages:set({ { id = `{id}-m`, role = "assistant", text = `This is {id}` } })
		-- (A side chat as the host publishes it: side-a runs Codex's GPT-6.1.)
		if id == "side-a" then
			lanes.view(slot).session:set({ id = id, state = "ready", harness = "codex", model = "codex/gpt-6.1", effort = "medium", access = "edits", project = "p1", device = "d", branch = "feature", cwd = "/proj/.zeron/worktrees/feature", running = false, context_tokens = 52000, context_window = 200000 })
		end
	end
end
-- A side chat as the host publishes it mid-turn: running, its queue, what it
-- asks, what it's at. (`tab1`, the first tab opened.)
local function side_says(fields: { [string]: any })
	local session = lanes.view("tab1").session
	local now = table.clone(session:peek() or {})
	for k, v in fields do
		now[k] = v
	end
	session:set(now)
end
ui.command { id = "side_running", title = "The side chat runs", run = function()
	side_says({ running = true })
end }
ui.command { id = "side_queue_with_file", title = "A message with a file waits in its queue", run = function()
	side_says({ queue = { { id = "q2", text = "Queued words" } } })
end }
ui.command { id = "side_queue_one", title = "A message waits in its queue", run = function()
	side_says({ queue = { { id = "q1", text = "Queued words" } } })
end }
-- The engine's copy of its steered message landing: the queued row's id is
-- its prompt's, and it leaves the queue.
ui.command { id = "side_steer_lands", title = "Its steered message joins its transcript", run = function()
	lanes.view("tab1").messages:push({ id = "q1", role = "user", text = "Queued words" })
	side_says({ queue = {} })
end }
ui.command { id = "side_approval", title = "It asks for approval", run = function()
	side_says({ ask = { request = "r1", kind = "approval", approval = { title = "Run this command?", command = "rm -rf build", cwd = "/proj" } } })
end }
ui.command { id = "side_question", title = "It asks a question", run = function()
	side_says({ ask = { request = "r2", kind = "question", question = { header = "Question", text = "Which branch?", options = { { value = "main", label = "main" }, { value = "fix", label = "A new branch" } } } } })
end }
ui.command { id = "side_todos", title = "It has todos", run = function()
	side_says({ todos = { { id = "t1", text = "Read the tests", status = "in_progress" } } })
end }
-- The rest of what a side chat's composer shows as the chat page's does.
ui.command { id = "side_attach_passage", title = "A passage attached to it", run = function()
	require("@app/screens/chat/shared").attach_quote({ text = "the whole passage", who = "Reply" }, "side-a")
end }
ui.command { id = "side_edit_lost", title = "Its edit let go", run = function()
	side_says({ queue = { { id = "q1", text = "Queued words" } } })
end }
ui.command { id = "side_on_box", title = "It runs on another computer", run = function()
	side_says({ device = "box" })
end }
ui.command { id = "side_offline", title = "Its device goes away", run = function()
	side_says({ state = "offline" })
end }
ui.command { id = "side_fast", title = "It runs on Fast, in plan mode", run = function()
	side_says({ fast = true, plan = true })
end }
ui.command { id = "side_failed", title = "A message of its didn't go", run = function()
	lanes.view("tab1").messages:push({ id = "lost1", role = "user", text = "Lost on the way", waiting = "failed" })
end }
ui.command { id = "side_renamed", title = "It's renamed", run = function()
	children[#children - 1].title = "Renamed by a person"
	publish()
end }
ui.command { id = "side_rests", title = "It rests", run = function()
	children[#children - 1].status = nil
	publish()
end }
ui.command { id = "side_no_project", title = "It works in no project", run = function()
	side_says({ project = ui.null })
end }
ui.command { id = "send_mod_enter", title = "⌘↩ sends", run = function()
	require("@app/screens/settings/prefs").send_with:set("mod_enter")
end }
ui.command { id = "escape_stops", title = "Escape stops the agent", run = function()
	require("@app/screens/settings/prefs").escape_stops:set(true)
end }
ui.command { id = "lane_find", title = "⌘F", run = function()
	note(`find:{Panel.find_in_lane()}`)
end }
ui.command { id = "lane_model", title = "⌘/", run = function()
	note(`model:{Panel.model_in_lane()}`)
end }
stub_store.retry = function(id: string, message: string)
	note(`retry:{id}:{message}`)
end
-- `@` and `/`: what's in the project, what the agent lists.
local files_stub = require("@app/screens/files/api") :: any
files_stub.search = function(target: any, query: string): any
	note(`search:{target.chat or target.space or "?"}:{query}`)
	note(`search_on:{target.device or "here"}`)
	local found = {}
	for _, path in { "src/lib.rs", "README.md" } do
		if query == "" or string.find(string.lower(path), string.lower(query), 1, true) then
			table.insert(found, { path = path, dir = false })
		end
	end
	return settled(true, found)
end
local invocations_stub = require("@app/screens/chat/invocations") :: any
invocations_stub.read = function(harness: string, target: any): any
	note(`invocations:{harness}:{target.chat or target.space or "?"}`)
	return settled(true, { rows = { { kind = "command", name = "review", label = "/review", detail = "Review the diff", link = "[/review](zeron-invoke:review)" } } })
end
stub_store.queue = function(id: string, op: any)
	note(`queue:{id}:{op.op}:{op.item}`)
end
stub_store.queue_edit = function(id: string, item: string): any
	note(`queue_edit:{id}:{item}`)
	side_says({ queue = { { id = item, text = "Queued words", editing = true } } })
	-- (`q2`: a message with a file, as the host hands it to an edit.)
	return settled(true, {
		text = "Queued words",
		attachments = if item == "q2"
			then { { path = "/host/uploads/ea958e1e-note.txt", name = "note.txt", kind = "file", on_host = true } }
			else nil,
	})
end
stub_store.queue_finish = function(id: string, item: string, action: string, text: string?, attachments: { any }?): any
	local files = ""
	if attachments then
		local paths = {}
		for _, a in attachments do
			table.insert(paths, `{a.path}{if a.on_host then "@host" else ""}`)
		end
		files = `:files={table.concat(paths, ",")}`
	end
	note(`queue_finish:{id}:{item}:{action}:{text or ""}{files}`)
	side_says({ queue = { { id = item, text = if action == "commit" then text else "Queued words" } } })
	return settled(true, nil)
end
stub_store.approve = function(id: string, request: string, scope: string)
	note(`approve:{id}:{request}:{scope}`)
end
stub_store.answer = function(id: string, request: string, _answer: any)
	note(`answer:{id}:{request}`)
end
local stub_toast = ds.toast :: any
stub_toast.show = function(t: any): number
	note(`toast:{t.title}`)
	return 0
end

-- A tab's menu past Close and its kin (Panel.tab_menu: what its secondary
-- click and Shift+F10 open), for the tab of chat `test_menu_of`: its labels
-- in `test_menu_labels`; `tab_menu_pick` does the one `test_menu_pick` names.
local menu_of = ui.state("test_menu_of", "")
local menu_labels = ui.state("test_menu_labels", "")
local menu_pick = ui.state("test_menu_pick", "")
local function tab_entries(): { any }
	for _, t in require("@app/screens/panel/state").of("chat:chat-1").tabs do
		if t.chat == menu_of:peek() then
			return Panel.tab_menu(t)
		end
	end
	return {}
end
ui.command { id = "tab_menu", title = "Tab menu", run = function()
	local out = {}
	for _, e in tab_entries() do
		table.insert(out, if e.separator then "---" elseif e.heading then `#{e.heading}` else e.label)
	end
	menu_labels:set(table.concat(out, ","))
end }
ui.command { id = "tab_menu_pick", title = "Tab menu pick", run = function()
	for _, e in tab_entries() do
		if e.label == menu_pick:peek() and e.on_select then
			e.on_select()
		end
	end
end }
-- Away from the tab in front (the panel's home) and back to it, as a
-- person switching tabs: its view made again.
local panel_state = require("@app/screens/panel/state")
-- (As the host says them: this fixture has no host.)
do
	local said = ui.shared("chat.subagents", {} :: { any })
	ui.effect(function()
		said:set(require("@app/screens/chat/lanes").subagents_of(require("@app/screens/chat/store").messages:get()))
	end)
end
local was_active: string? = nil
ui.command { id = "panel_home", title = "Panel home", run = function()
	was_active = panel_state.of("chat:chat-1").active
	panel_state.activate("chat:chat-1", nil)
end }
ui.command { id = "panel_back", title = "Back to the tab", run = function()
	panel_state.activate("chat:chat-1", was_active)
end }
ui.command { id = "fork_fails", title = "Forks fail", run = function()
	fork_fails:set(true)
end }
ui.command { id = "many", title = "Many side chats", run = function()
	for i = 1, 10 do
		table.insert(children, { id = `side-{i}`, parent = "chat-1", title = `Side {i}`, device = "d", updated = NOW - 1000 - i, created = NOW - 2000 })
	end
	publish()
end }
ui.command { id = "finish", title = "Subagent finishes", run = function()
	local now = table.clone(store.messages:peek()) :: any
	local reply = table.clone(now[2])
	local tools = table.clone(reply.tools)
	tools[1] = table.clone(tools[1])
	tools[1].subagent = { ref = "chat-1--sub--1", title = "scan the repo", status = "done" }
	reply.tools = tools
	now[2] = reply
	store.messages:set(now)
end }

route.open_session("chat-1")
-- (The card is off by default in the app: on here.)
lanes.enabled:set(true)
lanes.visible:set(true)

local session = ui.state("test_lanes_session", { id = "chat-1", cwd = "/proj" } :: any)

return ui.row {
	w_full = true,
	h_full = true,
	gap = "$space.4",
	ui.column {
		shrink = 0,
		p = "$space.3",
		gap = "$space.4",
		lanes.Card(),
		-- What's carried onto a side chat from elsewhere in the window: a
		-- sidebar session, the files pane's files.
		ui.row {
			gap = "$space.2",
			ui.div { id = "carry-session", w = 40, h = 20, on_drag = function(): any
				return { session = "chat-1" }
			end },
			ui.div { id = "carry-paths", w = 40, h = 20, on_drag = function(): any
				return { paths = { "src/lib.rs" }, root = "/proj" }
			end },
		},
		-- The transcript's spawn chips (a live group: open), each opening
		-- its subagent's lane.
		ui.column {
			id = "fixture-chips",
			w = 320,
			Activity {
				key = "fixture-chips",
				tools = function(): { any }
					return (store.messages:get()[2] :: any).tools
				end,
				live = function(): boolean
					return true
				end,
				open_subagent = function(sub: any)
					lanes.open_subagent("chat-1", sub)
				end,
			},
		},
	},
	ui.text {
		id = "subagent-count",
		function(): string
			return tostring(#lanes.subagents())
		end,
	},
	Panel.Panel {
		session = session,
		project = { id = "p1", path = "/proj", name = "proj" },
		grow = 1,
		h_full = true,
	},
}
