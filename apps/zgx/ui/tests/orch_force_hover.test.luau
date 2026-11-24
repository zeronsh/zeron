-- The force Map's hover, frame by frame (ds/orch_force), on both pivots:
-- the node and its lines lit only while the pointer is on the node (gone
-- the next frames after it leaves); the card and the pin a moment's grace
-- on the way to the card, none anywhere else; the pin, and the card, with
-- the node each frame it's dragged.

local ID = "th-reins-win"

local function open(pivot: string): any
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 2400)
	app:command("map_only")
	app:set_state("control_map_force_pivot", pivot)
	app:command("show_all")
	app:command("map_force")
	app:frames(90)
	app:wait(400)
	app:frames(20)
	return app
end
local function stats(app: any): any
	return app:state("control_map_force_stats") or {}
end
local function pointer(app: any, p: any)
	app:set_state("force_ptr", p)
	app:command("map_force_pointer")
end
local function at(app: any, id: string): (number, number, number)
	app:set_state("force_at_id", id)
	app:command("map_force_at")
	local a = app:state("force_at") or {}
	return a.x or 0, a.y or 0, a.r or 0
end
-- An element's box: x, y, w, h.
local function box(app: any, target: string): (number, number, number, number)
	local line = string.match(app:tree(target, 0), "[^\n]+") or ""
	local x, y, w, h = string.match(line, " (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
	return tonumber(x) or 0, tonumber(y) or 0, tonumber(w) or 0, tonumber(h) or 0
end
-- The pointer on the node, its card shown (rested on it).
local function on_node(app: any, id: string)
	app:set_state("control_map_force_hover", id)
	app:frames(3)
	app:hover(".orch-force-anchor-box")
	pointer(app, { op = "move", id = id, dx = 0, dy = 0 })
	app:frames(2)
	expect(app:wait(".orch-force-card", { timeout_ms = 1500 })).to_be(true)
	app:wait(700)
	app:frames(10)
	expect(app:state("control_map_force_lit")).to_be(id)
	expect(stats(app).lit).to_be(id)
end
-- Frames till what's drawn lit is `want`, at most `n`: how many it took.
local function frames_till_lit(app: any, want: string, n: number): number
	for f = 1, n do
		app:frames(1)
		if stats(app).lit == want then
			return f
		end
	end
	return n + 1
end
-- An empty place of the field far from the node and its card: at the
-- field's edge on its other side (the fit keeps every node and name in
-- from the edges), half way down.
local function far_from(app: any, id: string): (number, number)
	local x = at(app, id)
	local _, _, fw, fh = box(app, "#orch-force-field")
	return if x > fw / 2 then 4 else fw - 4, fh / 2
end

for _, pv in { "orchestrator", "machine" } do
	test(`force hover ({pv}): off the node, its light goes the next frames; off to empty space far from the card, the card too`, function()
		local app = open(pv)
		on_node(app, ID)
		local fx, fy = far_from(app, ID)
		pointer(app, { op = "move", x = fx, y = fy })
		-- (The light: gone within the fade's 120 ms, 7 frames at most.)
		expect(frames_till_lit(app, "", 7) <= 7).to_be(true)
		expect(app:state("control_map_force_lit")).to_be(nil)
		-- (Far from the card: no grace, gone at once.)
		expect(app:state("control_map_force_hover")).to_be(nil)
	end)

	test(`force hover ({pv}): on the way to the card the light goes, the card stays a moment, then goes`, function()
		local app = open(pv)
		on_node(app, ID)
		local _, cy = box(app, ".orch-force-card")
		local _, fy = box(app, "#orch-force-field")
		local nx, ny, r = at(app, ID)
		-- (Just past the node's edge, toward the card.)
		local dy = if cy - fy > ny then r + 10 else -(r + 10)
		pointer(app, { op = "move", x = nx, y = ny + dy })
		expect(frames_till_lit(app, "", 7) <= 7).to_be(true)
		expect(app:state("control_map_force_hover")).to_be(ID)
		-- On the card: kept, the node not lit.
		app:hover(".orch-force-card")
		app:wait(500)
		app:frames(5)
		expect(app:state("control_map_force_hover")).to_be(ID)
		expect(stats(app).lit).to_be("")
		-- Off it, nowhere near: gone after the grace.
		app:hover(".orch-force-legend")
		app:frames(2)
		app:wait(450)
		app:frames(5)
		expect(app:state("control_map_force_hover")).to_be(nil)
	end)

	test(`force hover ({pv}): onto another node, the light and the card move to it at once; out of the Map, both go`, function()
		local app = open(pv)
		on_node(app, ID)
		-- (The node farthest from it.)
		app:command("map_force_read")
		local places = app:state("force_places") or {}
		local me = places[ID]
		local other, best = "", -1
		for id, at_ in places do
			local d = (at_[1] - me[1]) ^ 2 + (at_[2] - me[2]) ^ 2
			if id ~= ID and d > best and string.sub(id, 1, 5) ~= "fold:" then
				other, best = id, d
			end
		end
		pointer(app, { op = "move", id = other, dx = 0, dy = 0 })
		expect(frames_till_lit(app, other, 3) <= 3).to_be(true)
		expect(app:state("control_map_force_hover")).to_be(other)
		-- Out of the Map, far from its card: nothing lit, no card.
		local fx, fy = far_from(app, other)
		pointer(app, { op = "move", x = fx, y = fy })
		pointer(app, { op = "leave" })
		expect(frames_till_lit(app, "", 7) <= 7).to_be(true)
		expect(app:state("control_map_force_hover")).to_be(nil)
	end)

	test(`force hover ({pv}): a pinned node dragged: its pin, and its card, with it every frame; lit while held`, function()
		local app = open(pv)
		on_node(app, ID)
		app:click("#orch-force-card-pin")
		app:frames(5)
		on_node(app, ID)
		expect(app:has("#orch-force-pin")).to_be(true)
		local px0, py0 = box(app, "#orch-force-pin")
		local cx0, cy0 = box(app, ".orch-force-card")
		local nx0, ny0 = at(app, ID)
		pointer(app, { op = "press", id = ID, dx = 0, dy = 0 })
		app:frames(1)
		for k = 1, 20 do
			pointer(app, { op = "move", x = nx0 + 6 * k, y = ny0 + 3 * k })
			app:frames(1)
			local nx, ny = at(app, ID)
			local px, py = box(app, "#orch-force-pin")
			-- (The pin moved as far as its node, this very frame.)
			expect(math.abs((px - px0) - (nx - nx0)) <= 1.5).to_be(true)
			expect(math.abs((py - py0) - (ny - ny0)) <= 1.5).to_be(true)
			expect(app:state("control_map_force_lit")).to_be(ID)
		end
		-- (The card with it too.)
		local nx, ny = at(app, ID)
		local cx, cy = box(app, ".orch-force-card")
		expect(app:has(".orch-force-card")).to_be(true)
		expect(math.abs((cx - cx0) - (nx - nx0)) <= 2).to_be(true)
		expect(math.abs((cy - cy0) - (ny - ny0)) <= 2).to_be(true)
		pointer(app, { op = "release", x = nx, y = ny })
		app:frames(10)
		expect(app:error()).to_be_nil()
	end)
end
