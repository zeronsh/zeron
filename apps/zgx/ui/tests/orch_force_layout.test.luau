-- The force Graph laid out for its field (ds/orch_force, ds/orch_force_sim):
-- each fleet shape (the live Map's two orchestrators with 25 and 12, a
-- sparse one, one with 40, four uneven, seven) in each field (the
-- Overview's 1300 by 700, a page's 1500 by 900, a narrow 700 by 600): no
-- name over another or over a mark, every mark and name inside the field
-- (Fit); a thread added, or one finishing, moves no other; a hub's many
-- quiet threads folded into one, opened by a click and folded back from
-- its hub's menu. Prints `FORCELAYOUT …` a shape and field.

local SHAPES = { "screenshot", "sparse", "one40", "uneven4", "seven" }
local FIELDS = { { 1300, 700 }, { 1500, 900 }, { 700, 600 } }

local function stats(app: any): any
	return app:state("control_map_force_stats") or {}
end

-- The Map widget alone, `w` wide, its field `h` high, a fleet shape on it;
-- settled, its names placed.
local function open(shape: string, w: number, h: number): any
	local app = mount("orch_map_fixture.luau")
	app:resize(w + 40, h + 160)
	app:command("map_bare")
	app:set_state("force_bare_w", w)
	app:set_state("control_map_force_field_h", h)
	app:command(`map_shape_{shape}`)
	app:command("map_force")
	for _ = 1, 300 do
		app:frames(1)
		local s = stats(app)
		if (s.settled or 0) > 0 and app:has(".orch-force-node") then
			break
		end
	end
	-- (Names: a slice a frame, again at the fitted zoom once it settles.)
	app:frames(5)
	app:wait(400)
	app:frames(25)
	app:wait(400)
	app:frames(25)
	return app
end

local function read(app: any): any
	app:command("map_force_labels")
	app:frames(1)
	return app:state("force_labels") or {}
end

local function places(app: any): { [string]: { number } }
	app:command("map_force_read")
	app:frames(1)
	return app:state("force_places") or {}
end

for _, shape in SHAPES do
	for _, f in FIELDS do
		local w, h = f[1], f[2]
		test(`force layout ({shape}, {w}x{h}): no name over another or a mark; every mark and name inside the field`, function()
			local app = open(shape, w, h)
			local d = read(app)
			local v = d.view
			assert(v, "no view read")
			local z = v.zoom
			local function sx(x: number): number
				return x * z + v.x
			end
			local function sy(y: number): number
				return y * z + v.y
			end
			-- (A hair's tolerance: the halo's.)
			local e = 1.5
			local over, out = 0, 0
			for a, la in d.labels do
				for b, lb in d.labels do
					if a < b and la.x0 + e < lb.x1 and la.x1 - e > lb.x0 and la.y0 + e < lb.y1 and la.y1 - e > lb.y0 then
						over += 1
						print(`FORCELAYOUT_OVER {shape} {w}x{h} {la.id} {lb.id}`)
					end
				end
				if sx(la.x0) < -e or sy(la.y0) < -e or sx(la.x1) > w + e or sy(la.y1) > h + e then
					out += 1
					print(`FORCELAYOUT_OUT {shape} {w}x{h} name {la.id}`)
				end
			end
			for _, n in d.nodes do
				local r = (if n.kind == "brain" then 19 elseif n.kind == "chat" then 13.5 else 21) * z
				if sx(n.x) - r < -e or sy(n.y) - r < -e or sx(n.x) + r > w + e or sy(n.y) + r > h + e then
					out += 1
					print(`FORCELAYOUT_OUT {shape} {w}x{h} mark {n.id}`)
				end
			end
			print(`FORCELAYOUT {shape} {w}x{h}: {#d.nodes} marks, {#d.labels} names, zoom {string.format("%.2f", z)}, {over} over, {out} outside, {stats(app).layouts or 0} layouts`)
			expect(over).to_be(0)
			expect(out).to_be(0)
		end)
	end
end

test("force layout: a thread added, then one finishing, moves no other; the field's own layout kept", function()
	local app = open("screenshot", 1300, 700)
	local a = places(app)
	local layouts = stats(app).layouts or 0
	expect(layouts >= 1).to_be(true)
	app:command("map_shape_add")
	for _ = 1, 200 do
		app:frames(1)
	end
	app:wait(300)
	app:frames(30)
	local b = places(app)
	local moved, kept, added = 0, 0, 0
	for id, p in b do
		local q = a[id]
		if q then
			kept += 1
			if math.abs(p[1] - q[1]) > 0.5 or math.abs(p[2] - q[2]) > 0.5 then
				moved += 1
				print(`FORCELAYOUT_MOVED add {id} {q[1]},{q[2]} -> {p[1]},{p[2]}`)
			end
		else
			added += 1
		end
	end
	print(`FORCELAYOUT add: {kept} kept, {moved} moved, {added} added`)
	expect(added).to_be(1)
	expect(moved).to_be(0)
	-- (Laid out for this field once: the add kept its layout.)
	expect(stats(app).layouts).to_be(layouts)
	-- One finishing: nothing moves (its name goes; its place stays).
	app:command("map_shape_finish")
	for _ = 1, 120 do
		app:frames(1)
	end
	app:wait(300)
	app:frames(30)
	local c = places(app)
	local moved2 = 0
	for id, p in c do
		local q = b[id]
		if q and (math.abs(p[1] - q[1]) > 0.5 or math.abs(p[2] - q[2]) > 0.5) then
			moved2 += 1
		end
	end
	print(`FORCELAYOUT finish: {moved2} moved`)
	expect(moved2).to_be(0)
end)

