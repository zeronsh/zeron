-- A new chat's composer stands by its top: what's written in it grows it down,
-- moving nothing above, unless it would run off the page's foot.

local function top_and_height(app: any): (number, number)
	local line = app:tree("#chat-composer"):match("[^\n]+") or ""
	local y, h = line:match(" %-?%d+,(%-?%d+) %d+x(%d+)")
	return tonumber(y) or 0, tonumber(h) or 0
end

local function new_chat(w: number, h: number): any
	local app = mount("chat_fixture.luau")
	app:resize(w, h)
	app:frames(30)
	app:command("go_home")
	app:frames(60)
	return app
end

local function draft(lines: number): string
	local out = {}
	for i = 1, lines do
		table.insert(out, `Line {i} of what's being written`)
	end
	return table.concat(out, "\n")
end

test("a new chat's composer keeps its top as its draft grows and shrinks", function()
	local app = new_chat(1000, 800)
	local top, tall = top_and_height(app)
	app:type('role:textbox "Message"', draft(4))
	for _ = 1, 12 do
		app:frames(2)
		local now = top_and_height(app)
		expect(math.abs(now - top) <= 1).to_be(true)
	end
	local grown_top, grown = top_and_height(app)
	expect(grown > tall).to_be(true)
	expect(math.abs(grown_top - top) <= 1).to_be(true)
	app:frames(30)
	-- All of it deleted: it eases back, its top still, however long it's left.
	app:set_state("chat.composer.text", "")
	for _ = 1, 40 do
		app:frames(1)
		local now = top_and_height(app)
		expect(math.abs(now - top) <= 1).to_be(true)
	end
	app:wait("#never-shown", { timeout_ms = 700 })
	local back_top, back = top_and_height(app)
	expect(math.abs(back_top - top) <= 1).to_be(true)
	expect(math.abs(back - tall) <= 1).to_be(true)
end)

test("a draft that would run off the foot moves it up by as much, and stays on the page", function()
	local app = new_chat(1000, 420)
	local top = top_and_height(app)
	app:type('role:textbox "Message"', draft(14))
	app:frames(40)
	local now, tall = top_and_height(app)
	expect(now < top).to_be(true)
	expect(now + tall <= 420).to_be(true)
end)

test("what a draft grows it down to still takes the pointer, and a send goes as ever", function()
	local app = new_chat(1000, 800)
	local top = top_and_height(app)
	app:type('role:textbox "Message"', draft(5))
	app:frames(40)
	-- (The footer's chips stand below where it was empty: past its row.)
	app:click('role:button "Model"')
	app:frames(20)
	expect(app:has('role:option "Sonnet"') or app:has('~"Reasoning"') or app:has('~"REASONING"')).to_be(true)
	app:key("Escape")
	app:frames(30)
	app:click('role:button "Send message"')
	app:frames(90)
	expect(app:state("calls")).to_contain("send")
	-- Docked at the foot, as a session's.
	local docked = top_and_height(app)
	expect(docked > top).to_be(true)
end)

test("the page resizing moves it with the page: there at once, no glide and no overshoot", function()
	local app = new_chat(1000, 800)
	for _, h in { 780, 740, 700, 660, 720, 790 } do
		app:resize(1000, h)
		app:frames(2)
		local at = top_and_height(app)
		for _ = 1, 12 do
			app:frames(2)
			expect(math.abs(top_and_height(app) - at) <= 1).to_be(true)
		end
	end
end)
