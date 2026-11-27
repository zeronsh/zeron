-- A launch before the engine has answered: nobody is called signed out, in
-- the sidebar's foot or on the Account page, nor told they have no sessions,
-- however long it takes; the engine saying so is what shows it. On the shell in ui/shell_fixture.luau, Settings'
-- host calls answered by ui/settings_stub.luau.

local SIDE = "#shell-sidebar"
local SIGNED_OUT_PAGE = "Sign in to show up as yourself across Zeron."

local function open(command: string): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 1000)
	app:frames(30)
	app:command(command)
	app:frames(20)
	return app
end

local function shows(app: any, scope: string, text: string): boolean
	return app:tree(scope):find(text, 1, true) ~= nil
end

test("restoring: the sidebar's foot says no workspace of its own", function()
	local app = open("settings_stub.restoring")
	expect(shows(app, SIDE, "Local")).to_be(false)
end)

test("signed out, said by the engine: the sidebar's foot says Local", function()
	local app = open("settings_stub.signed_out")
	expect(shows(app, SIDE, "Local")).to_be(true)
end)

test("restoring: the Account page isn't the signed-out one", function()
	local app = open("settings_stub.restoring")
	app:command("shell.settings")
	app:frames(20)
	app:click(`#settings-nav >> role:tab "Account"`)
	app:frames(30)
	expect(shows(app, "#settings-page", SIGNED_OUT_PAGE)).to_be(false)
end)

test("signed out, said by the engine: the Account page offers signing in", function()
	local app = open("settings_stub.signed_out")
	app:command("shell.settings")
	app:frames(20)
	app:click(`#settings-nav >> role:tab "Account"`)
	app:frames(30)
	expect(shows(app, "#settings-page", SIGNED_OUT_PAGE)).to_be(true)
end)

test("loading: the sidebar doesn't say there are no sessions", function()
	local app = open("fixture.sessions_loading")
	expect(app:has("#sidebar-empty")).to_be(false)
end)

test("answered with none: the sidebar says so", function()
	local app = open("fixture.no_sessions")
	expect(app:has("#sidebar-empty")).to_be(true)
end)

-- (Settings' host calls the stub answers, in order.)
local function asked(app: any, entry: string): boolean
	for _, c in (app:state("settings_stub_calls") :: any) or {} do
		if c == entry then
			return true
		end
	end
	return false
end

local function members(app: any)
	app:command("shell.settings")
	app:frames(20)
	app:click(`#settings-nav >> role:tab "Members"`)
	app:frames(30)
end

test("signed out: Members offers signing in, not a failure, and asks for no organization", function()
	local app = open("settings_stub.signed_out")
	members(app)
	expect(shows(app, "#settings-page", "Sign in to see your team")).to_be(true)
	expect(shows(app, "#settings-page", "Couldn't load your organization")).to_be(false)
	expect(app:has(`#settings-page >> role:button "Sign in"`)).to_be(true)
	expect(app:has(`#settings-page >> "Find someone"`)).to_be(false)
	expect(asked(app, "account.orgs")).to_be(false)
end)

test("signed in while Members shows: its organization is asked for then", function()
	local app = open("settings_stub.signed_out")
	members(app)
	app:command("settings_stub.sign_in")
	app:frames(30)
	expect(asked(app, "account.orgs")).to_be(true)
	expect(shows(app, "#settings-page", "Sign in to see your team")).to_be(false)
	expect(shows(app, "#settings-page", "Analytical Engines")).to_be(true)
end)