test("force layout: a field resized lays the web out for it; a pin stays where it was dropped", function()
	local app = open("screenshot", 1300, 700)
	local layouts = stats(app).layouts or 0
	-- (A pin: the first thread, held where it is.)
	local a = places(app)
	app:set_state("control_map_force_pins", { s1c1 = true })
	app:set_state("force_bare_w", 700)
	app:set_state("control_map_force_field_h", 600)
	for _ = 1, 300 do
		app:frames(1)
	end
	app:wait(400)
	app:frames(30)
	expect(stats(app).layouts > layouts).to_be(true)
	local b = places(app)
	expect(math.abs(b.s1c1[1] - a.s1c1[1]) < 0.5 and math.abs(b.s1c1[2] - a.s1c1[2]) < 0.5).to_be(true)
end)

test("force layout: a hub's many quiet threads fold into one; a click opens it, its hub's menu folds it back", function()
	local app = open("fold", 1300, 700)
	local p = places(app)
	-- (30 idle under the Engine: one fold; its 4 working alone.)
	expect(p["fold:orc-1"] ~= nil).to_be(true)
	local alone = 0
	for i = 1, 34 do
		if p[`s1c{i}`] then
			alone += 1
		end
	end
	expect(alone).to_be(4)
	-- (Named for assistive technology by how many it holds, no symbol.)
	expect(app:has('"30 idle or finished threads"')).to_be(true)
	-- Its card: how many, the first few, Show them; no typed separator.
	app:set_state("control_map_force_hover", "fold:orc-1")
	app:frames(3)
	app:hover(".orch-force-anchor-box")
	expect(app:wait(".orch-force-card-fold", { timeout_ms = 1500 })).to_be(true)
	app:wait(700)
	app:frames(10)
	local t = app:tree(".orch-force-card") :: string
	expect(string.find(t, "30 idle or finished threads", 1, true) ~= nil).to_be(true)
	expect(string.find(t, "·", 1, true)).to_be(nil)
	expect(app:count(".orch-force-card-fold-row")).to_be(5)
	-- A click on the fold: its threads out, each alone.
	app:set_state("control_map_force_hover", "fold:orc-1")
	app:frames(3)
	app:click(".orch-force-anchor-box")
	for _ = 1, 300 do
		app:frames(1)
	end
	app:wait(300)
	app:frames(20)
	expect((app:state("control_map_force_open_folds") or {})["orc-1"]).to_be(true)
	expect(stats(app).unfolds >= 1).to_be(true)
	local q = places(app)
	expect(q["fold:orc-1"]).to_be(nil)
	local out = 0
	for i = 1, 34 do
		if q[`s1c{i}`] then
			out += 1
		end
	end
	expect(out).to_be(34)
	-- Its hub's menu: fold them back.
	app:set_state("force_menu_for", "orc-1")
	app:command("map_force_menu_read")
	app:frames(1)
	local items = app:state("force_menu") or {}
	expect(table.find(items, "Fold idle and finished threads") ~= nil).to_be(true)
	app:set_state("force_menu_pick", "Fold idle and finished threads")
	app:command("map_force_menu_pick")
	for _ = 1, 300 do
		app:frames(1)
	end
	app:frames(20)
	expect(places(app)["fold:orc-1"] ~= nil).to_be(true)
end)

test("force layout: every thread at work, waiting or asking named; idle and finished ones where there's room; a fold never", function()
	local active = { working = true, waiting = true, input = true }
	for _, f in { { 1300, 700 }, { 700, 600 } } do
		local app = open("screenshot", f[1], f[2])
		local d = read(app)
		local named = {}
		for _, la in d.labels do
			named[la.id] = true
		end
		-- (The fixture's mix: s1c2 works, s1c5 waits, s1c11 asks; s1c1 idle.)
		for _, id in { "s1c2", "s1c5", "s1c11", "orc-1", "orc-2", "orc-brain" } do
			expect(`{id} {named[id] == true}`).to_be(`{id} true`)
		end
		local spare, spare_named = 0, 0
		for i = 1, 25 do
			local id = `s1c{i}`
			local mix = { "idle", "working", "idle", "done", "waiting", "idle", "idle", "working", "done", "idle", "input", "idle", "working", "idle", "done", "idle", "waiting", "idle" }
			if not active[mix[(i - 1) % #mix + 1]] then
				spare += 1
				if named[id] then
					spare_named += 1
				end
			end
		end
		print(`FORCELAYOUT names {f[1]}x{f[2]}: {spare_named} of {spare} idle or finished named`)
		if f[1] == 700 then
			-- (Narrow: not all of them have room.)
			expect(spare_named < spare).to_be(true)
		else
			expect(spare_named > 0).to_be(true)
		end
	end
end)
