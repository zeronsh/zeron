-- Settings → Plugins, mounted on its own with plugins made up here (no
-- engine in a test host): the page tests/plugins_in_place.test.luau
-- drives. What the host would publish is set here; what the page asks of
-- it is stubbed to change this data as the engine would, and `ui.command`s
-- change it as the engine would on its own (a plugin dropped in, a new
-- version, one edited after it was allowed). Here, beside app.luau, as a
-- mounted script loads modules from its own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
-- (No window chrome to restyle in a test host, as shell_fixture says.)
local actions = require("@app/screens/onboarding/actions");
(actions :: any).apply_scheme = function() end
local state = require("@app/screens/plugins/state")
local page = require("@app/screens/plugins/page")

type Extra = { permissions: { string }?, ask: { string }? }

local function plugin(id: string, name: string, kind: string, status: string, extra: Extra?): state.Item
	local p: state.Item = {
		id = id,
		name = name,
		version = "1.0.0",
		description = `What {name} does.`,
		author = if kind == "verified" then { kind = kind, username = "ada" } else { kind = kind },
		status = status,
		permissions = { "ui.add" },
		allowed = if status == "on" or status == "off" then { "ui.add" } else {},
		ask = if status == "pending" then { "ui.add" } else {},
		hash = `{id}-1`,
		own = kind == "you",
	}
	if extra then
		p.permissions = extra.permissions or p.permissions
		p.ask = extra.ask or p.ask
	end
	return p
end

local model: { state.Item } = {
	plugin("compact-sidebar", "Compact sidebar", "you", "on"),
	plugin("focus", "Focus", "verified", "on"),
	plugin("notes-tool", "Notes tool", "you", "off"),
}

local function find(id: string): (state.Item, number?)
	for i, p in model do
		if p.id == id then
			return p, i
		end
	end
	error(`no plugin {id} in the fixture`)
end

local function publish()
	local list: { state.Item } = {}
	for _, p in model do
		table.insert(list, table.clone(p))
	end
	table.sort(list, function(a: state.Item, b: state.Item): boolean
		return a.name < b.name
	end)
	state.now:set({
		state = "ready",
		dirs = { plugins = "/tmp/plugins", templates = "/tmp/templates" },
		list = list,
	})
end

-- The engine answering `setup_incomplete` (the account's setup isn't
-- finished: no organization chosen), as the host says it; then set up,
-- the list arriving.
ui.command {
	id = "setup_incomplete",
	title = "Fixture: the account's setup isn't finished",
	run = function()
		state.now:set({ state = "setup", list = {} } :: any)
	end,
}
ui.command { id = "set_up", title = "Fixture: setup finished", run = publish }

-- What the page asks, answered here.
local said: { string } = {}
local calls = ui.state("calls", "")
local function note(what: string)
	table.insert(said, what)
	calls:set(table.concat(said, ","))
end
local stub = state :: any
stub.review = function(id: string)
	note(`review:{id}`)
end
stub.set_off = function(id: string, off: boolean)
	note(`set_off:{id}:{tostring(off)}`)
	find(id).status = if off then "off" else "on"
	publish()
end
stub.remove = function(id: string)
	note(`remove:{id}`)
	local _, i = find(id)
	if i then
		table.remove(model, i)
	end
	publish()
end
stub.new_plugin = function(name: string)
	note(`new:{name}`)
	table.insert(model, plugin("my-plugin", name, "you", "on"))
	publish()
end
stub.open_folder = function(id: string?)
	note(`folder:{id or ""}`)
end

