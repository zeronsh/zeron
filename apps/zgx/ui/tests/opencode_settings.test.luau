-- Providers → OpenCode settings: its own tabs (the agent's own switches
-- first, then the sessions that show here and where its chats run), each one
-- short enough for the window and none of them moving the title or the tabs;
-- where chats run is one choice (the background service, a server per chat,
-- a remote server), never two at once; and a remote waits for Connect and,
-- connected, folds to one row with a way to change it. On the shell in
-- ui/shell_fixture.luau, the engine's answers in ui/settings_stub.luau.

local PAGE = "#settings-page"
local NAV = "#settings-nav"

local function shows(app: any, text: string): boolean
	return app:has(`role:dialog >> "{text}"`)
end

-- Providers, with OpenCode among its agents, and OpenCode's settings open.
local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("shell.settings")
	app:frames(20)
	app:click(`{NAV} >> role:tab "Providers"`)
	app:frames(30)
	local list = table.clone((app:state("agents") :: any).list)
	local oc = table.clone(list[1])
	oc.id = "opencode"
	oc.name = "OpenCode"
	oc.accounts = {}
	table.insert(list, oc)
	app:set_state("agents", { state = "ready", list = list })
	app:frames(10)
	app:click(`{PAGE} >> role:button "OpenCode settings"`)
	app:frames(20)
	return app
end

-- A tab of the dialog's own.
local function tab(app: any, label: string)
	app:click(`role:dialog >> role:tab "{label}"`)
	app:frames(10)
end

-- The "Chats run on" choice set to `label`.
local function run_on(app: any, label: string)
	app:click('role:button "Chats run on"')
	app:frames(10)
	app:click(`role:menuitem "{label}"`)
	app:frames(10)
end

local function reopen(app: any)
	app:click('role:button "Done"')
	app:frames(20)
	app:click(`{PAGE} >> role:button "OpenCode settings"`)
	app:frames(20)
end

-- The foot of `node`: how far down the window its bottom edge is.
local function bottom(app: any, node: string): number?
	local line = app:tree(node):match("[^\n]+")
	local y, h = string.match(line or "", " %d+,(%d+) %d+x(%d+)")
	if not y then
		return nil
	end
	return tonumber(y) + tonumber(h)
end

-- Where `node` stands: its top left.
local function place(app: any, node: string): { number }?
	local line = app:tree(node):match("[^\n]+")
	local x, y = string.match(line or "", " (%d+),(%d+) %d+x%d+")
	if not x then
		return nil
	end
	return { tonumber(x), tonumber(y) }
end

test("OpenCode settings: its own tabs, opened on the agent's, and no switch for the service or a remote", function()
	local app = open()
	expect(app:has('role:tab "Agent"')).to_be(true)
	expect(app:has('role:tab "Sessions"')).to_be(true)
	expect(app:has('role:tab "Server"')).to_be(true)
	-- (The agent's own switches, first: what this Mac's agent does.)
	expect(app:has('role:switch "Native subagents"')).to_be(true)
	expect(app:has('role:switch "Sync sessions"')).to_be(false)
	expect(app:has('role:button "Chats run on"')).to_be(false)
	tab(app, "Sessions")
	expect(app:has('role:switch "Sync sessions"')).to_be(true)
	expect(app:has('role:switch "Native subagents"')).to_be(false)
	-- (Where chats run is a tab of its own.)
	tab(app, "Server")
	expect(app:has('role:switch "Sync sessions"')).to_be(false)
	expect(app:has('role:button "Chats run on"')).to_be(true)
	expect(app:has('role:switch "Use the background service"')).to_be(false)
	expect(app:has('role:switch "Remote server"')).to_be(false)
end)

