-- How Zeron's surfaces stand: frosted (the window's glass; menus over the
-- platform's frosted material) or opaque (solid colours throughout). One
-- setting for the whole app, kept across launches; components read it.

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

return M
