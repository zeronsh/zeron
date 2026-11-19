-- Yours and publishing (screens/plugins/publish, screens/plugins/keys), on
-- the page in ui/plugins_store_fixture.luau, the engine's store calls
-- answered by ui/store_stub.luau: each of yours says where it stands in
-- the store, Publish… checks it here and says what to fix, Submit sends
-- exactly what was checked; each version's review and what you can still
-- do with it; deploy tokens made after a fresh sign-in, the token shown
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

-- Yours, scrolled to the account's deploy tokens at its end.
local function keys_page(): any
	local app = yours()
	app:scroll("#fixture-scroll", 2000)
	app:frames(10)
	return app
end

local function publishing(): any
	local app = yours()
	app:click('role:link "Publish…"')
	app:wait("#publish-checks", { timeout_ms = 3000 })
	settle(app)
	return app
end

-- What a row of yours has always said stays: an update found where it came
-- from, its line and Update, as on Installed; the store's line and
-- Publish… when there's nothing else to say.
test("one of yours with an update found keeps its update line and Update on Yours", function()
	local app = yours()
	expect(app:has('role:link "Publish…"')).to_be(true)
	app:command("store.own_update")
	settle(app)
	local tree = app:tree("#fixture-scroll")
	expect(tree).to_contain("Update available: 1.1.0.")
	expect(app:has('role:button "Update"')).to_be(true)
end)

test("each of yours in Yours says where it stands in the store, with Publish…", function()
	local app = yours()
	local tree = app:tree()
	-- The newest version people install, and whether the store has
	-- verified it (a reviewer declined to verify 1.1.0).
	expect(tree).to_contain("1.1.0 in the plugin store, not verified")
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
	expect(app:tree("#publish-submit")).to_contain("Publish 1.2.0")
end)

test("Publish sends what was checked with its ticket: listed at once, waiting for a reviewer, the older unverified one replaced", function()
	local app = publishing()
	app:command("store_stub.fixed")
	app:click('role:button "Check again"')
	settle(app)
	app:click("#publish-submit")
	settle(app)
	expect(calls(app)).to_contain("publish:pomodoro-clock:pomodoro-clock-1.2.0:ticket-1.2.0")
	local tree = app:tree()
	-- (Its badge says where its review stands.)
	expect(tree).to_contain("Not verified yet")
	-- 1.1.0, never verified, takes no new installs now: said why.
	expect(tree).to_contain("Replaced by 1.2.0 before it was verified")
	-- Yours says it too.
	app:click('role:button "Back to Plugins"')
	settle(app)
	expect(app:tree()).to_contain("1.2.0 in the plugin store, not verified yet")
end)

test("a plugin whose one version a reviewer declined to verify reads as listed, not verified, never as waiting", function()
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
	expect(app:tree()).to_contain("1.1.0 in the plugin store, not verified")
	expect(app:tree()).never.to_contain("not verified yet")
end)

