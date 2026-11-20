-- Settings → Devices and Settings → Projects: a row renamed where it
-- stands (Enter saves, Escape cancels, nothing moves), another device
-- removed from the account after a confirm that says what stays, this one
-- and one online never; a project removed after a confirm that says its
-- folder stays, its running sessions stopped first. And each page's
-- loading, empty and no-engine states. On the shell in ui/shell_fixture.luau
-- (the sessions as the engine sends them), Settings' calls answered by
-- ui/settings_stub.luau.

local PAGE = "#settings-page"
local NAV = "#settings-nav"

local function open(page: string, before: { string }?): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	for _, c in before or {} do
		app:command(c)
	end
	app:command("shell.settings")
	app:frames(20)
	app:click(`{NAV} >> role:tab "{page}"`)
	app:frames(30)
	return app
end

local function shows(app: any, scope: string, text: string): boolean
	return app:tree(scope):find(text, 1, true) ~= nil
end

-- What the shell asked of the sessions (the fixture's `calls`), and
-- Settings' own (the stub's).
local function said(app: any): string
	return (app:state("calls") :: any) or ""
end
local function asked(app: any, entry: string): boolean
	for _, c in (app:state("settings_stub_calls") :: any) or {} do
		if c == entry then
			return true
		end
	end
	return false
end

-- A node's box: the first line of its tree.
local function box(app: any, target: string): (number, number, number, number)
	local line = string.match(app:tree(target), "[^\n]+") or ""
	local x, y, w, h = line:match(" (%-?%d+),(%-?%d+) (%d+)x(%d+)")
	return tonumber(x) or 0, tonumber(y) or 0, tonumber(w) or 0, tonumber(h) or 0
end

-- A row's ⋯ opened, its menu settled.
local function menu(app: any, name: string)
	app:click(`{PAGE} >> role:button "More for {name}"`)
	expect(app:wait('role:menu >> role:button "Rename"', { timeout_ms = 15000 })).to_be(true)
	app:wait('"nothing"', { timeout_ms = 400 })
end

-- Devices ─────────────────────────────────────────────────────────────────

test("a device renames in place: Enter saves, Escape cancels, and nothing moves", function()
	local app = open("Devices")
	local _, y0, _, h0 = box(app, `{PAGE} >> "devbox"`)
	local _, ny, _, nh = box(app, `{PAGE} >> "MacBook Pro"`)
	menu(app, "devbox")
	app:click('role:menu >> role:button "Rename"')
	app:frames(10)
	-- The field stands where the name was, holding it, and the rows stay put.
	expect(app:has(`{PAGE} >> "Name for devbox"`)).to_be(true)
	expect(app:text(`{PAGE} >> "Name for devbox"`)).to_be("devbox")
	local _, fy, _, fh = box(app, `{PAGE} >> "Name for devbox"`)
	expect(math.abs((fy + fh / 2) - (y0 + h0 / 2)) <= 12).to_be(true)
	local _, ny2, _, nh2 = box(app, `{PAGE} >> "MacBook Pro"`)
	expect(ny2).to_be(ny)
	expect(nh2).to_be(nh)
	-- Escape: as it was, nothing asked.
	app:fill(`{PAGE} >> "Name for devbox"`, "nope")
	app:key("Escape")
	app:frames(10)
	expect(app:has(`{PAGE} >> "Name for devbox"`)).to_be(false)
	expect(shows(app, PAGE, "nope")).to_be(false)
	expect(asked(app, "sessions.rename_device:box:nope")).to_be(false)
	-- Enter: saved, shown at once.
	menu(app, "devbox")
	app:click('role:menu >> role:button "Rename"')
	app:frames(10)
	app:fill(`{PAGE} >> "Name for devbox"`, "  build-box ")
	app:key("Enter")
	app:frames(10)
	expect(asked(app, "sessions.rename_device:box:build-box")).to_be(true)
	expect(app:has(`{PAGE} >> "Name for devbox"`)).to_be(false)
	expect(shows(app, PAGE, "build-box")).to_be(true)
end)

test("an empty name can't be saved", function()
	local app = open("Devices")
	menu(app, "devbox")
	app:click('role:menu >> role:button "Rename"')
	app:frames(10)
	app:fill(`{PAGE} >> "Name for devbox"`, "   ")
	app:frames(5)
	expect(app:tree(`{PAGE} >> role:button "Save"`)).to_contain("disabled")
	app:key("Enter")
	app:frames(10)
	expect(app:has(`{PAGE} >> "Name for devbox"`)).to_be(true)
end)

test("another device is removed after a confirm that says what stays", function()
	local app = open("Devices")
	menu(app, "devbox")
	app:click('role:menu >> role:button "Remove…"')
	app:frames(20)
	local dialog = 'role:dialog "Remove “devbox”?"'
	expect(app:has(dialog)).to_be(true)
	expect(app:has(`{dialog} >> ~"Its sessions and projects stay"`)).to_be(true)
	expect(app:has(`{dialog} >> ~"If Zeron opens on it again, it comes back."`)).to_be(true)
	app:click(`{dialog} >> role:button "Remove device"`)
	app:frames(30)
	expect(said(app)).to_contain("remove_device:box")
	expect(app:state("settings_device_removing_open")).to_be(false)
	expect(shows(app, "#setting-devices", "devbox")).to_be(false)
	expect(shows(app, PAGE, "MacBook Pro")).to_be(true)
end)

