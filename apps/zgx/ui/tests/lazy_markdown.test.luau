-- A long reply landing whole (chat_fixture's `long_reply`: 400 blocks, about
-- 25 KB) is parsed lazily (ds/markdown's windowed path, ui.markdown's
-- `lazy`): its end shows at once, the blocks not parsed yet hold their
-- place, and they come in over the next frames, so no frame parses all of
-- it. Reading up into it soon after, its words are there.

local function wait(app: any, ms: number)
	app:wait("#never-shown", { timeout_ms = ms })
end

local function long_chat(): any
	local app = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:command("go_s6")
	app:frames(20)
	wait(app, 300)
	app:frames(5)
	return app
end

test("lazy markdown: a long reply landing whole shows its end at once", function()
	local app = long_chat()
	app:command("long_reply")
	app:frames(3)
	-- Its last block, where the transcript rests, drawn the frame it lands
	-- (the blocks before it hold their places meanwhile)…
	expect(app:has('~"fn part_100"')).to_be(true)
	-- …and the ones before it a few frames on, the end first.
	wait(app, 120)
	app:frames(10)
	expect(app:has('~"Part 100 explains"')).to_be(true)
	expect(app:has('~"Part 99 explains"')).to_be(true)
end)

test("lazy markdown: its earlier blocks are drawn once parsed, read up into soon after", function()
	local app = long_chat()
	app:command("long_reply")
	app:frames(3)
	-- Up into it at once, a screen at a time: what comes into view is its
	-- words, not held places, once they've had a few frames to parse.
	for _ = 1, 6 do
		app:scroll("#transcript", -500)
		app:frames(3)
		wait(app, 30)
	end
	wait(app, 200)
	app:frames(20)
	local shown = 0
	for i = 1, 100 do
		if app:has(`~"Part {i} explains"`) then
			shown += 1
		end
	end
	-- (A screen and its overscan: several parts, all as words.)
	expect(shown >= 3).to_be(true)
	expect(app:has('~"Part 100 explains"')).to_be(false)
end)
