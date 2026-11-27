-- A worktree of a whole project (screens/chat/where's repositories,
-- ds/repository_choice): a project folder holding several repositories,
-- and not one itself, offers a worktree too; its panel lists the
-- repositories to check out in place of branches, all of a few chosen at
-- first, and the send asks the host for one worktree holding those chosen
-- (ui/chat_fixture.luau notes `repositories:path:device` for the listing,
-- and `run:cwd|checkout|ref|paths` for the first run).

local function settle(app: any)
	app:frames(30)
end

local function rested(app: any)
	app:wait("#never-shown", { timeout_ms = 900 })
	settle(app)
end

local function showing(app: any, sel: string): boolean
	if not app:has(sel) then
		return false
	end
	local first = string.match(app:tree(sel), "[^\n]+") or ""
	return string.find(first, " hidden", 1, true) == nil
end

-- A new chat with Acme among this computer's projects.
local function home(app: any, project: string?)
	app:resize(960, 760)
	app:command(if project == "Clones" then "fixture.clones_project" else "fixture.acme_project")
	app:command("go_home")
	settle(app)
end

local function panel(app: any)
	if not showing(app, 'role:menuitem "Device"') then
		app:click('role:button "Branch"')
		settle(app)
	end
end

local function pick_in(app: any, row: string, option: string)
	panel(app)
	app:click(`role:menuitem "{row}"`)
	settle(app)
	app:click(`role:option ~"{option}"`)
	settle(app)
end

-- In Acme (or `project`), a worktree picked, the panel open on it.
local function in_project(app: any, project: string?)
	pick_in(app, "Project", project or "Acme")
	pick_in(app, "Checkout", "Worktree An isolated")
	panel(app)
end

local function chip(app: any): string
	rested(app)
	return app:text('role:button "Branch" >> #composer-place-branch[1]')
end

local function send(app: any, words: string)
	app:key("Escape")
	app:key("Escape")
	settle(app)
	app:type('role:textbox "Message"', words)
	app:key("Enter")
	settle(app)
end

test("a folder holding repositories offers a worktree of them all", function()
	local app = mount("chat_fixture.luau")
	home(app)
	pick_in(app, "Project", "Acme")
	expect(app:state("calls")).to_contain("repositories:/tmp/acme:here")
	panel(app)
	-- Its checkout can be chosen, as a repository's is.
	expect(showing(app, 'role:menuitem "Checkout"')).to_be(true)
	pick_in(app, "Checkout", "Worktree An isolated")
	panel(app)
	-- Its repositories in place of branches: each with its branch, the one
	-- with no commit dimmed and saying why; every other one chosen.
	expect(showing(app, "#composer-repositories")).to_be(true)
	expect(app:has('role:textbox "Search branches…"')).to_be(false)
	expect(app:has('role:checkbox "api" >> ~"main"')).to_be(true)
	expect(app:has('role:checkbox "web" >> ~"develop"')).to_be(true)
	expect(app:has('role:checkbox "packages/ui"')).to_be(true)
	expect(app:has('role:checkbox "scratch" >> ~"No commits yet"')).to_be(true)
	expect(app:text("#repositories-count")).to_be("3 of 3")
	-- The chip counts them, behind a worktree's glyph.
	expect(chip(app)).to_be("3 repositories")
	expect(app:has("#composer-place-lead >> .place-lead-worktree")).to_be(true)
end)

test("a send asks the host for one worktree holding those chosen", function()
	local app = mount("chat_fixture.luau")
	home(app)
	in_project(app)
	send(app, "Wire the login across the stack")
	local calls = app:state("calls")
	expect(calls).to_contain("made:acme|-|-|-")
	expect(calls).to_contain("run:/tmp/acme|worktree|-|api,packages/ui,web")
end)

