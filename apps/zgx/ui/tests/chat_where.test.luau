-- Where a new chat works (screens/chat/where, the composer's Work in
-- panel): the device's projects, the project's refs from git where it is,
-- the chip naming the ref, and a send carrying the device, project, checkout
-- and ref to the host (ui/chat_fixture.luau notes each: `refs:path:device`,
-- `made:project|device|branch|cwd` for the chat made, `run:cwd|checkout|ref`
-- for its first run).

local function settle(app: any)
	app:frames(30)
end

-- Settled, and a label's roll (RollText, about 750 ms) run out.
local function rested(app: any)
	app:wait("#never-shown", { timeout_ms = 900 })
	settle(app)
end

-- Whether a node is on screen (the tree marks one that isn't).
local function showing(app: any, sel: string): boolean
	if not app:has(sel) then
		return false
	end
	local first = string.match(app:tree(sel), "[^\n]+") or ""
	return string.find(first, " hidden", 1, true) == nil
end

-- A new chat, its panel open.
local function home(app: any)
	app:resize(960, 760)
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

-- What the panel's rows say: the project and the device chosen.
local function chosen(app: any, row: string): string
	rested(app)
	-- (Its whole line: a project's tile letter, then its name.)
	local words = {}
	for line in app:tree(`role:menuitem "{row}"`):gmatch("[^\n]+") do
		local t = line:match('text "([^"]*)"')
		if t then
			table.insert(words, t)
		end
	end
	return table.concat(words, " ")
end

-- What the chip names, once it has rolled to it: the branch, in a
-- project ([tile] project › branch); else what stands in its place, "No
-- project".
local function chip(app: any): string
	rested(app)
	if app:has('role:button "Branch" >> #composer-place-branch') then
		return app:text('role:button "Branch" >> #composer-place-branch[1]')
	end
	return app:text('role:button "Branch" >> #composer-place-project[1]')
end

local function send(app: any, words: string)
	app:key("Escape")
	app:key("Escape")
	settle(app)
	app:type('role:textbox "Message"', words)
	app:key("Enter")
	settle(app)
end

test("a new chat names the project's current branch, read from git where it is", function()
	local app = mount("chat_fixture.luau")
	home(app)
	expect(app:state("calls")).to_contain("refs:/tmp/atlas:here")
	expect(chip(app)).to_be("main")
	panel(app)
	-- Its branches, the current first, tagged.
	expect(app:has('role:option "main Current"')).to_be(true)
	expect(app:has('role:option "fix/render-loop Worktree"')).to_be(true)
	expect(app:has('role:option "release/2.4"')).to_be(true)
	-- A branch picked: the chip names it.
	app:click('role:option "release/2.4"')
	settle(app)
	expect(chip(app)).to_be("release/2.4")
end)

test("another device brings its projects and their branches", function()
	local app = mount("chat_fixture.luau")
	home(app)
	pick_in(app, "Device", "Build box")
	expect(chosen(app, "Device")).to_contain("Build box")
	expect(chosen(app, "Project")).to_contain("Borealis")
	-- The box's project's branches, asked of the box.
	expect(app:state("calls")).to_contain("refs:/srv/borealis:box")
	expect(chip(app)).to_be("trunk")
	-- Its projects are the box's alone.
	panel(app)
	app:click('role:menuitem "Project"')
	settle(app)
	expect(app:has('role:option ~"Borealis"')).to_be(true)
	expect(app:has('role:option ~"Atlas"')).to_be(false)
	-- And back: this computer's, the project last used there.
	app:key("Escape")
	settle(app)
	pick_in(app, "Device", "MacBook Pro")
	expect(chosen(app, "Project")).to_contain("Atlas")
	expect(chip(app)).to_be("main")
end)

test("a device out of reach shows why, and can't be picked", function()
	local app = mount("chat_fixture.luau")
	home(app)
	panel(app)
	app:click('role:menuitem "Device"')
	settle(app)
	expect(app:has('role:option ~"Old laptop" >> ~"Offline · last seen"')).to_be(true)
	app:click('role:option ~"Old laptop"')
	settle(app)
	expect(chosen(app, "Device")).to_contain("MacBook Pro")
end)

test("the branch search filters the branches", function()
	local app = mount("chat_fixture.luau")
	home(app)
	panel(app)
	app:type('role:textbox "Search branches…"', "rel")
	settle(app)
	expect(app:has('role:option "release/2.4"')).to_be(true)
	expect(app:has('role:option "main Current"')).to_be(false)
	app:type('role:textbox "Search branches…"', "zzz")
	settle(app)
	expect(app:has('~"No branch matches “relzzz”"')).to_be(true)
end)

test("a send in a new worktree goes off the chosen ref", function()
	local app = mount("chat_fixture.luau")
	home(app)
	panel(app)
	app:click('role:option "release/2.4"')
	settle(app)
	pick_in(app, "Checkout", "Worktree An isolated")
	send(app, "Cut the release")
	local calls = app:state("calls")
	expect(calls).to_contain("made:atlas|-|release/2.4|-")
	expect(calls).to_contain("run:/tmp/atlas|worktree|release/2.4")
end)

test("a folder with no git runs in place, though a worktree was picked", function()
	local app = mount("chat_fixture.luau")
	home(app)
	pick_in(app, "Checkout", "Worktree An isolated")
	pick_in(app, "Project", "Notes")
	expect(showing(app, 'role:menuitem "Checkout"')).to_be(false)
	expect(app:has("#composer-place-lead >> .place-lead-folder")).to_be(true)
	expect(app:has("#composer-place-lead >> .place-lead-worktree")).to_be(false)
	send(app, "Tidy the notes")
	local calls = app:state("calls")
	expect(calls).to_contain("made:notes|-|-|/tmp/notes")
	expect(calls).to_contain("run:/tmp/notes|local|-")
end)

test("a worktree picked waits out a folder with no git for the next repository", function()
	local app = mount("chat_fixture.luau")
	home(app)
	pick_in(app, "Checkout", "Worktree An isolated")
	pick_in(app, "Project", "Notes")
	pick_in(app, "Project", "Atlas")
	panel(app)
	expect(showing(app, 'role:menuitem "Checkout"')).to_be(true)
	expect(app:has("#composer-place-lead >> .place-lead-worktree")).to_be(true)
	send(app, "Cut the release")
	local calls = app:state("calls")
	expect(calls).to_contain("run:/tmp/atlas|worktree|main")
end)

test("a local checkout of a ref with its own worktree runs there", function()
	local app = mount("chat_fixture.luau")
	home(app)
	pick_in(app, "Checkout", "Local checkout Work in")
	panel(app)
	app:click('role:option "fix/render-loop Worktree"')
	settle(app)
	send(app, "Look at the loop")
	local calls = app:state("calls")
	expect(calls).to_contain("made:atlas|-|fix/render-loop|/tmp/atlas-wt/render-loop")
	expect(calls).to_contain("run:/tmp/atlas-wt/render-loop|local|fix/render-loop")
end)

test("a send on another device's project goes to that project there", function()
	local app = mount("chat_fixture.luau")
	home(app)
	pick_in(app, "Device", "Build box")
	pick_in(app, "Checkout", "Local checkout Work in")
	send(app, "Sync it")
	local calls = app:state("calls")
	expect(calls).to_contain("made:borealis|-|trunk|/srv/borealis")
	expect(calls).to_contain("run:/srv/borealis|local|trunk")
end)

test("no project: made on the device chosen, no ref", function()
	local app = mount("chat_fixture.luau")
	home(app)
	panel(app)
	app:click('role:menuitem "Project"')
	settle(app)
	app:click('~"Don\'t work in a project"')
	settle(app)
	expect(chip(app)).to_contain("No project")
	send(app, "Just a question")
	local calls = app:state("calls")
	expect(calls).to_contain("made:-|mac|-|-")
	expect(calls).to_contain("run:-|local|-")
end)

test("picking a device leaves the panel open, on to its project", function()
	local app = mount("chat_fixture.luau")
	home(app)
	pick_in(app, "Device", "Build box")
	expect(chosen(app, "Device")).to_contain("Build box")
	-- Still there: its list gone, the rows to go on from.
	expect(showing(app, 'role:menuitem "Device"')).to_be(true)
	expect(showing(app, 'role:menuitem "Project"')).to_be(true)
	expect(showing(app, 'role:option ~"Build box"')).to_be(false)
	app:click('role:menuitem "Project"')
	settle(app)
	expect(showing(app, 'role:menuitem "Project"')).to_be(true)
end)

test("the device comes first: on top of the panel, and first on the chip", function()
	local app = mount("chat_fixture.luau")
	home(app)
	expect(app:text("#composer-place-device[1]")).to_contain("MacBook Pro")
	panel(app)
	local function top(sel: string): number
		local first = string.match(app:tree(sel), "[^\n]+") or ""
		return tonumber(string.match(first, " %-?%d+,(%-?%d+) ")) or 0
	end
	expect(top('role:menuitem "Device"') < top('role:menuitem "Project"')).to_be(true)
	pick_in(app, "Device", "Build box")
	-- (Its name rolls in, a letter at a time: wait for it.)
	app:wait('#composer-place-device >> ~"Build"', { timeout_ms = 2000 })
	expect(string.find(app:tree("#composer-place-device[1]"), "Build", 1, true) ~= nil).to_be(true)
end)
