-- Switching chats on the session page (ui/chat_fixture.luau, the engine
-- stubbed): the transcript stays, only its rows are made for each chat
-- shown; and everything that is one chat's own stays that chat's across
-- switch after switch, back and forth: where it was read, the way back to
-- its latest, its draft, its model, and where a send goes.

local function settle(app: any)
	app:frames(30)
end

local function open(app: any, id: string)
	app:command(`go_{id}`)
	settle(app)
end

-- A node's identity in the tree, by a pattern on its line: `@id`.
local function node(app: any, pattern: string): string?
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(pattern, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

-- The first message's words at the view's top, and where (as
-- chat_in_place.test.luau reads it).
local function top(app: any): string
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local y = tonumber(line:match(" %-?%d+,(%-?%d+) "))
		if line:find('text "', 1, true) and y and y >= 38 then
			return `{line:match('text "([^"]*)"')} @{y}`
		end
	end
	return "?"
end

-- The chip's words, as they show (chat_catalog_device.test.luau's).
local function chip_text(app: any, name: string): string
	local out = {}
	for line in app:tree(`role:button "{name}"`):gmatch("[^\n]+") do
		if not line:find(" hidden", 1, true) then
			local text = line:match('text "([^"]+)"') or line:match('#[%w%-]+ "([^"]+)"')
			if text then
				table.insert(out, text)
			end
		end
	end
	return table.concat(out, " ")
end

-- (Where a scroll settles is written once it has rested: let it.)
local function rest(app: any)
	app:wait("#never-shown", { timeout_ms = 400 })
end

test("another chat swaps the transcript's rows; the transcript itself stays", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	expect(app:wait('~"Question number 30"', { timeout_ms = 2000 })).to_be(true)
	local rail = node(app, "#transcript-rail")
	local list = node(app, "#transcript ")
	expect(rail).never.to_be_nil()
	expect(list).never.to_be_nil()
	open(app, "s2")
	expect(app:wait('~"Hi! What shall we work on?"', { timeout_ms = 2000 })).to_be(true)
	expect(app:has('~"Question number 30"')).to_be(false)
	-- The same scrollbar; a list of its own for the chat shown.
	expect(`rail @{node(app, "#transcript-rail")}`).to_be(`rail @{rail}`)
	expect(`list @{node(app, "#transcript ")}`).never.to_be(`list @{list}`)
	open(app, "s1")
	expect(app:wait('~"Question number 30"', { timeout_ms = 2000 })).to_be(true)
	expect(app:has('~"Hi! What shall we work on?"')).to_be(false)
	expect(`rail @{node(app, "#transcript-rail")}`).to_be(`rail @{rail}`)
	expect(app:error()).to_be(nil)
end)

