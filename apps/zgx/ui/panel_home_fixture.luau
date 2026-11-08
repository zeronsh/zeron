-- The right panel's home (screens/panel/home) for a session, with no
-- engine: for tests/panel_home.test.luau and captures. The session
-- (chat-1, on this computer "work-laptop", project comet-native, a branch with
-- changes), its side chats and the open chat's subagents are made up here;
-- the home's host calls (screens/panel/home_api), the terminal's, and
-- what's asked of sessions (create, fork) and lanes (watch) are stood in
-- for, each noted in `calls`. Commands: `many` (300 side chats and 200
-- subagents), `away` (the device can't be reached), `actions_fail`,
-- `actions_ok`, `fixture.light` / `fixture.dark`.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local _stub = require("@app/files_stub")
local sessions = require("@account/sessions")
local store = require("@app/screens/chat/store")
local route = require("@app/screens/shell/route")
local lanes = require("@app/screens/chat/lanes")

local said = ui.state("calls", "")
local function note(what: string)
	said:set(if said:peek() == "" then what else `{said:peek()},{what}`)
end

-- A promise settled a beat later, as a host's is.
local function settled(ok: boolean, value: any): any
	return {
		after = function(_self: any, done: ((any) -> ())?, failed: ((any) -> ())?)
			ui.after(0, function()
				if ok and done then
					done(value)
				elseif not ok and failed then
					failed(value)
				end
			end)
		end,
	}
end

for _, scheme in { "light", "dark" } do
	ui.command {
		id = `fixture.{scheme}`,
		title = `Fixture: {scheme}`,
		menu = "Fixture",
		run = function()
			ui.theme.follow_system(false)
			ui.theme.current:set(`zeron-{scheme}`)
		end,
	}
end

local NOW = os.time()
local children = {
	{ id = "side-a", parent = "chat-1", title = "Why does the sync test flake?", device = "d", status = "working", updated = NOW - 60, created = NOW - 120, agent = "claude" },
	{ id = "side-b", parent = "chat-1", title = "Draft the release notes", device = "d", updated = NOW - 1800, created = NOW - 1900, agent = "openai" },
}
local online = true
-- (`box_project`: the project checked out on the devbox too.)
local box_project = false
local function publish()
	sessions.now:set({
		state = "ready",
		here = "d",
		devices = {
			{ id = "d", name = "work-laptop", platform = "macos", here = true, online = true },
			{ id = "box", name = "devbox", platform = "linux", here = false, online = online },
		},
		projects = if box_project
			then {
				{ id = "p1", name = "comet-native", device = "d", path = "/Users/me/comet-native", git = true },
				{ id = "p2", name = "comet-native", device = "box", path = "/srv/comet-native", git = true },
			}
			else { { id = "p1", name = "comet-native", device = "d", path = "/Users/me/comet-native", git = true } },
		chats = {
			{ id = "chat-1", title = "Parent", project = "p1", device = "d", branch = "zeron/panel-home", cwd = "/proj", status = "idle", updated = NOW, created = NOW - 3600, side_chats = #children },
			{ id = "chat-2", title = "On the devbox", device = "box", branch = "main", cwd = "/srv", updated = NOW, created = NOW - 3600 },
		},
		archived = {},
		children = { ["chat-1"] = table.clone(children) },
		places = { pinned = {}, sections = {} },
		places_ready = true,
	} :: any)
end
publish()

local subs = {
	{ id = "t1", kind = "agent", verb = "Ran a subagent", subagent = { ref = "chat-1--sub--1", title = "scan the repo", status = "running" } },
	{ id = "t2", kind = "agent", verb = "Ran a subagent", subagent = { ref = "chat-1--sub--2", title = "read the docs", status = "done" } },
}
local function publish_messages()
	store.messages:set({
		{ id = "m1", role = "user", text = "Scan it" },
		{ id = "m2", role = "assistant", text = "", started = NOW - 300, tools = table.clone(subs) },
	} :: any)
end
store.session:set({ id = "chat-1", state = "ready", title = "Parent" } :: any)
publish_messages()

-- What's asked of sessions and lanes.
local stub_sessions = sessions :: any
stub_sessions.create = function(req: any): any
	note(`create:{req.parent}:{req.id}`)
	-- (Where it's made, when its composer moved it.)
	if req.project or req.device or req.no_project then
		note(`place:{req.id}:{req.project or ""}:{req.device or ""}`)
	end
	return settled(true, req.id)
end
stub_sessions.fork = function(source: string, _parent: string?, id: string?): any
	note(`fork:{source}:{id}`)
	return settled(true, id)
end
local stub_store = store :: any
stub_store.send = function(req: any): any
	note(`send:{req.session}:{req.text}`)
	return settled(true, nil)
end
local stub_lanes = lanes :: any
stub_lanes.watch = function(slot: string, id: string?, frozen: boolean?, _chat: string?)
	note(`watch:{id or "none"}{if frozen then ":frozen" else ""}`)
	if id then
		lanes.view(slot).messages:set({ { id = `{id}-m`, role = "assistant", text = `This is {id}` } })
	end
end

-- The home's host calls: a project with one action and one to import from
-- its zeron.json; the branch +324962 −151 over main.
local home_api = require("@app/screens/panel/home_api") :: any
local actions_fail = false
local snapshot = {
	actions = { { id = "a1", name = "Dev server", command = "npm run dev", icon = "play", setup = false } },
	importable = { { name = "Tests", command = "cargo test", icon = "test", setup = false } },
}
home_api.actions = function(space: string, device: string?): any
	note(`actions:{space}:{device or ""}`)
	if actions_fail then
		return settled(false, { message = "The engine couldn't read zeron.json" })
	end
	return settled(true, table.clone(snapshot))
end
home_api.save_action = function(space: string, _device: string?, draft: any): any
	note(`save:{space}:{draft.id or "new"}:{draft.name}:{draft.command}:{draft.icon}:{tostring(draft.setup)}`)
	local next_actions = {}
	local found = false
	for _, a in snapshot.actions do
		if draft.id and a.id == draft.id then
			table.insert(next_actions, { id = a.id, name = draft.name, command = draft.command, icon = draft.icon, setup = draft.setup })
			found = true
		else
			table.insert(next_actions, a)
		end
	end
	if not found then
		table.insert(next_actions, { id = `a{#next_actions + 1}`, name = draft.name, command = draft.command, icon = draft.icon, setup = draft.setup })
	end
	snapshot = { actions = next_actions, importable = snapshot.importable }
	return settled(true, table.clone(snapshot))
end
home_api.delete_action = function(space: string, _device: string?, id: string): any
	note(`delete:{space}:{id}`)
	local next_actions = {}
	for _, a in snapshot.actions do
		if a.id ~= id then
			table.insert(next_actions, a)
		end
	end
	snapshot = { actions = next_actions, importable = snapshot.importable }
	return settled(true, table.clone(snapshot))
end
home_api.run_action = function(space: string, chat: string, device: string?, id: string): any
	note(`run:{space}:{chat}:{device or ""}:{id}`)
	return settled(true, "term-action")
end

-- The terminal's host and zgx's terminal, stood in for (as
-- panel_fixture's): its view says which terminal it shows.
local tapi = require("@app/screens/panel/terminal_api") :: any
tapi.start = function(_target: any): any
	note("term:open")
	return settled(true, { command = "bridge new", claim = "claim-1" })
end
tapi.claimed = function(_claim: string): any
	return settled(true, "term-new")
end
tapi.command = function(id: string, _device: string?): any
	note(`term:attach:{id}`)
	return settled(true, `bridge {id}`)
end
tapi.status = function(): any
	return settled(true, { busy = false, exited = false })
end
tapi.close = function(id: string): any
	note(`term:close:{id}`)
	return settled(true, nil)
end
local terms: { [string]: any } = {}
tapi.term = function(key: string, options: any?): any
	if not terms[key] and options then
		terms[key] = { command = options.command, close = function() end }
	end
	return terms[key]
end
tapi.view = function(props: any): Node
	return ui.column {
		id = "stub-terminal",
		grow = 1,
		ui.text { `terminal {if props.term then props.term.command else ""}`, literal = true },
	}
end

-- Plugins' tabs asked for (the branch's +N −M opens the Git tab).
local Panel = require("@app/screens/panel")
local open_plugin_tab = Panel.open_plugin_tab
Panel.open_plugin_tab = function(session: any, tab: any)
	local d = tab.data or {}
	note(`plugin_tab:{tab.plugin}:{tab.kind}:{d.view or ""}:{d.scope or ""}`)
	open_plugin_tab(session, tab)
end

ui.command { id = "many", title = "Many lanes", run = function()
	for i = 1, 300 do
		-- (One far down waits on you: it shows past the first few.)
		table.insert(children, { id = `side-{i}`, parent = "chat-1", title = `Side chat {i}`, device = "d", status = if i == 250 then "input" else nil, updated = NOW - 3000 - i, created = NOW - 9000 })
	end
	publish()
	for i = 3, 202 do
		table.insert(subs, { id = `t{i}`, kind = "agent", verb = "Ran a subagent", subagent = { ref = `chat-1--sub--{i}`, title = `Subagent {i}`, status = "done" } })
	end
	publish_messages()
end }
-- Draft the release notes finishes (again, each time: a new reply, later
-- than the last).
local finishes = 0
ui.command { id = "side_done", title = "Draft the release notes finishes", run = function()
	finishes += 1
	for _, c in children do
		if c.id == "side-b" then
			c.status = "done"
			-- (The first a moment ago; each after it later than it was
			-- seen, which is now.)
			c.updated = os.time() - 5 + (finishes - 1) * 10
		end
	end
	publish()
end }
-- A side chat on the devbox (a worktree there): its own device's group.
ui.command { id = "side_elsewhere", title = "A side chat on the devbox", run = function()
	table.insert(children, { id = "side-box", parent = "chat-1", title = "Try it on the devbox", device = "box", updated = NOW - 10, created = NOW - 20, agent = "claude" })
	publish()
end }
ui.command { id = "box_project", title = "The project on the devbox too", run = function()
	box_project = true
	publish()
end }
-- Lanes at work further down: a side chat working since long ago (under
-- the idle one, by when they moved), and a subagent running after the
-- finished one (spawned in the same turn).
ui.command { id = "lanes_at_work", title = "Lanes at work further down", run = function()
	table.insert(children, { id = "side-old", parent = "chat-1", title = "Long-running refactor", device = "d", status = "working", updated = NOW - 7200, created = NOW - 7300, agent = "claude" })
	publish()
	table.insert(subs, { id = "t3", kind = "agent", verb = "Ran a subagent", subagent = { ref = "chat-1--sub--3", title = "watch the build", status = "running" } })
	publish_messages()
end }
-- The rows' menus (a secondary click opens them): a session's, the
-- sidebar's own builder, for `test_menu_of`; a subagent's, the home's.
-- Their labels in `test_menu_labels`.
local menu_of = ui.state("test_menu_of", "")
local menu_labels = ui.state("test_menu_labels", "")
local function labels(entries: { any }?): string
	local out = {}
	for _, e in entries or {} do
		table.insert(out, if e.separator then "---" elseif e.heading then `#{e.heading}` else e.label)
	end
	return table.concat(out, ",")
end
ui.command { id = "session_menu", title = "Session menu", run = function()
	local sidebar: any = require("@app/screens/shell/sidebar")
	menu_labels:set(labels(sidebar.session_menu(menu_of:peek())))
end }
ui.command { id = "subagent_menu", title = "Subagent menu", run = function()
	local entries = require("@app/screens/panel/home").subagent_menu(function()
		lanes.open_subagent("chat-1", { ref = "chat-1--sub--1", title = "scan the repo", status = "running" })
	end)
	menu_labels:set(labels(entries))
	entries[1].on_select()
end }
-- A port's menu (Home.port_menu), for `http://web.proj.localhost:7400`:
-- its labels in `test_menu_labels`; `port_menu_pick` does the one
-- `test_menu_pick` names. Opening outside is noted (`external:<url>`).
local stub_browser = require("@app/screens/panel/browser_api") :: any
-- (As the host says them: this fixture has no host.)
do
	local said = ui.shared("chat.subagents", {} :: { any })
	ui.effect(function()
		said:set(require("@app/screens/chat/lanes").subagents_of(require("@app/screens/chat/store").messages:get()))
	end)
end
stub_browser.open_externally = function(url: string)
	note(`external:{url}`)
end
local port_pick = ui.state("test_menu_pick", "")
local function port_entries(): { any }
	return require("@app/screens/panel/home").port_menu("http://web.proj.localhost:7400", function()
		note("port_open")
	end)
end
ui.command { id = "port_menu", title = "Port menu", run = function()
	menu_labels:set(labels(port_entries()))
end }
ui.command { id = "port_menu_pick", title = "Port menu pick", run = function()
	for _, e in port_entries() do
		if e.label == port_pick:peek() then
			e.on_select()
		end
	end
end }
ui.command { id = "away", title = "Device away", run = function()
	online = false
	publish()
end }
ui.command { id = "actions_fail", title = "Actions fail", run = function()
	actions_fail = true
end }
ui.command { id = "actions_ok", title = "Actions work", run = function()
	actions_fail = false
end }

route.open_session("chat-1")

local session = ui.state("test_home_session", { id = "chat-1", cwd = "/proj" } :: any)
local generation = ui.state("test_home_generation", 1)

return ui.column {
	w_full = true,
	h_full = true,
	bg = "$zeron.window",
	-- (As the shell's root: every focus, and every key nothing took.)
	on_focus = function()
		Panel.focus_moved()
	end,
	on_key = function(e): boolean
		return Panel.home_key(e)
	end,
	ui.row {
		w_full = true,
		grow = 1,
		min_h = 0,
		-- Something outside the panel that takes the keyboard (a title
		-- bar's button), and a text field (the composer's).
		ui.column {
			w = 260,
			p = "$space.3",
			gap = "$space.2",
			ui.button { id = "outside", label = "Outside", h = 28, w_full = true, ui.text { "Outside" } },
			ds.TextField { value = ui.state("test_home_draft", ""), name = "Message", w_full = true },
		},
		ui.presence(function(): string
			return tostring(generation:get())
		end, function(): Node
			return Panel.Panel {
				session = session,
				project = { id = "p1", path = "/proj", name = "comet-native" },
				grow = 1,
				h_full = true,
			}
		end, { grow = 1, h_full = true, min_h = 0 }),
	},
}
