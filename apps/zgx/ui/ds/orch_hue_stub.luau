-- Stand-in for ds/orch_hue (ui-control's) until it lands: an orchestrator's
-- hue as a colour, at a fixed lightness and chroma for each scheme.
--
--   hue.color(h)      its ink (a dot, a stripe, a bar)
--   hue.wash(h)       a faint wash of it (a card's tint)
--
-- Replaced by ds/orch_hue on merge.

local M = {}

-- Light ink is a dark theme's.
local function dark(): boolean
	ui.theme.current:get()
	local ink = ui.theme.get("text.primary")
	local r, g, b = string.match(if type(ink) == "string" then ink else "", "^#(%x%x)(%x%x)(%x%x)")
	if not r then
		return true
	end
	return 0.2126 * tonumber(r, 16) + 0.7152 * tonumber(g :: string, 16) + 0.0722 * tonumber(b :: string, 16) > 128
end

function M.color(h: number): string
	local deg = math.floor((h or 0) % 360)
	return if dark() then ui.raw(`hsl({deg}, 68%, 62%)`, "an orchestrator's hue") else ui.raw(`hsl({deg}, 60%, 44%)`, "an orchestrator's hue")
end

function M.wash(h: number): string
	local deg = math.floor((h or 0) % 360)
	return if dark() then ui.raw(`hsla({deg}, 68%, 62%, 0.14)`, "an orchestrator's wash") else ui.raw(`hsla({deg}, 60%, 44%, 0.10)`, "an orchestrator's wash")
end

return M