-- Someone else's: installed from a link or file, updated from its source.
-- (Checking sources asks nothing of what's installed: kept apart.)
local checks = ui.state("checks", 0)
stub.check_updates = function()
	checks:set(checks:peek() + 1)
	return nil
end
-- (An install takes a while, as from a link: it answers a promise that
-- `installed` or `install_failed` settles, so the busy dialog shows.)
local open_install: any = nil
stub.install = function(from: { path: string?, url: string? })
	note(`install:{from.url or from.path or ""}`)
	return {
		after = function(_self: any, ok: (any) -> (), err: (any) -> ())
			open_install = { ok = ok, err = err }
		end,
	}
end
ui.command {
	id = "installed",
	title = "The install finishes: Clock, waiting for you",
	run = function()
		local clock: any = plugin("clock", "Clock", "verified", "pending")
		clock.source = { kind = "git", url = "https://github.com/ada/clock.git" }
		table.insert(model, clock)
		publish()
		local p = open_install
		open_install = nil
		if p then
			p.ok({ plugin = clock, waiting = true })
		end
	end,
}
ui.command {
	id = "install_failed",
	title = "The install fails",
	run = function()
		local p = open_install
		open_install = nil
		if p then
			p.err({ message = "That release has no .zeron-plugin file" })
		end
	end,
}
ui.command {
	id = "ping_from_github",
	title = "Ping was installed from GitHub",
	run = function()
		(find("ping") :: any).source = { kind = "git", url = "https://github.com/eve/ping.git" }
		publish()
	end,
}
stub.update = function(id: string)
	note(`update:{id}`)
	local p: any = find(id)
	p.version = p.update.version
	p.hash = `{id}-{p.version}`
	p.update = nil
	publish()
	return nil
end

-- What changes on the engine's side.
ui.command {
	id = "arrive",
	title = "A plugin is dropped in",
	run = function()
		table.insert(model, plugin("ping", "Ping", "unknown", "pending", { permissions = { "devices" }, ask = { "devices" } }))
		publish()
	end,
}
ui.command {
	id = "bump",
	title = "Focus gets a new version",
	run = function()
		find("focus").version = "1.1.0"
		find("focus").hash = "focus-2"
		publish()
	end,
}
ui.command {
	id = "tamper",
	title = "Focus is edited after it was allowed",
	run = function()
		local f = find("focus")
		f.status = "tampered"
		f.author = { kind = "unknown" }
		publish()
	end,
}
ui.command {
	id = "break",
	title = "Notes tool's manifest stops reading",
	run = function()
		local n = find("notes-tool")
		n.status = "broken"
		n.error = "Its manifest asks for a permission Zeron doesn't know: teleport"
		publish()
	end,
}
ui.command {
	id = "engine_off",
	title = "Focus turned off elsewhere (another window, the CLI)",
	run = function()
		find("focus").status = "off"
		publish()
	end,
}
ui.command {
	id = "engine_on",
	title = "Focus turned back on elsewhere",
	run = function()
		find("focus").status = "on"
		publish()
	end,
}
ui.command {
	id = "allowed",
	title = "Ping allowed in its window",
	run = function()
		local p = find("ping")
		p.status = "on"
		p.allowed = p.permissions
		p.ask = {}
		publish()
	end,
}
ui.command {
	id = "declined",
	title = "Notes tool declined in its window",
	run = function()
		local n = find("notes-tool")
		n.status = "off"
		n.ask = n.permissions
		publish()
	end,
}
ui.command {
	id = "hidden",
	title = "The template in use drops Focus's element",
	run = function()
		(find("focus") :: any).hidden = { "the title bar" }
		publish()
	end,
}
ui.command {
	id = "update_found",
	title = "A newer Focus is found where it came from, asking for more",
	run = function()
		local f: any = find("focus")
		f.source = { kind = "release", url = "https://github.com/ada/focus" }
		f.update = { version = "2.0.0", added = { "links.open" }, removed = {} }
		publish()
	end,
}
ui.command {
	id = "update_refused",
	title = "A newer Focus is found, signed by someone else",
	run = function()
		local f: any = find("focus")
		f.update = { version = "6.6.6", added = {}, removed = {}, refused = "The new version is signed by someone else, so it won't be installed" }
		publish()
	end,
}
ui.command {
	id = "builtin",
	title = "Zeron's own GitHub arrives",
	run = function()
		local p = plugin("github", "GitHub", "zeron", "on", { permissions = { "ui.add", "github" } })
		local any: any = p
		any.builtin = true
		any.allowed = { "github", "ui.add" }
		any.description = "Your sessions' pull requests."
		table.insert(model, p)
		publish()
	end,
}
ui.command {
	id = "problem",
	title = "Focus is placed where what it needs isn't",
	run = function()
		(find("focus") :: any).problem = "Focus needs a session, but it's in the title bar"
		publish()
	end,
}
ui.command {
	id = "empty",
	title = "Every plugin removed",
	run = function()
		model = {}
		publish()
	end,
}

publish()

-- Light, for pictures of it (src/audit_shots.rs: `!theme_light`).
ui.command { id = "theme_light", title = "Light theme", run = function() ui.theme.follow_system(false); ui.theme.current:set("zeron-light") end }

return ui.scroll {
	w_full = true,
	h_full = true,
	ui.column {
		w_full = true,
		px = "$space.6",
		pt = "$space.4",
		pb = "$space.8",
		items = "center",
		page.View(),
	},
}