test("OpenCode settings: the title and its tabs stay where they are, whichever tab shows", function()
	local app = open()
	local strip = place(app, 'role:tablist "OpenCode settings"')
	local title = place(app, 'role:heading "OpenCode settings"')
	expect(strip ~= nil and title ~= nil).to_be(true)
	for _, name in { "Sessions", "Server", "Agent" } do
		tab(app, name)
		expect(place(app, 'role:tablist "OpenCode settings"')).to_equal(strip)
		expect(place(app, 'role:heading "OpenCode settings"')).to_equal(title)
	end
	-- (In a short window too, where the tabs differ most in height.)
	app:resize(1200, 560)
	app:frames(20)
	local short = place(app, 'role:tablist "OpenCode settings"')
	local short_title = place(app, 'role:heading "OpenCode settings"')
	expect(short ~= nil and short_title ~= nil).to_be(true)
	for _, name in { "Server", "Sessions", "Agent" } do
		tab(app, name)
		expect(place(app, 'role:tablist "OpenCode settings"')).to_equal(short)
		expect(place(app, 'role:heading "OpenCode settings"')).to_equal(short_title)
	end
end)

test("OpenCode settings: one choice of where chats run, kept across a reopen", function()
	local app = open()
	tab(app, "Server")
	expect(shows(app, "Background service")).to_be(true)
	-- A server per chat: saved at once.
	run_on(app, "A server per chat")
	reopen(app)
	tab(app, "Server")
	expect(shows(app, "A server per chat")).to_be(true)
	-- Remote: its fields, nothing saved until Connect.
	run_on(app, "Remote server")
	expect(app:has('role:textbox "Server address"')).to_be(true)
	reopen(app)
	tab(app, "Server")
	expect(shows(app, "A server per chat")).to_be(true)
	expect(app:has('role:textbox "Server address"')).to_be(false)
end)

test("OpenCode settings: a remote connected folds to one row; Change opens it, Cancel folds it unchanged", function()
	local app = open()
	tab(app, "Server")
	run_on(app, "Remote server")
	app:type('role:textbox "Name"', "Studio")
	app:type('role:textbox "Server address"', "http://studio.tail:4096")
	app:type('role:textbox "Password"', "secret")
	app:click('role:button "Connect"')
	app:frames(20)
	expect(app:has('role:textbox "Server address"')).to_be(false)
	expect(shows(app, "Studio")).to_be(true)
	expect(app:tree("role:dialog"):find("http://studio.tail:4096", 1, true) ~= nil).to_be(true)
	app:click('role:button "Change"')
	app:frames(10)
	expect(app:has('role:textbox "Server address"')).to_be(true)
	app:click('role:dialog >> role:button "Cancel"')
	app:frames(10)
	expect(app:has('role:textbox "Server address"')).to_be(false)
	expect(shows(app, "Studio")).to_be(true)
	-- Back to this computer's service: the remote let go.
	run_on(app, "Background service")
	reopen(app)
	tab(app, "Server")
	expect(shows(app, "Background service")).to_be(true)
	expect(shows(app, "Studio")).to_be(false)
end)

test("OpenCode settings in a short window: every tab fits, its last row above Done", function()
	local app = open()
	-- (The window's least height, with the dialog open: opened before, as
	-- Settings' own pages would be squeezed out of reach at its size.)
	app:resize(1200, 560)
	app:frames(20)
	local foot = bottom(app, 'role:button "Done"')
	expect(foot ~= nil and foot <= 560).to_be(true)
	expect(bottom(app, 'role:button "Update policy"') <= foot).to_be(true)
	tab(app, "Sessions")
	expect(bottom(app, 'role:switch "Add projects"') <= foot).to_be(true)
	tab(app, "Server")
	expect(bottom(app, 'role:button "Chats run on"') <= foot).to_be(true)
end)

test("OpenCode settings: a remote's fields fit the window it is opened in", function()
	local app = open()
	app:resize(1240, 820)
	app:frames(20)
	tab(app, "Server")
	run_on(app, "Remote server")
	-- (Its tallest: a name, an address and a password, each under its
	-- label, then Connect.)
	local foot = bottom(app, 'role:button "Done"')
	expect(foot ~= nil and foot <= 820).to_be(true)
	expect(bottom(app, 'role:textbox "Password"') <= foot).to_be(true)
	expect(bottom(app, 'role:button "Connect"') <= foot).to_be(true)
end)
