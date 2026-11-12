-- Cloud's projects, a repository once: each Cloud chat runs on a machine of
-- its own, each with its own copy of the repository's project. Settings →
-- Projects lists them under one Cloud group, after your computers, one row
-- for each repository with every copy's sessions counted, and what that
-- row's ⋯ does (rename, change icon, remove) reaches every copy. Your
-- computers' groups and rows are as they were. A new chat's project, on
-- Cloud, lists the repository once too. On ui/shell_fixture.luau
-- (`fixture.cloud_projects`) and ui/chat_fixture.luau
-- (`fixture.cloud_copies`).

local PAGE = "#settings-page"
local NAV = "#settings-nav"
local CLOUD = 'role:group "Cloud"'

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("fixture.cloud_projects")
	app:command("shell.settings")
	app:frames(20)
	app:click(`{NAV} >> role:tab "Projects"`)
	app:frames(30)
	return app
end

local function said(app: any): string
	return (app:state("calls") :: any) or ""
end

local function count(hay: string, needle: string): number
	local n, at = 0, 1
	while true do
		local i = string.find(hay, needle, at, true)
		if not i then
			return n
		end
		n += 1
		at = i + #needle
	end
end

-- The headings of the page's groups, in order.
local function headings(app: any): { string }
	local out = {}
	for line in app:tree("#setting-projects"):gmatch("[^\n]+") do
		local name = line:match('heading "([^"]*)"')
		if name then
			table.insert(out, name)
		end
	end
	return out
end

local function cloud_menu(app: any)
	app:click(`{CLOUD} >> role:button "More for zeron"`)
	expect(app:wait('role:menu >> role:button "Rename"', { timeout_ms = 15000 })).to_be(true)
	app:wait('"nothing"', { timeout_ms = 400 })
end

test("Cloud's machines are one Cloud group, after your computers, a repository once with every copy's sessions", function()
	local app = open()
	local heads = headings(app)
	expect(count(table.concat(heads, "|"), "Cloud")).to_be(1)
	expect(heads[#heads]).to_be("Cloud")
	local cloud = app:tree(CLOUD)
	-- zeronsh/zeron on three machines: one row, 1 + 1 + 1 archived.
	expect(count(cloud, "zeronsh/zeron")).to_be(1)
	expect(cloud).to_contain("3 sessions · zeronsh/zeron")
	expect(cloud).to_contain("No sessions · acme/app")
	expect(count(cloud, "More for")).to_be(2)
end)

test("your computers' groups and rows are as they were", function()
	local app = open()
	local heads = headings(app)
	expect(heads[1]).to_be("MacBook Pro")
	expect(heads[2]).to_be("devbox")
	local page = app:tree(PAGE)
	expect(page).to_contain("4 sessions · /work/atlas")
	expect(page).to_contain("2 sessions · /srv/borealis")
	-- (This Mac's clone of zeronsh/zeron: its own row, its folder.)
	expect(page).to_contain("No sessions · /work/zeron")
	expect(app:tree(CLOUD)).never.to_contain("/work/zeron")
end)

test("the search finds Cloud's row by its name or its repository", function()
	local app = open()
	app:fill(`{PAGE} >> "Find a project"`, "acme")
	app:frames(20)
	expect(app:tree(PAGE)).to_contain("acme/app")
	expect(app:tree(PAGE)).never.to_contain("zeronsh/zeron")
	app:fill(`{PAGE} >> "Find a project"`, "zeron")
	app:frames(20)
	expect(count(app:tree(CLOUD), "zeronsh/zeron")).to_be(1)
	expect(app:tree(PAGE)).to_contain("/work/zeron")
end)

test("renaming Cloud's row renames every copy, and none of your computers'", function()
	local app = open()
	cloud_menu(app)
	app:click('role:menu >> role:button "Rename"')
	app:frames(10)
	app:fill(`{CLOUD} >> "Name for zeron"`, "Zeron app")
	app:key("Enter")
	app:frames(20)
	local calls = said(app)
	for _, id in { "zc1", "zc2", "zc3" } do
		expect(calls).to_contain(`rename_project:{id}:Zeron app`)
	end
	expect(calls).never.to_contain("rename_project:zmac")
	expect(calls).never.to_contain("rename_project:app1")
	expect(app:tree(CLOUD)).to_contain("Zeron app")
end)

test("changing Cloud's row's icon changes every copy's", function()
	local app = open()
	local dialog = 'role:dialog "Edit project"'
	cloud_menu(app)
	app:click('role:menu >> role:button "Change icon…"')
	app:frames(40)
	expect(app:has(dialog)).to_be(true)
	app:click(`{dialog} >> role:radio "Emoji"`)
	app:frames(20)
	app:fill(`#project-edit-emoji >> role:textbox`, "rocket")
	app:frames(10)
	app:key("Enter")
	app:frames(5)
	app:click(`{dialog} >> role:button "Save"`)
	app:frames(30)
	local calls = said(app)
	for _, id in { "zc1", "zc2", "zc3" } do
		expect(calls).to_contain(`set_project_icon:{id}:🚀::`)
	end
	expect(calls).never.to_contain("set_project_icon:zmac")
	expect(app:state("project_edit_open")).to_be(false)
end)

test("removing Cloud's row counts every copy's sessions, stops the running one, then removes every copy", function()
	local app = open()
	cloud_menu(app)
	app:click('role:menu >> role:button "Remove…"')
	app:frames(20)
	local dialog = 'role:dialog "Remove “zeron”?"'
	expect(app:has(dialog)).to_be(true)
	expect(app:has(`{dialog} >> ~"Its 3 sessions are deleted for good, on every device."`)).to_be(true)
	expect(app:has(`{dialog} >> ~"One of them is running now and is stopped first."`)).to_be(true)
	app:click(`{dialog} >> role:button "Remove project"`)
	app:frames(30)
	local calls = said(app)
	local stop_at = string.find(calls, "stop:zc1a", 1, true)
	expect(stop_at ~= nil).to_be(true)
	for _, id in { "zc1", "zc2", "zc3" } do
		local at = string.find(calls, `delete_project:{id}`, 1, true)
		expect(at ~= nil and at > (stop_at :: number)).to_be(true)
	end
	expect(calls).never.to_contain("delete_project:zmac")
	expect(app:state("settings_project_removing_open")).to_be(false)
	expect(app:tree(PAGE)).never.to_contain("zeronsh/zeron")
	expect(app:tree(CLOUD)).to_contain("acme/app")
	expect(app:tree(PAGE)).to_contain("/work/zeron")
end)

-- A new chat's project, on Cloud ────────────────────────────────────────────

test("a new chat on Cloud lists Cloud's copies of a repository once", function()
	local app = mount("chat_fixture.luau")
	app:resize(960, 760)
	app:command("go_home")
	app:frames(30)
	app:command("cloud_stub.offer")
	app:command("fixture.cloud_copies")
	app:frames(30)
	app:click('role:button "Branch"')
	app:frames(30)
	app:click('role:menuitem "Device"')
	app:frames(30)
	app:click('role:option ~"Cloud"')
	app:frames(30)
	app:click('role:menuitem "Project"')
	app:frames(30)
	local options = 0
	for line in app:tree():gmatch("[^\n]+") do
		if line:find("option", 1, true) and line:lower():find("atlas", 1, true) then
			options += 1
		end
	end
	-- This Mac's Atlas, and Cloud's three copies as one.
	expect(options).to_be(2)
end)
