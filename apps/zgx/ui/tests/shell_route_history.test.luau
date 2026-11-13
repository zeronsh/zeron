-- Back steps through every place the window has been since launch, past
-- the 100 zgx's `ui.router` keeps unless told otherwise
-- (screens/shell/route's `history = math.huge`). On the shell in
-- ui/shell_fixture.luau.

local SIDE = "#shell-sidebar"
local MOVES = 120

test("Back goes more than 100 places back, to where the window started", function()
	local app: any = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	local start = app:state("shell_route_path")
	-- Two chats in turn: each click a new place.
	local chats = { "Fix the build", "Write the docs" }
	local paths = {}
	for i = 1, MOVES do
		app:click(`{SIDE} >> role:button "{chats[(i - 1) % 2 + 1]}"`)
		app:frames(2)
		paths[i] = app:state("shell_route_path")
	end
	expect(paths[MOVES] ~= paths[MOVES - 1]).to_be(true)
	-- Each step back lands on the place before, all the way.
	for i = MOVES - 1, 1, -1 do
		app:command("router.back")
		app:frames(2)
		expect(app:state("shell_route_path")).to_be(paths[i])
	end
	app:command("router.back")
	app:frames(2)
	expect(app:state("shell_route_path")).to_be(start)
	expect(app:error()).to_be_nil()
end)
