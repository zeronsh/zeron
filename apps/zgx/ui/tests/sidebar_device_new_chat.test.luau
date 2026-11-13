-- A device's group inside a project's (Organize by project, then device)
-- has the project header's "+" under the pointer: a new chat in that
-- project, on that device, the composer's device picked for it. On the
-- shell in ui/shell_fixture.luau.

local SIDE = "#shell-sidebar"
local K = "screens/shell/sidebar.luau::"

local function open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 1000)
	app:frames(30)
	app:set_state(`{K}shell_sidebar_organize`, { "project", "device" })
	app:set_state(`{K}shell_sidebar_folded`, {})
	app:frames(40)
	return app
end

-- The `label` button that first follows `after`'s in the sidebar (a device's
-- header under its project's), as `@id`.
local function under(app: any, after: string, label: string): string
	local past = false
	for line in app:tree(SIDE):gmatch("[^\n]+") do
		if line:find(` button "{after}"`, 1, true) then
			past = true
		elseif past and line:find(` button "{label}"`, 1, true) then
			return "@" .. (line:match("@(%d+)") :: string)
		end
	end
	error(`no "{label}" under "{after}" in the sidebar`)
end

local function plus(label: string): string
	return `{SIDE} >> role:button "{label}"`
end

test("a device's header under its project shows the project's + and starts a new chat there", function()
	local app = open()
	-- From a session: Fix the build, Atlas on this computer.
	app:set_state("shell_selected", "a1")
	app:frames(30)
	expect(app:state("shell_route_path")).to_be("/session/a1")
	expect(app:has(plus("New chat in Borealis on devbox"))).to_be(false)

	-- Under the pointer: the same "+" as its project's header, saying where.
	app:hover(under(app, "Borealis", "devbox"))
	app:frames(20)
	expect(app:has(plus("New chat in Borealis on devbox"))).to_be(true)
	-- (Its project's header keeps its own.)
	app:hover(`{SIDE} >> role:button "Borealis"`)
	app:frames(20)
	expect(app:has(plus("New chat in Borealis"))).to_be(true)

	app:hover(under(app, "Borealis", "devbox"))
	app:frames(20)
	app:click(plus("New chat in Borealis on devbox"))
	app:frames(40)
	expect(app:state("shell_route_path")).to_be("/")
	expect(app:state("shell_project")).to_be("borealis")
	expect(app:state("chat_project")).to_be("borealis")
	expect(app:state("chat_device")).to_be("box")
end)

test("on a new chat already, a device's + picks its device: the project's folder there", function()
	local app = open()
	app:command("atlas_on_box")
	app:command("atlas_twin_on_box")
	app:frames(40)
	-- A new chat in Atlas, on this computer (the fixture starts on one).
	app:set_state("shell_project", "atlas")
	app:frames(30)
	expect(app:state("shell_route_path")).to_be("/")
	expect(app:state("chat_device")).to_be("mac")

	-- Atlas's devbox group (its session there): Atlas's folder on devbox.
	app:hover(under(app, "Atlas", "devbox"))
	app:frames(20)
	app:click(plus("New chat in Atlas on devbox"))
	app:frames(40)
	expect(app:state("shell_route_path")).to_be("/")
	expect(app:state("chat_device")).to_be("box")
	expect(app:state("chat_project")).to_be("atlas-box")
	-- (Taken: the next new chat isn't moved by it.)
	expect(app:state("shell_device_pick")).to_be(nil)

	-- Back on this computer, from Atlas's own.
	app:hover(under(app, "Atlas", "MacBook Pro"))
	app:frames(20)
	app:click(plus("New chat in Atlas on MacBook Pro"))
	app:frames(40)
	expect(app:state("chat_device")).to_be("mac")
	expect(app:state("chat_project")).to_be("atlas")
end)

test("a device the project has no folder on: that device, the composer picking its project there", function()
	local app = open()
	app:command("atlas_on_box")
	app:frames(40)
	app:set_state("shell_project", "atlas")
	app:frames(30)
	expect(app:state("chat_device")).to_be("mac")

	app:hover(under(app, "Atlas", "devbox"))
	app:frames(20)
	app:click(plus("New chat in Atlas on devbox"))
	app:frames(40)
	expect(app:state("chat_device")).to_be("box")
	-- (Devbox's own: Borealis, its only project.)
	expect(app:state("chat_project")).to_be("borealis")
end)
