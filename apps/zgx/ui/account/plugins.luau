-- Which plugins run in this window, for the app's own code: a hook that
-- belongs to one of Zeron's built-in plugins (plugins-builtin/<id>) shows
-- only while it's on, and follows a toggle in Settings → Plugins with no
-- restart.
--
--   plugins.on(id)       whether plugin `id` runs here: the engine lists it
--                        on and the window loaded it, and, a built-in, its
--                        app-side module is loaded. Read in a build or a
--                        computed, it's followed (src/plugins: `plugins.on`).
--   plugins.peek(id)     the engine's word alone, read now, untracked: for
--                        what's declared once a load (a command, its key),
--                        which a toggle's reload declares again
--   plugins.module(id)   a built-in's app-side module (`@app/builtin/<id>`:
--                        the plugin's own while it's on, else the app's
--                        stand-in), while `on(id)`, else nil: off, not
--                        loaded, or not one this app knows. Tracked as
--                        `on` is.
--   plugins.start()      the window's entry, at its top level once its own
--                        modules are in: built-ins on by then load now, so
--                        the window is built with them
--   plugins.set(on)      `{ [id] = true }`, for a fixture: what the window
--                        says otherwise. Off is the default everywhere. At
--                        a fixture's top level, a built-in it turns on is
--                        loaded at once: what it mounts is built with it.
--
-- A gated hook:
--
--   local plugins = require("@account/plugins")
--   ui.show(function() return plugins.on("orchestrators") end, ...)
--   local orch = plugins.module("orchestrators")
--   if orch then orch.proof.label() end
--
-- A built-in's module, and the parts of it the app's hooks read from a
-- build or a computed (`preload`), are loaded here outside any build or
-- effect: as the window loads, if it's on by then (`start`), else a frame
-- after it's turned on. What a module makes as it loads (its effects, its computeds)
-- is then the window's for as long as it runs, never the first hook's that
-- asked for it (zgx gives what a module makes to whatever runs as it's
-- required). Until then it reads as off.

local M = {}

M.now = ui.shared("plugins.on", {} :: { [string]: boolean })

-- The built-ins whose app-side module the app reaches, each by its stable
-- name. ui/builtin/<id>.luau is the app's own (`false`, or its parts while
-- their modules are still in ui/), reached only while it's on; the plugin
-- replaces it while on (docs/plugins.md, "Zeron's own plugins").
-- `preload`: its parts the app's hooks read where they're tracked (a
-- computed, a build), loaded with it. (Its pages load as they're opened,
-- or the shell's a while after the window opens.)
type Builtin = { load: () -> any, preload: { string } }
local BUILTIN: { [string]: Builtin } = {
	orchestrators = {
		load = function(): any
			return require("@app/builtin/orchestrators")
		end,
		preload = {
			"orchestration",
			"actions",
			"dialogs",
			"progress",
			"settings_page",
			"data",
			"decide",
			"name",
			"hue",
			"map",
			"ref",
			"injected",
			"reports",
			"hold",
		},
	},
}

-- Each built-in's module, as this load has it: a table once it's loaded
-- (and its preload), `false` for one that isn't one here. Told on
-- `loaded_tick` (the table itself is this load's, as its modules are).
local loaded: { [string]: any } = {}
local loaded_tick = ui.state("plugins_loaded_tick", 0)

local function said(id: string, now: any): boolean
	return type(now) == "table" and now[id] == true
end

function M.on(id: string): boolean
	if not said(id, M.now:get()) then
		return false
	end
	if BUILTIN[id] == nil then
		return true
	end
	loaded_tick:get()
	return type(loaded[id]) == "table"
end

function M.peek(id: string): boolean
	return said(id, M.now:peek())
end

function M.module(id: string): any
	if BUILTIN[id] == nil or not M.on(id) then
		return nil
	end
	return loaded[id]
end

-- A built-in on: its module and preload, loaded where nothing is built or
-- run (the window's load, a timer's run), then said. `settled`: as a frame
-- has it, when its module is what it will be; as the window loads, a
-- plugin's own replacement may not be in yet (its script comes after the
-- app's), so one that isn't a module is tried again a frame on.
local function load(id: string, b: Builtin, settled: boolean)
	local ok, module = pcall(b.load)
	if not ok or type(module) ~= "table" then
		if not ok then
			print(`zeron-zgx: plugin {id}'s app-side module didn't load: {module}`)
		end
		if not settled then
			return
		end
		loaded[id] = false
	else
		for _, part in b.preload do
			local fine, why = pcall(function(): any
				return module[part]
			end)
			if not fine then
				print(`zeron-zgx: plugin {id}'s {part} didn't load: {why}`)
			end
		end
		loaded[id] = module
	end
	loaded_tick:set(loaded_tick:peek() + 1)
end
-- (While the window loads: its first frame not come yet.)
local loading = true
ui.after(0, function()
	loading = false
end)

function M.set(on: { [string]: boolean })
	M.now:set(on)
	if loading then
		for id, b in BUILTIN do
			if said(id, on) and loaded[id] == nil then
				load(id, b, false)
			end
		end
	end
end

-- On as the window loads (its first load, or a reload after a toggle):
-- loaded now, so the window is built with it. Called by the window's entry
-- (screens/shell/init) at its top level, once its own modules are in: a
-- built-in's modules require the app's, which mustn't be half loaded then.
function M.start()
	for id, b in BUILTIN do
		if M.peek(id) and loaded[id] == nil then
			load(id, b, false)
		end
	end
end
-- Turned on since: a frame on (a timer's run is no build's nor effect's).
ui.effect(function()
	local now = M.now:get()
	for id, b in BUILTIN do
		if said(id, now) and loaded[id] == nil then
			ui.after(0, function()
				if loaded[id] == nil and M.peek(id) then
					load(id, b, true)
				end
			end)
		end
	end
end)

return M
