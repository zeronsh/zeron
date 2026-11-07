-- An engine this app can start that hasn't answered in a minute: the page
-- says Zeron can't start its engine, with Retry, which goes back to loading.
-- An engine that answers clears it by itself. Before that minute, nothing is
-- said. (The minute itself is the host's: src/engine/mod.rs `StartWatch`.)
-- On the shell in ui/shell_fixture.luau, and setup's window in
-- ui/engine_start_fixture.luau.

local ERROR = "#engine-start-failed"

local function shell(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	return app
end

local function setup(): any
	local app = mount("engine_start_fixture.luau")
	app:resize(560, 640)
	app:frames(30)
	return app
end

for _, case in { { "the main window", shell }, { "setup's window", setup } } do
	local where, open = case[1], case[2]

	test(`{where}: nothing is said while the engine is on its way`, function()
		local app = open()
		expect(app:has(ERROR)).to_be(false)
	end)

	test(`{where}: an engine that can't start is said so, plainly`, function()
		local app = open()
		app:command("fixture.engine_cant_start")
		app:frames(40)
		expect(app:has(ERROR)).to_be(true)
		expect(app:tree(ERROR):find("Zeron can't start its engine", 1, true) ~= nil).to_be(true)
	end)

	test(`{where}: Retry tries again and goes back to loading`, function()
		local app = open()
		app:command("fixture.engine_cant_start")
		app:frames(40)
		app:click(`{ERROR} >> role:button "Try again"`)
		app:frames(40)
		expect(app:state("fixture_engine_retries")).to_be(1)
		expect(app:has(ERROR)).to_be(false)
	end)

	test(`{where}: an engine that answers late clears it`, function()
		local app = open()
		app:command("fixture.engine_cant_start")
		app:frames(40)
		expect(app:has(ERROR)).to_be(true)
		app:command("fixture.engine_started")
		app:frames(40)
		expect(app:has(ERROR)).to_be(false)
	end)
end
