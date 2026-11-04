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

-- Whether a popup's glass can blur what's under it (the host's word,
-- src/transparency.rs: not on Windows, where zgx's popup windows have no
-- system backdrop). Unknown, it can.
local ok_blur, popups_blur = pcall(ui.shared, "system.popups_blur", true)
local menus_blur: State<boolean>? = if ok_blur then popups_blur :: any else nil

-- Whether glass shows what's behind it: read where a surface picks its
-- tint and blur, in a binding (it follows the setting and the system).
function M.see_through(): boolean
	return M.frosted:get() and not prefers.reduced_transparency:get() and not (system_solid ~= nil and system_solid:get())
end

-- The same for the menus' glass (ds/glass `menu`: menus, popovers, tooltips,
-- hover cards, toasts), which stands in popups of its own: solid, too,
-- where a popup can't blur (translucent with nothing blurred, what's behind
-- shows through sharp).
function M.menus_see_through(): boolean
	return M.see_through() and not (menus_blur ~= nil and menus_blur:get() == false)
end

return M
