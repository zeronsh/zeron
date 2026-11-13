-- A session's "how long ago" moves on by itself as time passes, nothing
-- else about it changing (ds/status `since`: one clock every corner reads).
-- On the shell in ui/shell_fixture.luau: "Write the docs" moved an hour
-- ago, "Tidy the tests" two.

local CLOCK = "ds/status.luau::ds_status_clock"

local function corner(app: any): string
	return app:tree("#shell-sidebar")
end

test("a row's time moves on as the clock does, with nothing else changing", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	local before = corner(app)
	expect(before).to_contain('text "1h"')
	expect(before).to_contain('text "2h"')
	-- Two hours on: the same rows, nothing else written.
	app:set_state(CLOCK, os.time() + 2 * 3600)
	app:frames(5)
	local after = corner(app)
	expect(after).to_contain('text "3h"')
	expect(after).to_contain('text "4h"')
	expect(after:find('text "1h"', 1, true) == nil).to_be(true)
	expect(app:error()).to_be_nil()
end)
