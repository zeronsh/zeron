-- How Zeron's surfaces stand: frosted (the window's glass; menus over the
-- platform's frosted material) or opaque (solid colours throughout). One
-- setting for the whole app, kept across launches.
--
-- `material.frosted` is the setting (what Settings → Appearance switches).
-- What draws reads `material.see_through()`: frosted, unless the system
-- asks for no transparency (macOS's Reduce transparency; Windows'
-- Transparency effects off, which the host reports, src/transparency.rs),
-- when every glass turns solid whatever the setting says.

local prefers = require("@self/prefers")

local M = {}

M.frosted = ui.persist("frosted", true)

-- Windows' Transparency effects off (zgx reports macOS's alone, as
-- `ui.prefers.reduced_transparency`). (Shared with plugins, which borrow
-- the design system, to read; a host that doesn't share it, none.)
local ok, shared = pcall(ui.shared, "system.reduced_transparency", false)
local system_solid: State<boolean>? = if ok then shared :: any else nil

-- Whether glass shows what's behind it: read where a surface picks its
-- tint and blur, in a binding (it follows the setting and the system).
function M.see_through(): boolean
	return M.frosted:get() and not prefers.reduced_transparency:get() and not (system_solid ~= nil and system_solid:get())
end

return M
