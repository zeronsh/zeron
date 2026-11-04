--!strict
-- The plugins on this Mac and your templates, as the host publishes them
-- (src/plugins), and what the Plugins page asks of it. Every list's items
-- carry an `id`, so rows key by it and change in place.
--
--   state.now:get()   { state = "loading" | "ready" | "offline" | "setup",
--                       dirs = { plugins, templates },
--                       list = { PluginItem }      (see the contract: id, name,
--                         version, description, author { kind, username? },
--                         status "on" | "off" | "pending" | "tampered" | "broken",
--                         permissions, allowed, ask, ui, tools, mcp, error?, own)
--                       replaced = { module }  what plugins and the template
--                         in use stand in for (where a failing screen is ours) }
--
--   state.review(id, device?, device_name?)  the consent window for it
--   state.set_off(id, off, device?)  a promise; turns an allowed plugin off or on
--   state.remove(id, device?)  a promise
--   state.new_plugin(name)    a promise of its id: a new plugin of yours
--   state.open_folder(id?)    its folder (or the plugins folder) in Finder
--   state.install({ path? , url?, device? })  a promise of `{ plugin, update,
--                             added, removed, waiting }`: someone else's plugin
--                             from a .zeron-plugin file (this device only) or a
--                             link; it waits for the consent window unless it's
--                             signed as yours
--   state.update(id, device?)  a promise of the same: the update found for it
--   state.check_updates(device?)  a promise: every installed plugin's source
--                             checked
--
-- Plugins are per device. Settings → Plugins can show another of your
-- devices' (src/plugins/remote.rs), never loaded here:
--
--   state.remote:get()   { device = id?, state = "idle" | "loading" |
--                          "ready" | "offline", list = { PluginItem } }
--   state.target(id?)    show that device's (nil: this one's); the same one
--                        again looks again
--   state.device()       the device shown (nil: this one)
--   state.shown          its list
--
-- Each `device?` above is that device's id (nil: this one).
--
-- Someone else's plugin carries `source` ({ kind = "file" | "link" |
-- "release" | "git", url?, ref? }) and, when a newer version was found
-- there, `update` ({ version, added, removed, refused? }).

local M = {}

export type Author = { kind: string, username: string? }
export type Item = {
	id: string,
	name: string,
	version: string,
	description: string?,
	author: Author,
	status: string,
	permissions: { string },
	allowed: { string },
	ask: { string },
	ui: { change: { string }?, replace: { string }? }?,
	tools: { any }?,
	mcp: boolean?,
	error: string?,
	own: boolean?,
	-- What exactly it is (the engine's content hash): a decision names it.
	hash: string?,
	-- Why it stopped working here, when it did (the app withdrew it).
	failed: string?,
	-- Where someone else's came from, and a newer version found there.
	source: { kind: string, url: string?, ref: string? }?,
	update: { version: string, added: { string }, removed: { string }, refused: string? }?,
}
export type Now = {
	state: string,
	-- The modules a plugin or template stands in for now.
	replaced: { string }?,
	dirs: { plugins: string?, templates: string? }?,
	list: { Item },
}

M.now = ui.shared("plugins", { state = "loading", list = {} } :: Now)

-- The list as it shows, in the host's order (by name).
M.list = ui.computed(function(): { Item }
	return M.now:get().list or {}
end)

M.waiting = ui.computed(function(): number
	local n = 0
	for _, p in M.now:get().list or {} do
		if p.status == "pending" or p.status == "tampered" then
			n += 1
		end
	end
	return n
end)

-- Another device's, as the host watches it.
M.remote = ui.shared("plugins.remote", { state = "idle", list = {} } :: { device: string?, state: string, list: { Item } })

function M.device(): string?
	local d = M.remote:get().device
	return if type(d) == "string" and d ~= "" then d else nil
end

function M.target(device: string?)
	ui.invoke("plugins.target", device or ui.null)
end

-- The list the page shows: the device shown's.
M.shown = ui.computed(function(): { Item }
	if M.device() then
		return M.remote:get().list or {}
	end
	return M.list:get()
end)

function M.review(id: string, device: string?, device_name: string?)
	if device then
		ui.invoke("plugins.review", { id = id, device = device, device_name = device_name or "" })
	else
		ui.invoke("plugins.review", id)
	end
end

function M.set_off(id: string, off: boolean, device: string?): any
	return ui.call("plugins.set_off", { id = id, off = off, device = device })
end

function M.remove(id: string, device: string?): any
	return ui.call("plugins.remove", if device then { id = id, device = device } else id)
end

function M.new_plugin(name: string): any
	return ui.call("plugins.new", name)
end

function M.install(from: { path: string?, url: string?, device: string? }): any
	return ui.call("plugins.install", from)
end

function M.update(id: string, device: string?): any
	return ui.call("plugins.update", if device then { id = id, device = device } else id)
end

function M.check_updates(device: string?): any
	return ui.call("plugins.check_updates", if device then { device = device } else nil)
end

function M.open_folder(id: string?)
	ui.invoke("plugins.open_folder", id or ui.null)
end

return M
