-- A tool row opens to what it did (ds/tool_detail through ds/activity;
-- ui/tool_detail_fixture.luau): an edit to its files' counts, and its
-- whole diff when asked; a command to its output, cut at 24 lines, and its
-- whole output when asked (a failure says so, and a tap tries again); a
-- read to its result; an older doc's edit to its diff at once. What was
-- fetched is kept: opened again, or the work made again as a reload makes
-- it, nothing is fetched twice.

local function work(): any
	local app = mount("tool_detail_fixture.luau")
	app:resize(900, 1600)
	app:frames(10)
	expect(app:error()).to_be_nil()
	-- (Settled work folds to its line: open it.)
	app:click('role:button "Read 1 file · ran 1 command · edited 2 files"')
	app:frames(30)
	return app
end

local function fetched(app: any): number
	local n = 0
	for _ in string.gmatch(app:state("test_tool_fetches") or "", "[^\n]+") do
		n += 1
	end
	return n
end

local function open_row(app: any, label: string)
	app:click(`role:button "{label}"`)
	app:frames(20)
end

test("an edit opens to its files' counts, and to its whole diff when asked", function()
	local app = work()
	open_row(app, "Edited app.rs")
	expect(app:has('.tool-stats >> ~"src/app.rs"')).to_be(true)
	expect(app:has('role:button "Show full diff"')).to_be(true)
	expect(fetched(app)).to_be(0)
	app:click('role:button "Show full diff"')
	app:frames(20)
	expect(app:has('.tool-diff >> ~"new_call();"')).to_be(true)
	expect(app:has('.tool-diff >> ~"old_call();"')).to_be(true)
	expect(app:has('role:button "Show full diff"')).to_be(false)
	expect(fetched(app)).to_be(1)
end)

test("a command opens to its output, cut at 24 lines, and to all of it when asked; a failure says so and tries again", function()
	local app = work()
	app:set_state("test_tool_fail", true)
	open_row(app, "Ran cargo test")
	expect(app:has('.tool-output >> ~"line 24"')).to_be(true)
	expect(app:has('.tool-output >> ~"line 25"')).to_be(false)
	expect(app:has('~"… 6 more lines"')).to_be(true)
	app:click('role:button "Show full output (13 KB)"')
	app:frames(20)
	expect(app:has('role:button "Couldn\'t load full output — tap to retry"')).to_be(true)
	app:click('role:button "Couldn\'t load full output — tap to retry"')
	app:frames(20)
	expect(app:has('.tool-output >> ~"line 30"')).to_be(true)
	expect(app:has('~"more lines"')).to_be(false)
	expect(fetched(app)).to_be(2)
end)

test("a read opens to its result; an older doc's edit to its diff at once", function()
	local app = work()
	open_row(app, "Read notes.md")
	expect(app:has('.tool-output >> ~"The first line."')).to_be(true)
	open_row(app, "Edited old.txt")
	expect(app:has('.tool-diff >> ~"after"')).to_be(true)
	expect(fetched(app)).to_be(0)
end)

test("what was fetched is kept: the work made again (as a reload makes it) fetches nothing twice", function()
	local app = work()
	open_row(app, "Edited app.rs")
	app:click('role:button "Show full diff"')
	app:frames(20)
	expect(fetched(app)).to_be(1)
	app:set_state("test_tool_generation", 2)
	app:frames(30)
	-- (Opened rows and the open group are kept by id too.)
	expect(app:has('.tool-diff >> ~"new_call();"')).to_be(true)
	expect(fetched(app)).to_be(1)
end)
