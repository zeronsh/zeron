-- Crew threads in the sidebar (ui/sidebar_crew_fixture.luau): a chat an
-- orchestrator started is listed under its project and device like any
-- chat, marked with its orchestrator's dot, once (not in Orchestrators, not
-- twice), wherever its cwd is; a side chat still isn't listed; an archived
-- crew thread is gone as an archived chat is.

local SIDE = "#shell-sidebar"
local LIST = `{SIDE} >> #sidebar`
local ORCH = `{SIDE} >> #sidebar-orchestration`
local ORGANIZE = "screens/shell/sidebar.luau::shell_sidebar_organize"
local ACTIVITY = "screens/shell/activity.luau::shell_sidebar_activity"

-- (The Activity view off: the list by project, as the person's.)
local function open(activity: boolean?): any
	local app = mount("sidebar_crew_fixture.luau")
	app:resize(1000, 900)
	app:set_state(ACTIVITY, activity == true)
	app:frames(30)
	return app
end

-- A row's top, by its label (the first such button).
local function top(app: any, label: string): number
	for line in (app:tree(SIDE) :: string):gmatch("[^\n]+") do
		if line:find(` button "{label}"`, 1, true) then
			return tonumber(line:match(" %-?%d+,(%-?%d+) ")) :: number
		end
	end
	error(`no "{label}" in the sidebar`)
end

-- How many buttons labelled `label` the sidebar holds.
local function count(app: any, label: string): number
	local n = 0
	for line in (app:tree(SIDE) :: string):gmatch("[^\n]+") do
		if line:find(` button "{label}"`, 1, true) then
			n += 1
		end
	end
	return n
end

local CREW = { "Control Room Q&A section", "Board card declutter", "Anchored action hover cards" }

test("sidebar crew: an orchestrator's threads are listed under their project", function()
	local app = open()
	local list = app:tree(LIST) :: string
	for _, title in CREW do
		expect(list).to_contain(`button "{title}"`)
	end
	expect(list).to_contain('button "Relay health check"')
	-- Under their project: zeron's header, its sessions, then exnomic's.
	app:set_state(ORGANIZE, "project")
	app:frames(30)
	local zeron, exnomic = top(app, "zeron"), top(app, "exnomic")
	expect(zeron < exnomic).to_be(true)
	for _, title in CREW do
		local y = top(app, title)
		expect(y > zeron and y < exnomic).to_be(true)
	end
	expect(top(app, "Relay health check") > exnomic).to_be(true)
	-- And under their device, by device (the relay thread on the macbook).
	app:set_state(ORGANIZE, "device")
	app:frames(30)
	local function under(title: string): string
		local y = top(app, title)
		local best, at = "", -math.huge
		for _, d in { "desk", "macbook" } do
			local h = top(app, d)
			if h < y and h > at then
				best, at = d, h
			end
		end
		return best
	end
	expect(under("Control Room Q&A section")).to_be("desk")
	expect(under("Anchored action hover cards")).to_be("desk")
	expect(under("Relay health check")).to_be("macbook")
end)

test("sidebar crew: a thread wears its orchestrator's dot, named", function()
	local app = open()
	expect(app:has(`{LIST} >> role:button "Control Room Q&A section" >> .session-crew`)).to_be(true)
	expect(app:tree(LIST)).to_contain('img "Started by Zeron fixes"')
	expect(app:tree(LIST)).to_contain('img "Started by Exnomic VPN"')
	-- A chat of the person's own wears none.
	expect(app:has(`{LIST} >> role:button "Linux titlebar toggle" >> .session-crew`)).to_be(false)
end)

test("sidebar crew: listed once, not in Orchestrators, its orchestrator not a session", function()
	local app = open()
	for _, title in CREW do
		expect(count(app, title)).to_be(1)
	end
	local orch = app:tree(ORCH) :: string
	for _, title in CREW do
		expect(orch).never.to_contain(title)
	end
	-- The Orchestrators section as it was: a row each.
	expect(orch).to_contain('button "Zeron fixes"')
	expect(orch).to_contain('button "Exnomic VPN"')
	local list = app:tree(LIST) :: string
	expect(list).never.to_contain('button "Zeron fixes"')
	expect(list).never.to_contain('button "Exnomic VPN"')
	-- A side chat stays under its chat, not in the list.
	expect(list).never.to_contain("Side question about tabs")
end)

test("sidebar crew: the Activity view shelves a working thread as any chat", function()
	local app = open(true)
	app:click(`{LIST} >> role:button ~"Working"`)
	app:frames(30)
	local list = app:tree(LIST) :: string
	expect(list).to_contain('button "Control Room Q&A section"')
	expect(list).to_contain('button "Anchored action hover cards"')
end)

test("sidebar crew: a thread not yet titled is named by its role", function()
	local app = open()
	local list = app:tree(LIST) :: string
	expect(list).to_contain('button "Sidebar crew threads"')
	expect(list).never.to_contain("[Message from Zeron chat")
end)

test("sidebar crew: a thread opens as its chat", function()
	local app = open()
	app:click(`{LIST} >> role:button "Control Room Q&A section"`)
	app:frames(10)
	expect(app:state("crew_route")).to_be("session:crew-qa")
end)

test("sidebar crew: archived, a thread is gone from the list", function()
	local app = open()
	expect(app:tree(LIST)).to_contain('button "Board card declutter"')
	app:command("archive_cards")
	app:frames(30)
	expect(app:tree(SIDE)).never.to_contain("Board card declutter")
	expect(app:tree(LIST)).to_contain('button "Control Room Q&A section"')
end)
