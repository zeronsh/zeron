-- Cloud in a new chat's Work in, whatever project is chosen: while Cloud is
-- on and can open some project, it's offered beside your computers, with No
-- project chosen or a project it can't open too. Picked, the projects are
-- those it can open, never none (a Cloud chat needs a repository): the one
-- chosen if Cloud can open it, else the one last used on Cloud, else the
-- first. A computer picked again brings its projects back as ever; Cloud
-- off, it's never offered. On ui/chat_fixture.luau (Atlas is acme/atlas on
-- GitHub; Notes has no repository), Cloud answered by ui/cloud_stub.luau.

local function settle(app: any)
	app:frames(30)
end

local function showing(app: any, sel: string): boolean
	if not app:has(sel) then
		return false
	end
	local first = string.match(app:tree(sel), "[^\n]+") or ""
	return string.find(first, " hidden", 1, true) == nil
end

local function open(cloud_on: boolean?): any
	local app = mount("chat_fixture.luau")
	app:resize(960, 760)
	app:command("go_home")
	settle(app)
	app:command("fixture.site_project")
	if cloud_on ~= false then
		app:command("cloud_stub.offer")
	end
	settle(app)
	return app
end

local function panel(app: any)
	if not showing(app, 'role:menuitem "Device"') then
		app:click('role:button "Branch"')
		settle(app)
	end
end

local function open_row(app: any, row: string)
	panel(app)
	app:click(`role:menuitem "{row}"`)
	settle(app)
end

local function pick_in(app: any, row: string, option: string)
	open_row(app, row)
	app:click(`role:option ~"{option}"`)
	settle(app)
end

-- Work in no project (the project menu's action).
local function no_project(app: any)
	open_row(app, "Project")
	app:click(`role:button "Don't work in a project"`)
	settle(app)
	expect(app:state("chat_project")).to_be("none")
end

local function close(app: any)
	app:key("Escape")
	settle(app)
	app:key("Escape")
	settle(app)
end

test("Cloud is offered with No project chosen, and with a project it can't open", function()
	local app = open()
	no_project(app)
	open_row(app, "Device")
	expect(app:has('role:option ~"Cloud"')).to_be(true)
	expect(app:has('role:option ~"machine of its own"')).to_be(false)
	close(app)
	pick_in(app, "Project", "Notes")
	expect(app:state("chat_project")).to_be("notes")
	open_row(app, "Device")
	expect(app:has('role:option ~"Cloud"')).to_be(true)
end)

test("picking Cloud with No project picks a project it can open, and lists only those, never none", function()
	local app = open()
	no_project(app)
	pick_in(app, "Device", "Cloud")
	expect(app:state("chat_device")).to_be("cloud")
	-- (The first it can open.)
	expect(app:state("chat_project")).to_be("atlas")
	open_row(app, "Project")
	expect(app:has('role:option ~"Atlas"')).to_be(true)
	expect(app:has('role:option ~"Site"')).to_be(true)
	expect(app:has('role:option ~"Notes"')).to_be(false)
	expect(app:has('role:option ~"Borealis"')).to_be(false)
	expect(showing(app, `role:button "Don't work in a project"`)).to_be(false)
	-- Its row is just its name, with no line under it.
	close(app)
	open_row(app, "Device")
	expect(app:has('role:option ~"Cloud"')).to_be(true)
	expect(app:has('role:option ~"machine of its own"')).to_be(false)
end)

test("picking a computer again brings its projects back, and Cloud keeps the project last used there", function()
	local app = open()
	no_project(app)
	pick_in(app, "Device", "Cloud")
	pick_in(app, "Project", "Site")
	expect(app:state("chat_project")).to_be("site")
	-- Back to this Mac: no project, as it was here.
	pick_in(app, "Device", "MacBook Pro")
	expect(app:state("chat_device")).to_be("mac")
	expect(app:state("chat_project")).to_be("none")
	open_row(app, "Project")
	expect(app:has('role:option ~"Notes"')).to_be(true)
	expect(showing(app, `role:button "Don't work in a project"`)).to_be(true)
	close(app)
	-- Cloud again: Site, the one last used there.
	pick_in(app, "Device", "Cloud")
	expect(app:state("chat_project")).to_be("site")
end)

test("picking Cloud keeps a project it can open", function()
	local app = open()
	pick_in(app, "Device", "Build box")
	pick_in(app, "Project", "Site")
	pick_in(app, "Device", "Cloud")
	expect(app:state("chat_project")).to_be("site")
end)

test("with Cloud off, Cloud is never offered", function()
	local app = open(false)
	no_project(app)
	open_row(app, "Device")
	expect(app:has('role:option ~"Cloud"')).to_be(false)
	close(app)
	pick_in(app, "Project", "Atlas")
	open_row(app, "Device")
	expect(app:has('role:option ~"Cloud"')).to_be(false)
end)
