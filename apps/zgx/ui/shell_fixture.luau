-- The main window's shell on its own, with sessions made up here and no
-- engine: what tests/shell_*.test.luau drive. The host's calls
-- (@account/sessions) are stubbed to change this data as the engine would;
-- `ui.command`s stand in for what the engine does on its own (a session
-- starting to work, finishing, renamed on another device, a new one
-- arriving, many at once). Here, beside app.luau, as a mounted script loads
-- modules from its own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
-- Addresses plain to start (hiding is the app's default): the tests read
-- them, and private_text's turn hiding on themselves.
ds.PrivateText.hidden:set(false)
local sessions = require("@account/sessions")
-- (Its start as it is, a new chat: not the launch's jump to the latest
-- session, which its tests drive themselves.)
require("@app/screens/shell/route").launched:set(true)
-- (The sidebar these tests were written on: grouped by project, Activity
-- off, empty projects listed. The app opens with Activity, by project then
-- device and no empty projects (tests/sidebar_defaults.test.luau); a test
-- of those views turns them on itself. Each set once, to what it stays: a
-- reload runs this again, and a value set and then changed back here moves
-- the sidebar still showing (its list crossfading out takes the keyboard
-- from its row before the reload can keep it there).)
require("@app/screens/shell/activity").on:set(false)
local sidebar_options = require("@app/screens/shell/sidebar").options
sidebar_options.organize:set("project")
sidebar_options.details.empty_projects:set(true)
local actions = require("@screens/onboarding/actions")
local store = require("@app/screens/chat/store")
-- The host's chats, as the engine has them: one record a chat (its session,
-- its messages, its reply streaming), shown by every view watching it: the
-- window's open chat (screens/chat/store) and each lane (a split's chats).
-- A test changes a chat by setting `fixture.chats` (`{ [id]: { session?,
-- messages?, draft? } }`); what's watched is kept in `split_lanes`. A chat
-- with no record shows its own words, "This is <id>", in a lane.
local lanes = require("@app/screens/chat/lanes")
local split_lanes = ui.state("split_lanes", {} :: { [string]: string })
local fixture_chats = ui.shared("fixture.chats", {} :: { [string]: any })
local open_chat = ui.state("fixture_open_chat", "")
local function project(view: any, id: string, lane: boolean)
	local c = fixture_chats:peek()[id]
	if not c then
		if lane then
			-- (Asked again for the chat it has, as a reload asks, it keeps
			-- what it has, as the host does.)
			local first = view.messages:peek()[1]
			if not (first and first.id == `{id}-m`) then
				view.messages:set({ { id = `{id}-m`, role = "assistant", text = `This is {id}` } })
			end
		end
		return
	end
	view.messages:set(c.messages or { { id = `{id}-m`, role = "assistant", text = `This is {id}` } })
	view.draft:set(c.draft or { id = "", text = "" })
	view.session:set(c.session)
end
local stub_lanes = lanes :: any
stub_lanes.watch = function(slot: string, id: string?)
	split_lanes:set_key(slot, id)
	if id then
		project(lanes.view(slot), id, true)
	end
end
-- (The window's open chat: in place of the store's, below.)
local function open_store(id: string?)
	open_chat:set(id or "")
	if id then
		project(store, id, false)
	end
