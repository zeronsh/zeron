-- Which plugins run in this window, for the app's own code: a hook that
-- belongs to one of Zeron's built-in plugins (plugins-builtin/<id>) shows
-- only while it's on, and follows a toggle in Settings → Plugins with no
-- restart.
--
--   plugins.on(id)       whether plugin `id` runs here: the engine lists it
--                        on and the window loaded it. Read in a build or a
--                        computed, it's followed (src/plugins: `plugins.on`).
--   plugins.module(id)   a built-in's app-side module (`@app/builtin/<id>`,
--                        the plugin's own while it's on), or nil: off, not
--                        loaded, or not one this app knows
--   plugins.set(on)      `{ [id] = true }`, for a fixture: what the window
--                        says otherwise. Off is the default everywhere.
--
-- A gated hook:
--
--   local plugins = require("@account/plugins")
--   ui.show(function() return plugins.on("orchestrators") end, ...)
--   local orch = plugins.module("orchestrators")
--   if orch then orch.proof.label() end

local M = {}

M.now = ui.shared("plugins.on", {} :: { [string]: boolean })

-- The built-ins whose app-side module the app reaches, each by its stable
-- name. ui/builtin/<id>.luau is what the app has with it off (`false`);
-- the plugin replaces it while on (docs/plugins.md, "Zeron's own plugins").
local BUILTIN: { [string]: () -> any } = {
	orchestrators = function(): any
		return require("@app/builtin/orchestrators")
	end,
}

function M.on(id: string): boolean
	local now = M.now:get()
	return type(now) == "table" and now[id] == true
end

function M.module(id: string): any
	local load = BUILTIN[id]
	if not load or not M.on(id) then
		return nil
	end
	local module = load()
	return if type(module) == "table" then module else nil
end

function M.set(on: { [string]: boolean })
	M.now:set(on)
end

return M
