-- Settings → Templates follows the templates in place, as Plugins does: one
-- made, one in use, one breaking or out of date leave every row (and its
-- own) the same node; the tabs split what's installed from what's yours;
-- Use, New template and the ⋯ menu's Show folder, Duplicate and Delete ask
-- the host; Zeron's own can't be deleted. On the page in
-- ui/templates_fixture.luau.

local function row(app: any, id: string): string?
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(`#template-{id} `, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

local function open(): any
	local app = mount("templates_fixture.luau")
	app:resize(1000, 900)
	app:frames(20)
	return app
end

-- Opens a row's ⋯ menu and picks `item` (a menu takes no pick in its first
-- moments: time passes, not only frames).
local function pick(app: any, name: string, item: string)
	app:click(`role:button "More for {name}"`)
	expect(app:wait(`role:menuitem "{item}"`, { timeout_ms = 15000 })).to_be(true)
	app:wait('"nothing"', { timeout_ms = 400 })
	app:click(`role:menuitem "{item}"`)
	app:frames(20)
end

test("Zeron's own comes first, built in and in use, with the rows' parts", function()
	local app = open()
	local tree = app:tree()
	expect(tree).to_contain("In use")
	expect(tree).to_contain("by Zeron")
	expect(tree).to_contain("by you")
	expect(tree).to_contain("0.2.131")
	expect(app:has('role:button "More for Zeron"')).to_be(true)
	expect(app:has('#template-airy >> role:button "Use"')).to_be(true)
	expect(app:has('#template-zeron >> role:button "Use"')).to_be(false)
	expect(app:has("#templates-hint")).to_be(false)
end)

test("Zeron's own menu has no Delete; yours does", function()
	local app = open()
	app:click('role:button "More for Zeron"')
	expect(app:wait('role:menuitem "Duplicate"', { timeout_ms = 15000 })).to_be(true)
	expect(app:has('role:menuitem "Delete"')).to_be(false)
	app:key("escape")
	app:frames(20)
	app:click('role:button "More for Airy"')
	expect(app:wait('role:menuitem "Delete"', { timeout_ms = 15000 })).to_be(true)
end)

test("using one, one arriving, one out of date and one breaking keep every row", function()
	local app = open()
	local zeron, airy = row(app, "zeron"), row(app, "airy")
	expect(zeron).never.to_be_nil()
	expect(airy).never.to_be_nil()
	app:click('#template-airy >> role:button "Use"')
	app:frames(20)
	expect(calls(app)).to_contain("use:airy")
	expect(`zeron @{row(app, "zeron")}`).to_be(`zeron @{zeron}`)
	expect(`airy @{row(app, "airy")}`).to_be(`airy @{airy}`)
	expect(app:has('#template-zeron >> role:button "Use"')).to_be(true)
	app:command("made")
	app:frames(20)
	expect(row(app, "dusk")).never.to_be_nil()
	expect(`airy @{row(app, "airy")}`).to_be(`airy @{airy}`)
	app:command("updated")
	app:frames(20)
	expect(app:tree()).to_contain("Made for Zeron 0.2.120")
	expect(`airy @{row(app, "airy")}`).to_be(`airy @{airy}`)
	app:command("broke")
	app:frames(20)
	expect(app:tree()).to_contain("Can't run")
	expect(`airy @{row(app, "airy")}`).to_be(`airy @{airy}`)
end)

test("Yours holds only yours; a search that finds nothing says so", function()
	local app = open()
	app:click('role:tab "Yours"')
	app:frames(20)
	expect(row(app, "airy")).never.to_be_nil()
	expect(row(app, "zeron")).to_be_nil()
	app:type('role:textbox "Search templates"', "nowhere")
	app:frames(20)
	expect(app:tree()).to_contain("Nothing matches")
	app:click('role:button "Clear search"')
	app:frames(20)
	expect(row(app, "airy")).never.to_be_nil()
end)

