-- Unpin all on the force Map (ds/orch_force), frame by frame, both pivots:
-- a web with hubs dropped onto each other and crews dragged to the edge,
-- kept through a relaunch (its places its slots then), let go of at once.
-- Every frame: no hub over another, nothing past the field's edge; at rest
-- within the bound, in the balanced layout.

local function open(pivot: string): any
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 1100)
	app:command("map_only")
	app:set_state("control_map_force_pivot", pivot)
	app:set_state("control_map_force_open_folds", { ["orc-eng"] = true, ["orc-vpn"] = true, ["orc-ui"] = true })
	app:command("map_force_big")
	app:command("map_force")
	app:frames(90)
	return app
end
local function stats(app: any): any
	return app:state("control_map_force_stats") or {}
end
local function screen(app: any): { any }
	app:command("map_force_screen")
	return (app:state("force_screen") or {}).nodes or {}
end
local function settle(app: any)
	for _ = 1, 600 do
		app:frames(1)
		local st = stats(app)
		if (st.settled or 0) >= 1 and (st.over_moving or 0) == 0 then
			break
		end
	end
	app:frames(10)
end
local function is_hub(n: any): boolean
	return n.kind == "orchestrator" or n.kind == "machine" or n.kind == "brain"
end
local function field(app: any): (number, number)
	local line = string.match(app:tree("#orch-force-field", 0), "[^\n]+") or ""
	local w, h = string.match(line, " [%-%d%.]+,[%-%d%.]+ ([%d%.]+)x([%d%.]+)")
	return tonumber(w) or 0, tonumber(h) or 0
end
-- Hubs over each other, and marks past the edge, on screen now.
local function faults(app: any): (number, number)
	local w, h = field(app)
	local nodes = screen(app)
	local hubs = {}
	local out = 0
	for _, n in nodes do
		if is_hub(n) then
			table.insert(hubs, n)
		end
		if n.x - n.r < -1 or n.y - n.r < -1 or n.x + n.r > w + 1 or n.y + n.r > h + 1 then
			out += 1
		end
	end
	local over = 0
	for a = 1, #hubs do
		for b = a + 1, #hubs do
			local p, q = hubs[a], hubs[b]
			if math.sqrt((p.x - q.x) ^ 2 + (p.y - q.y) ^ 2) < p.r + q.r - 1 then
				over += 1
			end
		end
	end
	return over, out
end

for _, pv in { "orchestrator", "machine" } do
	test(`force unpin all ({pv}): every frame no hub over another, nothing past the edge; at rest within 600 ms`, function()
		local app = open(pv)
		settle(app)
		-- Two hubs dropped one onto the other; a crew's threads dragged to
		-- the field's left edge.
		local hubs, chats = {}, {}
		for _, n in screen(app) do
			if n.kind == "orchestrator" or n.kind == "machine" then
				table.insert(hubs, n.id)
			elseif n.kind == "chat" then
				table.insert(chats, n)
			end
		end
		expect(#hubs >= 2).to_be(true)
		app:set_state("force_drag_onto", { id = hubs[1], onto = hubs[2], dx = 6, dy = 4 })
		app:command("map_force_drag_onto")
		app:frames(30)
		table.sort(chats, function(a: any, b: any): boolean
			return a.x < b.x
		end)
		for k = 1, math.min(4, #chats) do
			app:set_state("force_drag_onto", { id = chats[#chats - k + 1].id, onto = chats[1].id, dx = -60, dy = 40 * k })
			app:command("map_force_drag_onto")
			app:frames(10)
		end
		settle(app)
		local pins = app:state(if pv == "machine" then "control_map_force_pins_machine" else "control_map_force_pins") or {}
		expect(next(pins) ~= nil).to_be(true)
		-- A relaunch: the dropped places kept, each its slot now.
		app = open(pv)
		settle(app)
		local before = stats(app)
		app:command("map_force_unpin_all")
		local frames, worst_over, worst_out = 0, 0, 0
		for f = 1, 2000 do
			app:frames(1)
			local over, out = faults(app)
			worst_over, worst_out = math.max(worst_over, over), math.max(worst_out, out)
			local st = stats(app)
			frames = f
			if (st.relayouts or 0) > (before.relayouts or 0) and (st.settled or 0) > (before.settled or 0) then
				break
			end
		end
		local st = stats(app)
		print(`zeron-zgx: UNPINALL {pv}: frames={frames} ms={st.relayout_ms} worst_over={worst_over} worst_out={worst_out}`)
		expect((st.relayouts or 0) > (before.relayouts or 0)).to_be(true)
		expect(worst_over).to_be(0)
		expect(worst_out).to_be(0)
		expect((st.relayout_ms or 1e9) <= 600).to_be(true)
		-- Rested: nothing moving, no pins left, still clear and inside.
		app:frames(30)
		expect(stats(app).over_moving or 0).to_be(0)
		local over, out = faults(app)
		expect(over).to_be(0)
		expect(out).to_be(0)
		expect(next(app:state(if pv == "machine" then "control_map_force_pins_machine" else "control_map_force_pins") or {})).to_be(nil)
		expect(app:error()).to_be_nil()
	end)
end
