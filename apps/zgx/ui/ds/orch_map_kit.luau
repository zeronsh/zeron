-- What the Map's three views (ds/orch_map) draw with: a token's ink at an
-- alpha for a canvas, and the platform glyph a machine wears.
--
--   kit.ink(token, alpha?)   `#rrggbbaa` for a canvas, from a theme token
--                            ("zeron.status.failed"), followed (a theme
--                            change paints again)
--   kit.platform_icon(d)     a device's glyph
--   kit.platform_word(d)     its platform, as the Map names it
--   kit.since(t)             seconds since `t`, moving on with the clock

local status = require("@self/status")

local M = {}

local MADE: { [string]: string } = {}

local function parse(v: any): (string, number)
	if type(v) ~= "string" or string.sub(v, 1, 1) ~= "#" or #v < 7 then
		return "#808080", 1
	end
	local a = if #v >= 9 then (tonumber(string.sub(v, 8, 9), 16) or 255) / 255 else 1
	return string.sub(v, 1, 7), a
end

function M.ink(token: string, alpha: number?): string
	local theme = ui.theme.current:get()
	local key = `{theme}/{token}/{alpha or 1}`
	local kept = MADE[key]
	if kept then
		return kept
	end
	local base, a = parse(ui.theme.get(token))
	local out = base .. string.format("%02x", math.clamp(math.floor(a * (alpha or 1) * 255 + 0.5), 0, 255))
	MADE[key] = out
	return out
end

function M.platform_icon(d: any): any
	if d and (d.cloud == true or d.platform == "cloud") then
		return "cloud"
	elseif d and d.platform == "macos" then
		return "laptop"
	end
	return "monitor"
end

function M.platform_word(d: any): string
	if d and d.platform == "macos" then
		return "macOS"
	elseif d and d.platform == "windows" then
		return "Windows"
	elseif d and d.platform == "linux" then
		return "Linux"
	end
	return (d and d.platform) or ""
end

M.since = status.since

return M