test("this device can't be removed, and one online says why it can't yet", function()
	local app = open("Devices", { "fixture.box_online" })
	menu(app, "MacBook Pro")
	expect(app:has('role:menu >> role:button "Rename"')).to_be(true)
	expect(app:has('role:menu >> role:button "Remove…"')).to_be(false)
	app:key("Escape")
	app:frames(10)
	menu(app, "devbox")
	expect(app:has('role:menu >> role:button "Remove…"')).to_be(false)
	expect(app:tree('role:menu >> "Remove…"')).to_contain("disabled")
	expect(shows(app, "role:menu", "Online now. Quit Zeron on it first.")).to_be(true)
	app:frames(20)
	expect(app:has('role:dialog "Remove “devbox”?"')).to_be(false)
	expect(said(app)).never.to_contain("remove_device")
end)

test("Devices: loading, none and no engine each say so", function()
	local app = open("Devices", { "fixture.nothing_heard" })
	expect(app:has(`{PAGE} >> "Loading…"`)).to_be(true)
	expect(shows(app, PAGE, "No devices yet")).to_be(false)
	app = open("Devices", { "fixture.no_devices" })
	expect(shows(app, PAGE, "No devices yet")).to_be(true)
	app = open("Devices", { "fixture.engine_offline" })
	expect(shows(app, PAGE, "Can't show your devices")).to_be(true)
	expect(shows(app, PAGE, "Zeron isn't running on this computer.")).to_be(true)
end)

-- Projects ────────────────────────────────────────────────────────────────

test("Projects lists each device's projects under it, this one first", function()
	local app = open("Projects")
	expect(app:has(`{NAV} >> role:tab "Projects"`)).to_be(true)
	local _, mac_y = box(app, `{PAGE} >> "MacBook Pro"`)
	local _, box_y = box(app, `{PAGE} >> "devbox"`)
	expect(mac_y < box_y).to_be(true)
	-- Its sessions, archived ones too, then its folder.
	expect(shows(app, PAGE, "4 sessions · /work/atlas")).to_be(true)
	expect(shows(app, PAGE, "2 sessions · /srv/borealis")).to_be(true)
	expect(shows(app, PAGE, "This device")).to_be(true)
end)

test("a project renames in place, Enter saving", function()
	local app = open("Projects")
	menu(app, "Atlas")
	app:click('role:menu >> role:button "Rename"')
	app:frames(10)
	expect(app:text(`{PAGE} >> "Name for Atlas"`)).to_be("Atlas")
	app:fill(`{PAGE} >> "Name for Atlas"`, "Atlas Two")
	app:key("Enter")
	app:frames(10)
	expect(said(app)).to_contain("rename_project:atlas:Atlas Two")
	expect(shows(app, PAGE, "Atlas Two")).to_be(true)
	expect(app:has(`{PAGE} >> "Name for Atlas"`)).to_be(false)
end)

test("a project is removed after a confirm that says its folder stays, its running session stopped first", function()
	local app = open("Projects")
	menu(app, "Atlas")
	app:click('role:menu >> role:button "Remove…"')
	app:frames(20)
	local dialog = 'role:dialog "Remove “Atlas”?"'
	expect(app:has(dialog)).to_be(true)
	expect(app:has(`{dialog} >> ~"Its 4 sessions are deleted for good, on every device."`)).to_be(true)
	expect(app:has(`{dialog} >> ~"One of them is running now and is stopped first."`)).to_be(true)
	expect(app:has(`{dialog} >> ~"The folder /work/atlas stays on MacBook Pro with all its files. Zeron just forgets it."`)).to_be(true)
	app:click(`{dialog} >> role:button "Remove project"`)
	app:frames(30)
	local calls = said(app)
	local stop_at = string.find(calls, "stop:a1", 1, true)
	local delete_at = string.find(calls, "delete_project:atlas", 1, true)
	expect(stop_at ~= nil and delete_at ~= nil and stop_at < delete_at).to_be(true)
	expect(app:state("settings_project_removing_open")).to_be(false)
	expect(shows(app, PAGE, "/work/atlas")).to_be(false)
	expect(shows(app, PAGE, "Borealis")).to_be(true)
end)

test("Show in Finder is offered for this device's folders only", function()
	local app = open("Projects")
	menu(app, "Atlas")
	app:click('role:menu >> role:button "Show in Finder"')
	app:frames(10)
	expect(said(app)).to_contain("reveal:atlas:/work/atlas:")
	menu(app, "Borealis")
	-- (The menu open now: the one shown first.)
	expect(shows(app, "role:menu", "Rename")).to_be(true)
	expect(shows(app, "role:menu", "Show in Finder")).to_be(false)
end)

