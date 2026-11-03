-- New project and Cloud: Cloud's sessions' machines are never devices (not
-- in New project, Settings → Devices or a new chat's Work in); Cloud is one
-- row instead, whose page lists the GitHub repositories Cloud can open, by
-- owner, searchable, one picked a Cloud project landing in a new chat on
-- Cloud; without GitHub, with no repository and when the read fails, the
-- page says so. A step of the palette shows what it is (its title at the
-- head) and never ⌘K's scope tabs, which come back at ⌘K's root. On the
-- shell in ui/shell_fixture.luau, Cloud answered by ui/cloud_stub.luau.

local LIST = "#palette-list"

local function open(cloud_on: boolean?): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("fixture.cloud_machines")
	if cloud_on ~= false then
		app:command("cloud_stub.offer")
	end
	app:frames(20)
	return app
end

local function calls(app: any): string
	return app:state("calls") or ""
end

local function cloud_calls(app: any): string
	return table.concat((app:state("cloud_stub_calls") :: any) or {}, ",")
end

local function choose(app: any, label: string)
	expect(app:wait(`{LIST} >> role:option ~"{label}"`, { timeout_ms = 2000 })).to_be(true)
	app:click(`{LIST} >> role:option ~"{label}"`)
	app:frames(20)
end

local function new_project(app: any)
	app:command("shell.new_project")
	app:frames(20)
end

-- The labels of the rows listed, in order.
local function rows(app: any): { string }
	local out = {}
	for line in app:tree(LIST):gmatch("[^\n]+") do
		local label = line:match('option "([^"]*)"')
		if label then
			table.insert(out, label)
		end
	end
	return out
end

local function shows(app: any, text: string): boolean
	return app:tree(LIST):find(text, 1, true) ~= nil
end

test("Cloud's machines are never devices: New project lists one Cloud row instead", function()
	local app = open()
	new_project(app)
	local listed = table.concat(rows(app), "|")
	expect(listed).to_contain("MacBook Pro")
	expect(listed).to_contain("devbox")
	expect(listed).never.to_contain("Cloud session")
	-- One Cloud, last.
	local list = rows(app)
	expect(list[#list]).to_contain("Cloud")
	-- Nor in Settings → Devices.
	app:key("Escape")
	app:frames(10)
	app:command("shell.settings")
	app:frames(20)
	app:click('#settings-nav >> role:tab "Devices"')
	app:frames(30)
	expect(app:tree("#settings-page"):find("Cloud session", 1, true)).to_be(nil)
	expect(app:tree("#settings-page"):find("devbox", 1, true) ~= nil).to_be(true)
end)

test("Cloud isn't offered while the account has no Cloud", function()
	local app = open(false)
	new_project(app)
	local listed = table.concat(rows(app), "|")
	expect(listed).never.to_contain("Cloud")
end)

test("Cloud lists the repositories by owner, newest push first; search narrows them", function()
	local app = open()
	new_project(app)
	choose(app, "Cloud")
	expect(cloud_calls(app)).to_contain("cloud.repos")
	-- acme's newest push is newest: its two first, newest first; then octo's.
	local list = rows(app)
	expect(list[1]).to_contain("acme/atlas")
	expect(list[2]).to_contain("acme/site")
	expect(list[3]).to_contain("octo/tools")
	expect(shows(app, "Private")).to_be(true)
	-- Owners head their repositories.
	expect(shows(app, "acme")).to_be(true)
	expect(shows(app, "octo")).to_be(true)
	app:type('role:textbox "Search"', "tools")
	app:frames(20)
	list = rows(app)
	expect(#list).to_be(1)
	expect(list[1]).to_contain("octo/tools")
end)

test("a repository picked is a Cloud project, and a new chat there runs on Cloud", function()
	local app = open()
	new_project(app)
	choose(app, "Cloud")
	app:type('role:textbox "Search"', "site")
	app:frames(20)
	app:key("Enter")
	app:frames(30)
	expect(calls(app)).to_contain("create:cloud-account-1:/home/user/site:true:acme/site")
	expect(app:state("shell_add_project_open")).to_be(false)
	expect(app:wait('#shell-sidebar >> ~"site"', { timeout_ms = 2000 })).to_be(true)
	app:frames(20)
	expect(app:state("shell_project")).to_be("new1")
	-- Its new chat works in it on Cloud.
	expect(app:state("chat_device")).to_be("cloud")
end)

test("without GitHub, Cloud's page leads to connecting it", function()
	local app = open()
	app:command("cloud_stub.no_github")
	app:frames(10)
	new_project(app)
	choose(app, "Cloud")
	expect(shows(app, "GitHub isn't connected")).to_be(true)
	choose(app, "Connect GitHub")
	app:frames(30)
	expect(app:state("shell_route_path")).to_be("/settings/cloud")
end)

test("with no repository chosen, Cloud's page says so; a read that fails says why", function()
	local app = open()
	app:command("cloud_stub.no_repos")
	new_project(app)
	choose(app, "Cloud")
	expect(shows(app, "No repositories yet")).to_be(true)
	expect(shows(app, "Choose repositories in Settings")).to_be(true)
	app:key("Escape")
	app:frames(20)
	app:command("cloud_stub.repos_fail")
	choose(app, "Cloud")
	-- (Its title in the note's capitals; the line under it as written.)
	expect(app:wait('~"Go back and pick Cloud to try again."', { timeout_ms = 2000 })).to_be(true)
	expect(app:tree("#shell-add-project"):lower():find("couldn't load your repositories", 1, true) ~= nil).to_be(true)
end)

test("a step shows what it is, never ⌘K's scope tabs, and Esc gives ⌘K its tabs back", function()
	local app = open()
	-- New project on its own: its title heads it, no tabs, no Next scope.
	new_project(app)
	expect(app:has('role:button "All"')).to_be(false)
	expect(app:has('"Next scope"')).to_be(false)
	expect(app:has('"New project"')).to_be(true)
	-- Tab cycles no scope: it opens the row's page, as it does on any page,
	-- and the head says where it is.
	app:key("Tab")
	app:frames(20)
	expect(table.concat(rows(app), "|")).to_contain("Home")
	expect(app:has('role:button "All"')).to_be(false)
	expect(app:has('role:button "Back from New project"')).to_be(true)
	app:key("Escape")
	app:frames(20)
	app:key("Escape")
	app:frames(20)
	-- ⌘K's root has its tabs; its New project page doesn't, its crumb says it.
	app:command("shell.palette")
	app:frames(20)
	expect(app:has('role:button "All"')).to_be(true)
	expect(app:has('"Next scope"')).to_be(true)
	app:type('role:textbox "Search"', "New project")
	app:frames(30)
	app:click(`{LIST} >> role:option "New project"`)
	expect(app:wait(`{LIST} >> role:option ~"MacBook Pro"`, { timeout_ms = 2000 })).to_be(true)
	expect(app:has('role:button "All"')).to_be(false)
	expect(app:has('"Next scope"')).to_be(false)
	expect(app:has('role:button ~"New project"')).to_be(true)
	app:key("Escape")
	app:frames(20)
	expect(app:has('role:button "All"')).to_be(true)
	expect(app:has('"Next scope"')).to_be(true)
end)
