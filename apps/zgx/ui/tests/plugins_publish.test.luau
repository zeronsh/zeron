-- Yours and publishing (screens/plugins/publish, screens/plugins/keys), on
-- the page in ui/plugins_store_fixture.luau, the engine's store calls
-- answered by ui/store_stub.luau: each of yours says where it stands in
-- the store, Publish… checks it here and says what to fix, Submit sends
-- exactly what was checked; each version's review and what you can still
-- do with it; deploy keys made after a fresh sign-in, the secret shown
-- once, and revoked.

local function calls(app: any): string
	local ok, list = pcall(function()
		return app:state("store_stub_calls")
	end)
	return if ok and list then table.concat(list, "\n") else ""
end

local function settle(app: any, ms: number?)
	app:wait("#never-there", { timeout_ms = ms or 300 })
	app:frames(10)
end

local function yours(): any
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	app:click('role:tab "Yours"')
	settle(app)
	return app
end

local function publishing(): any
	local app = yours()
	app:click('role:button "Publish…"')
	app:wait("#publish-checks", { timeout_ms = 3000 })
	settle(app)
	return app
end

test("each of yours in Yours says where it stands in the store, with Publish…", function()
	local app = yours()
	local tree = app:tree()
	expect(tree).to_contain("1.0.0 in the plugin store")
	expect(app:has('role:button "Publish…"')).to_be(true)
	expect(calls(app)).to_contain("mine")
	-- Installed is as it was: no store line there, no Publish….
	app:click('role:tab "Installed"')
	settle(app)
	expect(app:has('role:button "Publish…"')).to_be(false)
end)

test("Publish… checks it here, and says what to fix before it can be sent", function()
	local app = publishing()
	expect(calls(app)).to_contain("prepare:pomodoro-clock")
	local checks = app:tree("#publish-checks")
	expect(checks).to_contain("Add README.md to its folder")
	expect(checks).to_contain("Add an SVG icon")
	expect(checks).to_contain("newer than 1.0.0")
	expect(app:tree('#publish-submit')).to_contain("disabled")
	expect(app:tree()).to_contain("CHANGELOG.md has nothing for this version")
	-- Fixed in its folder, and checked again: ready to send.
	app:command("store_stub.fixed")
	app:click('role:button "Check again"')
	settle(app)
	expect(app:tree("#publish-checks")).never.to_contain("Add README.md")
	expect(app:tree("#publish-submit")).never.to_contain("disabled")
	expect(app:tree("#publish-submit")).to_contain("Submit 1.2.0 for review")
end)

test("Submit sends what was checked with its ticket, and the version shows In review", function()
	local app = publishing()
	app:command("store_stub.fixed")
	app:click('role:button "Check again"')
	settle(app)
	app:click("#publish-submit")
	settle(app)
	expect(calls(app)).to_contain("publish:pomodoro-clock:pomodoro-clock-1.2.0:ticket-1.2.0")
	expect(app:tree()).to_contain("In review")
end)

test("each version says where its review stands, and the reviewer's words when it wasn't approved", function()
	local app = publishing()
	local tree = app:tree()
	expect(tree).to_contain("Not approved")
	expect(tree).to_contain("It reads every chat")
	expect(tree).to_contain("Approved")
	expect(app:has('role:button "Stop new installs"')).to_be(true)
end)

test("stopping new installs asks first, then the version says so and can allow them again", function()
	local app = publishing()
	app:scroll("#fixture-scroll", 600)
	app:frames(10)
	app:click('role:button "Stop new installs"')
	settle(app)
	expect(app:tree("role:dialog")).to_contain("Stop new installs of Pomodoro clock 1.0.0?")
	expect(calls(app)).never.to_contain("version:")
	app:click('role:dialog >> role:button "Stop new installs"')
	settle(app)
	expect(calls(app)).to_contain("version:pomodoro-clock:1.0.0:yank")
	expect(app:tree()).to_contain("No new installs")
	app:click('role:button "Allow installs again"')
	settle(app)
	expect(calls(app)).to_contain("version:pomodoro-clock:1.0.0:unyank")
end)

test("a deploy key is made after a fresh sign-in, its secret shown once to copy", function()
	local app = publishing()
	expect(app:tree()).to_contain("GitHub Actions")
	expect(app:tree()).to_contain("last used")
	app:scroll("#fixture-scroll", 2000)
	app:frames(10)
	app:click("#keys-new")
	settle(app)
	app:type('role:dialog >> role:textbox "Name"', "Release job")
	app:click('role:dialog >> role:button "Create key"')
	settle(app)
	-- Waiting for the browser's sign-in: said, and it can be given up.
	expect(app:tree("role:dialog")).to_contain("Sign in in your browser")
	expect(calls(app)).to_contain("create_key:pomodoro-clock:Release job:expires")
	app:command("store_stub.signed_in_again")
	settle(app, 400)
	local dialog = app:tree("role:dialog")
	expect(dialog).to_contain("Copy your key now")
	expect(dialog).to_contain("zpk_live_")
	app:click('role:dialog >> role:button "Copy"')
	settle(app)
	expect(app:clipboard()).to_contain("zpk_live_")
	app:click('role:dialog >> role:button "I\'ve copied it"')
	settle(app)
	-- Gone for good: never shown again.
	expect(app:tree()).never.to_contain("zpk_live_")
	expect(app:tree()).to_contain("Release job")
end)

test("waiting on the sign-in can be stopped", function()
	local app = publishing()
	app:scroll("#fixture-scroll", 2000)
	app:frames(10)
	app:click("#keys-new")
	settle(app)
	app:type('role:dialog >> role:textbox "Name"', "CI")
	app:click('role:dialog >> role:button "Create key"')
	settle(app)
	app:click('role:dialog >> role:button "Stop waiting"')
	settle(app)
	expect(calls(app)).to_contain("cancel_step_up")
end)

test("revoking a key asks first, then it's gone", function()
	local app = publishing()
	app:scroll("#fixture-scroll", 2000)
	app:frames(10)
	app:click('role:button "Revoke"')
	settle(app)
	expect(app:tree("role:dialog")).to_contain("Revoke GitHub Actions?")
	app:click('role:dialog >> role:button "Revoke"')
	settle(app)
	expect(calls(app)).to_contain("revoke_key:pomodoro-clock:k_gh")
	expect(app:tree()).never.to_contain("GitHub Actions")
end)

test("Back from publishing is Yours", function()
	local app = publishing()
	app:click('role:button "Back to Plugins"')
	settle(app)
	expect(app:has('role:button "Publish…"')).to_be(true)
end)