test("Show folder, Duplicate and Delete ask the host", function()
	local app = open()
	app:click('role:tab "Installed"')
	app:frames(20)
	pick(app, "Airy", "Show folder")
	expect(calls(app)).to_contain("folder:airy")
	pick(app, "Zeron", "Duplicate")
	expect(calls(app)).to_contain("duplicate:zeron")
	expect(app:tree()).to_contain("Zeron copy")
	app:click('role:tab "Installed"')
	app:frames(20)
	pick(app, "Airy", "Delete")
	app:click('role:button "Delete"')
	app:frames(20)
	expect(calls(app)).to_contain("delete:airy")
	expect(row(app, "airy")).to_be_nil()
end)

test("the palette lists every template, the one in use checked, and picking one uses it", function()
	local app = open()
	app:command("palette")
	app:frames(20)
	app:type('role:textbox "Search"', "template")
	app:frames(30)
	expect(app:has('role:option "Use template: Zeron"')).to_be(true)
	expect(app:has('role:option "Use template: Airy"')).to_be(true)
	-- A template's name finds it too.
	app:key("cmd+a")
	app:type('role:textbox "Search"', "Airy")
	app:frames(30)
	expect(app:has('role:option "Use template: Airy"')).to_be(true)
	app:click('role:option "Use template: Airy"')
	app:frames(20)
	expect(calls(app)).to_contain("use:airy")
	expect(app:has('#template-zeron >> role:button "Use"')).to_be(true)
end)

test("In use follows the name; Use's room holds, so a row's end holds still", function()
	local app = open()
	app:frames(20)
	-- A node's x and width in a row, by what its line says.
	local function box(id: string, what: string): (number, number)
		for line in app:tree(`#template-{id}`):gmatch("[^\n]+") do
			if line:find(what, 1, true) then
				local x, w = line:match(" (%-?%d+),%-?%d+ (%d+)x%d+")
				return tonumber(x) :: number, tonumber(w) :: number
			end
		end
		error(`no {what} in {id}'s row`)
	end
	-- "In use" sits just past the name, well left of the row's controls.
	local name_x, name_w = box("zeron", 'text "Zeron"')
	local in_x = box("zeron", 'text "In use"')
	expect(in_x > name_x + name_w - 1).to_be(true)
	expect(in_x < name_x + name_w + 40).to_be(true)
	expect(app:tree("#template-airy"):find('text "In use"', 1, true) ~= nil).to_be(false)
	-- The ⋯ in one place on every row.
	local more_x = box("zeron", 'button "More for Zeron"')
	expect(more_x).to_be((box("airy", 'button "More for Airy"')))
	-- Using Airy moves the badge; the ends stay put.
	app:click('#template-airy >> role:button "Use"')
	app:frames(20)
	expect(app:tree("#template-airy"):find('text "In use"', 1, true) ~= nil).to_be(true)
	expect(app:tree("#template-zeron"):find('text "In use"', 1, true) ~= nil).to_be(false)
	expect((box("airy", 'button "More for Airy"'))).to_be(more_x)
end)

