-- Chats that share a title (the same name on each machine) are told apart by
-- their id in every list, and by where they run on their rows: Activity's
-- rows are compact (one line), and three "School Management" would read
-- alike. On the shell in ui/shell_fixture.luau (`twins`: sm1 on MacBook Pro
-- in Atlas, sm2 working on devbox in Borealis, sm3 awaiting input on
-- MacBook Pro in Atlas).

local SIDE = "#shell-sidebar"

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:set_state("screens/shell/activity.luau::shell_sidebar_activity", true)
	app:frames(40)
	app:command("twins")
	app:frames(40)
	return app
end

-- The sidebar's tree lines that aren't hidden.
local function shown_lines(app: any): { string }
	local out = {}
	for line in app:tree(SIDE):gmatch("[^\n]+") do
		if not line:find(" hidden", 1, true) then
			table.insert(out, line)
		end
	end
	return out
end

local function count(app: any, text: string): number
	local n = 0
	for _, line in shown_lines(app) do
		if line:find(text, 1, true) then
			n += 1
		end
	end
	return n
end

test("same title: each chat is a row of its own, and its row says where it runs", function()
	local app = open()
	-- Two in the list, one under way in Working (folded).
	expect(count(app, 'button "School Management"')).to_be(2)
	app:click(`{SIDE} >> role:button "Working 2"`)
	app:frames(40)
	expect(count(app, 'button "School Management"')).to_be(3)
	-- Told apart: where each runs is shown, though the rows are compact.
	expect(count(app, 'text "Atlas · MacBook Pro"')).to_be(2)
	expect(count(app, 'text "Borealis · devbox"') >= 1).to_be(true)
	-- A chat with a title of its own stays one line.
	expect(count(app, 'text "Atlas · MacBook Pro · docs"')).to_be(0)
end)

test("same title: a row opens its own chat, whatever shares its name", function()
	local app = open()
	app:click(`{SIDE} >> role:button "Working 2"`)
	app:frames(40)
	app:command("twin_finish")
	app:frames(40)
	-- The one on devbox finished: its row is the one saying Completed, and
	-- opens sm2, not the first of that name.
	app:click(`{SIDE} >> "Completed"`)
	app:frames(40)
	expect(app:state("fixture_open_chat")).to_be("sm2")
end)

test("same title: a move of one leaves the others as they were", function()
	local app = open()
	app:command("twin_sleep")
	app:frames(40)
	-- sm2 sleeps in Asleep (folded); sm1 and sm3 stay in the list with theirs.
	expect(count(app, 'button "Asleep 1"')).to_be(1)
	expect(count(app, 'button "School Management"')).to_be(2)
	expect(count(app, '"Awaiting Input"') >= 1).to_be(true)
end)

test("same title: a chat whose title is its own stays one line", function()
	local app = open()
	for _, line in shown_lines(app) do
		if line:find('button "Tidy the tests"', 1, true) then
			expect(line:find(" 256x30", 1, true) ~= nil).to_be(true)
		end
	end
	expect(count(app, 'button "Tidy the tests"')).to_be(1)
end)
