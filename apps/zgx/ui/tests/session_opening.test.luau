-- A chat opened shows its rows as they come, with no skeleton on the way:
-- one kept on this device comes in a moment from its copy here, and before
-- then nothing shows at all. The skeleton is for a chat that doesn't come
-- (none of it here), once it has been a while. On the chat fixture: its
-- `slow_open` brings a chat 300 ms after it's opened, its `never_open` never.

local function skeleton(app: any): boolean
	return app:has("#chat-session-loading")
end

local function opened(app: any, mode: string): any
	app:command(mode)
	app:frames(2)
	app:command("go_s2")
	return app
end

test("a chat that comes in a moment opens with no skeleton on its way", function()
	local app: any = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:frames(20)
	opened(app, "slow_open")
	-- Every frame until it has come (300 ms) and a while after: no skeleton.
	for _ = 1, 40 do
		app:frames(1)
		expect(skeleton(app)).to_be(false)
	end
	expect(app:has("#transcript")).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("a chat that doesn't come shows its skeleton once it has been a while", function()
	local app: any = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:frames(20)
	opened(app, "never_open")
	-- A moment: nothing at all.
	app:frames(12)
	expect(skeleton(app)).to_be(false)
	-- A while (past 400 ms): it says it's loading.
	app:frames(30)
	expect(skeleton(app)).to_be(true)
	expect(app:has("#transcript")).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("a chat switched to that's here shows its rows on the first frame", function()
	local app: any = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:frames(20)
	app:command("go_s1")
	app:frames(30)
	for _, chat in { "go_s2", "go_s1", "go_s2" } do
		app:command(chat)
		-- The very first frame after the switch: its rows, no skeleton, nothing blank.
		app:frames(1)
		expect(skeleton(app)).to_be(false)
		expect(app:has("#transcript")).to_be(true)
		expect(string.find(app:tree("#transcript"), 'text "', 1, true) ~= nil).to_be(true)
	end
	expect(app:error()).to_be_nil()
end)