test("a repository let go is left out, and the choice is remembered", function()
	local app = mount("chat_fixture.luau")
	home(app)
	in_project(app)
	app:click('role:checkbox "packages/ui"')
	settle(app)
	-- The panel stays open on the choice.
	expect(showing(app, "#composer-repositories")).to_be(true)
	expect(app:text("#repositories-count")).to_be("2 of 3")
	expect(chip(app)).to_be("2 repositories")
	-- One that can't be chosen stays out.
	panel(app)
	app:click('role:checkbox "scratch"')
	settle(app)
	expect(app:text("#repositories-count")).to_be("2 of 3")
	-- Away to another project and back: Acme's choice as it was left.
	pick_in(app, "Project", "Atlas")
	expect(chip(app)).to_be("main")
	pick_in(app, "Project", "Acme")
	panel(app)
	expect(app:text("#repositories-count")).to_be("2 of 3")
	expect(chip(app)).to_be("2 repositories")
	send(app, "Only the services")
	expect(app:state("calls")).to_contain("run:/tmp/acme|worktree|-|api,web")
end)

test("with none chosen it can't start, and says why", function()
	local app = mount("chat_fixture.luau")
	home(app)
	in_project(app)
	app:click("#repositories-none")
	settle(app)
	expect(app:text("#repositories-count")).to_be("0 of 3")
	expect(chip(app)).to_be("0 repositories")
	send(app, "Nothing to work in")
	expect(app:has('~"Choose at least one repository"')).to_be(true)
	expect(string.find(app:state("calls"), "made:acme", 1, true)).to_be(nil)
	-- All brings every one back, and the send goes.
	panel(app)
	app:click("#repositories-all")
	settle(app)
	expect(app:text("#repositories-count")).to_be("3 of 3")
	send(app, "Now everything")
	expect(app:state("calls")).to_contain("run:/tmp/acme|worktree|-|api,packages/ui,web")
end)

test("a folder of many repositories starts with none chosen", function()
	local app = mount("chat_fixture.luau")
	home(app, "Clones")
	in_project(app, "Clones")
	expect(app:text("#repositories-count")).to_be("0 of 12")
	expect(app:has('~"Choose at least one repository"')).to_be(true)
	app:click('role:checkbox "clone-03"')
	settle(app)
	send(app, "Just the third")
	expect(app:state("calls")).to_contain("run:/tmp/clones|worktree|-|clone-03")
end)

test("a local checkout of such a folder runs in it", function()
	local app = mount("chat_fixture.luau")
	home(app)
	pick_in(app, "Project", "Acme")
	pick_in(app, "Checkout", "Local checkout Work in")
	panel(app)
	expect(showing(app, "#composer-repositories")).to_be(false)
	expect(app:has("#composer-place-lead >> .place-lead-folder")).to_be(true)
	send(app, "Look around")
	local calls = app:state("calls")
	expect(calls).to_contain("made:acme|-|-|/tmp/acme")
	expect(calls).to_contain("run:/tmp/acme|local|-")
end)

test("a host too old to list them runs in the folder, as before", function()
	local app = mount("chat_fixture.luau")
	home(app)
	app:command("repositories_refused")
	pick_in(app, "Checkout", "Worktree An isolated")
	pick_in(app, "Project", "Acme")
	panel(app)
	expect(showing(app, 'role:menuitem "Checkout"')).to_be(false)
	expect(app:has("#composer-place-lead >> .place-lead-folder")).to_be(true)
	send(app, "Tidy up")
	local calls = app:state("calls")
	expect(calls).to_contain("made:acme|-|-|/tmp/acme")
	expect(calls).to_contain("run:/tmp/acme|local|-")
end)

test("a repository's project never asks for repositories", function()
	local app = mount("chat_fixture.luau")
	home(app)
	pick_in(app, "Project", "Atlas")
	rested(app)
	expect(string.find(app:state("calls"), "repositories:", 1, true)).to_be(nil)
	panel(app)
	expect(showing(app, "#composer-repositories")).to_be(false)
end)
