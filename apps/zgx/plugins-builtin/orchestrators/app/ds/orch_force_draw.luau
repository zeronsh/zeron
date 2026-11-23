-- What the force Graph (ds/orch_force) draws a node with, and how big: the
-- Brain dump Material's Ghostish; an orchestrator a cookie (more scallops
-- the bigger its crew: ds/orch_m3_shapes), ringed by its own hue; a chat a
-- disc. A chat's or an orchestrator's provider mark (ds/orch_force_marks)
-- sits in its fill, MARK_SHARE of the fill's inner circle across (by the
-- mark's visual bounds, eased optically), in its middle.
--
--   draw.cookie_of(crew)              the shape for a crew of that size
--   draw.inner(shape)                 its inner circle's radius (of half its width)
--   draw.mark_hub(shape)              an orchestrator's mark size in that shape
--   draw.mark_of(provider)            the mark a provider wears (nil: none)
--   draw.mark(name, x, y, d)          { { path, rule } } of a mark, d its size
--   draw.shape(name, x, y, d)         a shape's path (ds/orch_m3_shapes)

local shapes = require("./orch_m3_shapes")
local marks = require("./orch_force_marks")

local M = {}

-- A chat: its disc's radius, its status ring's.
M.R_CHAT = 10
M.R_RING = 13.5
-- An orchestrator: its hue's outline across, the gap's, its fill's.
M.HUB_D = 42
M.HUB_GAP_D = 38.5
M.HUB_FILL_D = 34
-- The Brain dump across.
M.BRAIN_D = 38
-- A mark's share of its fill's inner circle (60–65% by its bounds, once
-- eased optically).
M.MARK_SHARE = 0.6
M.MARK_CHAT = M.MARK_SHARE * 2 * M.R_CHAT

-- The cookie for a crew this big: more scallops for more crew; past what a
-- cookie reads as, the soft burst.
function M.cookie_of(crew: number): string
	return if crew <= 3 then "cookie4" elseif crew <= 6 then "cookie6" elseif crew <= 10 then "cookie7" elseif crew <= 18 then "cookie9" elseif crew <= 30 then "cookie12" else "soft_burst"
end

-- A shape's inner circle: the least distance from its middle to its outline,
-- as a share of half its width (worked out once a shape).
local inner_of: { [string]: number } = {}
function M.inner(name: string): number
	local kept = inner_of[name]
	if kept then
		return kept
	end
	local cu = shapes.cubics(name)
	local least = math.huge
	for i = 1, #cu, 8 do
		local x0, y0, a, b, c, d, x1, y1 = cu[i], cu[i + 1], cu[i + 2], cu[i + 3], cu[i + 4], cu[i + 5], cu[i + 6], cu[i + 7]
		for k = 0, 24 do
			local t = k / 24
			local u = 1 - t
			local x = u * u * u * x0 + 3 * u * u * t * a + 3 * u * t * t * c + t * t * t * x1
			local y = u * u * u * y0 + 3 * u * u * t * b + 3 * u * t * t * d + t * t * t * y1
			least = math.min(least, math.sqrt((x - 0.5) ^ 2 + (y - 0.5) ^ 2))
		end
	end
	local r = least / 0.5
	inner_of[name] = r
	return r
end

-- An orchestrator's mark size, in its shape's fill.
function M.mark_hub(name: string): number
	return M.MARK_SHARE * M.inner(name) * M.HUB_FILL_D
end

-- The mark a provider wears (Codex wears OpenAI's); nil for one without.
local MARK_OF: { [string]: string } = {
	claude = "claude",
	openai = "openai",
	codex = "openai",
	cursor = "cursor",
	grok = "grok",
	gemini = "gemini",
	copilot = "copilot",
}
function M.mark_of(provider: string?): string?
	return MARK_OF[provider or ""]
end

-- A mark's paths at (x, y), `d` its size: each part with its fill rule.
function M.mark(name: string, x: number, y: number, d: number): { { any } }
	local m = (marks :: any)[name]
	local out = {}
	if not m then
		return out
	end
	for _, part in m.parts do
		local c = part.cmds
		local p = ui.path()
		local i = 1
		while i <= #c do
			local op = c[i]
			if op == 1 then
				p = p:move(x + c[i + 1] * d, y + c[i + 2] * d)
				i += 3
			elseif op == 2 then
				p = p:line(x + c[i + 1] * d, y + c[i + 2] * d)
				i += 3
			elseif op == 3 then
				p = p:cubic(x + c[i + 1] * d, y + c[i + 2] * d, x + c[i + 3] * d, y + c[i + 4] * d, x + c[i + 5] * d, y + c[i + 6] * d)
				i += 7
			else
				p = p:close()
				i += 1
			end
		end
		table.insert(out, { p, part.rule })
	end
	return out
end

M.shape = shapes.path

-- The pin glyph (the app's own, ui/assets/icons/ui/pin.svg: a pin's head
-- and its point, on a 24 grid, stroked 1.5) at (x, y), `d` across: its
-- path, and the stroke's width.
function M.pin(x: number, y: number, d: number): (any, number)
	local k = d / 24
	local function at(px: number, py: number): (number, number)
		return x + (px - 12) * k, y + (py - 12) * k
	end
	local p = ui.path()
	p = p:move(at(9, 3)):line(at(15, 3)):line(at(14, 9)):line(at(17, 12)):line(at(17, 14)):line(at(7, 14)):line(at(7, 12)):line(at(10, 9)):close()
	p = p:move(at(12, 14)):line(at(12, 21))
	return p, 1.5 * k
end

return M
