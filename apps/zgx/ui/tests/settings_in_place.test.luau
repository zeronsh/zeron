-- Settings' pages follow their data in place: a member joining, a role
-- changed, an invitation revoked, your name saved leave every other row
-- (and the page) the same node; a change the edge refuses goes back where
-- it was. The search finds pages and single settings and opens them; the
-- invite dialog sends and keeps what didn't go; signing out ends at setup.
-- On the shell in ui/shell_fixture.luau, Settings' host calls answered by
-- ui/settings_stub.luau.

-- A node's identity under `scope`, by its kind and label: `@id`.
local function node(app: any, scope: string, kind: string, label: string): string?
	for line in app:tree(scope):gmatch("[^\n]+") do
		if line:find(` {kind} "{label}"`, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

-- Whether `text` shows anywhere under `scope`.
local function shows(app: any, scope: string, text: string): boolean
	return app:tree(scope):find(text, 1, true) ~= nil
end

local function calls(app: any): { string }
	return (app:state("settings_stub_calls") :: any) or {}
end

local function asked(app: any, entry: string): boolean
	for _, c in calls(app) do
		if c == entry then
			return true
		end
	end
	return false
end

local PAGE = "#settings-page"
local NAV = "#settings-nav"

local function open(page: string?): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("shell.settings")
	app:frames(20)
	if page then
		app:click(`{NAV} >> role:tab "{page}"`)
		app:frames(30)
	end
	return app
end

test("the pages list under Personal, Devices and Organization", function()
	local app = open()
	for _, label in { "Personal", "Devices", "Organization", "General", "Appearance", "Notifications", "Account", "Providers", "Members" } do
		expect(app:has(`{NAV} >> "{label}"`)).to_be(true)
	end
	-- The account's page is "Account" (its id still `profile`): no "Profile".
	expect(app:has(`{NAV} >> "Profile"`)).to_be(false)
	expect(app:state("shell_route_path")).to_be("/settings/general")
	expect(shows(app, PAGE, "Send messages with")).to_be(true)
	expect(shows(app, PAGE, "Credit Zeron in commits")).to_be(true)
end)

test("the search finds a page and a setting, and opens the setting's page at it", function()
	local app = open()
	app:type(`{NAV} >> #settings-search`, "task completed")
	app:frames(10)
	-- The results stand in the pages' place.
	expect(app:has(`{NAV} >> "General"`)).to_be(false)
	expect(app:has(`#settings-search-results >> "Task completed"`)).to_be(true)
	expect(shows(app, "#settings-search-results", "Notifications · Sounds")).to_be(true)
	app:key("Enter")
	app:frames(30)
	expect(app:state("shell_route_path")).to_be("/settings/notifications")
	expect(app:state("settings_search_lit")).to_be("sound_done")
	-- A word that misses nothing else finds the page itself.
	app:fill(`{NAV} >> "Search settings"`, "members")
	app:frames(10)
	expect(app:has(`#settings-search-results >> "Members"`)).to_be(true)
	app:fill(`{NAV} >> "Search settings"`, "zzzz")
	app:frames(10)
	expect(app:has(`#settings-search-results >> "No settings match."`)).to_be(true)
	-- Cleared, the pages come back.
	app:fill(`{NAV} >> "Search settings"`, "")
	app:frames(10)
	expect(app:has(`{NAV} >> "General"`)).to_be(true)
end)

test("a member joining and a role changed leave the other rows the same nodes", function()
	local app = open("Members")
	expect(asked(app, "account.members:org_1")).to_be(true)
	local grace = node(app, PAGE, "text", "Grace Hopper")
	local alan = node(app, PAGE, "text", "Alan Turing")
	expect(grace).never.to_be_nil()
	expect(alan).never.to_be_nil()
	app:command("settings_stub.member_joins")
	app:frames(20)
	expect(shows(app, PAGE, "Edsger Dijkstra")).to_be(true)
	expect(node(app, PAGE, "text", "Grace Hopper")).to_be(grace)
	expect(node(app, PAGE, "text", "Alan Turing")).to_be(alan)
	-- Grace made an admin: her row stays hers.
	app:click(`{PAGE} >> "Grace Hopper's role"`)
	app:frames(10)
	app:click('role:menuitem "Admin"')
	app:frames(20)
	expect(asked(app, "account.set_role:om_grace")).to_be(true)
	expect(node(app, PAGE, "text", "Grace Hopper")).to_be(grace)
	expect(node(app, PAGE, "text", "Alan Turing")).to_be(alan)
end)

test("an invitation revoked goes at once, and comes back if the edge refuses", function()
	local app = open("Members")
	expect(shows(app, PAGE, "edsger@example.net")).to_be(true)
	expect(shows(app, PAGE, "barbara@example.net")).to_be(true)
	local barbara = node(app, PAGE, "text", "barbara@example.net")
	-- Refused: the row comes back, and a toast says why.
	app:command("settings_stub.refuse_next")
	app:click(`{PAGE} >> role:button "Revoke"[1]`)
	app:frames(20)
	expect(shows(app, PAGE, "edsger@example.net")).to_be(true)
	expect(app:has('"Couldn\'t revoke the invitation"')).to_be(true)
	-- Taken: gone, the other row the same node.
	app:click(`{PAGE} >> role:button "Revoke"[1]`)
	app:frames(20)
	expect(shows(app, PAGE, "edsger@example.net")).to_be(false)
	expect(node(app, PAGE, "text", "barbara@example.net")).to_be(barbara)
end)

test("inviting sends, keeps those that didn't go, and lists the rest as invited", function()
	local app = open("Members")
	app:click(`{PAGE} >> role:button "Invite"`)
	app:frames(20)
	app:type('"Email addresses"', "linus@example.net, taken@example.net,")
	app:frames(5)
	app:click('role:button "Send invites"')
	app:frames(20)
	expect(asked(app, "account.invite:2")).to_be(true)
	-- The one that didn't go stays in the field, saying why.
	expect(app:has('"Already a member or invited."')).to_be(true)
	expect(shows(app, PAGE, "linus@example.net")).to_be(true)
end)

test("a member who isn't an admin sees roles, not the ways to change them", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("settings_stub.not_admin")
	app:command("shell.settings")
	app:frames(20)
	app:click(`{NAV} >> "Members"`)
	app:frames(30)
	expect(shows(app, PAGE, "Grace Hopper")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Invite"`)).to_be(false)
	expect(app:has(`{PAGE} >> "Grace Hopper's role"`)).to_be(false)
	expect(asked(app, "account.invitations:org_1")).to_be(false)
end)

test("your name saves in place, the field and the page staying the same nodes", function()
	local app = open("Account")
	expect(app:text(`{PAGE} >> role:textbox "Name"`)).to_be("Ada Lovelace")
	local field = node(app, PAGE, "textbox", "Name")
	app:fill(`{PAGE} >> role:textbox "Name"`, "Ada King")
	app:frames(5)
	app:click(`{PAGE} >> role:button "Save changes"`)
	app:frames(20)
	expect(asked(app, "account.rename:Ada King")).to_be(true)
	expect(node(app, PAGE, "textbox", "Name")).to_be(field)
	expect(app:text(`{PAGE} >> role:textbox "Name"`)).to_be("Ada King")
	-- Refused: back to what it was.
	app:command("settings_stub.refuse_next")
	app:fill(`{PAGE} >> role:textbox "Name"`, "Countess")
	app:frames(5)
	app:click(`{PAGE} >> role:button "Save changes"`)
	app:frames(20)
	expect(app:has('"Couldn\'t save your name"')).to_be(true)
	expect(node(app, PAGE, "textbox", "Name")).to_be(field)
end)

test("an engine switch flips at once, and back if refused", function()
	local app = open()
	local credit = `{PAGE} >> role:switch "Credit Zeron in commits"`
	expect(app:tree(credit)).to_contain("unchecked")
	-- (Under General's other rows: brought into view.)
	app:scroll(PAGE, 600)
	app:frames(4)
	app:click(credit)
	app:frames(10)
	expect(asked(app, "settings.engine:SetCommitCredit")).to_be(true)
	expect(app:tree(credit)).never.to_contain("unchecked")
	-- Refused: it goes back on.
	app:command("settings_stub.refuse_next")
	app:click(credit)
	app:frames(10)
	expect(app:tree(credit)).never.to_contain("unchecked")
end)

test("opening a page asks once a visit, not again as it builds again", function()
	local app = open("Members")
	local n = 0
	for _, c in calls(app) do
		n += if c == "account.orgs" then 1 else 0
	end
	expect(n).to_be(1)
	app:frames(30)
	local again = 0
	for _, c in calls(app) do
		again += if c == "account.orgs" then 1 else 0
	end
	expect(again).to_be(1)
end)

test("signing out asks first, then ends at setup", function()
	local app = open("Account")
	app:scroll(PAGE, 3000)
	app:frames(4)
	app:click(`{PAGE} >> role:button "Sign out"`)
	app:frames(20)
	app:click('role:dialog "Sign out of Zeron?" >> role:button "Sign out"')
	app:frames(20)
	expect(asked(app, "account.sign_out")).to_be(true)
	expect(asked(app, "app.onboard")).to_be(true)
end)

test("each heading shows once, its pages under it, ahead of Archived", function()
	local app = open()
	local tree = app:tree(NAV)
	local function at(label: string): number
		return (tree:find(`"{label}"`, 1, true)) or 0
	end
	local _, headings = tree:gsub('"Organization"', "")
	expect(headings).to_be(1)
	expect(at("Personal") < at("General")).to_be(true)
	expect(at("Account") < at("Devices")).to_be(true)
	expect(at("Providers") < at("Organization")).to_be(true)
	expect(at("Organization") < at("Members")).to_be(true)
	expect(at("Members") < at("Archived")).to_be(true)
end)

test("Providers shows another device's agents when it's chosen", function()
	local app = open("Providers")
	expect(asked(app, "agents.load")).to_be(true)
	expect(shows(app, PAGE, "Claude Code")).to_be(true)
	app:click(`{PAGE} >> role:button "Device"`)
	app:frames(10)
	app:click('role:menuitem "devbox"')
	app:frames(20)
	expect(asked(app, "agents.target:box")).to_be(true)
	expect(shows(app, PAGE, "Codex")).to_be(true)
	expect(shows(app, PAGE, "Claude Code")).to_be(false)
	-- Back to this Mac: no device named, the engine's own.
	app:click(`{PAGE} >> role:button "Device"`)
	app:frames(10)
	app:click('role:menuitem "MacBook Pro"')
	app:frames(20)
	expect(asked(app, "agents.target:")).to_be(true)
	expect(shows(app, PAGE, "Claude Code")).to_be(true)
end)

-- A node's box under `scope`: x, y, w, h.
local function box(app: any, target: string): (number, number, number, number)
	local tree: string = app:tree(target)
	local line = string.match(tree, "[^\n]+") or ""
	local x, y, w, h = line:match(" (%-?%d+),(%-?%d+) (%d+)x(%d+)")
	return tonumber(x) or 0, tonumber(y) or 0, tonumber(w) or 0, tonumber(h) or 0
end

test("Profile, too narrow for two columns, centres the card over groups at the page's measure", function()
	local app = open("Account")
	-- The narrowest main window (760 wide).
	app:resize(760, 900)
	app:frames(30)
	local cx, cy, cw = box(app, "#setting-card")
	local gx, gy, gw = box(app, "#setting-photo")
	local tx = box(app, `{PAGE} >> role:heading "Account"`)
	expect(cw > 0 and gw > 0).to_be(true)
	-- Over the groups, and on their middle.
	expect(cy < gy).to_be(true)
	expect(math.abs((cx + cw / 2) - (gx + gw / 2)) <= 1).to_be(true)
	-- The groups take the page's measure: the title sits on the rows' text,
	-- 17 in from the card's edge (16 from a row's box, inside the card's 1).
	expect(tx - gx).to_be(16)
	expect(gw > 400).to_be(true)
	-- Wide again, the card stands beside them.
	app:resize(1200, 900)
	app:frames(30)
	local cx2, cy2 = box(app, "#setting-card")
	local gx2, gy2, gw2 = box(app, "#setting-photo")
	expect(cx2 > gx2 + gw2).to_be(true)
	expect(math.abs(cy2 - gy2) <= 1).to_be(true)
end)

test("a way in to the old Account page lands on Profile, at the account's rows", function()
	local app = open("General")
	app:command("settings_stub.open_account")
	app:frames(30)
	expect(app:state("shell_route_path")).to_be("/settings/profile")
	expect(app:state("settings_search_lit")).to_be("email")
	expect(shows(app, PAGE, "ada@example.net")).to_be(true)
end)

test("an accent chosen in Appearance is the one the window wears, kept as chosen", function()
	local app = open("Appearance")
	expect(app:has(`{PAGE} >> role:radio "Theme default"`)).to_be(true)
	app:click(`{PAGE} >> role:radio "Orange"`)
	app:frames(10)
	-- The setting the shell's `theme` reads (ds.look), and the swatch on.
	expect(app:state("accent")).to_be("orange")
	expect(app:tree(`{PAGE} >> role:radio "Orange"`)).to_contain("checked")
	expect(app:tree(`{PAGE} >> role:radio "Theme default"`)).to_contain("unchecked")
	expect(shows(app, PAGE, "Orange")).to_be(true)
	-- Back to the theme's own.
	app:click(`{PAGE} >> role:radio "Theme default"`)
	app:frames(10)
	expect(app:state("accent")).to_be("default")
end)

test("Devices lists your computers, this one first, and renames one", function()
	local app = open("Devices")
	expect(shows(app, PAGE, "MacBook Pro")).to_be(true)
	expect(shows(app, PAGE, "devbox")).to_be(true)
	expect(shows(app, PAGE, "This device")).to_be(true)
	local mac = node(app, PAGE, "text", "MacBook Pro")
	app:click(`{PAGE} >> role:button "Rename devbox"`)
	app:frames(10)
	app:fill('"Device name"', "build-box")
	app:click('role:dialog "Rename device" >> role:button "Rename"')
	app:frames(10)
	expect(asked(app, "sessions.rename_device:box:build-box")).to_be(true)
	expect(node(app, PAGE, "text", "MacBook Pro")).to_be(mac)
end)

test("Files: the autosave delay shows while Autosave is on, each kept as set", function()
	local app = open("Files")
	expect(shows(app, PAGE, "Word wrap")).to_be(true)
	expect(shows(app, PAGE, "Show hidden and ignored files")).to_be(true)
	expect(shows(app, PAGE, "Autosave delay")).to_be(false)
	app:click(`{PAGE} >> role:switch "Autosave"`)
	app:frames(10)
	expect(app:state("settings_files_autosave")).to_be(true)
	expect(shows(app, PAGE, "Autosave delay")).to_be(true)
	app:click(`{PAGE} >> role:switch "Word wrap"`)
	app:frames(10)
	expect(app:state("settings_files_word_wrap")).to_be(true)
end)

test("Profile asks whether a username is free only while you're choosing one", function()
	local app = open("Account")
	app:wait('"never shown, only waited past"', { timeout_ms = 500 })
	app:frames(10)
	for _, c in calls(app) do
		expect(string.find(c, "account.check_username", 1, true)).to_be_nil()
	end
	app:click(`{PAGE} >> role:button "Claim a username"`)
	app:frames(10)
	app:type('role:textbox "Username"', "ada")
	app:wait('"never shown, only waited past"', { timeout_ms = 500 })
	app:frames(10)
	expect(asked(app, "account.check_username:ada")).to_be(true)
end)

test("Profile's fields stay the same nodes as the card moves over and beside them", function()
	local app = open("Account")
	local field = node(app, PAGE, "textbox", "Name")
	expect(field).never.to_be_nil()
	app:resize(760, 900)
	app:frames(30)
	expect(node(app, PAGE, "textbox", "Name")).to_be(field)
	app:resize(1200, 900)
	app:frames(30)
	expect(node(app, PAGE, "textbox", "Name")).to_be(field)
end)

test("Notifications says when this Mac's banner settings can't be read, and holds its switches", function()
	local app = open("General")
	app:command("settings_stub.refuse_next")
	app:click(`{NAV} >> role:tab "Notifications"`)
	app:frames(20)
	expect(shows(app, PAGE, "Couldn't read this Mac's notification settings")).to_be(true)
	app:click(`{PAGE} >> role:switch "Desktop notifications"`)
	app:frames(10)
	expect(asked(app, "notifications.set")).to_be(false)
	app:click(`{PAGE} >> role:button "Try again"`)
	app:frames(20)
	expect(shows(app, PAGE, "Couldn't read this Mac's notification settings")).to_be(false)
end)

test("Appearance's code font size is kept as chosen, for the editor to read", function()
	local app = open("Appearance")
	expect(app:state("settings_code_font_size")).to_be(13)
	app:click(`{PAGE} >> role:button "Code & diff font size"`)
	app:frames(10)
	app:click('role:menuitem "15 pt"')
	app:frames(10)
	expect(app:state("settings_code_font_size")).to_be(15)
end)

test("Shortcuts lists the shell's shortcuts under their groups, each with its keys", function()
	local app = open("Shortcuts")
	-- (Until the shell's table is in, the page says so.)
	if shows(app, PAGE, "No shortcuts yet") then
		return
	end
	for _, title in { "New chat", "Search", "Settings" } do
		expect(shows(app, PAGE, title)).to_be(true)
	end
	expect(shows(app, PAGE, "Sessions")).to_be(true)
end)

test("Appearance's conversation width is a slider kept as set, within production's range", function()
	local app = open("Appearance")
	expect(app:state("settings_conversation_width")).to_be(660)
	local slider = `{PAGE} >> role:slider ~"Conversation width"`
	expect(app:has(slider)).to_be(true)
	-- (Under the theme and motion sections: brought into view.)
	app:scroll(PAGE, 600)
	app:frames(4)
	app:click(slider)
	app:key("End")
	app:frames(10)
	expect(app:state("settings_conversation_width")).to_be(1200)
	app:key("Home")
	app:frames(10)
	expect(app:state("settings_conversation_width")).to_be(560)
end)

test("the interface text size scales the window's text and lines, at once and back", function()
	local app = open("Appearance")
	local function height(): number
		local _, _, _, h = box(app, `{PAGE} >> "Accent color"`)
		return h
	end
	expect(app:state("text_size")).to_be(13)
	local before = height()
	expect(before).to_be(18)
	app:set_state("text_size", 16)
	app:frames(20)
	expect(height()).to_be(22)
	app:set_state("text_size", 13)
	app:frames(20)
	expect(height()).to_be(before)
end)

test("a new chat's picture: chosen, kept, named, removed; its effect said, and gone with it", function()
	local app = open("Appearance")
	-- (Tall enough that the New chat section shows whole.)
	app:resize(1200, 1800)
	app:set_state("shell_wallpaper", "none")
	app:set_state("shell_wallpaper_effect", "halftone")
	app:frames(6)
	-- Nothing to draw over: no effect to choose.
	expect(shows(app, PAGE, "colored print dots")).to_be(false)
	app:set_state("shell_wallpaper", "image")
	app:frames(6)
	expect(shows(app, PAGE, "No image chosen")).to_be(true)
	app:click(`{PAGE} >> role:button "Choose Image"`)
	app:frames(10)
	-- Kept by the host (its copy), then shown by its name.
	expect(asked(app, "wallpaper.keep:assets/favicons/zeron.png")).to_be(true)
	expect(shows(app, PAGE, "zeron.png")).to_be(true)
	expect(shows(app, PAGE, "colored print dots")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Adjust"`)).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Replace"`)).to_be(true)
	local image = app:state("shell_wallpaper_image") :: any
	expect(image and image.zoom).to_be(1)
	app:click(`{PAGE} >> role:button "Remove"`)
	app:frames(10)
	expect(app:state("shell_wallpaper")).to_be("none")
	expect(app:state("shell_wallpaper_image")).to_be_nil()
	expect(asked(app, "wallpaper.forget:assets/favicons/zeron.png")).to_be(true)
	expect(shows(app, PAGE, "colored print dots")).to_be(false)
end)

-- About & updates: the host's `about` stood in for, each state of an update.
local ABOUT = "About & updates"
local function about(app: any, fields: { [string]: any })
	local now = { app_version = "0.2.131", app_build = "1793000000", available = false }
	for k, v in fields do
		now[k] = v
	end
	app:set_state("about", now)
	app:frames(6)
end

test("About says which Zeron this is, and that this app isn't what updates", function()
	local app = open(ABOUT)
	about(app, {})
	expect(shows(app, PAGE, "Not connected to the engine")).to_be(true)
	about(app, { engine_version = "0.2.131", engine_build = "abcdef1234.1793000000" })
	expect(shows(app, PAGE, '"0.2.131"')).to_be(true)
	expect(shows(app, PAGE, "abcdef1 · ")).to_be(true)
	expect(shows(app, PAGE, "Not packaged yet")).to_be(true)
	expect(shows(app, PAGE, "Updates go to the engine on this Mac")).to_be(true)
	-- Copied for a bug report.
	app:click(`{PAGE} >> role:button "Copy"`)
	app:frames(6)
	expect(app:tree()).to_contain("Version info copied")
	-- The links open in the browser, the notes at this version's.
	app:scroll(PAGE, 2000)
	app:frames(4)
	app:click(`{PAGE} >> role:button "Release notes"`)
	app:frames(4)
	expect(asked(app, "open_url:https://github.com/zeronsh/comet/releases/tag/v0.2.131")).to_be(true)
end)

test("an engine up to date says so and when it was checked; a check finds a newer one", function()
	local app = open(ABOUT)
	about(app, { engine_version = "0.2.131", latest = "0.2.131", checked_at = os.time() * 1000 })
	expect(shows(app, PAGE, "Zeron is up to date")).to_be(true)
	expect(shows(app, PAGE, "checked just now")).to_be(true)
	-- Checking: said while the feed is asked.
	app:set_state("settings_stub_about_next", "pending")
	app:click(`{PAGE} >> role:button "Check for updates"`)
	app:frames(6)
	expect(shows(app, PAGE, "Checking for updates…")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Check for updates"`)).to_be(false)
	-- Offline: said, and asked again.
	app:set_state("about_doing", "")
	app:set_state("settings_stub_about_next", "offline")
	app:click(`{PAGE} >> role:button "Check for updates"`)
	app:frames(6)
	expect(shows(app, PAGE, "This Mac seems to be offline")).to_be(true)
	app:set_state("settings_stub_about_next", "ok")
	app:click(`{PAGE} >> role:button "Try again"`)
	app:frames(6)
	expect(asked(app, "about.check")).to_be(true)
	-- What the host then says: a newer release.
	about(app, { engine_version = "0.2.131", latest = "0.2.140", available = true, checked_at = os.time() * 1000 })
	expect(shows(app, PAGE, "Zeron 0.2.140 is available")).to_be(true)
	expect(shows(app, PAGE, "You have 0.2.131")).to_be(true)
end)

test("updating the engine says so, then that it runs the new version; the app's own engine sends you to the app", function()
	local app = open(ABOUT)
	about(app, { engine_version = "0.2.131", latest = "0.2.140", available = true })
	app:set_state("settings_stub_about_next", "pending")
	app:click(`{PAGE} >> role:button "Update engine"`)
	app:frames(6)
	expect(shows(app, PAGE, "Updating the engine…")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Update engine"`)).to_be(false)
	-- Done: starting again, then running it.
	app:set_state("about_doing", "")
	app:set_state("settings_stub_about_next", "ok")
	app:click(`{PAGE} >> role:button "Update engine"`)
	app:frames(6)
	expect(asked(app, "about.apply")).to_be(true)
	expect(shows(app, PAGE, "Updated to Zeron 0.2.140")).to_be(true)
	expect(shows(app, PAGE, "The engine is starting again on it")).to_be(true)
	about(app, { engine_version = "0.2.140", latest = "0.2.140", available = false })
	expect(shows(app, PAGE, "The engine is running the new version")).to_be(true)
	-- The Zeron app's own engine: updated from the app.
	app:set_state("about_doing", "")
	about(app, { engine_version = "0.2.131", latest = "0.2.140", available = true })
	app:set_state("settings_stub_about_next", "unmanaged")
	app:click(`{PAGE} >> role:button "Update engine"`)
	app:frames(6)
	expect(shows(app, PAGE, "Update from the Zeron app")).to_be(true)
	-- The engine's own check failing, with none here since.
	app:set_state("about_doing", "")
	about(app, { engine_version = "0.2.131", error = "feed unreachable" })
	expect(shows(app, PAGE, "The engine couldn't reach the release feed")).to_be(true)
end)

test("an agent's settings turn its $ skills and / commands on and off, kept for the composer by agent", function()
	local app = open("Providers")
	app:click(`{PAGE} >> role:button "Claude Code settings"`)
	app:frames(10)
	expect(app:has('role:switch "Use $ for skills"')).to_be(true)
	expect(app:tree('role:switch "Use $ for skills"')).never.to_contain("unchecked")
	app:click('role:switch "Use $ for skills"')
	app:frames(6)
	expect(app:tree('role:switch "Use $ for skills"')).to_contain("unchecked")
	app:click('role:switch "Separate / commands"')
	app:frames(6)
	-- Kept by agent, only what's off.
	local kept = (app:state("settings_skill_completion") :: any)["claude-code"]
	expect(kept.dollar).to_be(false)
	expect(kept.separate).to_be(false)
	-- Back on: nothing kept for it.
	app:click('role:switch "Use $ for skills"')
	app:click('role:switch "Separate / commands"')
	app:frames(6)
	expect((app:state("settings_skill_completion") :: any)["claude-code"]).to_be_nil()
end)

test("a sign-in that asks for a code takes it pasted back, and the account arrives; Cancel gives it up", function()
	local app = open("Providers")
	app:click(`{PAGE} >> role:button "Connect a Claude Code account"`)
	app:frames(10)
	expect(shows(app, PAGE, "Paste the authorization code")).to_be(true)
	expect(app:tree(`{PAGE} >> role:button "Finish"`)).to_contain("disabled")
	app:type('role:textbox "Authorization code"', "  abc123#state  ")
	app:frames(4)
	app:click(`{PAGE} >> role:button "Finish"`)
	app:frames(10)
	-- Trimmed; the account in the code's place.
	expect(asked(app, "agents.complete:claude-code:abc123#state")).to_be(true)
	expect(shows(app, PAGE, "Paste the authorization code")).to_be(false)
	expect(shows(app, PAGE, "Max")).to_be(true)
	-- Another, given up.
	app:click(`{PAGE} >> role:button "Add account"`)
	app:frames(10)
	app:click(`{PAGE} >> role:button "Cancel"`)
	app:frames(10)
	expect(asked(app, "agents.cancel:claude-code")).to_be(true)
	expect(shows(app, PAGE, "Paste the authorization code")).to_be(false)
end)

test("an account whose window is used up says when it starts over, in this Mac's time", function()
	local app = open("Providers")
	local now = os.time()
	local at = now + 2 * 3600
	local t = os.date("*t", at) :: any
	local h = t.hour % 12
	local clock = string.format("%d:%02d %s", if h == 0 then 12 else h, t.min, if t.hour < 12 then "AM" else "PM")
	local list = table.clone((app:state("agents") :: any).list)
	local one = table.clone(list[1])
	one.accounts = {
		{ id = "acc_1", email = "ada@example.net", plan = "Max", session = 1, week = 0.5, active = true, session_resets = at, week_resets = now + 3 * 86400 },
	}
	list[1] = one
	app:set_state("agents", { state = "ready", list = list })
	app:frames(10)
	expect(shows(app, PAGE, `Max · Limit reached, resets {clock}`)).to_be(true)
end)

test("Reduce motion in Appearance is the one setting every move asks", function()
	local app = open("Appearance")
	expect(app:state("motion_reduce")).to_be(false)
	app:scroll(PAGE, 500)
	app:frames(4)
	app:click(`{PAGE} >> role:switch "Reduce motion"`)
	app:frames(6)
	expect(app:state("motion_reduce")).to_be(true)
	app:click(`{PAGE} >> role:switch "Reduce motion"`)
	app:frames(6)
	expect(app:state("motion_reduce")).to_be(false)
end)

test("signed out, Account shows no forms: what an account gives, and the way to sign in", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("shell.settings")
	app:frames(20)
	app:click(`{NAV} >> role:tab "Account"`)
	app:frames(30)
	expect(app:has(`{PAGE} >> role:heading "Account"`)).to_be(true)
	expect(shows(app, PAGE, "Sign in to show up as yourself across Zeron.")).to_be(true)
	-- What an account gives, and the card yours will be.
	expect(shows(app, PAGE, "Your member card")).to_be(true)
	expect(shows(app, PAGE, "Yours when you sign in")).to_be(true)
	for _, field in { 'role:textbox "Name"', 'role:textbox "Title"', 'role:button "Save changes"', 'role:button "Change"' } do
		expect(app:has(`{PAGE} >> {field}`)).to_be(false)
	end
	for _, words in { "Username", "Sign out", "Delete account", "Photo" } do
		expect(app:has(`{PAGE} >> "{words}"`)).to_be(false)
	end
	-- One Sign in, the page's primary (not the danger row's ink), and it
	-- starts signing in.
	expect(app:has(`{PAGE} >> role:button "Sign in"`)).to_be(true)
	app:click(`{PAGE} >> role:button "Sign in"`)
	app:frames(10)
	expect(asked(app, "app.onboard")).to_be(true)
	-- Signed in: the forms, and Sign out in its place.
	app:command("settings_stub.sign_in")
	app:frames(30)
	expect(app:has(`{PAGE} >> role:textbox "Name"`)).to_be(true)
	expect(shows(app, PAGE, "Yours when you sign in")).to_be(false)
	app:scroll(PAGE, 3000)
	app:frames(4)
	expect(app:has(`{PAGE} >> role:button "Sign out"`)).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Sign in"`)).to_be(false)
end)

-- Profile: your title, your photo and deleting your account.
test("a title is saved with the name's Save, and comes back from the edge", function()
	local app = open("Account")
	app:type('role:textbox "Title"', "Analyst")
	app:frames(6)
	app:click(`{PAGE} >> role:button "Save changes"`)
	app:frames(10)
	expect(asked(app, "account.set_title:Analyst")).to_be(true)
	-- Only what changed: the name wasn't.
	for _, c in calls(app) do
		expect(string.find(c, "account.rename", 1, true)).to_be_nil()
	end
	expect(((app:state("settings_me") :: any) or {}).title).to_be("Analyst")
	expect(app:tree(`{PAGE} >> role:button "Save changes"`)).to_contain("disabled")
	-- Too long: said, and not saved.
	app:type('role:textbox "Title"', string.rep("x", 81))
	app:frames(6)
	expect(shows(app, PAGE, "longer than a title needs to be")).to_be(true)
	expect(app:tree(`{PAGE} >> role:button "Save changes"`)).to_contain("disabled")
end)

test("a photo is placed in a circle, cut from the middle at first, and can be removed", function()
	local app = open("Account")
	expect(app:has(`{PAGE} >> role:button "Remove"`)).to_be(false)
	app:click(`{PAGE} >> role:button "Change"`)
	app:frames(10)
	expect(app:has('role:dialog "Your photo"')).to_be(true)
	expect(app:has('role:dialog "Your photo" >> #photo-crop')).to_be(true)
	app:click('role:dialog "Your photo" >> role:button "Use photo"')
	app:frames(10)
	-- 1200 × 800 at first: the middle 800 square.
	local crop = app:state("settings_stub_last_crop") :: any
	expect(crop.side).to_be(800)
	expect(crop.x).to_be(200)
	expect(crop.y).to_be(0)
	-- (Closed: it eases out.)
	expect(app:state("settings_photo_cropping")).to_be(false)
	-- Yours now: said, removable, and the sidebar's too.
	expect(shows(app, PAGE, "Shows beside your name across Zeron")).to_be(true)
	-- Its buttons at the row's end, beside the words, not wrapped under them.
	local rx, ry = box(app, `{PAGE} >> role:button "Remove"`)
	local _, py, _, ph = box(app, "#setting-photo")
	local lx = box(app, `#setting-photo >> "Photo"`)
	expect(rx > lx + 200).to_be(true)
	expect(ry < py + ph / 2).to_be(true)
	expect(string.find(tostring(app:state("account_photo").avatar), "/auth/avatars/", 1, true)).never.to_be_nil()
	app:click(`{PAGE} >> role:button "Remove"`)
	app:frames(10)
	expect(asked(app, "account.remove_photo")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Remove"`)).to_be(false)
end)

test("zooming the photo cuts a smaller square, kept inside the picture", function()
	local app = open("Account")
	app:click(`{PAGE} >> role:button "Change"`)
	app:frames(10)
	app:set_state("settings_photo_zoom", 2)
	app:set_state("settings_photo_centre", { x = 0.95, y = 0.05 })
	app:frames(4)
	-- (Dragging past the edge settles it back; set here directly, the cut
	-- is what the edge's host is asked for.)
	app:click('role:dialog "Your photo" >> role:button "Use photo"')
	app:frames(10)
	local crop = app:state("settings_stub_last_crop") :: any
	expect(crop.side).to_be(400)
end)

-- Deleting: opened from its row, typed, confirmed. `word` is what the stub's
-- edge answers ("ok", or a refusal).
local DELETE_DIALOG = 'role:dialog "Delete your account?"'
local function try_delete(app: any, typed: string, word: string)
	app:set_state("settings_stub_delete_word", word)
	app:scroll(PAGE, 3000)
	app:frames(4)
	app:click(`{PAGE} >> role:button "Delete account"`)
	app:frames(10)
	app:type(`{DELETE_DIALOG} >> role:textbox ~"Type"`, typed)
	app:frames(6)
	app:click(`{DELETE_DIALOG} >> role:button "Delete account"`)
	app:frames(20)
end

local function deletes_asked(app: any): number
	local n = 0
	for _, c in calls(app) do
		n += if c == "account.delete" then 1 else 0
	end
	return n
end

test("deleting the account takes your address typed back, then signs out to setup", function()
	local app = open("Account")
	app:scroll(PAGE, 3000)
	app:frames(4)
	app:click(`{PAGE} >> role:button "Delete account"`)
	app:frames(10)
	expect(app:tree(`{DELETE_DIALOG} >> role:button "Delete account"`)).to_contain("disabled")
	-- Another address: still can't.
	app:type(`{DELETE_DIALOG} >> role:textbox ~"Type"`, "someone@example.net")
	app:frames(6)
	expect(app:tree(`{DELETE_DIALOG} >> role:button "Delete account"`)).to_contain("disabled")
	app:click(`{DELETE_DIALOG} >> role:button "Delete account"`)
	app:frames(10)
	expect(deletes_asked(app)).to_be(0)
	app:key("Escape")
	app:frames(20)
	-- The only admin: refused, said why in plain words, nothing signed out.
	try_delete(app, "ada@example.net", "last_admin")
	expect(app:has('"Couldn\'t delete your account"')).to_be(true)
	expect(app:has('"You\'re the only admin of your organization. Make someone else an admin first."')).to_be(true)
	expect(asked(app, "account.sign_out")).to_be(false)
	-- Allowed: gone, signed out, setup.
	try_delete(app, "ada@example.net", "ok")
	expect(asked(app, "account.sign_out")).to_be(true)
	expect(asked(app, "app.onboard")).to_be(true)
end)

test("typing your address and then cancelling deletes nothing", function()
	local app = open("Account")
	app:scroll(PAGE, 3000)
	app:frames(4)
	app:click(`{PAGE} >> role:button "Delete account"`)
	app:frames(10)
	app:type(`{DELETE_DIALOG} >> role:textbox ~"Type"`, "ada@example.net")
	app:frames(6)
	app:click(`{DELETE_DIALOG} >> role:button "Cancel"`)
	app:frames(20)
	expect(deletes_asked(app)).to_be(0)
	expect(app:state("settings_deleting_account")).to_be(false)
	-- Nor clicking away inside it.
	app:click(`{PAGE} >> role:button "Delete account"`)
	app:frames(10)
	app:type(`{DELETE_DIALOG} >> role:textbox ~"Type"`, "ada@example.net")
	app:frames(6)
	app:click(DELETE_DIALOG)
	app:frames(10)
	expect(deletes_asked(app)).to_be(0)
	-- Enter in the field is a confirm, once.
	app:click(`{DELETE_DIALOG} >> role:textbox ~"Type"`)
	app:key("Enter")
	app:frames(20)
	expect(deletes_asked(app)).to_be(1)
end)

test("a delete that didn't go through says so plainly and can be tried again", function()
	local app = open("Account")
	-- Offline: said as what happened, no mechanism; still signed in.
	try_delete(app, "ada@example.net", "offline")
	expect(app:has('"Zeron can\'t be reached right now."')).to_be(true)
	expect(asked(app, "account.sign_out")).to_be(false)
	-- The edge failed: said plainly.
	try_delete(app, "ada@example.net", "failed")
	expect(app:has('"Something went wrong."')).to_be(true)
	expect(asked(app, "account.sign_out")).to_be(false)
	-- Again, and through: the row isn't stuck busy from before.
	expect(app:has(`{PAGE} >> role:button "Delete account"`)).to_be(true)
	try_delete(app, "ada@example.net", "ok")
	expect(deletes_asked(app)).to_be(3)
	expect(asked(app, "account.sign_out")).to_be(true)
	expect(asked(app, "app.onboard")).to_be(true)
end)

-- Only just after signing in can the account be deleted (no agent on this
-- Mac can): the engine asks for it, the browser opens, and it goes once
-- you're back.
test("deleting asks you to sign in again first, then deletes once you have", function()
	local app = open("Account")
	app:set_state("settings_stub_step_up_word", "wait")
	try_delete(app, "ada@example.net", "step_up_required")
	expect(asked(app, "account.delete_signed_in")).to_be(true)
	-- Waiting: said in the dialog, still open, nothing signed out.
	expect(app:state("settings_deleting_account")).to_be(true)
	expect(app:state("settings_delete_signing_in")).to_be(true)
	expect(app:has(`{DELETE_DIALOG} >> "To delete your account, sign in again"`)).to_be(true)
	expect(app:state("settings_delete_busy")).to_be(true)
	expect(asked(app, "account.sign_out")).to_be(false)
	-- Signed in again: gone, signed out here, setup.
	app:command("settings_stub.signed_in_again")
	app:frames(20)
	expect(app:state("settings_delete_signing_in")).to_be(false)
	expect(asked(app, "account.sign_out")).to_be(true)
	expect(asked(app, "app.onboard")).to_be(true)
end)

test("closing the dialog while you sign in again stops waiting, and deletes nothing", function()
	local app = open("Account")
	app:set_state("settings_stub_step_up_word", "wait")
	try_delete(app, "ada@example.net", "step_up_required")
	expect(app:state("settings_delete_signing_in")).to_be(true)
	app:click(`{DELETE_DIALOG} >> role:button "Cancel"`)
	app:frames(20)
	expect(asked(app, "account.cancel_step_up")).to_be(true)
	expect(app:state("settings_delete_signing_in")).to_be(false)
	expect(app:state("settings_delete_busy")).to_be(false)
	-- You stopped it: nothing to say, nothing signed out.
	expect(app:has('"Couldn\'t delete your account"')).to_be(false)
	expect(asked(app, "account.sign_out")).to_be(false)
	-- And it can be asked again.
	app:set_state("settings_stub_step_up_word", "ok")
	try_delete(app, "ada@example.net", "step_up_required")
	expect(asked(app, "app.onboard")).to_be(true)
end)

test("a sign-in again that doesn't land, or a refusal after it, says so plainly", function()
	local app = open("Account")
	app:set_state("settings_stub_step_up_word", "timed_out")
	try_delete(app, "ada@example.net", "step_up_required")
	expect(app:has('"Signing in again took too long, so nothing was deleted. Try again."')).to_be(true)
	expect(app:state("settings_delete_signing_in")).to_be(false)
	expect(app:state("settings_deleting_account")).to_be(false)
	-- Signed in again, then the only admin: refused, said why.
	app:set_state("settings_stub_step_up_word", "last_admin")
	try_delete(app, "ada@example.net", "step_up_required")
	expect(app:has('"You\'re the only admin of your organization. Make someone else an admin first."')).to_be(true)
	-- Busy, or the edge couldn't: nothing was deleted, try again.
	try_delete(app, "ada@example.net", "busy")
	expect(app:has('"Someone in your organization is changing roles right now. Try again in a moment."')).to_be(true)
	try_delete(app, "ada@example.net", "try_again")
	expect(app:has('"Your account couldn\'t be deleted just now. Nothing was removed. Try again in a moment."')).to_be(true)
	expect(asked(app, "account.sign_out")).to_be(false)
end)

-- Signing keys: the keys your devices sign what you make with.
-- How many shown lines under `scope` hold `text` (a closing dialog's
-- hidden buttons aside).
local function count(app: any, scope: string, text: string): number
	local n = 0
	for line in app:tree(scope):gmatch("[^\n]+") do
		if line:find(text, 1, true) and not line:find(" hidden", 1, true) then
			n += 1
		end
	end
	return n
end

test("signing keys list this Mac first, then the others; a revoked one stays quietly", function()
	local app = open("Account")
	app:scroll(PAGE, 2000)
	app:frames(10)
	expect(app:has(`{PAGE} >> "Signing keys"`)).to_be(true)
	expect(app:has(`{PAGE} >> #setting-signing_keys >> ~"If you don't recognise a key, revoke it."`)).to_be(true)
	local _, mac = box(app, `{PAGE} >> "This Mac"`)
	local _, dev = box(app, `{PAGE} >> "dev"`)
	local _, old = box(app, `{PAGE} >> "old-laptop"`)
	expect(mac < dev and dev < old).to_be(true)
	-- Short fingerprints, the day each was added; the revoked one says when
	-- it went and has nothing to do.
	expect(shows(app, PAGE, "a1b2 c3d4 e5f6 0718")).to_be(true)
	expect(shows(app, PAGE, "2930 4a5b")).to_be(false)
	expect(shows(app, PAGE, "Added ")).to_be(true)
	expect(shows(app, PAGE, "Revoked ")).to_be(true)
	expect(count(app, PAGE, 'button "Revoke"')).to_be(2)
end)

test("revoking a signing key asks first, then the engine revokes it and it shows Revoked", function()
	local app = open("Account")
	app:scroll(PAGE, 2000)
	app:frames(10)
	-- Asked, then let go: nothing revoked.
	app:click(`{PAGE} >> role:button "Revoke"[2]`)
	app:frames(10)
	local dialog = 'role:dialog "Revoke this key?"'
	expect(app:has(`{dialog} >> "Plugins and templates it signed will stop counting as yours."`)).to_be(true)
	app:click(`{dialog} >> role:button "Cancel"`)
	app:frames(20)
	expect(asked(app, "account.revoke_key:k_dev")).to_be(false)
	-- Asked, and confirmed: the engine revokes it; the row says so.
	app:click(`{PAGE} >> role:button "Revoke"[2]`)
	app:frames(10)
	app:click(`{dialog} >> role:button "Revoke"`)
	app:frames(20)
	expect(asked(app, "account.revoke_key:k_dev")).to_be(true)
	expect(count(app, PAGE, 'button "Revoke"')).to_be(1)
	expect(count(app, PAGE, "Revoked ")).to_be(2)
end)

test("a signing key added on another device is New for one visit", function()
	local app = open("Account")
	app:scroll(PAGE, 2000)
	app:frames(10)
	expect(app:has(`{PAGE} >> "New"`)).to_be(true)
	-- The next visit: seen.
	app:click(`{NAV} >> role:tab "General"`)
	app:frames(20)
	app:click(`{NAV} >> role:tab "Account"`)
	app:frames(30)
	app:scroll(PAGE, 2000)
	app:frames(10)
	expect(app:has(`{PAGE} >> "dev"`)).to_be(true)
	expect(app:has(`{PAGE} >> "New"`)).to_be(false)
end)

test("no signing keys yet: the card says how one is made", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:set_state("settings_stub_keys", "none")
	app:command("settings_stub.sign_in")
	app:command("shell.settings")
	app:frames(20)
	app:click(`{NAV} >> role:tab "Account"`)
	app:frames(30)
	app:scroll(PAGE, 2000)
	app:frames(10)
	expect(shows(app, PAGE, "No signing keys yet.")).to_be(true)
	expect(shows(app, PAGE, "One is made when you sign a plugin.")).to_be(true)
	expect(count(app, PAGE, 'button "Revoke"')).to_be(0)
end)

test("your photo shows at both sidebars' feet once chosen, and goes when removed", function()
	local app = open("Account")
	local foot = `{NAV} >> role:button "Ada"`
	expect(app:has(foot)).to_be(true)
	local before = app:tree(foot)
	expect(before).never.to_contain("img")
	app:click(`{PAGE} >> role:button "Change"`)
	app:frames(10)
	app:click('role:dialog "Your photo" >> role:button "Use photo"')
	app:frames(20)
	-- The file shown is the photo's (the stub's: the app's own PNG).
	local photo = app:state("account_photo") :: any
	expect(string.find(photo.avatar, "/auth/avatars/", 1, true)).never.to_be_nil()
	expect((app:state("account_photo_files") :: any)[photo.avatar]).to_be("assets/favicons/zeron.png")
	expect(app:tree(foot)).to_contain("img")
	-- The chats' sidebar too.
	app:click(`{NAV} >> role:button "Back to chats"`)
	app:frames(30)
	expect(app:tree('#shell-sidebar >> role:button "Ada"')).to_contain("img")
	app:command("shell.settings")
	app:frames(20)
	app:click(`{NAV} >> role:tab "Account"`)
	app:frames(20)
	app:click(`{PAGE} >> role:button "Remove"`)
	app:frames(20)
	expect(app:tree(foot)).never.to_contain("img")
end)

test("a photo the edge won't take says why, and the dialog stays to try another", function()
	local app = open("Account")
	app:click(`{PAGE} >> role:button "Change"`)
	app:frames(10)
	app:set_state("settings_stub_photo_word", "too_large")
	app:click('role:dialog "Your photo" >> role:button "Use photo"')
	app:frames(20)
	expect(app:has('"Couldn\'t change your photo"')).to_be(true)
	expect(app:has('"That picture is too large. Try a smaller one."')).to_be(true)
	expect(app:state("settings_photo_cropping")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Remove"`)).to_be(false)
end)

test("the pages are one tab list: the open one selected, the arrows between them; the search's hint reads as one", function()
	local app = open("Members")
	expect(app:has(`{NAV} >> role:tablist "Settings pages"`)).to_be(true)
	-- "Back to chats" is a way out, not a page.
	expect(app:has(`{NAV} >> role:button "Back to chats"`)).to_be(true)
	-- From the search, Tab lands on the open page (the selected tab), not
	-- the first; one more Tab leaves the list for the page.
	app:click(`{NAV} >> role:textbox "Search settings"`)
	app:key("Tab")
	app:frames(4)
	expect(app:tree(`{NAV} >> role:tab "Members"`)).to_contain("focused")
	expect(app:tree(`{NAV} >> role:tab "General"`)).never.to_contain("focused")
	-- ↓: the next page has the focus, still in the list.
	app:key("ArrowDown")
	app:frames(4)
	expect(app:tree(`{NAV} >> role:tab "Members"`)).never.to_contain("focused")
	expect(app:tree(`{NAV} >> role:tablist "Settings pages"`)).to_contain("focused")
	app:key("Tab")
	app:frames(4)
	-- (Out of the pages' list: on to the foot's you and settings.)
	expect(app:tree(`{NAV} >> role:tablist "Settings pages"`)).never.to_contain("focused")
	-- ⌘F, said once.
	expect(app:has(`{NAV} >> #settings-search >> "Command F"`)).to_be(true)
end)

test("Shortcuts shows the Appshot hotkey while Appshots are on, as set, and sends you to change it", function()
	local app = open("Shortcuts")
	expect(shows(app, PAGE, "Capture Appshot")).to_be(false)
	app:set_state("appshots_enabled", true)
	app:frames(10)
	expect(shows(app, PAGE, "Capture Appshot")).to_be(true)
	expect(app:has(`{PAGE} >> "Control Option Space"`)).to_be(true)
	app:set_state("appshots_shortcut", "cmd+shift+2")
	app:frames(10)
	-- (Said in the order macOS writes them: ⇧⌘2.)
	expect(app:has(`{PAGE} >> "Shift Command 2"`)).to_be(true)
	-- (The wheel over the page's heading: its middle is past the window's foot.)
	app:scroll(`{PAGE} >> role:heading "Shortcuts"`, 3000)
	app:frames(4)
	app:click(`{PAGE} >> role:button "Change"`)
	app:frames(20)
	expect(app:has(`{NAV} >> role:tab "Appshots"`)).to_be(true)
	expect(shows(app, PAGE, "Capture Appshots")).to_be(true)
end)

test("Providers stands on the page's middle, as General does", function()
	local app = open("General")
	app:resize(1400, 900)
	app:frames(30)
	local gx = box(app, `{PAGE} >> role:heading "General"`)
	app:click(`{NAV} >> role:tab "Providers"`)
	app:frames(40)
	local px = box(app, `{PAGE} >> role:heading "Providers"`)
	expect(gx > 0).to_be(true)
	expect(math.abs(px - gx) < 1).to_be(true)
end)

-- A node's box under `scope`, by a line's text: { x, y, w, h }.
local function box_of(app: any, scope: string, part: string): { x: number, y: number, w: number, h: number }?
	for l in app:tree(scope):gmatch("[^\n]+") do
		if l:find(part, 1, true) then
			local x, y, w, h = l:match("(%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
			if x then
				return { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number, h = tonumber(h) :: number }
			end
		end
	end
	return nil
end

-- Each of Appearance's wallpaper states: one card, a hairline only
-- between rows that are there (none under the last), and every control
-- inline at its row's end (to the right of the words, on their line).
local NEW_CHAT = "#appearance-new-chat"
local function inline(app: any, label: string, control: string)
	local words, ctl = box_of(app, NEW_CHAT, `text "{label}"`), box_of(app, NEW_CHAT, control)
	expect(words ~= nil and ctl ~= nil).to_be(true)
	local w, c = words :: any, ctl :: any
	expect(c.x > w.x + 300).to_be(true)
	expect(math.abs((c.y + c.h / 2) - (w.y + w.h / 2)) < 24).to_be(true)
end

test("the wallpaper's rows stand inline in one card, a hairline only between them, in each state", function()
	local app = open("Appearance")
	app:resize(1200, 1800)
	-- None: one row, no hairline at all.
	app:set_state("shell_wallpaper", "none")
	app:set_state("shell_wallpaper_image", nil)
	app:frames(10)
	expect(app:count(`{NEW_CHAT} >> .settings-rule`)).to_be(0)
	inline(app, "Wallpaper", `button "Wallpaper"`)
	-- Image, none chosen yet: its row under a hairline; no effect to choose.
	app:set_state("shell_wallpaper", "image")
	app:frames(10)
	expect(app:count(`{NEW_CHAT} >> .settings-rule`)).to_be(2)
	inline(app, "Wallpaper", `button "Wallpaper"`)
	inline(app, "No image chosen", `button "Choose Image"`)
	-- A picture, plain and under an effect: three rows, two hairlines.
	for _, effect in { "none", "dither" } do
		app:set_state("shell_wallpaper_image", { path = "assets/favicons/zeron.png", name = "zeron.png", w = 512, h = 512, x = 0.5, y = 0.5, zoom = 1 })
		app:set_state("shell_wallpaper_effect", effect)
		app:frames(10)
		expect(app:count(`{NEW_CHAT} >> .settings-rule`)).to_be(2)
		inline(app, "Wallpaper", `button "Wallpaper"`)
		inline(app, "zeron.png", `button "Replace"`)
		inline(app, "Effect", `button "Effect"`)
	end
	expect(shows(app, NEW_CHAT, "Rebuilds the artwork with a dithered")).to_be(true)
	-- Back to None: the picture's rows go, hairlines with them.
	app:set_state("shell_wallpaper", "none")
	app:frames(10)
	expect(app:count(`{NEW_CHAT} >> .settings-rule`)).to_be(0)
	expect(app:state("shell_wallpaper_image")).never.to_be_nil()
end)
