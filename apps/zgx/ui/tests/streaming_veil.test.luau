-- A reply streaming in fades its words in (ds/markdown's veil), links and
-- inline code too: as each fade ends, its colour is the run's own ink again,
-- which for a link or inline code is a function (the accent's, ds/look):
-- read, not handed on as the colour. (Handed on, each such fade's end was a
-- script error.)

test("a streaming reply's links and inline code fade in and settle without an error", function()
	local parts = {}
	for i = 1, 30 do
		table.insert(parts, `The plan costs $5 and $10 a seat, or \\$3 with the discount; the bound is $O(n \\log n)$ for part {i}, and **the rest** follows from it. A [link](https://example.com/{i}) and \`inline code\` too.\n\n`)
	end
	local text = table.concat(parts)
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:frames(2)
	app:command("go_s2")
	app:frames(30)
	app:command("start_block_reply")
	app:frames(5)
	for n = 24, #text + 23, 24 do
		local now = string.sub(text, 1, math.min(n, #text))
		app:set_state("chat.draft", { id = "live", text = now, part = "t", part_text = now })
		app:frames(1)
		expect(app:error()).to_be(nil)
	end
	app:frames(30)
	expect(app:error()).to_be(nil)
	expect(app:has('~"for part 30,"')).to_be(true)
end)