test("someone else's template says who signed it, and isn't yours", function()
	local app = open()
	app:command("imported")
	app:frames(20)
	local tree = app:tree()
	expect(tree).to_contain("by @ada")
	-- One Zeron can't vouch for: where it came from, and Unverified by its
	-- version; never on a verified author's, yours or Zeron's own.
	expect(tree).to_contain("From github.com/eve/mystery")
	expect(app:has('#template-mystery >> "Unverified"')).to_be(true)
	for _, id in { "ledger", "mine-again", "zeron", "airy" } do
		expect(`{id} {tostring(app:has(`#template-{id} >> "Unverified"`))}`).to_be(`{id} false`)
	end
	expect(row(app, "ledger") ~= nil).to_be(true)
	-- One you signed yourself, brought from another device, is yours.
	app:click('role:tab "Yours"')
	app:frames(20)
	expect(row(app, "mine-again") ~= nil).to_be(true)
	expect(row(app, "ledger")).to_be(nil)
	expect(row(app, "mystery")).to_be(nil)
end)

test("a waiting package is named above the rows, with Review", function()
	local app = open()
	expect(app:has(".template-pending")).to_be(false)
	app:command("waiting")
	app:frames(20)
	expect(app:has("#template-pending-ledger-1700000000")).to_be(true)
	expect(app:tree()).to_contain("Ledger is waiting for you")
	app:click('#template-pending-ledger-1700000000 >> role:button "Review"')
	app:frames(20)
	expect(calls(app)).to_contain("review:ledger-1700000000")
	app:command("answered")
	app:frames(20)
	expect(app:has(".template-pending")).to_be(false)
end)

test("no New template for now; yours can be exported, Zeron's can't", function()
	local app = open()
	expect(app:has('role:button "New template"')).to_be(false)
	app:click('role:button "More for Zeron"')
	expect(app:wait('role:menuitem "Duplicate"', { timeout_ms = 15000 })).to_be(true)
	expect(app:has('role:menuitem "Export…"')).to_be(false)
	app:key("escape")
	app:frames(20)
	app:click('role:button "More for Airy"')
	expect(app:wait('role:menuitem "Export…"', { timeout_ms = 15000 })).to_be(true)
end)

-- Install's dialog, open.
local function install_dialog(app: any)
	app:click('role:button "Install"')
	app:frames(20)
end

test("Install takes a link or a file, as Plugins' does", function()
	local app = open()
	-- Checked where they came from as the page opens.
	expect(calls(app)).to_contain("check")
	install_dialog(app)
	expect(app:tree()).to_contain("Install a template")
	expect(app:has('role:button "Choose File"')).to_be(true)
	-- Nothing typed: nothing asked, the dialog stays.
	app:click('role:dialog >> role:button "Install"')
	app:frames(10)
	expect(calls(app)).never.to_contain("import:")
	expect(app:state("templates_installing")).to_be(true)
	app:type('role:textbox "Link"', "ada/dusk")
	app:click('role:dialog >> role:button "Install"')
	app:frames(10)
	expect(calls(app)).to_contain("import:ada/dusk")
	-- On its way: the dialog stays, its Install working.
	expect(app:state("templates_installing")).to_be(true)
	expect(app:state("templates_install_busy")).to_be(true)
	expect(app:has("#templates-choose-file")).to_be(true)
	-- Yours: in, said so, the dialog gone.
	app:command("installed_yours")
	app:frames(20)
	expect(app:state("templates_installing")).to_be(false)
	expect(row(app, "dusk") ~= nil).to_be(true)
end)

test("someone else's from a link waits for Review; a failure keeps the dialog to try again", function()
	local app = open()
	install_dialog(app)
	app:type('role:textbox "Link"', "eve/ledger")
	app:click('role:dialog >> role:button "Install"')
	app:frames(10)
	app:command("installed_theirs")
	app:frames(20)
	expect(app:state("templates_installing")).to_be(false)
	expect(app:has("#template-pending-ledger-1700000001")).to_be(true)

	install_dialog(app)
	app:type('role:textbox "Link"', "eve/nothing")
	app:click('role:dialog >> role:button "Install"')
	app:frames(10)
	app:command("install_failed")
	app:frames(20)
	-- (Why, in a toast; the dialog kept, to try again.)
	expect(app:state("templates_installing")).to_be(true)
	expect(app:state("templates_install_busy")).to_be(false)
	expect(app:has('role:dialog >> role:button "Install"')).to_be(true)
end)

test("a newer version found says so and updates in place; a refused one says why", function()
	local app = open()
	app:command("imported")
	app:command("update_found")
	app:frames(20)
	expect(app:has('#template-ledger >> ~"Update available: 2.1.0."')).to_be(true)
	app:click('#template-ledger >> role:button "Update"')
	app:frames(10)
	expect(calls(app)).to_contain("update:ledger")
	expect(app:tree("#template-ledger")).to_contain("Updating")
	local before = row(app, "ledger")
	app:command("updated_ledger")
	app:frames(20)
	expect(app:has('#template-ledger >> role:button "Update"')).to_be(false)
	expect(`ledger @{row(app, "ledger")}`).to_be(`ledger @{before}`)

	app:command("update_refused")
	app:frames(20)
	expect(app:has('#template-ledger >> ~"wasn\'t installed"')).to_be(true)
	expect(app:has('#template-ledger >> role:button "Update"')).to_be(false)
end)
