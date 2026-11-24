-- Setup's Sign in and the server the account will live on (screens/server).
-- Zeron's goes unsaid: no line, no pill. File → Connect to Server… (here
-- `server.ask()`) opens the dialog: an address that isn't a server says why
-- under the field, one that is is used at once (signed out, nothing to
-- ask), and a pill over the title names it from then on, the way back to
-- the dialog. An invite (`server.ask(address)`) opens the dialog on its
-- address, looked up, and waits: the way on reads "Connect to Acme", and
-- only its click switches (signed in, the question to sign out is). A
-- launch's environment naming the server makes the pill plain words, and
-- an invite does nothing. On ui/signin_server_fixture.luau, the host's
-- calls answered there.

local DIALOG = 'role:dialog "Choose a server"'
local ACME = 'role:button "Server, Acme, zeron.acme.com"'

local function open(): any
	local app = mount("signin_server_fixture.luau")
	app:resize(680, 500)
	app:frames(30)
	return app
end

local function asked(app: any, entry: string): boolean
	for _, c in (app:state("signin_server_calls") :: any) or {} do
		if c == entry then
			return true
		end
	end
	return false
end

local function shows(app: any, scope: string, text: string): boolean
	return app:tree(scope):find(text, 1, true) ~= nil
end

-- Whether `sel` is on screen: a closed dialog's content is kept, hidden.
local function showing(app: any, sel: string): boolean
	if not app:has(sel) then
		return false
	end
	local first = string.match(app:tree(sel), "[^\n]+") or ""
	return string.find(first, " hidden", 1, true) == nil
end

-- Opens the dialog as File → Connect to Server… does, on Self-hosted,
-- `address` typed.
local function choose(app: any, address: string)
	app:command("signin_server.ask")
	app:frames(20)
	app:click(`{DIALOG} >> role:radio "Self-hosted"`)
	app:frames(4)
	app:fill(`{DIALOG} >> role:textbox "Server address"`, address)
	app:frames(4)
end

-- Acme's, chosen from the dialog (signed out: used at once).
local function on_acme(app: any)
	choose(app, "zeron.acme.com")
	app:click(`{DIALOG} >> role:button "Connect"`)
	app:frames(30)
end

test("Sign in's server: Zeron's goes unsaid", function()
	local app = open()
	expect(app:has("#server-pill")).to_be(false)
	expect(shows(app, "#copy", "Server")).to_be(false)
	expect(showing(app, DIALOG)).to_be(false)
end)

test("Sign in's server: another, chosen, is named over the title, the way back to the dialog", function()
	local app = open()
	on_acme(app)
	expect(asked(app, "find_server:zeron.acme.com")).to_be(true)
	expect(asked(app, "use_server:zeron.acme.com")).to_be(true)
	-- Signed out: nothing to ask first.
	expect(showing(app, 'role:dialog "Switch to Acme?"')).to_be(false)
	expect(asked(app, "changed:false")).to_be(true)
	expect(showing(app, DIALOG)).to_be(false)
	expect(app:has(ACME)).to_be(true)
	app:click(ACME)
	app:frames(20)
	expect(showing(app, DIALOG)).to_be(true)
	expect(app:tree(`{DIALOG} >> role:radio "Self-hosted"`)).to_contain(" checked")
	-- (On its address, to change it.)
	expect(app:tree(`{DIALOG} >> role:textbox "Server address"`)).to_contain("zeron.acme.com")
	expect(app:tree(`{DIALOG} >> role:textbox "Server address"`)).to_contain("focused")
	-- Zeron's again: the pill goes.
	app:click(`{DIALOG} >> role:radio "Zeron"`)
	app:frames(4)
	app:click(`{DIALOG} >> role:button "Use Zeron"`)
	app:frames(30)
	expect(asked(app, "use_server:zeron")).to_be(true)
	expect(app:has("#server-pill")).to_be(false)
end)

test("Sign in's server: an address that isn't a server says so under the field, and typing again clears it", function()
	local app = open()
	choose(app, "nowhere.example")
	app:key("Enter")
	app:frames(20)
	expect(shows(app, DIALOG, "nowhere.example isn't a Zeron server.")).to_be(true)
	expect(asked(app, "use_server:nowhere.example")).to_be(false)
	-- (Still open, the address as typed.)
	expect(showing(app, DIALOG)).to_be(true)
	app:fill(`{DIALOG} >> role:textbox "Server address"`, "nowhere.example.org")
	app:frames(4)
	expect(shows(app, DIALOG, "isn't a Zeron server.")).to_be(false)
end)

test("Sign in's server: Cancel changes nothing, and the dialog opens again as the account stands", function()
	local app = open()
	choose(app, "nowhere.example")
	app:key("Enter")
	app:frames(20)
	app:click(`{DIALOG} >> role:button "Cancel"`)
	app:frames(30)
	expect(showing(app, DIALOG)).to_be(false)
	expect(asked(app, "use_server:nowhere.example")).to_be(false)
	app:command("signin_server.ask")
	app:frames(20)
	expect(app:tree(`{DIALOG} >> role:radio "Zeron"`)).to_contain(" checked")
	expect(app:tree(`{DIALOG} >> role:textbox "Server address"`)).to_contain('="" ')
	expect(shows(app, DIALOG, "nowhere.example")).to_be(false)
end)

