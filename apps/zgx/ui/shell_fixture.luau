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
local actions = require("@screens/onboarding/actions")
local store = require("@app/screens/chat/store")
-- Settings' host calls, answered as the host would (settings_stub.luau).
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
	if type(f) == "function" then
		chat_store[name] = function() end
	end
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
	},
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
		state = "ready",
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
	id = "fixture.cloud_machines",
	title = "Fixture: Cloud sessions' machines among the devices",
	run = function()
		table.insert(model.devices, { id = "cloud-s1", name = "Cloud session", platform = "cloud", here = false, online = false, cloud = true })
		table.insert(model.devices, { id = "cloud-s2", name = "Cloud session", platform = "cloud", here = false, online = false, cloud = true })
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

-- What changes on the engine's side.
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
	id = "archive_many",
	title = "Thirty sessions archived elsewhere",
	run = function()
		for i = 1, 30 do
			table.insert(model.archived, { id = `old{i}`, title = `Old {i}`, project = "atlas", device = "mac", updated = NOW - (100 + i) * HOUR, created = NOW - (200 + i) * HOUR, agent = "claude" })
		end
		publish()
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
-- Five hundred repositories on this Mac and fifteen hundred sessions in
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
-- Six more projects on this Mac, a session each, older than Atlas's (Proj 1
-- the newest): by device then project, MacBook Pro holds seven project
-- groups, its first five then "Show 2 more" (tests/sidebar_tree).
ui.command {
	id = "many_projects",
	title = "Six more projects on this Mac",
	run = function()
		for i = 1, 6 do
			table.insert(model.projects, { id = `p{i}`, name = `Proj {i}`, device = "mac", path = `/work/p{i}`, git = true })
			table.insert(model.chats, { id = `q{i}`, title = `Work in Proj {i}`, project = `p{i}`, device = "mac", updated = NOW - (10 + i) * HOUR, created = NOW - (20 + i) * HOUR, agent = "claude" })
		end
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
-- Fix the build's side chats, on this Mac as Tidy the tests is (no branch):
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
-- and to nonsense (a plugin's mistake, read as none).
local sidebar_filter = ui.shared("sidebar.filter", nil :: any)
ui.command {
	id = "focus",
	title = "Focus shows working sessions only",
	run = function()
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
		settings.add { id = "devices", title = "This Mac", group = "Devices", before = "archived", view = function(): Node
			return ui.text { "This Mac" }
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

-- Grouped by project (a view option; one list is the default): the tests
-- here follow projects' headers.
require("@app/screens/shell/sidebar").options.organize:set("project")

-- The plugins screen (what plugins ask of the shell: a tab in a panel, a
-- link), as the app loads it beside the shell: on for the tests that need
-- it (`shell_fixture.plugins`), so the rest stay as they are.
local plugins_on = ui.state("shell_fixture.plugins", false)
ui.effect(function()
	if plugins_on:get() then
		require("@app/screens/plugins")
	end
end)
local shell = require("@app/screens/shell")
return shell()
