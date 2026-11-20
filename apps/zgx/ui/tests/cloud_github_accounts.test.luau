-- Settings → Cloud's GitHub logins: listed with what can be done with each, one
-- disconnected behind a confirm, another connected through the device flow, a
-- dead one saying so, and the repositories saying whose they are once there's
-- more than one. An engine from before several logins still shows its one. On
-- the shell in ui/shell_fixture.luau, answered by ui/cloud_stub.luau.

local PAGE = "#settings-page"
local NAV = "#settings-nav"

local function shows(app: any, text: string): boolean
	return app:has(`{PAGE} >> ~"{text}"`)
end

-- In a row's anchor of the page: "github" or "repos".
local function under(app: any, row: string, text: string): boolean
	return app:has(`{PAGE} >> #setting-{row} >> ~"{text}"`)
end

local function asked(app: any, entry: string): boolean
	for _, c in (app:state("cloud_stub_calls") :: any) or {} do
		if c == entry then
			return true
		end
	end
	return false
end

local function asked_count(app: any, entry: string): number
	local n = 0
	for _, c in (app:state("cloud_stub_calls") :: any) or {} do
		if c == entry then
			n += 1
		end
	end
	return n
end

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 1400)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("cloud_stub.offer")
	app:command("shell.settings")
	app:frames(20)
	app:click(`{NAV} >> role:tab "Cloud"`)
	app:frames(30)
	return app
end

test("one login: named, with another on offer, and repositories name no one", function()
	local app = open()
	expect(shows(app, "octo-example")).to_be(true)
	expect(shows(app, "Connected")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Disconnect"`)).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Connect another"`)).to_be(true)
	expect(shows(app, "acme/atlas")).to_be(true)
	expect(under(app, "repos", "octo-example")).to_be(false)
end)

test("two logins: both listed, and each repository says whose", function()
	local app = open()
	app:command("cloud_stub.two_logins")
	app:frames(30)
	expect(under(app, "github", "octo-example")).to_be(true)
	expect(under(app, "github", "home-example")).to_be(true)
	expect(under(app, "repos", "home/blog")).to_be(true)
	expect(under(app, "repos", "home-example")).to_be(true)
	expect(under(app, "repos", "octo-example")).to_be(true)
end)

test("disconnecting one asks first, says what happens, and leaves the other", function()
	local app = open()
	app:command("cloud_stub.two_logins")
	app:frames(30)
	local calls = asked_count(app, "cloud.repos")
	expect(app:has(`{PAGE} >> #setting-github >> "home-example"`)).to_be(true)
	app:click(`{PAGE} >> #setting-github >> role:button "Disconnect"[2]`)
	app:frames(20)
	expect(app:has(`"Disconnect home-example?"`)).to_be(true)
	expect(asked(app, "github.forget:a2")).to_be(false)
	app:click(`role:dialog >> role:button "Disconnect"`)
	app:frames(30)
	expect(asked(app, "github.forget:a2")).to_be(true)
	expect(under(app, "github", "octo-example")).to_be(true)
	expect(app:has(`{PAGE} >> #setting-github >> "home-example"`)).to_be(false)
	-- Its repositories are read again, and gone with it.
	expect(asked_count(app, "cloud.repos") > calls).to_be(true)
	expect(shows(app, "home/blog")).to_be(false)
end)

test("a login that needs reconnecting says which, and offers it; the other stays", function()
	local app = open()
	app:command("cloud_stub.two_logins")
	app:command("cloud_stub.login_dead")
	app:frames(30)
	expect(shows(app, "Needs reconnecting")).to_be(true)
	expect(shows(app, "can't push until you do")).to_be(true)
	expect(shows(app, "Connected")).to_be(true)
	app:click(`{PAGE} >> role:button "Reconnect"`)
	app:frames(30)
	expect(asked(app, "github.connect")).to_be(true)
	expect(shows(app, "WDJB-MJHT")).to_be(true)
end)

test("connecting another shows the code beside the logins, which hold still", function()
	local app = open()
	app:click(`{PAGE} >> role:button "Connect another"`)
	app:frames(30)
	expect(asked(app, "github.connect")).to_be(true)
	expect(shows(app, "WDJB-MJHT")).to_be(true)
	expect(shows(app, "switch accounts on github.com")).to_be(true)
	expect(shows(app, "octo-example")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Disconnect"`)).to_be(false)
	expect(app:has(`{PAGE} >> role:button "Connect another"`)).to_be(false)
	-- Approved: it joins the list, its repositories with it.
	app:command("cloud_stub.approve")
	app:frames(30)
	expect(shows(app, "WDJB-MJHT")).to_be(false)
	expect(under(app, "github", "home-example")).to_be(true)
	expect(shows(app, "home/blog")).to_be(true)
end)

test("the same login approved again says so and what to do", function()
	local app = open()
	app:command("cloud_stub.already_connected")
	app:click(`{PAGE} >> role:button "Connect another"`)
	app:frames(20)
	app:command("cloud_stub.approve")
	app:frames(30)
	expect(shows(app, "octo-example is already connected")).to_be(true)
	expect(shows(app, "Switch accounts on github.com")).to_be(true)
end)

test("an engine from before several logins shows its one, and disconnects all of it", function()
	local app = open()
	app:command("cloud_stub.old_host")
	app:frames(30)
	expect(under(app, "github", "octo-example")).to_be(true)
	expect(shows(app, "acme/atlas")).to_be(true)
	app:click(`{PAGE} >> #setting-github >> role:button "Disconnect"`)
	app:frames(20)
	app:click(`role:dialog >> role:button "Disconnect"`)
	app:frames(30)
	expect(asked(app, "github.forget:all")).to_be(true)
	expect(asked(app, "github.disconnect")).to_be(true)
	expect(shows(app, "Not connected")).to_be(true)
end)

test("no logins: the block connects, as ever", function()
	local app = open()
	app:command("cloud_stub.no_github")
	app:frames(30)
	expect(shows(app, "Not connected")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Connect another"`)).to_be(false)
	app:click(`{PAGE} >> role:button "Connect"`)
	app:frames(30)
	expect(asked(app, "github.connect")).to_be(true)
end)
