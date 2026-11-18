-- The store's screens laid out as Settings' are, measured by their boxes
-- (the captures' findings): on the page in ui/plugins_store_fixture.luau.
-- One text edge (a card's outer edge + 17, where rows' tiles and words
-- start), one content edge (a card's outer edge − 17, where rows' controls
-- end); nothing under the pointer moves as things arrive; a skeleton that
-- waits stops breathing after a bound.

local SCROLL = "#fixture-scroll"
local INSET = 17

local function settle(app: any, ms: number?)
	app:wait("#never-there", { timeout_ms = ms or 300 })
	app:frames(10)
end

local function box(app: any, target: string): { x: number, y: number, w: number, h: number }
	local line = app:tree(target):match("[^\n]+") or ""
	local x, y, w, h = line:match("(%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
	return { x = tonumber(x) or 0, y = tonumber(y) or 0, w = tonumber(w) or 0, h = tonumber(h) or 0 }
end

local function mounted(): any
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	return app
end

local function page(id: string): any
	local app = mounted()
	app:set_state("plugins_view", `store:{id}`)
	app:wait("#store-header", { timeout_ms = 3000 })
	settle(app)
	return app
end

-- The content's own edges, from a card on the page: its outer edge in by
-- the rows' inset.
local function edges(app: any, card: string): (number, number)
	local c = box(app, card)
	return c.x + INSET, c.x + c.w - INSET
end

test("a plugin's head: its tile on the text edge at its name's top, the action ending on the content edge, 28 tall", function()
	local app = page("focus-timer")
	local text, content = edges(app, "#store-readme")
	-- (The README's card is 1 in from its group's edge: its own box.)
	text, content = text - 1, content + 1
	local header = box(app, "#store-header")
	local install = box(app, '#store-header >> role:button "Install"')
	expect(math.abs(install.x + install.w - content) < 0.6).to_be(true)
	expect(install.h).to_be(28)
	local heading = box(app, '#store-header >> role:heading')
	-- Its tile at the text edge, its top on the name's.
	local tile = box(app, "#store-tile")
	expect(math.abs(tile.x - text) < 0.6).to_be(true)
	expect(math.abs(tile.y - heading.y) < 0.6).to_be(true)
	expect(header.y < box(app, "#store-readme").y).to_be(true)
end)

test("a 64-character name with no spaces shows whole on its page, its tile still at its top", function()
	local app = page("extremes")
	local heading = box(app, '#store-header >> role:heading')
	expect(heading.h > 26).to_be(true)
	-- (Two lines of the title's: the whole of it, wrapped, never cut.)
	expect(heading.h >= 2 * 26).to_be(true)
	expect(math.abs(box(app, "#store-tile").y - heading.y) < 0.6).to_be(true)
end)

test("the facts say only what the head doesn't, and a requirement once", function()
	local app = page("focus-timer")
	local facts = app:tree("#store-facts")
	expect(facts).never.to_contain("By")
	expect(facts).never.to_contain("Installs")
	expect(facts).to_contain("Works with")
	app:set_state("plugins_view", "catalog")
	app:frames(5)
	app:set_state("plugins_view", "store:cost-meter")
	app:wait("#store-header", { timeout_ms = 3000 })
	settle(app)
	local _, n = app:tree():gsub("Needs Zeron 9%.0%.0", "")
	expect(n).to_be(1)
end)

-- The pictures' strip, by how many: one fills the column, two share it 12
-- apart, three or more show two whole and the next at the column's end;
-- each 16:10, the strip's first at the cards' edge.
local function strip(app: any): ({ x: number, y: number, w: number, h: number }, { x: number, y: number, w: number, h: number })
	local card = box(app, "#store-readme")
	-- (The README's box is 1 in from its card's edge: the card's own.)
	return box(app, "#store-shots"), { x = card.x - 1, y = card.y, w = card.w + 2, h = card.h }
end

test("one screenshot fills the column, at 16:10", function()
	local app = mounted()
	app:command("store_stub.one_shot")
	app:set_state("plugins_view", "store:focus-timer")
	app:wait("#store-header", { timeout_ms = 3000 })
	settle(app)
	local shots, column = strip(app)
	local shot = box(app, '#store-shots >> role:button "Open header.png"')
	expect(math.abs(shot.x - column.x) < 0.6).to_be(true)
	expect(math.abs(shot.w - column.w) < 0.6).to_be(true)
	expect(math.abs(shot.w / shot.h - 1.6) < 0.02).to_be(true)
	-- (No taller than a reply's picture.)
	expect(shot.h <= 420).to_be(true)
	expect(shots.h).to_be(shot.h)
end)

test("two screenshots share the column, 12 apart, each 16:10", function()
	local app = page("focus-timer")
	local _, column = strip(app)
	local a = box(app, '#store-shots >> role:button "Open header.png"')
	local b = box(app, '#store-shots >> role:button "Open done.png"')
	expect(math.abs(a.x - column.x) < 0.6).to_be(true)
	expect(math.abs(b.x + b.w - (column.x + column.w)) < 0.6).to_be(true)
	expect(math.abs(b.x - (a.x + a.w) - 12) < 0.6).to_be(true)
	expect(math.abs(a.w - b.w) < 0.6).to_be(true)
	expect(math.abs(a.w / a.h - 1.6) < 0.02).to_be(true)
end)

test("three or more show two whole and the next at the column's end, the strip as tall as they are", function()
	local app = mounted()
	app:command("store_stub.three_shots")
	app:set_state("plugins_view", "store:focus-timer")
	app:wait("#store-header", { timeout_ms = 3000 })
	settle(app)
	local shots, column = strip(app)
	local first = box(app, '#store-shots >> role:button "Open header.png"')
	local third = box(app, '#store-shots >> role:button "Open break.png"')
	expect(math.abs(shots.x - column.x) < 0.6).to_be(true)
	expect(math.abs(shots.w - column.w) < 0.6).to_be(true)
	expect(shots.h).to_be(first.h)
	expect(math.abs(first.w / first.h - 1.6) < 0.02).to_be(true)
	-- Two whole, and the third begun inside the column, cut by its end.
	expect(2 * first.w + 2 * 12 < column.w).to_be(true)
	expect(third.x < column.x + column.w and third.x + third.w > column.x + column.w).to_be(true)
	-- (The same at another width: as wide as two and a bit of what the
	-- column is.)
	app:resize(760, 900)
	settle(app)
	shots, column = strip(app)
	first = box(app, '#store-shots >> role:button "Open header.png"')
	third = box(app, '#store-shots >> role:button "Open break.png"')
	expect(2 * first.w + 2 * 12 < column.w).to_be(true)
	expect(third.x < column.x + column.w and third.x + third.w > column.x + column.w).to_be(true)
end)

test("Browse's filters: one quiet row, the categories' words on the text edge, the order ending on the content edge, its height held", function()
	local app = mounted()
	app:command("store_stub.hold")
	app:click('role:tab "Browse"')
	app:frames(20)
	local card = box(app, "#store-card")
	local text, content = card.x + INSET, card.x + card.w - INSET
	local bar = box(app, "#store-filters")
	local order = box(app, '#store-filters >> role:button "Order"')
	expect(math.abs(order.x + order.w - content) < 0.6).to_be(true)
	local search_before = box(app, '#store-card >> role:textbox "Search plugins"').y
	local bar_before = bar.h
	app:command("store_stub.release")
	settle(app, 500)
	local all = app:tree('#store-filters >> role:tab "All"'):match("[^\n]+") or ""
	local first_tab = box(app, '#store-filters >> role:tab "All"')
	-- (A quiet tab's words are 12 inside it.)
	expect(math.abs(first_tab.x + 12 - text) < 0.6).to_be(true)
	expect(box(app, "#store-filters").h).to_be(bar_before)
	expect(box(app, '#store-filters >> role:tab "Agents"').h).to_be(order.h)
	expect(box(app, '#store-card >> role:textbox "Search plugins"').y).to_be(search_before)
	expect(all).never.to_contain("All categories")
end)

test("the featured hold their room as they come, and stand a section's step from the card", function()
	local app = mounted()
	app:command("store_stub.hold")
	app:click('role:tab "Browse"')
	app:frames(20)
	expect(app:has("#store-featured-loading")).to_be(true)
	local loading = box(app, "#store-featured-loading")
	app:command("store_stub.release")
	settle(app, 500)
	local cards = box(app, "#store-featured")
	expect(cards.h).to_be(loading.h)
	-- Each with its cover on top at 16:10.
	local first = box(app, '#store-featured >> role:button "Focus timer"')
	local cover_line = app:tree('#store-featured >> role:button "Focus timer"'):match("\n%s*(@%d+ img [^\n]+)") or ""
	local cw, ch = cover_line:match("([%d.]+)x([%d.]+)")
	expect(math.abs((tonumber(cw) or 0) - first.w) < 1).to_be(true)
	expect(math.abs((tonumber(cw) or 0) / (tonumber(ch) or 1) - 1.6) < 0.02).to_be(true)
	local card = box(app, "#store-card")
	expect(card.y - (cards.y + cards.h)).to_be(32)
end)

test("a skeleton that waits too long rests: the first load's, the next page's and a plugin page's", function()
	local app = mounted()
	app:command("store.short_wait")
	app:command("store_stub.hold")
	app:click('role:tab "Browse"')
	app:frames(5)
	expect(app:has("#store-loading")).to_be(true)
	settle(app, 500)
	expect(app:has("#store-loading-still")).to_be(true)
	app:command("store_stub.release")
	settle(app, 400)
	app:command("store_stub.hold")
	app:scroll(SCROLL, 4000)
	app:frames(10)
	expect(app:has("#store-more")).to_be(true)
	settle(app, 500)
	expect(app:has("#store-more-still")).to_be(true)
	app:set_state("plugins_view", "store:focus-timer")
	app:frames(5)
	settle(app, 500)
	expect(app:has("#store-loading-still")).to_be(true)
end)

test("Yours' Publish… follows its sentence", function()
	local app = mounted()
	app:click('role:tab "Yours"')
	settle(app)
	local link = box(app, 'role:button "Publish…"')
	local words = box(app, '"1.0.0 in the plugin store"')
	expect(math.abs(link.x - (words.x + words.w + 12)) < 1).to_be(true)
end)

-- An ask's words, top to bottom, under `root` (each text node's label, with
-- its top and bottom).
local function lines_of(app: any, root: string): { { text: string, y: number, b: number } }
	local out = {}
	for line in app:tree(root):gmatch("[^\n]+") do
		local label, y, h = line:match('text "([^"]+)" %-?[%d.]+,(%-?[%d.]+) [%d.]+x([%d.]+)')
		if label then
			table.insert(out, { text = label, y = tonumber(y) :: number, b = (tonumber(y) :: number) + (tonumber(h) :: number) })
		end
	end
	return out
end

test("what a plugin asks for reads as the consent sheet reads it: the same words for the same asks", function()
	local sheet = mount("consent_fixture.luau")
	sheet:resize(560, 640)
	sheet:frames(5)
	sheet:command("hosts")
	sheet:frames(30)
	local said = {}
	for _, l in lines_of(sheet, "#consent-scroll") do
		said[l.text] = true
	end
	local app = page("api-probe")
	local page_lines = lines_of(app, "#store-asks")
	expect(#page_lines > 2).to_be(true)
	for _, l in page_lines do
		expect(`{l.text}: {tostring(said[l.text] == true)}`).to_be(`{l.text}: true`)
	end
end)

-- Every node under `root` ends at or before `edge`.
local function within(app: any, root: string, edge: number): (boolean, string)
	local first = true
	for line in app:tree(root):gmatch("[^\n]+") do
		-- (Its descendants: the root is the card's own frame.)
		if first then
			first = false
			continue
		end
		local x, w = line:match(" (%-?[%d.]+),%-?[%d.]+ ([%d.]+)x[%d.]+")
		if x and (tonumber(x) :: number) + (tonumber(w) :: number) > edge + 0.6 then
			return false, line
		end
	end
	return true, ""
end

test("a listing at every limit stays inside its page: its whole name, its facts' long links", function()
	local app = page("extremes")
	local _, content = edges(app, "#store-readme")
	content += 1
	local install = box(app, '#store-header >> role:button "Install"')
	local heading = box(app, '#store-header >> role:heading')
	expect(heading.x + heading.w <= install.x).to_be(true)
	local ok, line = within(app, "#store-facts", content)
	expect(`{ok} {line}`).to_be("true ")
end)

test("a different plugin with the same id says so in a callout under the summary, with the way to it", function()
	local app = page("focus-timer")
	app:set_state("plugins", (function()
		local now = app:state("plugins")
		local list = {}
		for _, p in now.list do
			table.insert(list, p)
		end
		table.insert(list, { id = "focus-timer", name = "Focus timer", version = "9.9.9", author = { kind = "unknown" }, status = "on", permissions = {}, allowed = {}, ask = {}, hash = "x", source = { kind = "link", url = "https://e.example/f.zeron-plugin" } })
		return { state = now.state, list = list }
	end)())
	settle(app)
	expect(box(app, "#store-cannot").y > box(app, "#store-summary").y).to_be(true)
	app:click('#store-cannot >> role:button "Show it in Installed"')
	settle(app)
	expect(app:has('role:tab "Installed" selected') or app:has("#catalog-card")).to_be(true)
end)

test("where it stands is one line under the button, ending on the content edge; Installed is that line, not a button", function()
	local app = page("focus-timer")
	local text, content = edges(app, "#store-readme")
	content += 1
	-- Plain Install: the button alone, nothing said under it.
	expect(app:has("#store-state")).to_be(false)
	app:click('#store-header >> role:button "Install"')
	app:command("store.install_done")
	settle(app)
	local button = box(app, '#store-slot >> role:button "Review"')
	local line = box(app, "#store-state")
	expect(app:tree("#store-state")).to_contain("It's waiting for you to allow it.")
	expect(math.abs(line.x + line.w - content) < 0.6).to_be(true)
	expect(math.abs(button.x + button.w - content) < 0.6).to_be(true)
	expect(line.y >= button.y + button.h).to_be(true)
	-- (One line of the small type.)
	expect(line.h <= 18).to_be(true)
	app:command("store.allowed")
	settle(app)
	expect(app:tree("#store-state")).to_contain("Installed")
	-- (With settings of its own, Open settings over it; else the line alone.)
	expect(app:count("#store-slot >> role:button") <= 1).to_be(true)
	expect(math.abs(box(app, "#store-state").x + box(app, "#store-state").w - content) < 0.6).to_be(true)
	expect(text > 0).to_be(true)
end)

test("a README's headings are drawn a level down, none larger than the page's title, and a title that repeats the name isn't drawn", function()
	local app = page("focus-timer")
	local readme = app:tree("#store-readme")
	expect(readme).never.to_contain('heading "Focus timer"')
	expect(readme).to_contain('heading "Questions"')
	local title = box(app, '#store-header >> role:heading')
	local n = 0
	for line in readme:gmatch("[^\n]+") do
		if line:find(" heading ") then
			n += 1
			local h = tonumber(line:match("[%d.]+x([%d.]+)")) or 0
			-- (A heading's box holds its room above, ds/markdown's: 12 over
			-- the first two levels, 4 over the third; the rest its line.)
			local room = if h >= 36 then 12 else 4
			expect(h - room <= title.h).to_be(true)
		end
	end
	expect(n).to_be(3)
end)

test("twelve versions of 64 KB of notes each keep Report within a screen of Versions", function()
	local app = page("extremes")
	app:scroll(SCROLL, 1e6)
	settle(app)
	local versions = box(app, 'role:group "Versions"')
	local report = box(app, 'role:link "Report this plugin"')
	-- The latest few, the latest's notes to about ten lines; Show all for
	-- the rest.
	expect(app:count("#store-shots") <= 1).to_be(true)
	expect(app:has('role:button "Show all 12 versions"')).to_be(true)
	expect(report.y - versions.y < 900).to_be(true)
	app:click('role:button "Show all 12 versions"')
	settle(app)
	app:scroll(SCROLL, 1e6)
	settle(app)
	versions = box(app, 'role:group "Versions"')
	report = box(app, 'role:link "Report this plugin"')
	expect(app:has('role:button "Show all 12 versions"')).to_be(false)
	-- (Each older one a line of its notes.)
	expect(report.y - versions.y < 2 * 900).to_be(true)
end)

test("publishing's buttons and its versions' actions end on one edge, the content's", function()
	local app = mounted()
	app:click('role:tab "Yours"')
	settle(app)
	app:click('role:button "Publish…"')
	app:wait("#publish-checks", { timeout_ms = 3000 })
	settle(app, 500)
	local _, content = edges(app, "#publish-checks")
	-- (The checks' column is 1 in from its card's edge.)
	content += 1
	-- (Each row's last: Check again stands before Submit.)
	for _, target in { "#publish-submit", 'role:button "Stop new installs"', "#keys-new" } do
		local b = box(app, target)
		expect(`{target} {math.abs(b.x + b.w - content) < 0.6}`).to_be(`{target} true`)
	end
end)

test("a deploy key's secret is one line in the code face, never broken inside it", function()
	local app = mounted()
	app:click('role:tab "Yours"')
	settle(app)
	app:click('role:button "Publish…"')
	app:wait("#publish-checks", { timeout_ms = 3000 })
	settle(app, 400)
	app:scroll(SCROLL, 2000)
	app:frames(10)
	app:click("#keys-new")
	settle(app)
	app:type('role:dialog >> role:textbox "Name"', "CI")
	app:click('role:dialog >> role:button "Create key"')
	settle(app)
	app:command("store_stub.signed_in_again")
	settle(app, 400)
	local line = app:tree("#key-secret")
	local text_h = tonumber(line:match('text "zpk_[^"]*" [%-%d.]+,[%-%d.]+ [%d.]+x([%d.]+)')) or 0
	expect(text_h <= 20).to_be(true)
end)
