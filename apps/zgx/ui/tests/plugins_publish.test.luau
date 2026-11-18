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
	app:click('role:link "Publish…"')
	app:wait("#publish-checks", { timeout_ms = 3000 })
	settle(app)
	return app
end

test("each of yours in Yours says where it stands in the store, with Publish…", function()
	local app = yours()
	local tree = app:tree()
	-- What people install, and the newest past it that's yours to hear
	-- about (1.1.0 wasn't approved).
	expect(tree).to_contain("1.0.0 in the plugin store · 1.1.0 not approved")
	expect(app:has('role:link "Publish…"')).to_be(true)
	expect(calls(app)).to_contain("mine")
	-- Installed is as it was: no store line there, no Publish….
	app:click('role:tab "Installed"')
	settle(app)
	expect(app:has('role:link "Publish…"')).to_be(false)
end)

test("Publish… checks it here, and says what to fix before it can be sent", function()
	local app = publishing()
	expect(calls(app)).to_contain("prepare:pomodoro-clock")
	local checks = app:tree("#publish-checks")
	expect(checks).to_contain("Add README.md to its folder")
	expect(checks).to_contain("Add an SVG icon")
	-- (Newer than every version sent: the engine asked the store, and it is.)
	expect(checks).to_contain("Newer than the last")
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
	-- Yours says it too: the newest is the one in review now.
	app:click('role:button "Back to Plugins"')
	settle(app)
	expect(app:tree()).to_contain("1.0.0 in the plugin store · 1.2.0 in review")
end)

test("a plugin whose versions were all turned down never reads as in review", function()
	local app = yours()
	app:set_state("store_mine", (function()
		local now = app:state("store_mine")
		local p = table.clone(now.plugins["pomodoro-clock"])
		local only = {}
		for _, v in p.versions do
			if v.version == "1.1.0" then
				table.insert(only, v)
			end
		end
		p.versions = only
		return { state = now.state, plugins = { ["pomodoro-clock"] = p } }
	end)())
	settle(app)
	expect(app:tree()).to_contain("1.1.0 not approved")
	expect(app:tree()).never.to_contain("In review for the plugin store")
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
	expect(app:tree()).to_contain("Last used")
	-- (The key itself, short, after its name: as the store's site shows it.)
	expect(app:tree(".key-short")).to_contain("k_gh")
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
	-- Escape doesn't lose it: only its button closes it.
	app:key("Escape")
	settle(app)
	expect(app:tree("role:dialog")).to_contain("zpk_live_")
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
	expect(app:has('role:link "Publish…"')).to_be(true)
end)

test("newer than every version sent is a check of its own, or the engine's note when it couldn't ask", function()
	local app = yours()
	app:command("store_stub.not_newer")
	app:click('role:link "Publish…"')
	app:wait("#publish-checks", { timeout_ms = 3000 })
	settle(app)
	expect(app:tree("#publish-checks")).to_contain("Give it a version newer than every one")
	app:command("store_stub.store_unchecked")
	app:click('role:button "Check again"')
	settle(app)
	expect(app:tree("#publish-checks")).to_contain("Couldn't ask the store which versions")
	-- Not asked, no newer-than answer: only the note.
	expect(app:tree("#publish-checks")).never.to_contain("Give it a version newer")
end)

test("an id the store keeps for its own pages says so, and nothing is sent", function()
	local app = yours()
	-- (Its one problem.)
	app:command("store_stub.fixed")
	app:command("store_stub.id_reserved")
	app:click('role:link "Publish…"')
	app:wait("#publish-checks", { timeout_ms = 3000 })
	settle(app)
	expect(app:tree("#publish-checks")).to_contain("This id is taken")
	-- (Zeron's words, not the engine's.)
	expect(app:tree("#publish-checks")).never.to_contain("pick another id")
	expect(app:tree("#publish-submit")).to_contain("disabled")
end)

test("handed a newer-than problem it can't have (the store not asked), the page still shows only the note", function()
	local app = yours()
	app:set_state("store_stub_force_problem", true)
	app:command("store_stub.store_unchecked")
	app:command("store_stub.not_newer")
	app:click('role:link "Publish…"')
	app:wait("#publish-checks", { timeout_ms = 3000 })
	settle(app)
	expect(app:tree("#publish-checks")).to_contain("Couldn't ask the store which versions")
	expect(app:tree("#publish-checks")).never.to_contain("Give it a version newer")
end)

test("a deploy key waits for the first version, sent from this computer", function()
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	app:command("store_stub.never_published")
	app:click('role:tab "Yours"')
	settle(app)
	expect(app:tree()).to_contain("Not in the plugin store")
	app:click('role:link "Publish…"')
	app:wait("#publish-checks", { timeout_ms = 3000 })
	settle(app)
	expect(app:tree()).to_contain("Send its first version from this computer")
	expect(app:tree("#keys-new")).to_contain("disabled")
end)

test("Publish…, a row's inline link, shows the app's focus ring when the keys reach it", function()
	local app = yours()
	-- (At rest, its words underlined.)
	expect(app:has(".settings-inline-link >> .inline-link-underlined")).to_be(true)
	local function words_box(sel: string): string
		return (app:tree(sel):match("%-?[%d.]+,%-?[%d.]+ [%d.]+x[%d.]+")) or ""
	end
	local at_rest = words_box(".settings-inline-link >> .inline-link-underlined")
	app:click('role:textbox "Search plugins"')
	app:frames(5)
	local reached = false
	for _ = 1, 30 do
		app:key("Tab")
		app:frames(3)
		if app:tree('role:link "Publish…"'):find("focused", 1, true) then
			reached = true
			break
		end
	end
	expect(reached).to_be(true)
	expect(app:tree(".settings-inline-link")).to_contain('link "Publish…"')
	expect(app:tree(".settings-inline-link")).to_contain("focused")
	-- Focused, the ring says it's the control: no underline while it holds
	-- the focus, the words where they were; back as focus leaves.
	expect(app:has(".settings-inline-link >> .inline-link-plain")).to_be(true)
	expect(app:has(".settings-inline-link >> .inline-link-underlined")).to_be(false)
	expect(words_box(".settings-inline-link >> .inline-link-plain")).to_be(at_rest)
	app:key("Tab")
	app:frames(3)
	expect(app:has(".settings-inline-link >> .inline-link-underlined")).to_be(true)
	expect(app:has(".settings-inline-link >> .inline-link-plain")).to_be(false)
end)

test("a session that ended mid-page: its keys say to sign in, never couldn't, and Submit says so in its line", function()
	for _, word in { "signed_out", "unauthenticated", "forbidden" } do
		local app = publishing()
		app:command("store_stub.fixed")
		app:click('role:button "Check again"')
		settle(app)
		-- (The page open, then the store no longer takes the session.)
		app:set_state("store_stub_account_fails", word)
		app:click("#publish-submit")
		settle(app)
		expect(`{word}: {app:tree("#publish-note")}`).to_contain("This needs you signed in to Zeron.")
		-- Its keys, read again (back, then Publish… again), say to sign in.
		app:click('role:button "Back to Plugins"')
		settle(app)
		app:click('role:link "Publish…"')
		settle(app, 600)
		local tree = app:tree()
		for _, said in { "Sign in to see its keys", "This needs you signed in to Zeron." } do
			expect(`{word}: {said} {tree:find(said, 1, true) ~= nil}`).to_be(`{word}: {said} true`)
		end
		for _, never in { "Couldn't see its keys", "The store didn't know who sent it.", "The store didn't allow it.", `"{word}"` } do
			expect(`{word}: {never} {tree:find(never, 1, true) ~= nil}`).to_be(`{word}: {never} false`)
		end
		-- Signed in again: Try again reads them.
		app:set_state("store_stub_account_fails", "")
		while app:has('role:button "Try again"') do
			local y = tonumber((app:tree('role:button "Try again"[1]'):match(" %-?[%d.]+,(%-?[%d.]+) "))) or 0
			app:scroll("#fixture-scroll", y - 400)
			app:frames(5)
			app:click('role:button "Try again"[1]')
			settle(app, 600)
		end
		expect(app:error()).to_be_nil()
		expect(app:tree()).never.to_contain("Sign in to see its keys")
	end
end)

test("signed out before Yours is read, its versions say to sign in", function()
	for _, word in { "signed_out", "unauthenticated", "forbidden" } do
		local app = mount("plugins_store_fixture.luau")
		app:resize(1000, 900)
		app:frames(10)
		app:set_state("store_stub_account_fails", word)
		app:click('role:tab "Yours"')
		settle(app)
		app:click('role:link "Publish…"')
		settle(app, 600)
		local tree = app:tree()
		expect(`{word} {tree:find("Sign in to see your versions", 1, true) ~= nil}`).to_be(`{word} true`)
		expect(`{word} {tree:find("Couldn't see your versions", 1, true) ~= nil}`).to_be(`{word} false`)
	end
end)

test("HTML in its README and a footnote in its CHANGELOG are advice under the checks, in Zeron's words, and Submit still sends", function()
	local app = publishing()
	app:command("store_stub.fixed")
	app:command("store_stub.html_and_footnote")
	app:click('role:button "Check again"')
	settle(app)
	local checks = app:tree("#publish-checks")
	expect(checks).to_contain("It has HTML, which the store shows as typed.")
	expect(checks).to_contain("It has a footnote, which the store shows as typed.")
	expect(checks).to_contain("README.md")
	expect(checks).to_contain("CHANGELOG.md")
	-- (Never the engine's English as it came.)
	expect(checks).never.to_contain("line 3 has HTML")
	expect(app:tree("#publish-submit")).never.to_contain("disabled")
	app:click("#publish-submit")
	settle(app)
	expect(calls(app)).to_contain("publish:pomodoro-clock:")
end)
