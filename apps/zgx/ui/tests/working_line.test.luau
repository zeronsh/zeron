-- The working line (ds/working): its words and clock as before, all lines
-- on one clock, and the line a box of a set size, so what changes in it
-- (its clock, its word rolling, its ring) lays out the line alone, never
-- the transcript or the crew around it.

local NOW = "ds/working.luau::ds_working_now"

test("working line: its clock and word say what they did, a second at a time", function()
	local app = mount("working_fixture.luau")
	app:frames(5)
	-- 65 s in: "1m 5s", the word for 65 // 7 = 9 turns past its seed.
	-- 65 s in, seed 3: the 13th word, (3 + 65 // 7) % 21 + 1, and "1m 5s".
	local tree = app:tree("#fixture-transcript")
	expect(tree).to_contain("Sketching…")
	expect(tree).to_contain("1m 5s")
	-- Five seconds on (its turn begun five seconds earlier) and its clock's next step: "1m 10s",
	-- and the word turns at 70 s, to the 14th.
	app:set_state("working_fixture_since", app:state("working_fixture_since") - 5)
	app:frames(61)
	tree = app:tree("#fixture-transcript")
	expect(tree).to_contain("1m 10s")
	expect(tree).to_contain("Plotting…")
end)

test("working line: every line steps on the one clock, once a second", function()
	local app = mount("working_fixture.luau")
	app:frames(5)
	local seen: { [number]: boolean } = {}
	local n = 0
	for _ = 1, 20 do
		app:frames(15) -- a quarter second
		local t = app:state(NOW)
		if not seen[t] then
			seen[t] = true
			n += 1
		end
	end
	-- Five seconds: about five writes, whatever the number of lines.
	expect(n >= 4).to_be(true)
	expect(n <= 7).to_be(true)
end)

test("working line: the line is its room's width at its own height (a box laid out alone)", function()
	local app = mount("working_fixture.luau")
	app:frames(10)
	local tree = app:tree("#fixture-transcript")
	-- The line's row (.ds-working) and the room it stands in.
	local line = string.match(tree, "[^\n]*%.ds%-working[^\n]*") or string.match(tree, "[^\n]*Working[^\n]*")
	expect(line ~= nil).to_be(true)
	local w, h = string.match(line :: string, "(%d+)x(%d+)")
	expect(tonumber(h)).to_be(26)
	expect(tonumber(w) > 200).to_be(true)
end)