test("the search finds projects by name or folder, and says when none match", function()
	local app = open("Projects")
	app:fill(`{PAGE} >> "Find a project"`, "srv")
	app:frames(20)
	expect(shows(app, PAGE, "Borealis")).to_be(true)
	expect(shows(app, PAGE, "/work/atlas")).to_be(false)
	app:fill(`{PAGE} >> "Find a project"`, "zzz")
	app:frames(20)
	expect(shows(app, PAGE, "No projects match")).to_be(true)
end)

test("Projects: loading, none and no engine each say so", function()
	local app = open("Projects", { "fixture.nothing_heard" })
	expect(app:has(`{PAGE} >> "Loading…"`)).to_be(true)
	expect(shows(app, PAGE, "No projects yet")).to_be(false)
	app = open("Projects", { "fixture.no_sessions" })
	expect(shows(app, PAGE, "No projects yet")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Add project"`)).to_be(true)
	app = open("Projects", { "fixture.engine_offline" })
	expect(shows(app, PAGE, "Can't show your projects")).to_be(true)
end)

test("a project's icon and colour change from its row, in the window's one Edit project", function()
	local app = open("Projects")
	local dialog = 'role:dialog "Edit project"'
	-- Its tile, at a click.
	app:click(`{PAGE} >> role:button "Change icon for Borealis"`)
	app:frames(40)
	expect(app:has(dialog)).to_be(true)
	expect(app:text(`{dialog} >> role:textbox`)).to_be("Borealis")
	app:key("Escape")
	app:frames(30)
	-- Or its ⋯: an emoji on amber, saved, and shown on its row.
	menu(app, "Borealis")
	app:click('role:menu >> role:button "Change icon…"')
	app:frames(40)
	app:click(`{dialog} >> role:radio "Emoji"`)
	app:frames(20)
	app:fill(`#project-edit-emoji >> role:textbox`, "rocket")
	app:frames(10)
	app:key("Enter")
	app:frames(5)
	app:click(`{dialog} >> role:radio "Amber"`)
	app:frames(5)
	app:click(`{dialog} >> role:button "Save"`)
	app:frames(30)
	expect(said(app)).to_contain("set_project_icon:borealis:🚀::amber")
	expect(app:state("project_edit_open")).to_be(false)
end)

test("an edge without image icons says so plainly, and an emoji still saves", function()
	local app = open("Projects")
	local dialog = 'role:dialog "Edit project"'
	app:click(`{PAGE} >> role:button "Change icon for Borealis"`)
	app:frames(40)
	app:click(`{dialog} >> role:radio "Image"`)
	app:frames(20)
	app:copy_file("assets/favicons/zeron.png", "unavailable.png")
	app:pick({ "unavailable.png" })
	app:click(`#project-edit-drop >> role:button "Choose an image"`)
	app:frames(30)
	app:click(`{dialog} >> role:button "Save"`)
	app:frames(30)
	expect(app:has(dialog)).to_be(true)
	expect(app:has(`#project-edit-error >> "Image icons aren't available for your account yet. Choose an emoji or a color instead."`)).to_be(true)
	expect(said(app)).never.to_contain("set_project_icon")
	app:click(`{dialog} >> role:radio "Emoji"`)
	app:frames(20)
	app:fill(`#project-edit-emoji >> role:textbox`, "rocket")
	app:frames(10)
	app:key("Enter")
	app:frames(5)
	app:click(`{dialog} >> role:button "Save"`)
	app:frames(30)
	expect(said(app)).to_contain("set_project_icon:borealis:🚀::")
	expect(app:state("project_edit_open")).to_be(false)
end)

test("the confirm says how many of a project's sessions are running, or that its only one is", function()
	local app = open("Projects", { "fixture.running_projects" })
	menu(app, "Atlas")
	app:click('role:menu >> role:button "Remove…"')
	app:frames(20)
	local dialog = 'role:dialog "Remove “Atlas”?"'
	expect(app:has(`{dialog} >> ~"2 of them are running now and are stopped first."`)).to_be(true)
	app:key("Escape")
	app:wait('"nothing"', { timeout_ms = 400 })
	menu(app, "Solo")
	app:click('role:menu >> role:button "Remove…"')
	app:frames(20)
	dialog = 'role:dialog "Remove “Solo”?"'
	expect(app:has(`{dialog} >> ~"Its session is deleted for good, on every device. It's running now and is stopped first."`)).to_be(true)
end)

test("This device stands right after its name on Projects, as on Devices", function()
	local app = open("Projects")
	local nx, ny, nw = box(app, `{PAGE} >> role:heading "MacBook Pro"`)
	local bx, by = box(app, `{PAGE} >> "This device"`)
	expect(bx > nx + nw and bx - (nx + nw) <= 12).to_be(true)
	expect(math.abs(by - ny) <= 6).to_be(true)
end)
