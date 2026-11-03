-- The consent sheet, mounted on its own with plugins made up here (no
-- engine in a test host): what tests/plugins_consent.test.luau drives. The
-- host's `consent` is set here; its two actions are answered as the engine
-- would (the decision taken, or "It's mine" making the author you), and
-- `ui.command`s show another plugin, or this one changed.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local consent = require("@screens/plugins/consent")

local shown = ui.shared("consent", { busy = false } :: any)

local PLUGINS: { [string]: any } = {
	focus = {
		id = "focus",
		name = "Focus",
		version = "1.2.0",
		hash = "focus-1",
		status = "pending",
		author = { kind = "verified", username = "ada" },
		permissions = { "ui.replace", "ui.add", "ui.change", "sessions.read" },
		allowed = {},
		ask = { "ui.replace", "ui.add", "ui.change", "sessions.read" },
		ui = { change = { "#sidebar-actions" }, replace = { "@app/screens/shell/sidebar" } },
	},
	ping = {
		id = "ping",
		name = "Ping",
		version = "0.1.0",
		hash = "ping-1",
		status = "pending",
		author = { kind = "unknown" },
		permissions = { "devices", "native" },
		allowed = {},
		ask = { "devices", "native" },
	},
	more = {
		id = "more",
		name = "Notes tool",
		version = "2.0.0",
		hash = "more-2",
		status = "pending",
		author = { kind = "you" },
		permissions = { "tools", "network" },
		allowed = { "tools" },
		ask = { "network" },
		tools = { { name = "count_words" } },
	},
	-- A plugin with a service (its headless half), by someone unknown.
	sync = {
		id = "sync",
		name = "Checkout sync",
		version = "0.3.0",
		hash = "sync-1",
		status = "pending",
		author = { kind = "unknown" },
		permissions = { "service", "commands" },
		allowed = {},
		ask = { "service", "commands" },
		commands = { "git" },
	},
}

local said: { string } = {}
local calls = ui.state("calls", "")
local function note(what: string)
	table.insert(said, what)
	calls:set(table.concat(said, ","))
end

-- (`on`: another device it's on, by name, as the host says it.)
local function show(id: string?, on: string?)
	shown:set({ busy = false, item = if id then table.clone(PLUGINS[id]) else nil, device_name = on } :: any)
end

ui.actions {
	["plugins.decide"] = function(arg: any)
		note(`decide:{arg.id}:{tostring(arg.allow)}:{arg.hash}`)
		show(nil)
	end,
	["plugins.sign"] = function(arg: any)
		note(`sign:{arg.id}`)
		PLUGINS[arg.id].author = { kind = "you" }
		show(arg.id)
	end,
}

ui.command { id = "focus", title = "Show Focus", run = function() show("focus") end }
ui.command { id = "ping", title = "Show Ping", run = function() show("ping") end }
ui.command { id = "more", title = "Show Notes tool asking more", run = function() show("more") end }
ui.command { id = "sync", title = "Show Checkout sync, here", run = function() show("sync") end }
ui.command { id = "sync_devbox", title = "Show Checkout sync, on Devbox", run = function() show("sync", "Devbox") end }
ui.command {
	id = "tamper",
	title = "Ping, edited after it was allowed",
	run = function()
		PLUGINS.ping.status = "tampered"
		PLUGINS.ping.hash = "ping-2"
		show("ping")
	end,
}

show("focus")

-- Light, for pictures of it.
ui.command { id = "theme_light", title = "Light theme", run = function() ui.theme.follow_system(false); ui.theme.current:set("zeron-light") end }

return ui.column {
	w = 460,
	h = 540,
	bg = "$background",
	consent.View(),
}
