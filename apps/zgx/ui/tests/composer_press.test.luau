-- A press anywhere on the composer that isn't one of its controls gives its
-- draft the keys (ds/composer): the pill's padding, the blank room in its
-- controls' rows, under the draft's last line. A control pressed does its
-- own thing. On the chat page (ui/chat_fixture.luau), and a new chat.

local BOX = 'role:textbox "Message"'

local function settle(app: any)
	app:frames(20)
end

local function has_keys(app: any): boolean
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(" focused", 1, true) then
			return line:find('textbox "Message"', 1, true) ~= nil
		end
	end
	return false
end

-- The keys away from the draft first (Escape in it gives them back).
local function away(app: any)
	app:click(BOX)
	app:key("Escape")
	settle(app)
	expect(has_keys(app)).to_be(false)
end

local function chat(go: string): any
	local app = mount("chat_fixture.luau")
	app:resize(1000, 800)
	app:command(go)
	settle(app)
	return app
end

for _, go in { "go_s2", "go_home" } do
	test(`{go}: a press on the composer's blank room gives the draft the keys`, function()
		local app = chat(go)
		-- (A session's toolbar is blank in its middle; a new chat's has its
		-- "work in" chips there, controls of their own.)
		local targets = if go == "go_s2" then { "#chat-composer", "#composer-toolbar" } else { "#chat-composer" }
		for _, target in targets do
			away(app)
			app:click(target)
			settle(app)
			expect(`{target}: {has_keys(app)}`).to_be(`{target}: true`)
		end
	end)
end

test("a control pressed does its own thing, not the draft's", function()
	local app = chat("go_s2")
	away(app)
	app:click('role:button "Access"')
	settle(app)
	expect(has_keys(app)).to_be(false)
	app:key("Escape")
	settle(app)
	away(app)
	app:click('role:button "Attach"')
	settle(app)
	expect(app:error()).to_be_nil()
end)
