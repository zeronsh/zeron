-- Settings → Plugins with more than one computer, mounted on its own (no
-- engine in a test host): what tests/plugins_devices.test.luau drives.
-- Three computers: this computer, a devbox that's online, and a laptop that's
-- away. What the host would publish is set here (`plugins` for this computer,
-- `plugins.remote` for the device shown); what the page asks of it is
-- stubbed to note which device each call names (`calls`), and to change
-- the device's list as its engine would.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local sessions = require("@account/sessions")
-- (No window chrome to restyle in a test host, as shell_fixture says.)
local actions = require("@app/screens/onboarding/actions");
(actions :: any).apply_scheme = function() end
local state = require("@app/screens/plugins/state")
local page = require("@app/screens/plugins/page")

local function plugin(id: string, name: string, status: string, extra: { [string]: any }?): any
	local p: any = {
		id = id,
		name = name,
		version = "1.0.0",
		description = `What {name} does.`,
		author = { kind = "verified", username = "ada" },
		status = status,
		permissions = { "ui.add" },
		allowed = if status == "on" or status == "off" then { "ui.add" } else {},
		ask = if status == "pending" then { "ui.add" } else {},
		hash = `{id}-1`,
	}
	for k, v in extra or {} do
		p[k] = v
	end
	return p
end

-- What each computer has.
local HERE = {
	plugin("focus", "Focus", "on"),
	plugin("compact-sidebar", "Compact sidebar", "off", { author = { kind = "you" }, own = true }),
}
local DEVBOX = {
	plugin("checkout", "Checkout status", "on", {
		icon_svg = '<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="currentColor"/></svg>',
		update = { version = "1.1.0", added = {}, removed = {} },
	}),
	plugin("ping", "Ping", "pending", { author = { kind = "unknown" } }),
	plugin("teleport", "Teleport", "broken", { error = "Its manifest asks for a permission Zeron doesn't know: teleport" }),
}

sessions.now:set({
	state = "ready",
	devices = {
		{ id = "mac", name = "MacBook Pro", platform = "macos", here = true, online = true },
		{ id = "devbox", name = "devbox", platform = "linux", here = false, online = true },
		{ id = "laptop", name = "Old laptop", platform = "macos", here = false, online = false },
	},
	projects = {},
	chats = {},
	archived = {},
	places = { pinned = {}, sections = {} },
	places_ready = true,
} :: any)

state.now:set({ state = "ready", dirs = { plugins = "/tmp/plugins", templates = "/tmp/templates" }, list = HERE } :: any)

local said: { string } = {}
local calls = ui.state("calls", "")
local function note(what: string)
	table.insert(said, what)
	calls:set(table.concat(said, ","))
end

-- The devbox, reachable or not (`unreachable`: its engine doesn't answer).
local reachable = true

local function find(list: { any }, id: string): any
	for _, p in list do
		if p.id == id then
			return p
		end
	end
	return nil
end

local function publish_remote(device: string?)
	if device == nil then
		state.remote:set({ state = "idle", list = {} } :: any)
	elseif device == "devbox" and reachable then
		local list = {}
		for _, p in DEVBOX do
			table.insert(list, table.clone(p))
		end
		state.remote:set({ device = device, state = "ready", list = list } :: any)
	else
		state.remote:set({ device = device, state = "offline", list = {} } :: any)
	end
end

local stub = state :: any
stub.target = function(device: string?)
	note(`target:{device or ""}`)
	publish_remote(device)
end
stub.review = function(id: string, device: string?, name: string?)
	note(`review:{id}@{device or ""}:{name or ""}`)
end
stub.set_off = function(id: string, off: boolean, device: string?)
	note(`set_off:{id}:{tostring(off)}@{device or ""}`)
	find(if device then DEVBOX else HERE, id).status = if off then "off" else "on"
	if device then
		publish_remote(device)
	end
	return nil
end
stub.remove = function(id: string, device: string?)
	note(`remove:{id}@{device or ""}`)
	return nil
end
stub.install = function(from: any)
	note(`install:{from.url or from.path or ""}@{from.device or ""}`)
	return nil
end
stub.update = function(id: string, device: string?)
	note(`update:{id}@{device or ""}`)
	return nil
end
stub.check_updates = function(device: string?)
	note(`check:{device or ""}`)
	return nil
end
stub.open_folder = function(id: string?)
	note(`folder:{id or ""}`)
end

ui.command {
	id = "unreachable",
	title = "The devbox stops answering",
	run = function()
		reachable = false
		if state.device() == "devbox" then
			publish_remote("devbox")
		end
	end,
}
ui.command {
	id = "devbox_away",
	title = "The devbox's presence goes, then comes back",
	run = function()
		local now: any = table.clone(sessions.now:peek())
		now.devices = table.clone(now.devices)
		now.devices[2] = table.clone(now.devices[2])
		now.devices[2].online = not now.devices[2].online
		sessions.now:set(now)
	end,
}
ui.command {
	id = "devbox_empty",
	title = "Every plugin on the devbox removed",
	run = function()
		table.clear(DEVBOX)
		publish_remote(state.device())
	end,
}
ui.command {
	id = "switch_here",
	title = "This computer's plugins shown",
	run = function()
		publish_remote(nil)
	end,
}
ui.command {
	id = "reachable",
	title = "The devbox answers again",
	run = function()
		reachable = true
	end,
}

-- Light, for pictures of it.
ui.command { id = "theme_light", title = "Light theme", run = function() ui.theme.follow_system(false); ui.theme.current:set("zeron-light") end }

return ui.scroll {
	w_full = true,
	h_full = true,
	bg = "$background",
	ui.column {
		w_full = true,
		px = "$space.6",
		pt = "$space.4",
		pb = "$space.8",
		items = "center",
		page.View(),
	},
}