end
ui.effect(function()
	fixture_chats:get()
	for slot, id in split_lanes:peek() do
		if id then
			project(lanes.view(slot), id, true)
		end
	end
	local id = open_chat:peek()
	if id ~= "" then
		project(store, id, false)
	end
end)
-- Settings' host calls, answered as the host would (settings_stub.luau).
-- (Answered at once, as the host's calls do a beat later.)
local function answered(value: any): any
	return {
		after = function(_, ok: (any) -> ())
			ui.after(0, function()
				ok(value)
			end)
		end,
	}
end
-- (A terminal's host and zgx's terminal, stood in for: an engine terminal
-- a number, its view a column saying which, as ui/panel_fixture's.)
local tapi = require("@app/screens/panel/terminal_api") :: any
local terms = ui.state("fixture_terms", 0)
local term_sessions: { [string]: any } = {}
tapi.start = function(_target: any): any
	terms:add(1)
	return answered({ command = `bridge term-{terms:peek()}`, claim = `claim-{terms:peek()}` })
end
tapi.claimed = function(claim: string): any
	return answered(`term-{string.match(claim, "%d+")}`)
end
tapi.command = function(id: string): any
	return answered(`bridge {id}`)
end
tapi.status = function(): any
	return answered({ busy = false, exited = false })
end
tapi.close = function(): any
	return answered(nil)
end
tapi.write = function(): any
	return answered(nil)
end
tapi.term = function(key: string, options: any?): any
	local s = term_sessions[key]
	if not s and options then
		s = { title = nil, close = function()
			term_sessions[key] = nil
		end, focus = function() end }
		term_sessions[key] = s
	end
	return s
end
tapi.view = function(props: any): Node
	return ui.column { grow = 1, label = "Terminal", ui.text { `terminal {props.label}`, literal = true } }
end
-- (Before Settings' stand-ins, which put theirs over these: the
-- registry's sign-in terminal, every other one as here.)
require("./settings_stub")

-- No window chrome to restyle headless, and no chat to open (the chat
-- page's own fixture drives that).
local stub_actions = actions :: any
stub_actions.apply_scheme = function() end
-- (Its look set by hand, for captures in either scheme.)
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
-- (A headless host draws no WebP: a new chat on no picture.)
local appearance = require("@app/screens/shell/appearance")
appearance.wallpaper:set("none")
-- A wallpaper chosen, for what stands over a new chat's art (v2's
-- title-bar island): its picture needn't draw.
ui.command {
	id = "fixture.wallpaper",
	title = "Fixture: a wallpaper",
	menu = "Fixture",
	run = function()
		appearance.image:set({ path = "/fixture/wallpaper.png", name = "wallpaper.png", w = 1600, h = 1000, x = 0.5, y = 0.5, zoom = 1 })
		appearance.choose("image")
	end,
}
-- The files pane's host calls (its own fixture drives them): a folder of
-- one file, a beat later, as the host settles.
local files_api = require("@app/screens/files/api") :: any
local function settle(value: any): any
	return {
		after = function(_, ok: (any) -> ())
			ui.after(0, function()
				ok(value)
			end)
		end,
	}
end
local README = { name = "README.md", path = "README.md", dir = false, kind = "file", size = 7, ignored = false, readonly = false, rev = "r1" }
for name, f in files_api do
	if type(f) == "function" then
		files_api[name] = function()
			return settle(nil)
		end
	end
end
files_api.list = function(): any
	return settle({ checkout = "c1", can_move = false, can_delete = false, truncated = false, entries = { README } })
end
files_api.read = function(): any
	return settle({ text = "# Atlas", rev = "r1", kind = "text" })
end
-- (Watching answers whether what was read is still current: nothing kept.)
files_api.watch = function(): any
	return settle(nil)
end
files_api.unwatch = function() end
-- (What's in the files: one line found, for anything.)
files_api.search_content = function(): any
	return settle({
		matches = { { path = "README.md", line = 1, column = 3, text = "# Atlas", ranges = { { from = 2, to = 7 } } } },
		truncated = false,
		indexing = false,
	})
end
-- (The workspace's search: what's found for anything, best first.)
files_api.search = function(): any
	return settle({
		{ path = "README.md", dir = false },
		{ path = "docs/GUIDE.md", dir = false },
		{ path = "docs", dir = true },
	})
end
local chat_store = store :: any
for name, f in chat_store do
	-- (A sent prompt's id is the store's own: its row is keyed by it.)
	if type(f) == "function" and name ~= "new_id" then
		chat_store[name] = function() end
	end
end
-- (Its open chat shown as the host's chats have it, above.)
chat_store.open = open_store
-- A message sent, answered as the host would (taken); each kept in
-- `fixture_sent`, for a test to read.
local sent = ui.state("fixture_sent", {} :: { any })
chat_store.send = function(req: any): any
	local now = table.clone(sent:peek())
	table.insert(now, req)
	sent:set(now)
	return {
		after = function(_, ok: (any) -> ())
			ui.after(0, function()
				ok(nil)
			end)
		end,
	}
end
store.catalog:set({
	state = "ready",
	harnesses = { { id = "claude-code", label = "Claude", mark = "claude" } },
	models = { { value = "opus", label = "Opus", harness = "claude-code" } },
} :: any)

local HOUR = 3600
local DAY = 24 * HOUR
local NOW = os.time()

-- The registry icons the app has heard of (src/agents.rs `agent_icons`).
ui.shared("agent_icons", {} :: { [string]: string }):set({
	["acp.factory-droid"] = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="currentColor" d="M8 1l2 5 5 2-5 2-2 5-2-5-5-2 5-2z"/></svg>',
})

local model = {
	here = "mac",
	devices = {
		{ id = "mac", name = "MacBook Pro", platform = "macos", here = true },
		{ id = "box", name = "devbox", platform = "linux", here = false },
	} :: { any },
	projects = {
		{ id = "atlas", name = "Atlas", device = "mac", path = "/work/atlas", git = true },
		{ id = "borealis", name = "Borealis", device = "box", path = "/srv/borealis", git = true },
	},
	chats = {
		{ id = "a1", title = "Fix the build", project = "atlas", device = "mac", branch = "fix/build", status = "working", updated = NOW - 60, created = NOW - 2 * HOUR, agent = "claude" },
		{ id = "a2", title = "Write the docs", project = "atlas", device = "mac", branch = "docs", updated = NOW - HOUR, created = NOW - 3 * HOUR, agent = "openai", harness = "codex", harness_session = "th 7/a" },
		{ id = "a3", title = "Tidy the tests", project = "atlas", device = "mac", updated = NOW - 2 * HOUR, created = NOW - 5 * HOUR, agent = "claude" },
		{ id = "b1", title = "Ship the release", project = "borealis", device = "box", status = "input", updated = NOW - 3 * HOUR, created = NOW - 6 * HOUR, agent = "claude" },
		{ id = "b2", title = "Answer the review", project = "borealis", device = "box", updated = NOW - 24 * HOUR, created = NOW - 30 * HOUR, agent = "cursor" },
	} :: { any },
	archived = {
		{ id = "z1", title = "Old experiment", project = "atlas", device = "mac", updated = NOW - 48 * HOUR, created = NOW - 50 * HOUR, agent = "claude" },
	} :: { any },
	places = { pinned = {}, sections = {} } :: any,
}

-- (`fixture.sessions_loading`: an engine that hasn't answered yet, nothing heard.)
local unheard = false
-- (`fixture.engine_offline`: no engine to ask, nothing heard.)
local no_engine = false
local function publish()
	local chats = {}
	for _, c in model.chats do
		table.insert(chats, table.clone(c))
	end
	local archived = {}
	for _, c in model.archived do
		table.insert(archived, table.clone(c))
	end
	local sections = {}
	for _, s in model.places.sections do
		table.insert(sections, { id = s.id, name = s.name, sessions = table.clone(s.sessions), collapsed = s.collapsed == true })
	end
	sessions.now:set({
		state = if no_engine then "offline" elseif unheard then "loading" else "ready",
		here = model.here,
		devices = model.devices,
		projects = model.projects,
		chats = chats,
		archived = archived,
		places = { pinned = table.clone(model.places.pinned), sections = sections },
		places_ready = true,
		connection = (model :: any).connection,
		-- Side chats, by their session (`side_chats`).
		children = (model :: any).children,
	} :: any)
end

local function find(id: string): any
	for _, c in model.chats do
		if c.id == id then
			return c
		end
	end
	return nil
end

-- What the shell asked of the host, in order (`calls`).
local said: { string } = {}
local calls = ui.state("calls", "")
local function note(what: string)
	table.insert(said, what)
	calls:set(table.concat(said, ","))
end

-- An engine that answers a place when told (`hold_echo`, then `echo`).
local holding = false
local held: any = nil
local stub = sessions :: any
stub.place = function(p: any)
	-- (The same places again is nothing: the host sends only changes.)
	local before = model.places
	local same = #before.pinned == #p.pinned and #before.sections == #p.sections
	if same then
		for i, id in p.pinned do
			same = same and before.pinned[i] == id
		end
		for i, s in p.sections do
			local o = before.sections[i]
			same = same and o ~= nil and o.id == s.id and o.name == s.name and #o.sessions == #s.sessions
		end
	end
	if same then
		return
	end
	note(`place:{table.concat(p.pinned, "+")}`)
	-- (Held: the engine answers when told, `echo`, as a real one answers a
	-- beat later.)
	if holding then
		held = p
		return
	end
	-- (A section's fold is the host's: kept.)
	for _, s in p.sections do
		for _, o in before.sections do
			if o.id == s.id then
				s.collapsed = o.collapsed
			end
		end
	end
	model.places = p
	publish()
end
stub.archive = function(id: string)
	note(`archive:{id}`)
	for i, c in model.chats do
		if c.id == id then
			table.insert(model.archived, 1, table.remove(model.chats, i))
			break
		end
	end
	publish()
end
stub.unarchive = function(id: string)
	note(`unarchive:{id}`)
	for i, c in model.archived do
		if c.id == id then
			table.insert(model.chats, table.remove(model.archived, i))
			break
		end
	end
	publish()
end
-- (`archived_delete_fails`: the host refuses those it names.)
local refused: { [string]: boolean } = {}
stub.delete = function(id: string): any
	note(`delete:{id}`)
	if refused[id] then
		return {
			after = function(_, _ok: any, failed: any)
				if failed then
					failed("refused")
				end
			end,
		}
	end
	for _, list in { model.chats, model.archived } do
		for i, c in list do
			if c.id == id then
				table.remove(list, i)
				break
			end
		end
	end
	publish()
	return {
		after = function(_, ok: any)
			if ok then
				ok(nil)
			end
		end,
	}
end
-- What a delete takes with it, as the host counts it (`tidy_side_chats`).
stub.deleted_with = function(id: string): any
	return settle(((model :: any).deleted_with or {})[id] or { subagents = 0, sides = 0 })
end
-- (A link as the host makes it: its workspace hashed there.)
stub.link = function(id: string): any
	note(`link:{id}`)
	return {
		after = function(_, ok: (any) -> ())
			ok(`zeron://open/chat/{id}?workspace=0123456789abcdef`)
		end,
	}
end
stub.collapse = function(id: string, folded: boolean)
	note(`collapse:{id}:{tostring(folded)}`)
	for _, s in model.places.sections do
		if s.id == id then
			s.collapsed = folded
		end
	end
	publish()
end
stub.rename = function(id: string, title: string)
	note(`rename:{id}:{title}`)
	find(id).title = title
	publish()
end
-- Sleep, as the engine keeps it: until a time (none, awake).
stub.sleep = function(id: string, until_at: number?): any
	note(`sleep:{id}:{tostring(until_at)}`)
	local c = find(id)
	if c then
		c.sleep_until = until_at
		c.woke = nil
	end
	publish()
	return { after = function(_, ok: any)
		if ok then
			ok(nil)
		end
	end }
end
-- Stop waiting: the processes it waited on no longer hold it; with nothing
-- else to wait on, its turn's end stands (finished, unseen).
stub.stop_waiting = function(id: string): any
	note(`stop_waiting:{id}`)
	local c = find(id)
	if c and c.waiting then
		local kept = {}
		for _, w in c.waiting do
			if w.kind ~= "process" then
				table.insert(kept, w)
			end
		end
		c.waiting = if #kept > 0 then kept else nil
		if not c.waiting then
			c.status = "done"
		end
	end
	publish()
	return { after = function(_, ok: any)
		if ok then
			ok(nil)
		end
	end }
end
stub.seen = function(id: string)
	note(`seen:{id}`)
	local c = find(id)
	if c and c.status == "done" then
		c.status = nil
		publish()
	end
end
stub.create = function(): any
	return ui.call("sessions.create", {})
end
stub.rename_project = function(id: string, name: string)
	note(`rename_project:{id}:{name}`)
	for _, p in model.projects do
		if p.id == id then
			p.name = name
		end
	end
	publish()
end
-- (`fixture.icons_outdated`: refused as an engine from before project
-- icons refuses it.)
local icons_refused: string? = nil
-- Cloud's machines among the devices, as the engine lists them: two
-- sessions' (their platform, id and capability say Cloud's), never a device
-- to pick.
ui.command {
	id = "fixture.sessions_loading",
	title = "Fixture: an engine that hasn't answered, no chats heard",
	run = function()
		unheard = true
		model.chats = {}
		model.projects = {}
		model.places.pinned = {}
		publish()
	end,
}
ui.command {
	id = "fixture.no_sessions",
	title = "Fixture: an engine that answered, with no chats",
	run = function()
		unheard = false
		model.chats = {}
		model.projects = {}
		model.places.pinned = {}
		publish()
	end,
}
ui.command {
	id = "fixture.cloud_machines",
	title = "Fixture: Cloud sessions' machines among the devices",
	run = function()
		table.insert(model.devices, { id = "cloud-s1", name = "Cloud session", platform = "cloud", here = false, online = false, cloud = true })
		table.insert(model.devices, { id = "cloud-s2", name = "Cloud session", platform = "cloud", here = false, online = false, cloud = true })
		publish()
	end,
}
-- Cloud chats' machines, each with its own copy of the repository's
-- project (as the engine stamps them, one repository identity), as Wing's
-- beta.5 had them: zeronsh/zeron on three, acme/app on a fourth, and a
-- clone of zeronsh/zeron on this Mac too.
ui.command {
	id = "fixture.cloud_projects",
	title = "Fixture: Cloud machines, each with a copy of a repository's project",
	run = function()
		for i = 1, 4 do
			table.insert(model.devices, { id = `cloud-m{i}`, name = "Cloud session", platform = "cloud", here = false, online = false, cloud = true })
		end
		local zeron = { github = "zeronsh/zeron", repository = "github.com/zeronsh/zeron", path = "/home/user/zeron", git = true }
		for i, created in { 100, 300, 200 } do
			table.insert(model.projects, { id = `zc{i}`, name = "zeron", device = `cloud-m{i}`, path = zeron.path, git = true, github = zeron.github, repository = zeron.repository, created = created })
		end
		table.insert(model.projects, { id = "app1", name = "app", device = "cloud-m4", path = "/home/user/app", git = true, github = "acme/app", repository = "github.com/acme/app", created = 150 })
		table.insert(model.projects, { id = "zmac", name = "zeron", device = "mac", path = "/work/zeron", git = true, github = "zeronsh/zeron", repository = "github.com/zeronsh/zeron", created = 50 })
		table.insert(model.chats, { id = "zc1a", title = "Cloud fix", project = "zc1", device = "cloud-m1", status = "working", updated = NOW - 120, created = NOW - HOUR, agent = "claude" })
		table.insert(model.chats, { id = "zc2a", title = "Cloud docs", project = "zc2", device = "cloud-m2", updated = NOW - 2 * HOUR, created = NOW - 3 * HOUR, agent = "claude" })
		table.insert(model.archived, { id = "zc3a", title = "Cloud old", project = "zc3", device = "cloud-m3", updated = NOW - 30 * HOUR, created = NOW - 31 * HOUR, agent = "claude" })
		publish()
	end,
}
ui.command { id = "fixture.icons_outdated", title = "Fixture: an engine without project icons", menu = "Fixture", run = function()
	icons_refused = "outdated"
end }
stub.set_project_icon = function(id: string, icon: any): any
	local i = icon or {}
	if icons_refused then
		local word = icons_refused
		note(`set_project_icon_refused:{id}`)
		return {
			after = function(_, _ok: any, fail: ((any) -> ())?)
				ui.after(0, function()
					if fail then
						fail({ message = word })
					end
				end)
			end,
		}
	end
	note(`set_project_icon:{id}:{i.emoji or ""}:{i.image or ""}:{i.hue or ""}`)
	for _, p in model.projects do
		if p.id == id then
			p.icon = if i.emoji or i.image or i.hue then { emoji = i.emoji, image = i.image, hue = i.hue } else nil
		end
	end
	publish()
	return {
		after = function(_, ok: (any) -> (), _fail: any)
			ui.after(0, function()
				ok(nil)
			end)
		end,
	}
end

-- Adding a project (@account/projects), as the engine answers: each
-- device's home a folder of three (one a repository, one hidden), a drive,
-- and the project made arriving a beat after the answer, as the engine
-- tells the sessions.
local projects_stub = require("@account/projects") :: any
local function answer(value: any): any
	return {
		after = function(_, ok: (any) -> (), fail: ((any) -> ())?)
			ui.after(0, function()
				if type(value) == "table" and value.refused then
					if fail then
						fail(value.refused)
					end
				else
					ok(value)
				end
			end)
		end,
	}
end
local HOMES = { mac = "/Users/me", box = "/home/me" }
projects_stub.folders = function(device: string, path: string?): any
	note(`folders:{device}:{path or "~"}`)
	local at = path or HOMES[device]
	if path == "/gone" then
		return answer({ refused = "No such file or directory (os error 2)" })
	end
	if at == HOMES[device] then
		return answer({
			path = at,
			truncated = false,
			entries = {
				{ name = "code", is_dir = true, is_repo = false },
				{ name = "notes", is_dir = true, is_repo = true },
				{ name = ".cache", is_dir = true, is_repo = false },
				{ name = "todo.txt", is_dir = false, is_repo = false },
			},
		})
	end
	-- (A system folder: one folder, and one a link to a folder, which the
	-- engine lists as no folder.)
	if at == "/var" then
		return answer({
			path = at,
			truncated = false,
			entries = { { name = "folders", is_dir = true, is_repo = false }, { name = "run", is_dir = false, is_repo = false } },
		})
	end
	-- (A drive with a project in it already.)
	if at == "/work" then
		return answer({ path = at, truncated = false, entries = { { name = "atlas", is_dir = true, is_repo = true } } })
	end
	return answer({ path = at, truncated = false, entries = { { name = "zephyr", is_dir = true, is_repo = true } } })
end
projects_stub.drives = function(device: string): any
	note(`drives:{device}`)
	return answer({ { name = "Work", path = "/work" } })
end
-- (Home's index: still scanning at a query's first ask, then two found,
-- one of them a folder Home lists already.)
local searched: { [string]: number } = {}
projects_stub.search_home = function(device: string, query: string): any
	note(`search_home:{device}:{query}`)
	searched[query] = (searched[query] or 0) + 1
	if query == "" or searched[query] == 1 then
		return answer({ matches = {}, indexing = query ~= "" })
	end
	local home = HOMES[device]
	return answer({
		matches = {
			{ path = `{home}/code/zephyr`, relative = "code/zephyr", is_repo = true },
			{ path = `{home}/code`, relative = "code", is_repo = false },
		},
		indexing = false,
	})
end
projects_stub.release_home = function(device: string)
	note(`release_home:{device}`)
end
projects_stub.clone = function(device: string, url: string, parent: string): any
	note(`clone:{device}:{url}:{parent}`)
	local name = string.match(url, "([^/]+)%.git$") or string.match(url, "([^/]+)$") or "repo"
	return answer({ path = `{parent}/{name}`, name = name })
end
local made = 0
projects_stub.create = function(device: string, path: string, git: boolean, repo: string?): any
	-- (A Cloud project's repository after it.)
	note(`create:{device}:{path}:{tostring(git)}{if repo then `:{repo}` else ""}`)
	made += 1
	local id = `new{made}`
	ui.after(0, function()
		local name = string.match(path, "([^/\\]+)$") or path
		table.insert(model.projects, { id = id, name = name, device = device, path = path, git = git, github = repo })
		publish()
	end)
	return answer(id)
end
-- A project's icon (@account/projects): no repository artwork here (the
-- initials show), a picture's size as the host reads it, the upload
-- answered with an edge URL, and that URL's file the picture itself.
local uploaded: { [string]: string } = {}
projects_stub.artwork = function(_p: any, done: (string?) -> ())
	ui.after(0, function()
		done(nil)
	end)
end
projects_stub.picture_size = function(path: string): any
	note(`picture_size:{path}`)
	-- (The captures' photo, at its own size.)
	if string.find(path, "plate-3-sunrise", 1, true) then
		return answer({ w = 2100, h = 700 })
	end
	return answer({ w = 64, h = 64 })
end
projects_stub.upload_icon = function(path: string, crop: any): any
	note(`upload_icon:{path}:{math.floor(crop.side + 0.5)}`)
	if string.find(path, "offline", 1, true) then
		return answer({ refused = { message = "offline" } })
	end
	-- (An edge without images of yours, as production's before its deploy.)
	if string.find(path, "unavailable", 1, true) then
		return answer({ refused = { message = "unavailable" } })
	end
	local url = `https://edge.test/auth/avatars/{string.rep("a", 32)}`
	uploaded[url] = path
	return answer(url)
end
projects_stub.forget_icon = function(url: string)
	note(`forget_icon:{url}`)
end
projects_stub.icon_file = function(url: string): any
	return answer(uploaded[url] or "assets/favicons/zeron.png")
end
projects_stub.delete = function(id: string): any
	note(`delete_project:{id}`)
	for i, p in model.projects do
		if p.id == id then
			table.remove(model.projects, i)
			break
		end
	end
	for i = #model.chats, 1, -1 do
		if model.chats[i].project == id then
			table.remove(model.chats, i)
		end
	end
	publish()
	return answer(nil)
end
-- Settings → Devices' Remove and Projects' rename, as the engine answers:
-- this device and one online refused, in the host's words.
stub.remove_device = function(id: string): any
	note(`remove_device:{id}`)
	for i, d in model.devices do
		if d.id == id then
			if d.here then
				return answer({ refused = "here" })
			elseif d.online then
				return answer({ refused = "online" })
			end
			table.remove(model.devices, i)
			break
		end
	end
	publish()
	return answer(nil)
end
stub.rename_project_or_say = function(id: string, name: string): any
	stub.rename_project(id, name)
	return answer(nil)
end
-- (Settings → Projects' Show in Finder.)
files_api.reveal = function(target: any, path: string): any
	note(`reveal:{target.space or target.chat or ""}:{target.root or ""}:{path}`)
	return settle(nil)
end
-- A session's turn stopped (Remove project stops those running first).
stub.stop = function(id: string): any
	note(`stop:{id}`)
	local c = find(id)
	if c then
		c.status = nil
	end
	publish()
	return answer(nil)
end
ui.command {
	id = "fixture.engine_offline",
	title = "Fixture: no engine to ask",
	run = function()
		no_engine = true
		model.devices = {}
		model.projects = {}
		model.chats = {}
		publish()
	end,
}
-- Sessions running in projects: two of Atlas's, and Solo's only one.
ui.command {
	id = "fixture.running_projects",
	title = "Fixture: Atlas busy twice, and Solo's one session running",
	run = function()
		local c = find("a2")
		if c then
			c.status = "working"
		end
		table.insert(model.projects, { id = "solo", name = "Solo", device = "mac", path = "/work/solo", git = true })
		table.insert(model.chats, { id = "s1", title = "Only one", project = "solo", device = "mac", status = "working", updated = NOW - 60, created = NOW - HOUR, agent = "claude" })
		publish()
	end,
}
ui.command {
	id = "fixture.nothing_heard",
	title = "Fixture: an engine that hasn't answered, nothing heard at all",
	run = function()
		unheard = true
		model.devices = {}
		model.projects = {}
		model.chats = {}
		publish()
	end,
}
ui.command {
	id = "fixture.no_devices",
	title = "Fixture: an engine that answered, with no devices",
	run = function()
		model.devices = {}
		publish()
	end,
}
ui.command {
	id = "fixture.box_online",
	title = "Fixture: devbox online now",
	run = function()
		for _, d in model.devices do
			if d.id == "box" then
				d.online = true
			end
		end
		publish()
	end,
}
-- More computers, as a long-lived account has: an old laptop not seen in
-- weeks, and a desktop on today; and a project on each.
ui.command {
	id = "fixture.more_devices",
	title = "Fixture: an old laptop and a desktop too",
	run = function()
		for _, d in model.devices do
			if d.id == "mac" then
				d.version = "0.2.140"
			elseif d.id == "box" then
				d.version = "0.2.139"
				d.seen = NOW - 3 * HOUR
			end
		end
		table.insert(model.devices, { id = "studio", name = "Mac Studio", platform = "macos", here = false, online = true, version = "0.2.140" })
		table.insert(model.devices, { id = "air", name = "Old MacBook Air", platform = "macos", here = false, seen = NOW - 41 * DAY, version = "0.2.88" })
		table.insert(model.projects, { id = "cartography", name = "Cartography", device = "studio", path = "/Users/ada/src/cartography", git = true })
		table.insert(model.projects, { id = "notes", name = "notes", device = "mac", path = "/Users/ada/Documents/notes", git = false })
		table.insert(model.projects, { id = "thesis", name = "Thesis", device = "air", path = "/Users/ada/thesis", git = true })
		publish()
	end,
}

-- What opens tabs in the panel, as plugins, links and the panel home do:
-- each its own tab (a PR from the GitHub plugin's board, a file, a page, a
-- terminal), named by a count.
local opened_tabs = ui.state("fixture_opened_tabs", 0)
local function panel_screen(): any
	return require("@app/screens/panel")
end
local function here(): string?
	local s = require("@app/screens/shell/route").session:peek()
	return if s then s.id else nil
end
for _, source in { "pr", "file", "url", "terminal" } do
	ui.command {
		id = `fixture.open_{source}`,
		title = `Fixture: open a {source} in the panel`,
		run = function()
			opened_tabs:add(1)
			local n, p = opened_tabs:peek(), panel_screen()
			if source == "pr" then
				p.open_plugin_tab(here(), { plugin = "github", kind = "pr", title = `PR #{n}`, data = { key = `pr{n}` } })
			elseif source == "file" then
				p.open_file(here(), `src/file{n}.rs`)
			elseif source == "url" then
				p.open_url(here(), `https://example.com/{n}`)
			else
				local state = require("@app/screens/panel/state")
				local k = state.key(here(), nil)
				if k then
					state.open(k, "terminal")
				end
			end
		end,
	}
end

-- What changes on the engine's side.
-- The engine's Waiting: Write the docs ends its turn waiting on its tests.
ui.command {
	id = "wait_on_tests",
	title = "Write the docs waits on its tests",
	run = function()
		local c = find("a2")
		c.status = "waiting"
		c.waiting = { { id = "bn1", kind = "process", label = "cargo test" } }
		publish()
	end,
}
-- Put to sleep for two hours and a minute (its corner: "3h").
ui.command {
	id = "sleep_a3",
	title = "Tidy the tests sleeps",
	run = function()
		stub.sleep("a3", os.time() + 2 * 3600 + 60)
	end,
}
-- Woken from sleep (its time came), not looked at since.
ui.command {
	id = "woke",
	title = "Tidy the tests woke from sleep",
	run = function()
		local c = find("a3")
		c.sleep_until = nil
		c.woke = os.time() - 30
		c.updated = os.time() - 30
		publish()
	end,
}
ui.command {
	id = "work",
	title = "Write the docs starts working",
	run = function()
		find("a2").status = "working"
		find("a2").updated = os.time()
		publish()
	end,
}
ui.command {
	id = "finish",
	title = "Fix the build finishes",
	run = function()
		find("a1").status = "done"
		publish()
	end,
}
ui.command {
	id = "rename_elsewhere",
	title = "Tidy the tests renamed on another device",
	run = function()
		find("a3").title = "Tidy every test"
		publish()
	end,
}
-- The engine's connection, as the host says it (`connection_<word>`:
-- unsaved, offline, reconnecting; `connection_fine`, healthy).
for _, word in { "unsaved", "offline", "reconnecting", "fine" } do
	ui.command {
		id = `connection_{word}`,
		title = `Connection: {word}`,
		run = function()
			(model :: any).connection = if word == "fine" then nil else word
			publish()
		end,
	}
end
-- Archived over the days (Settings → Archived): 37, by when each last
-- moved (today, yesterday, this week, earlier; anchored to today's start,
-- so a run near midnight groups them the same), two on Cloud machines of
-- their own.
local function day_start(at: number): number
	local d = os.date("*t", at) :: any
	d.hour, d.min, d.sec = 0, 0, 0
	return os.time(d)
end
ui.command {
	id = "archived_days",
	title = "Archived: 37 sessions over the days, two on Cloud",
	run = function()
		local titles = { "Port the sidebar fold", "Fix flaky login test", "Draft release notes", "Investigate memory spike", "Refactor engine RPC", "Theme picker polish", "Cloud machine warmup", "Update onboarding copy", "Bisect the scroll bug", "Rename settings keys", "Wire the panel home", "Audit the icon set" }
		local now = os.time()
		local today = day_start(now)
		local function at(i: number): number
			if i <= 3 then
				return math.max(today, now - i)
			elseif i <= 6 then
				return day_start(today - 43200) + 43200 + i
			elseif i <= 10 then
				return day_start(today - 3 * 86400) + 43200 + i
			end
			return day_start(today - (10 + i) * 86400) + 43200
		end
		model.archived = {}
		for i = 1, 37 do
			local p = if i % 3 == 0 then "borealis" else "atlas"
			local cloud = i == 4 or i == 20
			table.insert(model.archived, {
				id = `ar{i}`,
				title = if i <= #titles then titles[i] else `{titles[(i - 1) % #titles + 1]} {i}`,
				project = p,
				device = if cloud then `cloud-ar{i}` elseif p == "borealis" then "box" else "mac",
				updated = at(i),
				created = at(i) - 3 * HOUR,
				agent = if i % 4 == 1 then "openai" else "claude",
				harness = if i % 4 == 1 then "codex" else nil,
			})
		end
		publish()
	end,
}
-- One archived session, on a Cloud machine of its own.
ui.command {
	id = "archived_one_cloud",
	title = "Archived: one session, on Cloud",
	run = function()
		model.archived = { { id = "solo", title = "On a machine of its own", project = "atlas", device = "cloud-solo", updated = os.time() - 60, created = os.time() - HOUR, agent = "claude" } }
		publish()
	end,
}
ui.command {
	id = "archived_none",
	title = "Archived: none",
	run = function()
		model.archived = {}
		publish()
	end,
}
-- Restored elsewhere (another device): the first archived session.
ui.command {
	id = "archived_restored_elsewhere",
	title = "Archived: the first restored on another device",
	run = function()
		local c = table.remove(model.archived, 1)
		if c then
			table.insert(model.chats, c)
		end
		publish()
	end,
}
ui.command {
	id = "archived_delete_fails",
	title = "Archived: the host refuses to delete two",
	run = function()
		refused.ar2 = true
		refused.ar3 = true
	end,
}
ui.command {
	id = "session_delete_fails",
	title = "The host refuses to delete “Tidy the tests”",
	run = function()
		refused.a3 = true
	end,
}
ui.command {
	id = "archive_many",
	title = "Thirty sessions archived elsewhere",
	run = function()
		for i = 1, 30 do
			table.insert(model.archived, { id = `old{i}`, title = `Old {i}`, project = "atlas", device = "mac", updated = NOW - (100 + i) * HOUR, created = NOW - (200 + i) * HOUR, agent = "claude" })
		end
		publish()
	end,
}
-- Thousands archived (Settings → Archived builds what it shows): `Chat <n>`,
-- each seventh with no project nor a device still known, so nothing under
-- its name. (Kept across a reload, as the host's sessions are.)
local thousands = ui.state("fixture_archive_thousands", false)
local function archive_thousands()
	for i = 1, 2000 do
		local plain = i % 7 == 0
		table.insert(model.archived, {
			id = `many{i}`,
			title = `Chat {i}`,
			project = if plain then nil else "atlas",
			device = if plain then "gone" else "mac",
			updated = NOW - (200 + i) * HOUR,
			created = NOW - (300 + i) * HOUR,
			agent = "claude",
		})
	end
	publish()
end
if thousands:peek() then
	archive_thousands()
end
ui.command {
	id = "archive_thousands",
	title = "Two thousand sessions archived",
	run = function()
		thousands:set(true)
		archive_thousands()
	end,
}
-- Subagents running under Fix the build (`subagents_<n>`).
for _, n in { 0, 3, 120 } do
	ui.command {
		id = `subagents_{n}`,
		title = `Fix the build runs {n} subagents`,
		run = function()
			for _, c in model.chats do
				if c.id == "a1" then
					(c :: any).running_subagents = n
				end
			end
			publish()
		end,
	}
end
ui.command {
	id = "none_left",
	title = "Every session archived elsewhere",
	run = function()
		for _, c in model.chats do
			table.insert(model.archived, 1, c)
		end
		model.chats = {}
		publish()
	end,
}
ui.command {
	id = "no_projects",
	title = "No projects yet (a first launch)",
	run = function()
		model.projects = {}
		model.chats = {}
		model.archived = {}
		publish()
	end,
}
-- A session on an agent from the ACP registry, named by its harness alone
-- (its icon among `agent_icons`).
ui.command {
	id = "registry_session",
	title = "A session on Factory Droid",
	run = function()
		table.insert(model.chats, { id = "reg1", title = "Moved to Factory Droid", project = "atlas", device = "mac", updated = NOW - 3 * HOUR, created = NOW - 6 * HOUR, agent = "acp.factory-droid", harness = "acp.factory-droid" })
		publish()
	end,
}
-- A message in a chat that's running: the third most recent comes to the
-- top, as the engine's list says once a reply moves (src/slide_bench_tests.rs
-- sends one every two seconds). Its time by a count, not the clock, so every
-- run moves the same rows at the same frames.
local messages = 0
ui.command {
	id = "message",
	title = "A message lands in a running chat",
	run = function()
		local live = {}
		for _, c in model.chats do
			table.insert(live, c)
		end
		table.sort(live, function(a: any, b: any): boolean
			return a.updated > b.updated
		end)
		local c = live[math.min(3, #live)]
		if c then
			messages += 1
			c.updated = NOW + messages
			publish()
		end
	end,
}
ui.command {
	id = "arrive",
	title = "A new session arrives",
	run = function()
		table.insert(model.chats, 1, { id = "a4", title = "Brand new", project = "atlas", device = "mac", updated = os.time(), created = os.time(), agent = "claude" })
		publish()
	end,
}
ui.command {
	id = "many",
	title = "Four hundred sessions",
	run = function()
		for i = 1, 400 do
			table.insert(model.chats, { id = `m{i}`, title = `Many {i}`, project = "borealis", device = "box", updated = NOW - 100 * HOUR - i, created = NOW - 100 * HOUR - i, agent = "claude" })
		end
		publish()
	end,
}
-- Five hundred repositories on this computer and fifteen hundred sessions in
-- them, three each on branches of their own (`r<i>-<k>`): fifty touched in
-- the last day, four hundred and fifty in the last fortnight, the rest
-- older (GitHub's watch at scale: examples_tests' github_scale).
ui.command {
	id = "scale",
	title = "Five hundred repositories, fifteen hundred sessions",
	run = function()
		model.chats = {}
		for i = 1, 500 do
			table.insert(model.projects, { id = `r{i}`, name = `Repo {i}`, device = "mac", path = `/work/r{i}`, git = true })
			for k = 1, 3 do
				local n = (i - 1) * 3 + k
				local age = if n <= 50 then n * 60 elseif n <= 500 then DAY + n * 600 else 30 * DAY + n * 60
				table.insert(model.chats, { id = `s{n}`, title = `Session {n}`, project = `r{i}`, device = "mac", branch = `r{i}-{k}`, updated = os.time() - age, created = os.time() - age - HOUR, agent = "claude" })
			end
		end
		publish()
	end,
}
-- One of them, out of sight and old, starts working (the active tier).
ui.command {
	id = "scale_work",
	title = "An old session starts working",
	run = function()
		local s = find("s900")
		s.status = "working"
		s.updated = os.time()
		publish()
	end,
}
-- A plugin's page open (route `plugin`, a page of its own with a panel of
-- its own; no plugin draws it here): its panel's width beside a session's
-- (tests/panel_widths).
ui.command {
	id = "plugin_page",
	title = "A plugin's page",
	run = function()
		require("@app/screens/shell/route").plugin_page("fixture", "board")
	end,
}
-- Six more projects on this computer, a session each, older than Atlas's (Proj 1
-- the newest): by device then project, MacBook Pro holds seven project
-- groups, its first five then "Show 2 more" (tests/sidebar_tree).
ui.command {
	id = "many_projects",
	title = "Six more projects on this computer",
	run = function()
		for i = 1, 6 do
			table.insert(model.projects, { id = `p{i}`, name = `Proj {i}`, device = "mac", path = `/work/p{i}`, git = true })
			table.insert(model.chats, { id = `q{i}`, title = `Work in Proj {i}`, project = `p{i}`, device = "mac", updated = NOW - (10 + i) * HOUR, created = NOW - (20 + i) * HOUR, agent = "claude" })
		end
		publish()
	end,
}
-- Three more projects on this computer: Cobalt and Dune with every session
-- archived (empty: folded into the sidebar's "2 empty projects"), and Ember,
-- just added, with none at all (not empty: a new chat is about to start
-- in it) (tests/sidebar_empty_projects).
ui.command {
	id = "empty_projects",
	title = "Two projects with only archived sessions, one just added",
	run = function()
		table.insert(model.projects, { id = "cobalt", name = "Cobalt", device = "mac", path = "/work/cobalt", git = true })
		table.insert(model.projects, { id = "dune", name = "Dune", device = "box", path = "/srv/dune", git = true })
		table.insert(model.projects, { id = "ember", name = "Ember", device = "mac", path = "/work/ember", git = true })
		table.insert(model.archived, { id = "c1", title = "Old Cobalt work", project = "cobalt", device = "mac", updated = NOW - 72 * HOUR, created = NOW - 80 * HOUR, agent = "claude" })
		table.insert(model.archived, { id = "d1", title = "Old Dune work", project = "dune", device = "box", updated = NOW - 96 * HOUR, created = NOW - 100 * HOUR, agent = "claude" })
		publish()
	end,
}
-- Sixty projects with every session archived (Gone 1…60): the fold opened
-- is a long run of headers, built only as they show.
ui.command {
	id = "many_empty_projects",
	title = "Sixty projects with only archived sessions",
	run = function()
		for i = 1, 60 do
			table.insert(model.projects, { id = `g{i}`, name = `Gone {i}`, device = "mac", path = `/work/g{i}`, git = true })
			table.insert(model.archived, { id = `ga{i}`, title = `Gone work {i}`, project = `g{i}`, device = "mac", updated = NOW - (200 + i) * HOUR, created = NOW - (210 + i) * HOUR, agent = "claude" })
		end
		publish()
	end,
}
-- Links clicked in a chat: a pull request's, an issue's, another site's
-- (tests/plugin_links).
for id, url in {
	click_pr_link = "https://github.com/acme/atlas/pull/12/files",
	click_issue_link = "https://github.com/acme/atlas/issues/12",
	click_other_link = "https://example.com/acme/atlas/pull/12",
} do
	ui.command {
		id = id,
		title = id,
		run = function()
			require("@ds/links").take(url)
		end,
	}
end
-- Atlas cloned on the devbox too, with a session there (tests/sidebar_repository).
ui.command {
	id = "atlas_cloned_on_box",
	title = "Atlas is cloned on devbox",
	run = function()
		for _, p in model.projects do
			if p.id == "atlas" then
				p.repository, p.created = "commit:atlas", 100
			end
		end
		table.insert(model.projects, { id = "atlas-clone", name = "atlas-clone", device = "box", path = "/srv/atlas-clone", git = true, repository = "commit:atlas", created = 200 })
		table.insert(model.chats, { id = "cl1", title = "On the clone", project = "atlas-clone", device = "box", updated = NOW - 30 * HOUR, created = NOW - 30 * HOUR })
		publish()
	end,
}
-- A chat beside each of the five, to compose tabs from (tests/session_tabs).
ui.command {
	id = "partner_chats",
	title = "Five more chats",
	run = function()
		for i = 1, 5 do
			table.insert(model.chats, { id = `p{i}`, title = `Partner {i}`, project = "atlas", device = "mac", updated = NOW - 40 * HOUR - i, created = NOW - 40 * HOUR - i })
		end
		publish()
	end,
}
-- Two chats' titles long enough that each one's tab is as wide as its title may
-- go (tests/session_tabs).
ui.command {
	id = "long_titles",
	title = "Two chats with long titles",
	run = function()
		find("a1").title = "Desktop app performance investigation, first pass"
		find("a2").title = "Desktop app performance investigation, second pass"
		publish()
	end,
}
-- A session starts in Cobalt (another device's, say): it has one again.
ui.command {
	id = "cobalt_arrives",
	title = "A session starts in Cobalt",
	run = function()
		table.insert(model.chats, 1, { id = "c2", title = "Back in Cobalt", project = "cobalt", device = "mac", status = "working", updated = os.time(), created = os.time(), agent = "claude" })
		publish()
	end,
}
-- Proj 6's session (its oldest group) asks you something.
ui.command {
	id = "proj6_asks",
	title = "Work in Proj 6 waits on you",
	run = function()
		find("q6").status = "input"
		publish()
	end,
}
-- Atlas on two devices: a session of it on devbox, so
-- by project then device Atlas holds two device groups (tests/
-- sidebar_tree).
ui.command {
	id = "atlas_on_box",
	title = "Atlas has a session on devbox",
	run = function()
		table.insert(model.chats, { id = "a5", title = "Profile on the box", project = "atlas", device = "box", updated = NOW - 30 * 60, created = NOW - 40 * 60, agent = "claude" })
		publish()
	end,
}
-- Atlas's folder on devbox too, a project of its own there (the same
-- repository on another computer): a new chat in Atlas on devbox starts in
-- it (tests/sidebar_device_new_chat).
ui.command {
	id = "atlas_twin_on_box",
	title = "Atlas has a folder on devbox too",
	run = function()
		table.insert(model.projects, { id = "atlas-box", name = "Atlas", device = "box", path = "/srv/atlas", git = true })
		publish()
	end,
}
-- Fix the build's side chats, on this computer as Tidy the tests is (no branch):
-- the panel home's rows to set beside the sidebar's (tests/
-- sidebar_panel_parity). One moved last and made first, one the other way.
ui.command {
	id = "side_chats",
	title = "Fix the build has two side chats",
	run = function()
		(model :: any).children = {
			a1 = {
				{ id = "s1", parent = "a1", title = "Look into the flake", device = "mac", updated = NOW - 30, created = NOW - 5 * HOUR, agent = "claude" },
				{ id = "s2", parent = "a1", title = "Write the changelog", device = "mac", updated = NOW - 2 * HOUR, created = NOW - HOUR, agent = "openai" },
			},
		}
		publish()
	end,
}
-- Tidy the tests' side chats, one with a side chat of its own, and a Cloud
-- delegate (a computer's chat keeps that one when it's deleted): what its
-- delete says goes with it, as the host counts it (tests/
-- sidebar_delete_children; the counting is src/sessions.rs `deleted_with`).
ui.command {
	id = "tidy_side_chats",
	title = "Tidy the tests has three side chats",
	run = function()
		(model :: any).deleted_with = { a3 = { subagents = 0, sides = 3 } };
		(model :: any).children = {
			a3 = {
				{ id = "t1", parent = "a3", title = "Split the slow suite", device = "mac", updated = NOW - 40, created = NOW - HOUR, agent = "claude" },
				{ id = "t2", parent = "a3", title = "Drop the dead fixtures", device = "mac", updated = NOW - 50, created = NOW - HOUR, agent = "claude", child_kind = "side" },
				{ id = "t3", parent = "a3", title = "Run it on Cloud", device = "cloud-t3", updated = NOW - 60, created = NOW - HOUR, agent = "claude", child_kind = "subagent" },
			},
			t1 = {
				{ id = "t4", parent = "t1", title = "Time each test", device = "mac", updated = NOW - 70, created = NOW - HOUR, agent = "claude" },
			},
		}
		publish()
	end,
}
-- And a subagent of its own besides.
ui.command {
	id = "tidy_subagent",
	title = "Tidy the tests has a subagent",
	run = function()
		local kids = (model :: any).children or {}
		kids.a3 = kids.a3 or {}
		table.insert(kids.a3, { id = "t5", parent = "a3", title = "Find the flaky one", device = "mac", updated = NOW - 80, created = NOW - HOUR, agent = "claude", child_kind = "subagent" })
		;(model :: any).children = kids
		local with = (model :: any).deleted_with or {}
		with.a3 = { subagents = 1, sides = if with.a3 then with.a3.sides else 0 };
		(model :: any).deleted_with = with
		publish()
	end,
}
-- Atlas crowded: six more sessions under its three, older than them
-- ("Atlas 1" the newest), so it shows its first five and "Show 4 more".
ui.command {
	id = "crowd_atlas",
	title = "Six more sessions in Atlas",
	run = function()
		for i = 1, 6 do
			table.insert(model.chats, { id = `x{i}`, title = `Atlas {i}`, project = "atlas", device = "mac", updated = NOW - (3 + i) * HOUR, created = NOW - (10 + i) * HOUR, agent = "claude" })
		end
		publish()
	end,
}
-- Atlas's oldest asks you something (past its first five).
ui.command {
	id = "atlas_asks",
	title = "Atlas 6 waits on you",
	run = function()
		find("x6").status = "input"
		publish()
	end,
}
ui.command {
	id = "atlas_answered",
	title = "Atlas 6 answered",
	run = function()
		find("x6").status = nil
		publish()
	end,
}
-- Answer the review moves (Borealis, on devbox, the newest).
ui.command {
	id = "borealis_moves",
	title = "Answer the review moves",
	run = function()
		find("b2").updated = os.time() + 5
		publish()
	end,
}
-- The chat page says what an agent asks (`chat.asks`): an approval.
local asks = ui.shared("chat.asks", {} :: { [string]: string })
ui.command {
	id = "ask",
	title = "Write the docs asks for approval",
	run = function()
		find("a2").status = "input"
		publish()
		asks:set({ a2 = "approval" })
	end,
}
-- A plugin narrowing the sidebar (`sidebar.filter`): to working sessions,
-- and to nonsense (a plugin's mistake, read as none). The host names the
-- plugin allowed to (`sidebar.filter_by`), as src/plugins does.
local sidebar_filter = ui.shared("sidebar.filter", nil :: any)
local sidebar_filter_by = ui.shared("sidebar.filter_by", nil :: any)
ui.command {
	id = "focus",
	title = "Focus shows working sessions only",
	run = function()
		sidebar_filter_by:set("Focus")
		sidebar_filter:set({ statuses = { "working", "nonsense" }, by = "Focus" })
	end,
}
ui.command {
	id = "focus_garbled",
	title = "A filter of the wrong shape",
	run = function()
		sidebar_filter:set({ statuses = "working", by = 7 })
	end,
}
-- The engine reconnecting: the sessions not there yet, then back.
ui.command {
	id = "reconnect",
	title = "The sessions go loading, then arrive",
	run = function()
		sessions.now:set({ state = "loading", devices = {}, projects = {}, chats = {}, archived = {}, places = { pinned = {}, sections = {} }, places_ready = false } :: any)
		ui.after(0, publish)
	end,
}
-- Write the docs pinned (on another device).
ui.command {
	id = "pin",
	title = "Write the docs pinned",
	run = function()
		model.places = { pinned = { "a2" }, sections = model.places.sections }
		publish()
	end,
}
-- A section made, and folded on another device.
ui.command {
	id = "section",
	title = "A section of yours",
	run = function()
		model.places = { pinned = {}, sections = { { id = "later", name = "Later", sessions = { "b2" } } } }
		publish()
	end,
}
-- A section named longer than the sidebar is wide.
ui.command {
	id = "long_section",
	title = "A section with a long name",
	run = function()
		model.places = { pinned = {}, sections = { { id = "later", name = "Everything to read again before the release goes out on Friday", sessions = { "b2" } } } }
		publish()
	end,
}
ui.command {
	id = "fold_elsewhere",
	title = "Later folded on another device",
	run = function()
		local later = model.places.sections[1]
		if later then
			later.collapsed = true
			publish()
		end
	end,
}
ui.command {
	id = "hold_echo",
	title = "The engine answers places only when told",
	run = function()
		holding = true
	end,
}
ui.command {
	id = "tick",
	title = "A session's state moves (the places as the engine has them)",
	run = function()
		local c = model.chats[1]
		c.updated += 1
		publish()
	end,
}
ui.command {
	id = "echo",
	title = "The engine answers the places held",
	run = function()
		if held then
			for _, s in held.sections do
				for _, o in model.places.sections do
					if o.id == s.id then
						s.collapsed = o.collapsed
					end
				end
			end
			model.places = held
			held = nil
			publish()
		end
	end,
}
ui.command {
	id = "project_arrives",
	title = "A project arrives",
	run = function()
		table.insert(model.projects, { id = "cosmos", name = "Cosmos", device = "mac", path = "/work/cosmos", git = false })
		table.insert(model.chats, { id = "c1", title = "First in Cosmos", project = "cosmos", device = "mac", updated = os.time(), created = os.time(), agent = "claude" })
		publish()
	end,
}

local typed = ui.state("fixture_settings_query", "")
ui.command {
	id = "settings_typed",
	title = "Something typed in the settings search",
	run = function()
		typed:set("theme")
	end,
}
ui.command {
	id = "grouped_pages",
	title = "Settings pages under headings, a search over them",
	run = function()
		local settings = require("@app/screens/shell/settings")
		settings.head(function(): Node
			return ui.text { id = "settings-search", "Search settings" }
		end, { replaces = function(): boolean
			return typed:get() ~= ""
		end })
		settings.add { id = "profile", title = "Profile", group = "Personal", before = "general", view = function(): Node
			return ui.text { "Your profile" }
		end }
		settings.add { id = "devices", title = "This computer", group = "Devices", before = "archived", view = function(): Node
			return ui.text { "This computer" }
		end }
	end,
}

-- Edit project, opened on each kind of icon, and projects wearing icons
-- of their own (an emoji, a picture), for captures.
local function editor(): any
	return require("@app/screens/shell/project_editor")
end
ui.command { id = "fixture.edit_project", title = "Fixture: edit project", menu = "Fixture", run = function()
	editor().open("borealis")
end }
-- (Each copy of Atlas after `atlas_cloned_on_box`: tests/project_default.)
ui.command { id = "fixture.edit_project_atlas", title = "Fixture: edit Atlas", menu = "Fixture", run = function()
	editor().open("atlas")
end }
ui.command { id = "fixture.edit_project_atlas_clone", title = "Fixture: edit Atlas's clone", menu = "Fixture", run = function()
	editor().open("atlas-clone")
end }
ui.command { id = "fixture.edit_project_emoji", title = "Fixture: edit project, emoji", menu = "Fixture", run = function()
	editor().open("borealis", { kind = "emoji", emoji = "🚀", hue = "amber" })
end }
ui.command { id = "fixture.edit_project_image", title = "Fixture: edit project, image", menu = "Fixture", run = function()
	editor().open("borealis", {
		kind = "image",
		picture = { path = "assets/art/plate-3-sunrise.jpg", w = 2100, h = 700 },
		zoom = 1.35,
		centre = { x = 0.62, y = 0.5 },
	})
end }
ui.command { id = "fixture.edit_project_drop", title = "Fixture: edit project, image to drop", menu = "Fixture", run = function()
	editor().open("borealis", { kind = "image" })
end }
ui.command { id = "fixture.project_icons", title = "Fixture: projects with icons", menu = "Fixture", run = function()
	for _, p in model.projects do
		if p.id == "atlas" then
			p.icon = { emoji = "🚀", hue = "amber" }
		elseif p.id == "borealis" then
			p.icon = { image = "https://edge.test/auth/avatars/" .. string.rep("b", 32) }
		end
	end
	publish()
end }

publish()

-- The plugins screen (what plugins ask of the shell: a tab in a panel, a
-- link), as the app loads it beside the shell: on for the tests that need
-- it (`shell_fixture.plugins`), so the rest stay as they are.
local plugins_on = ui.state("shell_fixture.plugins", false)
ui.effect(function()
	if plugins_on:get() then
		require("@app/screens/plugins")
	end
end)
-- The theme, for captures (as ui/chat_fixture.luau's).
ui.command { id = "theme_light", title = "Light theme", run = function() ui.theme.follow_system(false); ui.theme.current:set("zeron-light") end }
ui.command { id = "theme_dark", title = "Dark theme", run = function() ui.theme.follow_system(false); ui.theme.current:set("zeron-dark") end }
local shell = require("@app/screens/shell")
return shell()