test("Sign in's server: opening on Zeron's chooses nothing; the arrows choose, the address with them", function()
	local app = open()
	app:command("signin_server.ask")
	app:frames(20)
	-- (Focus starts on the first choice, the one made.)
	expect(app:tree(`{DIALOG} >> role:radio "Zeron"`)).to_contain("focused")
	expect(app:tree(`{DIALOG} >> role:radio "Zeron"`)).to_contain(" checked")
	expect(app:tree(`{DIALOG} >> role:radio "Self-hosted"`)).to_contain("unchecked")
	app:key("ArrowDown")
	app:frames(10)
	expect(app:tree(`{DIALOG} >> role:radio "Self-hosted"`)).to_contain(" checked")
	expect(app:tree(`{DIALOG} >> role:textbox "Server address"`)).to_contain("focused")
	app:key("Escape")
	app:frames(30)
	expect(showing(app, DIALOG)).to_be(false)
end)

test("Sign in's server: Connect waits for an address", function()
	local app = open()
	choose(app, "")
	expect(app:tree(`{DIALOG} >> role:button "Connect"`)).to_contain("disabled")
end)

test("Sign in's server: a switch that fails says why and keeps the dialog", function()
	local app = open()
	app:command("signin_server.refuse_next")
	choose(app, "zeron.acme.com")
	app:click(`{DIALOG} >> role:button "Connect"`)
	app:frames(20)
	expect(shows(app, DIALOG, "Zeron's engine isn't answering")).to_be(true)
	expect(showing(app, DIALOG)).to_be(true)
	expect(app:has("#server-pill")).to_be(false)
end)

test("Sign in's server: waiting on the browser, the pill stays and can't be clicked; another server ends a failure", function()
	local app = open()
	on_acme(app)
	app:command("signin_server.waiting")
	app:frames(30)
	expect(app:tree(ACME)).to_contain("disabled")
	app:command("signin_server.failed")
	app:frames(30)
	expect(app:tree(ACME)).never.to_contain("disabled")
	-- Another server: the failure was the last one's.
	choose(app, "")
	app:click(`{DIALOG} >> role:radio "Zeron"`)
	app:frames(4)
	app:click(`{DIALOG} >> role:button "Use Zeron"`)
	app:frames(30)
	expect(app:state("signin_server_status")).to_be("ready")
end)

test("Sign in's server: an invite opens on its address, found, and waits for the click", function()
	local app = open()
	app:command("signin_server.invite")
	app:frames(30)
	expect(showing(app, DIALOG)).to_be(true)
	expect(asked(app, "find_server:zeron.acme.com")).to_be(true)
	expect(app:tree(`{DIALOG} >> role:radio "Self-hosted"`)).to_contain(" checked")
	expect(app:has(`{DIALOG} >> role:button "Connect to Acme"`)).to_be(true)
	-- Nothing changed yet.
	expect(asked(app, "use_server:zeron.acme.com")).to_be(false)
	expect(app:has("#server-pill")).to_be(false)
	-- On the address: Enter is the way on.
	expect(app:tree(`{DIALOG} >> role:textbox "Server address"`)).to_contain("focused")
	app:key("Enter")
	app:frames(30)
	expect(asked(app, "use_server:zeron.acme.com")).to_be(true)
	expect(showing(app, DIALOG)).to_be(false)
	expect(app:has(ACME)).to_be(true)
end)

test("Sign in's server: an invite typed over is the address typed", function()
	local app = open()
	app:command("signin_server.invite")
	app:frames(30)
	app:fill(`{DIALOG} >> role:textbox "Server address"`, "zeron.acme.com.example")
	app:frames(4)
	expect(app:has(`{DIALOG} >> role:button "Connect to Acme"`)).to_be(false)
	expect(app:has(`{DIALOG} >> role:button "Connect"`)).to_be(true)
end)

test("Sign in's server: an invite to the server in use opens on it, and changes nothing", function()
	local app = open()
	on_acme(app)
	app:command("signin_server.invite")
	app:frames(30)
	expect(showing(app, DIALOG)).to_be(true)
	expect(app:tree(`{DIALOG} >> role:textbox "Server address"`)).to_contain("zeron.acme.com")
	local switches = 0
	for _, c in (app:state("signin_server_calls") :: any) or {} do
		if c == "use_server:zeron.acme.com" then
			switches += 1
		end
	end
	expect(switches).to_be(1)
end)

test("Sign in's server: signed in, an invite asks before signing out", function()
	local app = open()
	app:command("signin_server.signed_in")
	app:command("signin_server.invite")
	app:frames(30)
	expect(showing(app, 'role:dialog "Switch to Acme?"')).to_be(true)
	expect(asked(app, "use_server:zeron.acme.com")).to_be(false)
	app:click('role:dialog "Switch to Acme?" >> role:button "Sign Out and Switch"')
	app:frames(30)
	expect(asked(app, "use_server:zeron.acme.com")).to_be(true)
	expect(asked(app, "changed:true")).to_be(true)
end)

test("Sign in's server: named by the launch's environment, the pill is words, and an invite does nothing", function()
	local app = open()
	app:command("signin_server.environment")
	app:frames(30)
	expect(app:has("#server-pill")).to_be(true)
	expect(app:has('role:button "Server, localhost:8787"')).to_be(false)
	expect(shows(app, "#server-pill", "localhost:8787")).to_be(true)
	app:command("signin_server.invite")
	app:frames(30)
	expect(showing(app, DIALOG)).to_be(false)
	expect(asked(app, "find_server:zeron.acme.com")).to_be(false)
end)