test("each version says whether the store verified it, the reviewer's words when they declined, and why one takes no new installs", function()
	local app = publishing()
	local tree = app:tree()
	expect(tree).to_contain("The reviewer's note:")
	expect(tree).to_contain("It reads every chat")
	expect(tree).to_contain("Verified on")
	expect(tree).to_contain("Replaced by 1.1.0 before it was verified")
	-- (Listed ones can be stopped; the superseded one can't be undone.)
	expect(app:count('role:button "Stop new installs"')).to_be(2)
	expect(app:has('role:button "Allow installs again"')).to_be(false)
end)

test("stopping new installs asks first, then the version says so and can allow them again", function()
	local app = publishing()
	app:scroll("#fixture-scroll", 600)
	app:frames(10)
	-- (1.0.0: the second listed one, under 1.1.0.)
	app:click('role:button "Stop new installs"[2]')
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

test("the account's deploy tokens are at the end of Yours, each saying what it publishes and where it was last used", function()
	local app = keys_page()
	local tree = app:tree()
	expect(tree).to_contain("GitHub Actions")
	expect(tree).to_contain("on Pomodoro clock")
	expect(tree).to_contain("All your plugins")
	-- (The token's short id after its name: as the store's site shows it.)
	expect(app:tree(".key-short")).to_contain("9fK2mQ7x")
	expect(calls(app)).to_contain("tokens")
	-- Not on a plugin's Publish page any more.
	app = publishing()
	expect(app:has("#keys-new")).to_be(false)
end)

test("a deploy token is made for the plugins chosen, after a fresh sign-in, the token shown once to copy", function()
	local app = keys_page()
	app:click("#keys-new")
	settle(app)
	app:type('role:dialog >> role:textbox "Name"', "Release job")
	-- Chosen plugins, none ticked yet: it can't be made.
	app:click('#key-scope >> role:radio "Chosen plugins"')
	settle(app)
	expect(app:tree('role:dialog >> role:button "Create token"')).to_contain("disabled")
	app:click('#key-scope >> role:checkbox "Pomodoro clock"')
	settle(app)
	app:click('role:dialog >> role:button "Create token"')
	settle(app)
	-- Waiting for the browser's sign-in: said, and it can be given up.
	expect(app:tree("role:dialog")).to_contain("Sign in in your browser")
	expect(calls(app)).to_contain("create_token:pomodoro-clock:Release job:expires")
	app:command("store_stub.signed_in_again")
	settle(app, 400)
	local dialog = app:tree("role:dialog")
	expect(dialog).to_contain("Copy your token now")
	expect(dialog).to_contain("zdt_")
	-- Whole, wrapped inside the dialog: never cut behind a scroll.
	local block = app:tree("#key-secret")
	local bx, _, bw = block:match("#key%-secret (%-?%d+),(%-?%d+) (%d+)x")
	local kx, _, kw, kh = block:match('text "zdt_[^"]*" (%-?%d+),(%-?%d+) (%d+)x(%d+)')
	assert(bx and kx, block)
	expect(tonumber(kx) + tonumber(kw) <= tonumber(bx) + tonumber(bw)).to_be(true)
	expect(tonumber(kh) > 20).to_be(true)
	expect(block).never.to_contain("scroll")
	-- Escape doesn't lose it: only its button closes it.
	app:key("Escape")
	settle(app)
	expect(app:tree("role:dialog")).to_contain("zdt_")
	-- (The code block's own Copy, as a chat reply's.)
	app:click('role:dialog >> role:button "Copy code"')
	settle(app)
	expect(app:clipboard()).to_contain("zdt_")
	app:click('role:dialog >> role:button "I\'ve copied it"')
	settle(app)
	-- Gone for good: never shown again.
	expect(app:tree()).never.to_contain("zdt_")
	expect(app:tree()).to_contain("Release job")
end)

-- What a new token can publish is labelled as its Name and Ends are, by the
-- same ds piece (ds/field): the same words' place and size.
test("a new token's Can publish is labelled as its Name and Ends are", function()
	local app = keys_page()
	app:click("#keys-new")
	settle(app)
	local tree = app:tree("role:dialog")
	local function label(words: string): (number, number)
		local x, h = string.match(tree, `text "{words}" (%-?%d+),%-?%d+ %d+x(%d+)`)
		assert(x, `no label "{words}" in:\n{tree}`)
		return tonumber(x) :: number, tonumber(h) :: number
	end
	local name_x, name_h = label("Name")
	local ends_x, ends_h = label("Ends")
	local scope_x, scope_h = label("Can publish")
	expect(scope_x).to_be(name_x)
	expect(ends_x).to_be(name_x)
	expect(scope_h).to_be(name_h)
	expect(ends_h).to_be(name_h)
	-- Above the choice it names.
	local _, label_y = string.match(tree, 'text "Can publish" (%-?%d+),(%-?%d+)')
	local _, radio_y = string.match(tree, 'radio "All your plugins" (%-?%d+),(%-?%d+)')
	assert(label_y and radio_y, tree)
	expect((tonumber(label_y) :: number) < (tonumber(radio_y) :: number)).to_be(true)
end)

test("waiting on the sign-in can be stopped; a token for all your plugins is the first choice", function()
	local app = keys_page()
	app:click("#keys-new")
	settle(app)
	app:type('role:dialog >> role:textbox "Name"', "CI")
	app:click('role:dialog >> role:button "Create token"')
	settle(app)
	expect(calls(app)).to_contain("create_token:all:CI:expires")
	app:click('role:dialog >> role:button "Stop waiting"')
	settle(app)
	expect(calls(app)).to_contain("cancel_step_up")
end)

test("revoking a token asks first, then it's gone", function()
	local app = keys_page()
	app:click('role:button "Revoke"')
	settle(app)
	expect(app:tree("role:dialog")).to_contain("Revoke GitHub Actions?")
	app:click('role:dialog >> role:button "Revoke"')
	settle(app)
	expect(calls(app)).to_contain("revoke_token:t_gh")
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

test("a deploy token waits for a plugin of yours in the store, its first version sent from this computer", function()
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	app:command("store_stub.never_published")
	app:click('role:tab "Yours"')
	settle(app)
	expect(app:tree()).to_contain("Not in the plugin store")
	app:scroll("#fixture-scroll", 2000)
	app:frames(10)
	expect(app:tree()).to_contain("Publish one of your plugins from this computer first.")
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

test("a session that ended mid-page: its tokens say to sign in, never couldn't, and Submit says so in its line", function()
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
		-- The tokens, read again with Yours (back to it), say to sign in.
		app:click('role:button "Back to Plugins"')
		settle(app, 600)
		app:scroll("#fixture-scroll", 2000)
		app:frames(10)
		local tree = app:tree()
		for _, said in { "Sign in to see your tokens", "This needs you signed in to Zeron." } do
			expect(`{word}: {said} {tree:find(said, 1, true) ~= nil}`).to_be(`{word}: {said} true`)
		end
		for _, never in { "Couldn't see your tokens", "The store didn't know who sent it.", "The store didn't allow it.", `"{word}"` } do
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
		expect(app:tree()).never.to_contain("Sign in to see your tokens")
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

-- What's in the way of a listing says which thing and why, by the cause
-- the engine names, the word shown as it was written; never the engine's
-- English.
test("a refused name or keyword says which word and why, in Zeron's words", function()
	local app = publishing()
	app:command("store_stub.fixed")
	app:command("store_stub.claims")
	app:click('role:button "Check again"')
	settle(app)
	local checks = app:tree("#publish-checks")
	expect(checks).to_contain("Its name calls it “Verified”. The store's mark says")
	expect(checks).to_contain("The keyword “timer” is there twice.")
	-- (Never the engine's English as it came.)
	expect(checks).never.to_contain("only the store marks a plugin verified")
	expect(checks).never.to_contain("each once")
	expect(app:tree("#publish-submit")).to_contain("disabled")
end)

-- The store refusing the publish itself says the same way.
test("a publish the store refuses for a listing's cause says which and why", function()
	local app = publishing()
	app:command("store_stub.fixed")
	app:click('role:button "Check again"')
	settle(app)
	app:command("store_stub.refuse_listing")
	app:click("#publish-submit")
	settle(app)
	expect(app:tree()).to_contain("Its description has “✅”, which reads as the store's mark.")
end)

test("HTML and math in its README and a footnote in its CHANGELOG are advice under the checks, in Zeron's words, and Submit still sends", function()
	local app = publishing()
	app:command("store_stub.fixed")
	app:command("store_stub.html_and_footnote")
	app:click('role:button "Check again"')
	settle(app)
	local checks = app:tree("#publish-checks")
	expect(checks).to_contain("It has HTML, which the store shows as typed.")
	expect(checks).to_contain("It has a footnote, which the store shows as typed.")
	expect(checks).to_contain("It has math, which the store's website shows as typed.")
	-- Each where it is: its file and its line.
	expect(checks).to_contain("README.md, line 3")
	expect(checks).to_contain("CHANGELOG.md, line 7")
	expect(checks).to_contain("README.md, line 12")
	-- (Never the engine's English as it came.)
	expect(checks).never.to_contain("line 3 has HTML")
	expect(app:tree("#publish-submit")).never.to_contain("disabled")
	app:click("#publish-submit")
	settle(app)
	expect(calls(app)).to_contain("publish:pomodoro-clock:")
end)
