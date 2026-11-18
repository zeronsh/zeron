-- The store's screens laid out as Settings' are, measured by their boxes
-- (the captures' findings): on the page in ui/plugins_store_fixture.luau.
-- One text edge (a card's outer edge + 17, where rows' tiles and words
-- start), one content edge (a card's outer edge − 17, where rows' controls
-- end); nothing under the pointer moves as things arrive; a skeleton that
-- waits stops breathing after a bound.
--
-- (Browse's rows are built as the page's scroll shows them, so its tests
-- scroll a point first: zgx-asks/ancestor-list-fading-in.)

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

test("the pictures' strip is as tall as its pictures", function()
	local app = page("focus-timer")
	expect(box(app, "#store-shots").h).to_be(125)
end)

test("Browse's filters are controls on the card's even inset, settings-select wide, one line whatever comes", function()
	local app = mounted()
	app:command("store_stub.hold")
	app:click('role:tab "Browse"')
	app:frames(20)
	local card = box(app, "#store-card")
	local bar = box(app, "#store-filters")
	local select = box(app, '#store-filters >> role:button "Category"')
	expect(bar.h).to_be(44)
	expect(math.abs(select.x - (card.x + 1 + 8)) < 0.6).to_be(true)
	expect(select.w).to_be(168)
	local search_before = box(app, '#store-card >> role:textbox "Search plugins"').y
	app:command("store_stub.release")
	settle(app, 500)
	-- The chips came: the bar's height, and everything under the search, held.
	expect(app:tree("#store-filters")).to_contain('button "review"')
	expect(box(app, "#store-filters").h).to_be(44)
	expect(box(app, '#store-card >> role:textbox "Search plugins"').y).to_be(search_before)
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
	app:scroll(SCROLL, 1)
	app:frames(4)
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
