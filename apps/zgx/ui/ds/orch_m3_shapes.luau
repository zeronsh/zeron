--!native
-- Material 3's expressive shapes (Ghostish, the cookies, Sunny, Clover,
-- Burst, …), exactly as Android draws them: ported from AndroidX
-- (Apache-2.0):
--   compose/material3/material3/src/commonMain/kotlin/androidx/compose/
--     material3/MaterialShapes.kt            (the shapes' vertices and rounding)
--   graphics/graphics-shapes/src/commonMain/kotlin/androidx/graphics/
--     shapes/RoundedPolygon.kt, Shapes.kt, Cubic.kt   (rounding with
--     smoothing, star vertices, the circular arc as a cubic, normalizing)
-- Pure (no requires): each shape worked out once into cubics in the unit
-- square (as `normalized()` puts it, (0,0)–(1,1)), kept, and drawn at any
-- place and size from those.
--
--   shapes.cubics(name)          { x0,y0, c0x,c0y, c1x,c1y, x1,y1, … } (unit square)
--   shapes.path(name, cx, cy, d) a ui.path of it, `d` across, centred on (cx, cy)
--   shapes.NAMES                 the shapes there are

local M = {}

M.NAMES = { "ghostish", "cookie4", "cookie6", "cookie7", "cookie9", "cookie12", "sunny", "very_sunny", "clover4", "clover8", "burst", "soft_burst", "gem", "pentagon" }

local EPS = 1e-4

type P = { number } -- { x, y }
type Round = { r: number, s: number } -- radius, smoothing

local function sub(a: P, b: P): P
	return { a[1] - b[1], a[2] - b[2] }
end
local function add(a: P, b: P): P
	return { a[1] + b[1], a[2] + b[2] }
end
local function mul(a: P, k: number): P
	return { a[1] * k, a[2] * k }
end
local function dist(a: P): number
	return math.sqrt(a[1] * a[1] + a[2] * a[2])
end
local function dir(a: P): P
	local d = dist(a)
	return { a[1] / d, a[2] / d }
end
local function dot(a: P, b: P): number
	return a[1] * b[1] + a[2] * b[2]
end
local function rot90(a: P): P
	return { -a[2], a[1] }
end

-- Cubic.circularArc: the arc round `c` from p0 to p1 as one cubic.
local function arc(c: P, p0: P, p1: P, out: { number })
	local p0d, p1d = dir(sub(p0, c)), dir(sub(p1, c))
	local r0, r1 = rot90(p0d), rot90(p1d)
	local clockwise = dot(r0, sub(p1, c)) >= 0
	local cosa = dot(p0d, p1d)
	if cosa > 0.999 then
		table.move({ p0[1], p0[2], p0[1], p0[2], p1[1], p1[2], p1[1], p1[2] }, 1, 8, #out + 1, out)
		return
	end
	local k = dist(sub(p0, c)) * 4 / 3 * (math.sqrt(2 * (1 - cosa)) - math.sqrt(1 - cosa * cosa)) / (1 - cosa) * (if clockwise then 1 else -1)
	table.move({ p0[1], p0[2], p0[1] + r0[1] * k, p0[2] + r0[2] * k, p1[1] - r1[1] * k, p1[2] - r1[2] * k, p1[1], p1[2] }, 1, 8, #out + 1, out)
end

-- RoundedCorner (RoundedPolygon.kt): the corner at p1 between p0 and p2.
local function corner(p0: P, p1: P, p2: P, rd: Round): any
	local v01, v21 = sub(p0, p1), sub(p2, p1)
	local d01, d21 = dist(v01), dist(v21)
	local c: any = { p0 = p0, p1 = p1, p2 = p2 }
	if d01 > 0 and d21 > 0 then
		c.d1, c.d2 = mul(v01, 1 / d01), mul(v21, 1 / d21)
		c.radius, c.smoothing = rd.r, rd.s
		c.cos = dot(c.d1, c.d2)
		c.sin = math.sqrt(1 - c.cos * c.cos)
		c.round_cut = if c.sin > 1e-3 then c.radius * (c.cos + 1) / c.sin else 0
	else
		c.d1, c.d2 = { 0, 0 }, { 0, 0 }
		c.radius, c.smoothing, c.cos, c.sin, c.round_cut = 0, 0, 0, 0, 0
	end
	c.cut = (1 + c.smoothing) * c.round_cut
	return c
end

local function smoothing_for(c: any, allowed: number): number
	if allowed > c.cut then
		return c.smoothing
	elseif allowed > c.round_cut then
		return c.smoothing * (allowed - c.round_cut) / (c.cut - c.round_cut)
	end
	return 0
end

local function line_cross(p0: P, d0: P, p1: P, d1: P): P?
	local r = rot90(d1)
	local den = dot(d0, r)
	if math.abs(den) < EPS then
		return nil
	end
	local num = dot(sub(p1, p0), r)
	if math.abs(den) < EPS * math.abs(num) then
		return nil
	end
	return add(p0, mul(d0, num / den))
end

-- computeFlankingCurve: from the (cut) side onto the circle, smoothly.
local function flank(round_cut: number, sm: number, cornerp: P, side_start: P, hit: P, other_hit: P, center: P, r: number): { number }
	local side_dir = dir(sub(side_start, cornerp))
	local curve_start = add(cornerp, mul(side_dir, round_cut * (1 + sm)))
	local mid = mul(add(hit, other_hit), 0.5)
	local p = { hit[1] + (mid[1] - hit[1]) * sm, hit[2] + (mid[2] - hit[2]) * sm }
	local curve_end = add(center, mul(dir(sub(p, center)), r))
	local tangent = rot90(sub(curve_end, center))
	local anchor_end = line_cross(side_start, side_dir, curve_end, tangent) or hit
	local anchor_start = mul(add(curve_start, mul(anchor_end, 2)), 1 / 3)
	return { curve_start[1], curve_start[2], anchor_start[1], anchor_start[2], anchor_end[1], anchor_end[2], curve_end[1], curve_end[2] }
end

local function reversed(cu: { number }): { number }
	return { cu[7], cu[8], cu[5], cu[6], cu[3], cu[4], cu[1], cu[2] }
end

-- getCubics: the corner's cubics given the cuts its two sides allow.
local function corner_cubics(c: any, allowed0: number, allowed1: number): { { number } }
	local allowed = math.min(allowed0, allowed1)
	if c.round_cut < EPS or allowed < EPS or c.radius < EPS then
		return { { c.p1[1], c.p1[2], c.p1[1], c.p1[2], c.p1[1], c.p1[2], c.p1[1], c.p1[2] } }
	end
	local round_cut = math.min(allowed, c.round_cut)
	local sm0, sm1 = smoothing_for(c, allowed0), smoothing_for(c, allowed1)
	local r = c.radius * round_cut / c.round_cut
	local center_d = math.sqrt(r * r + round_cut * round_cut)
	local center = add(c.p1, mul(dir(mul(add(c.d1, c.d2), 0.5)), center_d))
	local hit0 = add(c.p1, mul(c.d1, round_cut))
	local hit2 = add(c.p1, mul(c.d2, round_cut))
	local f0 = flank(round_cut, sm0, c.p1, c.p0, hit0, hit2, center, r)
	local f2 = reversed(flank(round_cut, sm1, c.p1, c.p2, hit2, hit0, center, r))
	local mid: { number } = {}
	arc(center, { f0[7], f0[8] }, { f2[1], f2[2] }, mid)
	return { f0, mid, f2 }
end

-- RoundedPolygon(vertices, perVertexRounding): its cubics, corners and the
-- edges between them, in order.
local function polygon(vs: { P }, rounds: { Round }): { number }
	local n = #vs
	local corners = {}
	for i = 1, n do
		corners[i] = corner(vs[(i - 2) % n + 1], vs[i], vs[i % n + 1], rounds[i])
	end
	-- How much of each side's cut each corner may take.
	local adjust = {}
	for i = 1, n do
		local j = i % n + 1
		local round_cut = corners[i].round_cut + corners[j].round_cut
		local cut = corners[i].cut + corners[j].cut
		local side = dist(sub(vs[i], vs[j]))
		if round_cut > side then
			adjust[i] = { side / round_cut, 0 }
		elseif cut > side then
			adjust[i] = { 1, (side - round_cut) / (cut - round_cut) }
		else
			adjust[i] = { 1, 1 }
		end
	end
	local per: { { { number } } } = {}
	for i = 1, n do
		local cuts = {}
		for delta = 0, 1 do
			local a = adjust[(i - 2 + delta) % n + 1]
			cuts[delta + 1] = corners[i].round_cut * a[1] + (corners[i].cut - corners[i].round_cut) * a[2]
		end
		per[i] = corner_cubics(corners[i], cuts[1], cuts[2])
	end
	local out: { number } = {}
	for i = 1, n do
		for _, cu in per[i] do
			table.move(cu, 1, 8, #out + 1, out)
		end
		local last = per[i][#per[i]]
		local nxt = per[i % n + 1][1]
		-- (The edge to the next corner: a straight cubic.)
		local x0, y0, x1, y1 = last[7], last[8], nxt[1], nxt[2]
		table.move({ x0, y0, x0 + (x1 - x0) / 3, y0 + (y1 - y0) / 3, x0 + (x1 - x0) * 2 / 3, y0 + (y1 - y0) * 2 / 3, x1, y1 }, 1, 8, #out + 1, out)
	end
	return out
end

-- MaterialShapes.doRepeat + customPolygon.
local function custom(pnr: { { any } }, reps: number, mirroring: boolean?): { number }
	local center = { 0.5, 0.5 }
	local pts: { P } = {}
	local rnd: { Round } = {}
	local function push(p: P, r: Round)
		table.insert(pts, p)
		table.insert(rnd, r)
	end
	if mirroring then
		local angles, dists = {}, {}
		for i, q in pnr do
			local o = sub(q[1], center)
			angles[i] = math.deg(math.atan2(o[2], o[1]))
			dists[i] = dist(o)
		end
		local actual = reps * 2
		local section = 360 / actual
		for it = 0, actual - 1 do
			for index = 1, #pnr do
				local i = if it % 2 == 0 then index else #pnr - index + 1
				if i > 1 or it % 2 == 0 then
					local a = math.rad(section * it + (if it % 2 == 0 then angles[i] else section - angles[i] + 2 * angles[1]))
					push({ math.cos(a) * dists[i] + center[1], math.sin(a) * dists[i] + center[2] }, pnr[i][2])
				end
			end
		end
	else
		local np = #pnr
		for it = 0, np * reps - 1 do
			local q = pnr[it % np + 1]
			local a = math.rad((it // np) * 360 / reps)
			local o = sub(q[1], center)
			push({ o[1] * math.cos(a) - o[2] * math.sin(a) + center[1], o[1] * math.sin(a) + o[2] * math.cos(a) + center[2] }, q[2])
		end
	end
	return polygon(pts, rnd)
end

-- RoundedPolygon.star, then rotated -90° (the cookies'), as MaterialShapes has them.
local function star(k: number, inner: number, round: Round, rotate: boolean): { number }
	local pts: { P } = {}
	local rnd: { Round } = {}
	for i = 0, k - 1 do
		local a = math.pi / k * 2 * i
		table.insert(pts, { math.cos(a), math.sin(a) })
		a = math.pi / k * (2 * i + 1)
		table.insert(pts, { math.cos(a) * inner, math.sin(a) * inner })
		table.insert(rnd, round)
		table.insert(rnd, round)
	end
	local cu = polygon(pts, rnd)
	if rotate then
		-- (Matrix.rotateZ(-90): (x, y) → (y, -x).)
		for i = 1, #cu, 2 do
			cu[i], cu[i + 1] = cu[i + 1], -cu[i]
		end
	end
	return cu
end

local function R(r: number, s: number?): Round
	return { r = r, s = s or 0 }
end
local NONE = R(0)
local function pt(x: number, y: number, r: Round?): { any }
	return { { x, y }, r or NONE }
end

local DEFS: { [string]: () -> { number } } = {
	ghostish = function()
		return custom({ pt(0.5, 0, R(1)), pt(1, 0, R(1)), pt(1, 1.14, R(0.254, 0.106)), pt(0.575, 0.906, R(0.253)) }, 1, true)
	end,
	cookie4 = function()
		return custom({ pt(1.237, 1.236, R(0.258)), pt(0.5, 0.918, R(0.233)) }, 4)
	end,
	cookie6 = function()
		return custom({ pt(0.723, 0.884, R(0.394)), pt(0.5, 1.099, R(0.398)) }, 6)
	end,
	cookie7 = function()
		return star(7, 0.75, R(0.5), true)
	end,
	cookie9 = function()
		return star(9, 0.8, R(0.5), true)
	end,
	cookie12 = function()
		return star(12, 0.8, R(0.5), true)
	end,
	sunny = function()
		return star(8, 0.8, R(0.15), false)
	end,
	very_sunny = function()
		return custom({ pt(0.5, 1.08, R(0.085)), pt(0.358, 0.843, R(0.085)) }, 8)
	end,
	clover4 = function()
		return custom({ pt(0.5, 0.074), pt(0.725, -0.099, R(0.476)) }, 4, true)
	end,
	clover8 = function()
		return custom({ pt(0.5, 0.036), pt(0.758, -0.101, R(0.209)) }, 8)
	end,
	burst = function()
		return custom({ pt(0.5, -0.006, R(0.006)), pt(0.592, 0.158, R(0.006)) }, 12)
	end,
	soft_burst = function()
		return custom({ pt(0.193, 0.277, R(0.053)), pt(0.176, 0.055, R(0.053)) }, 10)
	end,
	gem = function()
		return custom({ pt(0.499, 1.023, R(0.241, 0.778)), pt(-0.005, 0.792, R(0.208)), pt(0.073, 0.258, R(0.228)), pt(0.433, 0, R(0.491)) }, 1, true)
	end,
	pentagon = function()
		return custom({ pt(0.5, -0.009, R(0.172)), pt(1.03, 0.365, R(0.164)), pt(0.828, 0.97, R(0.169)) }, 1, true)
	end,
}

-- normalized(): into the unit square, centred (its approximate bounds:
-- every anchor and control point, as calculateBounds() takes them).
local function normalized(cu: { number }): { number }
	local x0: number, y0: number, x1: number, y1: number = math.huge, math.huge, -math.huge, -math.huge
	for i = 1, #cu, 2 do
		local px, py = cu[i] :: number, cu[i + 1] :: number
		x0, x1 = math.min(x0, px), math.max(x1, px)
		y0, y1 = math.min(y0, py), math.max(y1, py)
	end
	local w, h = x1 - x0, y1 - y0
	local side = math.max(w, h)
	local ox, oy = (side - w) / 2 - x0, (side - h) / 2 - y0
	local out = table.create(#cu, 0)
	for i = 1, #cu, 2 do
		out[i], out[i + 1] = (cu[i] + ox) / side, (cu[i + 1] + oy) / side
	end
	return out
end

local made: { [string]: { number } } = {}
function M.cubics(name: string): { number }
	local c = made[name]
	if not c then
		local def = DEFS[name] or DEFS.cookie4
		c = normalized(def())
		made[name] = c
	end
	return c
end

-- A ui.path of the shape, `d` across, centred on (cx, cy).
function M.path(name: string, cx: number, cy: number, d: number): any
	local cu = M.cubics(name)
	local ox, oy = cx - d / 2, cy - d / 2
	local p = ui.path():move(ox + cu[1] * d, oy + cu[2] * d)
	for i = 1, #cu, 8 do
		p = p:cubic(ox + cu[i + 2] * d, oy + cu[i + 3] * d, ox + cu[i + 4] * d, oy + cu[i + 5] * d, ox + cu[i + 6] * d, oy + cu[i + 7] * d)
	end
	return p:close()
end

return M
