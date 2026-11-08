-- Cloud in a new chat's Work in (screens/chat/where): offered beside your
-- computers only while Cloud is on and can open the project's GitHub
-- repository; picked, the branches are GitHub's, there's no checkout to
-- choose, and a send makes the chat on Cloud and runs it in its machine's
-- checkout. On ui/chat_fixture.luau (Atlas is acme/atlas on GitHub), Cloud
-- answered by ui/cloud_stub.luau; the fixture notes `made:…|cloud` for a
-- chat made on Cloud and `run:cwd|checkout|ref` for its first run.

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

local function home(app: any)
	app:resize(960, 760)
	app:command("go_home")
	settle(app)
end

local function panel(app: any)
	if not showing(app, 'role:button "Device"') then
		app:click('role:button "Branch"')
		settle(app)
	end
end

local function open_row(app: any, row: string)
	panel(app)
	app:click(`role:button "{row}"`)
	settle(app)
end

local function pick_in(app: any, row: string, option: string)
	open_row(app, row)
	app:click(`role:option ~"{option}"`)
	settle(app)
end

local function chosen(app: any, row: string): string
	rested(app)
	local words = {}
	for line in app:tree(`role:button "{row}"`):gmatch("[^\n]+") do
		local t = line:match('text "([^"]*)"')
		if t then
			table.insert(words, t)
		end
	end
	return table.concat(words, " ")
end

local function chip(app: any): string
	rested(app)
	-- (The chip: [tile] project › branch. The branch, in a project; else
	-- what stands in its place, "No project".)
	if app:has('role:button "Branch" >> #composer-place-branch') then
		return app:text('role:button "Branch" >> #composer-place-branch[1]')
	end
	return app:text('role:button "Branch" >> #composer-place-project[1]')
end
local function close(app: any)
	app:key("Escape")
	app:key("Escape")
	settle(app)
end

local function cloud_calls(app: any): string
	return table.concat((app:state("cloud_stub_calls") :: any) or {}, ",")
end

test("Cloud is offered for a project it can open, and only while it's on", function()
	local app = mount("chat_fixture.luau")
	home(app)
	-- No Cloud for the account yet: your computers alone.
	open_row(app, "Device")
	expect(app:has('role:option ~"Cloud"')).to_be(false)
	close(app)
	app:command("cloud_stub.offer")
	settle(app)
	open_row(app, "Device")
	expect(app:has('role:option ~"Cloud"')).to_be(true)
	expect(app:has('role:option ~"Cloud" >> ~"A machine of its own, from acme/atlas"')).to_be(true)
	close(app)
	-- A project Cloud can't open (no GitHub repository): no Cloud.
	pick_in(app, "Project", "Notes")
	open_row(app, "Device")
	expect(app:has('role:option ~"Cloud"')).to_be(false)
end)

test("on Cloud, the branches are GitHub's and there's no checkout to choose", function()
	local app = mount("chat_fixture.luau")
	home(app)
	app:command("cloud_stub.offer")
	settle(app)
	pick_in(app, "Device", "Cloud")
	expect(chosen(app, "Device")).to_contain("Cloud")
	expect(chosen(app, "Project")).to_contain("Atlas")
	expect(cloud_calls(app)).to_contain("cloud.branches:acme/atlas")
	expect(chip(app)).to_be("main")
	panel(app)
	expect(showing(app, 'role:button "Checkout"')).to_be(false)
	expect(app:has('role:option "main Default"')).to_be(true)
	expect(app:has('role:option "release/3.0"')).to_be(true)
	-- On Cloud, the projects are those it can open.
	app:click('role:button "Project"')
	settle(app)
	expect(app:has('role:option ~"Atlas"')).to_be(true)
	expect(app:has('role:option ~"Notes"')).to_be(false)
	expect(app:has('role:option ~"Borealis"')).to_be(false)
	app:key("Escape")
	settle(app)
	-- And back to this computer: its checkout again, git's branches.
	pick_in(app, "Device", "MacBook Pro")
	expect(chosen(app, "Project")).to_contain("Atlas")
	panel(app)
	expect(showing(app, 'role:button "Checkout"')).to_be(true)
end)

test("a send on Cloud makes the chat there and runs it in its machine's checkout", function()
	local app = mount("chat_fixture.luau")
	home(app)
	app:command("cloud_stub.offer")
	settle(app)
	pick_in(app, "Device", "Cloud")
	expect(chip(app)).to_be("main")
	close(app)
	app:type('role:textbox "Message"', "Fix the flaky test")
	app:key("Enter")
	settle(app)
	local calls = app:state("calls")
	expect(calls).to_contain("made:atlas|-|main|-|cloud")
	expect(calls).to_contain("run:/home/user/atlas|local|main")
end)

test("Cloud turned off while picked falls back to the project's device", function()
	local app = mount("chat_fixture.luau")
	home(app)
	app:command("cloud_stub.offer")
	settle(app)
	pick_in(app, "Device", "Cloud")
	expect(chosen(app, "Device")).to_contain("Cloud")
	close(app)
	app:command("cloud_stub.no_github")
	settle(app)
	expect(chosen(app, "Device")).to_contain("MacBook Pro")
	expect(chosen(app, "Project")).to_contain("Atlas")
end)
