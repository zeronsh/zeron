-- Settings → Plugins with the plugin store: Browse, a store plugin's page,
-- installing and updating from it, mounted on its own (no host in a test
-- host). The store is ui/store_stub.luau's made-up catalog; this device's
-- plugins are made up here, and what the page asks of them changes them
-- as the engine would. tests/plugins_browse.test.luau and
-- tests/plugins_listing.test.luau drive it.
--
-- `ui.command`s, as the engine would change things on its own:
--   store.installed_focus   Focus timer arrives, from the store, waiting
--   store.update_found      a newer Focus timer found, asking for more
--   store.blocked           Zeron takes Focus timer 1.2.0 down
--   store.allowed           Focus timer allowed, and on
--   store.old_zeron         this device's Zeron is 0.2.0

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local actions = require("@app/screens/onboarding/actions");
(actions :: any).apply_scheme = function() end
local Lightbox = require("@ds/lightbox")
local state = require("@app/screens/plugins/state")
local page = require("@app/screens/plugins/page")
local sessions = require("@account/sessions")
local stub = require("./store_stub")

-- This device, its Zeron as the engine says.
local function devices(version: string)
	local now = table.clone(sessions.now:peek())
	now.devices = { { id = "mac", name = "This Mac", platform = "macos", here = true, online = true, version = version } :: any }
	sessions.now:set(now)
end
devices("0.4.2")

local model: { any } = {
	{
		id = "branch-badge",
		name = "Branch badge",
		version = "1.1.0",
		description = "A branch on each sidebar row.",
		author = { kind = "verified", username = "grace" },
		status = "on",
		permissions = { "ui.add" },
		allowed = { "ui.add" },
		ask = {},
		hash = "branch-badge-1",
		source = { kind = "store", id = "branch-badge", version = "1.1.0" },
	},
}

local function publish()
	local list = {}
	for _, p in model do
		table.insert(list, table.clone(p))
	end
	state.now:set({ state = "ready", dirs = { plugins = "/tmp/plugins", templates = "/tmp/templates" }, list = list } :: any)
end

local function find(id: string): any
	for _, p in model do
		if p.id == id then
			return p
		end
	end
	return nil
end

-- What the page asks, answered here.
local s = state :: any
local open_install: any = nil
s.install = function(from: any): any
	stub.note(`install:{from.store or from.url or from.path or ""}@{from.version or ""}`)
	return {
		after = function(_self: any, ok: (any) -> (), err: (any) -> ())
			open_install = { ok = ok, err = err, from = from }
		end,
	}
end
s.update = function(id: string): any
	stub.note(`update:{id}`)
	local p = find(id)
	if p and p.update then
		p.version = p.update.version
		p.hash = `{id}-{p.version}`
		p.update = nil
		publish()
	end
	return nil
end
s.set_off = function(id: string, off: boolean): any
	stub.note(`set_off:{id}:{tostring(off)}`)
	local p = find(id)
	if p then
		p.status = if off then "off" else "on"
		publish()
	end
	return nil
end
s.review = function(id: string)
	stub.note(`review:{id}`)
end
s.check_updates = function(): any
	return nil
end
local store = require("@app/screens/plugins/store") :: any
store.unblock = function(id: string): any
	stub.note(`unblock:{id}`)
	local p = find(id)
	if p then
		p.blocked = nil
		p.status = "on"
		publish()
	end
	return stub.answer(function(): any
		return nil
	end)
end
store.report = function(id: string, reason: string, text: string): any
	stub.note(`report:{id}:{reason}:{text}`)
	return stub.answer(function(): any
		return nil
	end)
end

ui.command {
	id = "store.install_done",
	title = "The install finishes: Focus timer, waiting for you",
	run = function()
		local focus = {
			id = "focus-timer",
			name = "Focus timer",
			version = "1.2.0",
			description = "A 25 minute timer.",
			author = { kind = "verified", username = "ada" },
			status = "pending",
			permissions = { "ui.add", "notifications", "network" },
			allowed = {},
			ask = { "ui.add", "notifications", "network" },
			hash = "focus-timer-1.2.0",
			source = { kind = "store", id = "focus-timer", version = "1.2.0" },
		}
		table.insert(model, focus)
		publish()
		local p = open_install
		open_install = nil
		if p then
			p.ok({ plugin = focus, waiting = true })
		end
	end,
}
ui.command {
	id = "store.install_failed",
	title = "The install fails",
	run = function()
		local p = open_install
		open_install = nil
		if p then
			p.err({ message = "hash_mismatch: the package isn't the one the store reviewed" })
		end
	end,
}
ui.command {
	id = "store.allowed",
	title = "Focus timer allowed, and on",
	run = function()
		local p = find("focus-timer")
		p.status = "on"
		p.allowed = p.permissions
		p.ask = {}
		publish()
	end,
}
ui.command {
	id = "store.update_found",
	title = "A newer Focus timer, asking for more",
	run = function()
		local p = find("focus-timer")
		p.update = { version = "1.3.0", added = { "links.open" }, removed = { "network" }, notes = "- Longer breaks.\n- Opens its guide." }
		publish()
	end,
}
ui.command {
	id = "store.branch_update",
	title = "A newer Branch badge is found",
	run = function()
		find("branch-badge").update = { version = "1.2.0", added = {}, removed = {} }
		publish()
	end,
}
ui.command {
	id = "store.blocked",
	title = "Zeron takes Focus timer 1.2.0 down",
	run = function()
		local p = find("focus-timer")
		p.status = "off"
		p.blocked = { version = "1.2.0", reason = "It sent what you typed to a server it didn't name." }
		publish()
	end,
}
ui.command {
	id = "store.old_zeron",
	title = "This device's Zeron is 0.2.0",
	run = function()
		devices("0.2.0")
	end,
}

publish()

ui.command { id = "theme_light", title = "Light theme", run = function() ui.theme.follow_system(false); ui.theme.current:set("zeron-light") end }

return ui.column {
	w_full = true,
	h_full = true,
	ui.scroll {
		id = "fixture-scroll",
		w_full = true,
		grow = 1,
		min_h = 0,
		ui.column {
			w_full = true,
			px = "$space.6",
			pt = "$space.4",
			pb = "$space.8",
			items = "center",
			page.View(),
		},
	},
	Lightbox.Host(),
}
