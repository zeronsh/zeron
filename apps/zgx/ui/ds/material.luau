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

local glass_tint = require("@self/glass_tint")

local M = {}

M.frosted = ui.persist("frosted", true)

-- How much the desktop blurs behind the window's glass, in points, where
-- the app can tell (Hyprland, by its blur settings: src/desktop_blur.rs);
-- nil elsewhere (macOS's glass is a 60-point blur).
M.blur = ui.shared("window.blur", nil :: number?)

-- The window's glass where the desktop blurs less than macOS's: the tint
-- (`background`) more opaque, so what is behind the window shows as colour
-- and not as a sharp picture (ds/glass_tint); nil where the theme's own
-- suits.
function M.glass_theme(): { [string]: any }?
	local keep = glass_tint.clarity(M.blur:get())
	if keep >= 1 then
		return nil
	end
	local tint = ui.theme.get("glass.tint")
	local thicker = if tint == nil then nil else glass_tint.thicker(tostring(tint), keep)
	return if thicker then { background = thicker } else nil
end

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
