-- At the end while a reply is written, the view glides to the end as the
-- reply grows rather than stepping a line (or a block) at a time, as Zeron
-- 0.2's transcript did; it says it's at the end all the while (no way back
-- to the latest shows), and lands there. (Nov 14: "scrolling doesn't feel
-- smooth, it feels line by line while streaming".)
-- On chat_fixture's `long_chat` (ui/long_chat_fixture.luau).

local function settle(app: any)
	app:wait("#never-shown", { timeout_ms = 250 })
	app:frames(10)
end

local function offset(app: any): number
	return app:state("chat.transcript.offset")
end

test("at the end, a reply being written is glided to as it grows", function()
	local app = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:frames(20)
	app:command("long_chat_sent")
	app:command("go_s7")
	app:frames(30)
	settle(app)
	app:type("role:textbox", "three and one sounds like the most optional option")
	app:key("Enter")
	app:frames(40)
	settle(app)
	-- Written past the view (the runway under the prompt filled).
	for _ = 1, 40 do
		app:command("long_chat_streams")
		app:frames(2)
	end
	settle(app)
	-- More of the reply, at a window's pace: the view moves over several
	-- frames each time. (Frames a refresh apart in real time:
	-- the glide steps on the clock, as a window's frames do, not on the
	-- test's frame count.)
	local before = offset(app)
	local seen: { number } = {}
	for _ = 1, 12 do
		app:command("long_chat_streams")
		for _ = 1, 6 do
			app:wait("#never-shown", { timeout_ms = 16 })
			app:frames(1)
			local now = offset(app)
			if now ~= (seen[#seen] or before) then
				table.insert(seen, now)
			end
		end
	end
	-- Many small moves forward; back only where the reply's end itself came
	-- up (a block settling shorter: the view at the end goes with it, as it
	-- did before it glided).
	local forward, back = 0, 0
	for i = 2, #seen do
		if seen[i] > seen[i - 1] then
			forward += 1
		else
			back += 1
		end
	end
	expect(forward >= 24).to_be(true)
	expect(back <= 2).to_be(true)
	-- At the end all the while: no way back to the latest.
	expect(app:has('role:button "Jump to latest"')).to_be(false)
	-- (A second and a half, as a window's frames come.)
	for _ = 1, 90 do
		app:wait("#never-shown", { timeout_ms = 16 })
		app:frames(1)
	end
	-- Landed on the end.
	local at = offset(app)
	app:scroll("#transcript", 1e6)
	app:frames(4)
	expect(offset(app) - at < 1).to_be(true)
end)