test("each chat keeps where it was read and its way back, switch after switch", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	-- s1 read partway up, s7 (tall replies) further up, s2 at its end.
	open(app, "s1")
	app:scroll("#transcript", -900)
	settle(app)
	rest(app)
	local s1 = top(app)
	expect(s1).never.to_be("?")
	open(app, "s7")
	app:scroll("#transcript", -2500)
	settle(app)
	rest(app)
	local s7 = top(app)
	expect(s7).never.to_be("?")
	open(app, "s2")
	rest(app)
	for round = 1, 6 do
		open(app, "s1")
		rest(app)
		expect(`{round}: s1 {top(app)}`).to_be(`{round}: s1 {s1}`)
		expect(app:wait('role:button "Jump to latest"', { timeout_ms = 1000 })).to_be(true)
		open(app, "s7")
		rest(app)
		expect(`{round}: s7 {top(app)}`).to_be(`{round}: s7 {s7}`)
		expect(app:wait('role:button "Jump to latest"', { timeout_ms = 1000 })).to_be(true)
		open(app, "s2")
		rest(app)
		-- At its end, all of it shown, with no way back to show.
		expect(app:has('~"Hi! What shall we work on?"')).to_be(true)
		expect(app:wait('role:button "Jump to latest"', { gone = true, timeout_ms = 1000 })).to_be(true)
	end
	-- Taking s1's way back there leaves s7's place as it was.
	open(app, "s1")
	app:click('role:button "Jump to latest"')
	settle(app)
	expect(app:wait('~"Question number 30"', { timeout_ms = 2000 })).to_be(true)
	rest(app)
	open(app, "s7")
	rest(app)
	expect(top(app)).to_be(s7)
	open(app, "s1")
	rest(app)
	expect(app:has('~"Question number 30"')).to_be(true)
	expect(app:wait('role:button "Jump to latest"', { gone = true, timeout_ms = 1000 })).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("drafts, models and sends stay with their chat, back and forth", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s1")
	app:type('role:textbox "Message"', "Draft for one")
	settle(app)
	open(app, "s2")
	app:type('role:textbox "Message"', "Draft for two")
	settle(app)
	for round = 1, 6 do
		open(app, "s1")
		expect(`{round}: {app:text('role:textbox "Message"')}`).to_be(`{round}: Draft for one`)
		expect(chip_text(app, "Model")).to_contain("Opus")
		open(app, "s2")
		expect(`{round}: {app:text('role:textbox "Message"')}`).to_be(`{round}: Draft for two`)
		expect(chip_text(app, "Model")).to_contain("Sonnet")
	end
	-- Sent from s2, after all that: to s2, shown in s2, and s1's draft stays.
	app:key("Enter")
	settle(app)
	app:wait("#never-shown", { timeout_ms = 700 })
	local calls = app:state("calls")
	expect(calls).to_contain("send:s2:Draft for two")
	expect(calls).never.to_contain("send:s1:")
	expect(app:has('~"Draft for two"')).to_be(true)
	open(app, "s1")
	expect(app:text('role:textbox "Message"')).to_be("Draft for one")
	expect(app:has('~"Draft for two"')).to_be(false)
	expect(app:error()).to_be(nil)
end)

test("a chat left mid-glide: its prompt goes to it, the next chat's view stays its own", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	open(app, "s7")
	app:scroll("#transcript", -2500)
	settle(app)
	rest(app)
	local s7 = top(app)
	open(app, "s1")
	app:type('role:textbox "Message"', "Sent as I left")
	app:key("Enter")
	-- Away before its prompt has glided to the top.
	app:frames(1)
	app:command("go_s7")
	settle(app)
	app:wait("#never-shown", { timeout_ms = 700 })
	expect(app:state("calls")).to_contain("send:s1:Sent as I left")
	-- s7 opens where it was read, not moved by s1's glide.
	expect(top(app)).to_be(s7)
	expect(app:has('~"Sent as I left"')).to_be(false)
	-- And back again: each its own still.
	open(app, "s1")
	open(app, "s7")
	rest(app)
	expect(top(app)).to_be(s7)
	expect(app:error()).to_be(nil)
end)

-- The scrollbar shows while the view is scrolled, not as another chat's
-- rows place themselves: the transcript staying, a switch was taken for a
-- scroll, and the bar lit and faded over some 60 frames after every one.
test("a switch doesn't show the scrollbar; a scroll does", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	local function rail_shows(): boolean
		local line = app:tree("#transcript-rail"):match("[^\n]+") or ""
		return not line:find(" hidden", 1, true)
	end
	open(app, "s1")
	rest(app)
	expect(rail_shows()).to_be(false)
	for _, id in { "s7", "s1", "s7", "s2", "s1" } do
		app:command(`go_{id}`)
		app:frames(3)
		expect(`{id}: {rail_shows()}`).to_be(`{id}: false`)
		rest(app)
		expect(`{id}: {rail_shows()}`).to_be(`{id}: false`)
	end
	app:scroll("#transcript", -300)
	app:frames(3)
	expect(rail_shows()).to_be(true)
	expect(app:error()).to_be(nil)
end)
