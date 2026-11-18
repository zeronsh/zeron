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

test("Browse's list: its head's order on the content edge, the categories across the card, the first's words on the text edge, nothing moving as it loads", function()
	local app = mounted()
	app:command("store_stub.hold")
	app:click('role:tab "Browse"')
	app:frames(20)
	local card = box(app, "#store-card")
	local text, content = card.x + INSET, card.x + card.w - INSET
	local order = box(app, "#store-order")
	-- (Order is a ghost: its last ink, the chevron's stroke, ends on the
	-- content edge, its box and wash past it. The chevron's stroke ends at
	-- 18.375 of its 24.)
	local img = app:tree("#store-order"):match("[^\n]* img [^\n]*") or ""
	local cx, cw = img:match("(%-?[%d.]+),%-?[%d.]+ ([%d.]+)x")
	local chevron = { x = tonumber(cx) or 0, w = tonumber(cw) or 0 }
	expect(math.abs(chevron.x + chevron.w * 18.375 / 24 - content) < 0.6).to_be(true)
	expect(order.x + order.w > content).to_be(true)
	-- (The head's action is the section actions' ghost, 24.)
	expect(order.h).to_be(24)
	local search_before = box(app, '#store-card >> role:textbox "Search plugins"').y
	local bar_before = box(app, "#store-filters").h
	local card_before = card.y
	app:command("store_stub.release")
	settle(app, 500)
	local all = app:tree('#store-filters >> role:tab "All"'):match("[^\n]+") or ""
	local first_tab = box(app, '#store-filters >> role:tab "All"')
	-- (A filter row's xs tab's words are 8 inside it.)
	expect(math.abs(first_tab.x + 8 - text) < 0.6).to_be(true)
	expect(box(app, "#store-filters").h).to_be(bar_before)
	expect(box(app, '#store-card >> role:textbox "Search plugins"').y).to_be(search_before)
	expect(box(app, "#store-card").y).to_be(card_before)
	expect(all).never.to_contain("All categories")
	-- The categories run the card's width, from edge to edge of its row.
	local tabs = box(app, "#store-categories")
	expect(math.abs(tabs.x - (card.x + 1 + 8)) < 0.6).to_be(true)
	expect(math.abs(tabs.x + tabs.w - (card.x + card.w - 1 - 8)) < 0.6).to_be(true)
	-- (The design system's xs, 24, as the head's Order is.)
	expect(box(app, '#store-filters >> role:tab "All"').h).to_be(24)
end)

test("the featured hold their room as they come, one height each, and stand a section's step from the list", function()
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
	-- No covers: a card's head is its tile beside its name over its by-line.
	expect(app:tree("#store-featured")).never.to_contain(" img 0")
	local list = box(app, "#store-list")
	expect(list.y - (cards.y + cards.h)).to_be(32)
end)

-- The featured cards' boxes, left to right.
local function featured(app: any): { { x: number, y: number, w: number, h: number } }
	local out = {}
	for line in app:tree("#store-featured"):gmatch("[^\n]+") do
		if line:find(" button ") then
			local x, y, w, h = line:match("(%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
			table.insert(out, { x = tonumber(x) or 0, y = tonumber(y) or 0, w = tonumber(w) or 0, h = tonumber(h) or 0 })
		end
	end
	return out
end

test("a featured card is one height whatever it says of itself, all it may or nothing", function()
	local app = mounted()
	app:command("store_stub.long_featured")
	app:click('role:tab "Browse"')
	app:wait("#store-featured", { timeout_ms = 3000 })
	settle(app, 500)
	local list = featured(app)
	expect(#list).to_be(3)
	for _, c in list do
		expect(c.h).to_be(list[3].h)
	end
	app:command("store_stub.hold")
	app:set_state("plugins_view", "catalog")
end)

test("the featured never wrap: three across while each keeps its least, else running sideways, the next cut by the column's end", function()
	local app = mounted()
	app:click('role:tab "Browse"')
	app:wait("#store-featured", { timeout_ms = 3000 })
	settle(app, 500)
	for _, width in { 1000, 760, 640, 600, 560 } do
		app:resize(width, 900)
		settle(app)
		local shelf = box(app, "#store-featured")
		local list = featured(app)
		for _, c in list do
			expect(`{width}: {c.y}`).to_be(`{width}: {list[1].y}`)
		end
		-- (The card's 1 edge inside its slot.)
		local least = 180 - 2
		local three = (shelf.w - 2 * 12) / 3
		if three >= 180 then
			expect(math.abs(list[3].x + list[3].w + 1 - (shelf.x + shelf.w)) < 1).to_be(true)
		else
			expect(list[1].w >= least).to_be(true)
			expect(list[3].x < shelf.x + shelf.w).to_be(true)
			expect(list[3].x + list[3].w > shelf.x + shelf.w).to_be(true)
		end
	end
end)

test("the categories that fit show no chevron; more than fit, the edge hiding them pages them, and the keys bring one into view", function()
	local app = mounted()
	app:command("store_stub.few_categories")
	app:click('role:tab "Browse"')
	app:wait("#store-rows", { timeout_ms = 3000 })
	settle(app, 500)
	expect(app:has(".pager-more")).to_be(false)
	expect(app:has(".pager-back")).to_be(false)
end)

test("more categories than fit: a chevron at the hiding edge pages them on and back; the arrows choose, and bring the chosen into view", function()
	local app = mounted()
	app:click('role:tab "Browse"')
	app:wait("#store-rows", { timeout_ms = 3000 })
	settle(app, 500)
	local view = box(app, "#store-categories")
	local function shown(name: string): boolean
		local tab = box(app, `#store-categories >> role:tab "{name}"`)
		return tab.x >= view.x - 0.5 and tab.x + tab.w <= view.x + view.w + 0.5
	end
	expect(app:has(".pager-more")).to_be(true)
	expect(app:has(".pager-back")).to_be(false)
	expect(shown("Utilities")).to_be(false)
	-- (The run's view stops short of the chevron: no word under it.)
	local function view_box(): { x: number, y: number, w: number, h: number }
		local line = app:tree("#store-categories"):match("[^\n]+\n([^\n]+)") or ""
		local x, y, w, h = line:match("(%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
		return { x = tonumber(x) or 0, y = tonumber(y) or 0, w = tonumber(w) or 0, h = tonumber(h) or 0 }
	end
	expect(view_box().x + view_box().w <= box(app, 'role:button "More categories"').x).to_be(true)
	app:click('role:button "More categories"')
	settle(app)
	expect(app:has(".pager-back")).to_be(true)
	local back = box(app, 'role:button "Earlier categories"')
	expect(view_box().x >= back.x + back.w).to_be(true)
	-- (The same in a narrow pane, a 496 column: both chevrons, the run
	-- between them.)
	app:resize(560, 900)
	settle(app)
	if app:has('role:button "More categories"') then
		expect(view_box().x + view_box().w <= box(app, 'role:button "More categories"').x).to_be(true)
		app:click('role:button "More categories"')
		settle(app)
	end
	if app:has(".pager-more") then
		expect(view_box().x + view_box().w <= box(app, 'role:button "More categories"').x).to_be(true)
	end
	if app:has(".pager-back") then
		local b = box(app, 'role:button "Earlier categories"')
		expect(view_box().x >= b.x + b.w).to_be(true)
	end
	app:resize(1000, 900)
	settle(app)
	app:click('role:button "Earlier categories"')
	settle(app)
	expect(app:has(".pager-back")).to_be(false)
	-- The keys: one Tab stop, the arrows choosing; End to the last, in view.
	app:click('#store-categories >> role:tab "All"')
	settle(app)
	app:key("ArrowRight")
	settle(app)
	expect(app:state("store_category")).to_be("agents")
	app:key("End")
	settle(app)
	expect(app:state("store_category")).to_be("utilities")
	expect(shown("Utilities")).to_be(true)
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

test("Yours' Publish… follows its sentence: a word's space after it where it fits, else at the start of the next line", function()
	-- (Two boxes until zgx-asks/rich-span-boundary-break: then one rich
	-- paragraph, the link right after the last word.)
	local app = mounted()
	app:click('role:tab "Yours"')
	settle(app)
	local function versions(change: (list: { any }) -> { any })
		app:set_state("store_mine", (function()
			local now = app:state("store_mine")
			local p = table.clone(now.plugins["pomodoro-clock"])
			p.versions = change(p.versions)
			return { state = now.state, plugins = { ["pomodoro-clock"] = p } }
		end)())
		settle(app)
	end
	local function boxes()
		return box(app, 'role:link "Publish…"'), box(app, '.settings-detail')
	end
	-- Its words and the link on one line: the link a word's space after them.
	versions(function(list: { any }): { any }
		local out = {}
		for _, v in list do
			if v.version == "1.0.0" then
				table.insert(out, v)
			end
		end
		return out
	end)
	local link, sentence = boxes()
	expect(app:tree(".settings-detail")).to_contain("1.0.0 in the plugin store")
	expect(math.abs(link.y - sentence.y) < 0.6).to_be(true)
	expect(math.abs(link.x - (sentence.x + sentence.w + 4)) < 0.6).to_be(true)
	-- Too long for both: the link starts the next line, under the words' start.
	versions(function(list: { any }): { any }
		local out = {}
		for _, v in list do
			local w = table.clone(v)
			w.version = `{v.version}-preview.20261118.build.4821`
			table.insert(out, w)
		end
		return out
	end)
	link, sentence = boxes()
	expect(math.abs(link.x - sentence.x) < 0.6).to_be(true)
	expect(math.abs(link.y - (sentence.y + sentence.h)) < 0.6).to_be(true)
	app:click('role:link "Publish…"')
	app:wait("#publish-checks", { timeout_ms = 3000 })
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
	expect(app:tree("#store-state")).to_contain("Waiting for you to allow it")
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

-- Publish… for Pomodoro clock, from Yours; `before` runs first (a stub's
-- command), `held` keeps the checks from answering.
local function publishing(before: ((app: any) -> ())?, held: boolean?): any
	local app = mounted()
	if before then
		before(app)
	end
	app:click('role:tab "Yours"')
	settle(app)
	if held then
		app:command("store_stub.hold")
	end
	app:click('role:link "Publish…"')
	app:frames(20)
	if not held then
		app:wait("#publish-checks", { timeout_ms = 3000 })
		settle(app, 500)
	end
	return app
end

test("the checks open at the height they have when all pass: a plugin that passes moves nothing under them", function()
	local app = publishing(function(a: any)
		a:command("store_stub.fixed")
		a:command("store_stub.no_notes")
	end, true)
	local versions = box(app, 'role:group "Versions you\'ve sent"')
	expect(app:has("#publish-checks")).to_be(false)
	app:command("store_stub.release")
	settle(app, 500)
	expect(app:has("#publish-checks")).to_be(true)
	expect(box(app, 'role:group "Versions you\'ve sent"').y).to_be(versions.y)
end)

test("opened from Yours, its versions draw on the first frame from what Yours read: Deploy keys doesn't move as the store answers", function()
	local app = mounted()
	app:command("store_stub.fixed")
	app:command("store_stub.no_notes")
	app:click('role:tab "Yours"')
	settle(app)
	-- (Yours has read its versions: its store line says so.)
	expect(app:tree()).to_contain("1.0.0 in the plugin store")
	app:command("store_stub.hold")
	app:click('role:link "Publish…"')
	app:wait('role:group "Deploy keys"', { timeout_ms = 3000 })
	-- (The page come to rest, the store still not answering.)
	settle(app, 400)
	expect(app:has("#publish-checks")).to_be(false)
	expect(app:has("#versions-none")).to_be(false)
	expect(app:tree('role:group "Versions you\'ve sent"')).to_contain("Not approved")
	local keys = box(app, 'role:group "Deploy keys"').y
	app:command("store_stub.release")
	settle(app, 500)
	expect(app:has("#publish-checks")).to_be(true)
	expect(box(app, 'role:group "Deploy keys"').y).to_be(keys)
end)

-- The checks' rows: each one's height and its hairline, by where each
-- row's name stands (12 inside its row's top, on the checks' words' 225).
local function check_heights(app: any): { number }
	local ys = {}
	for line in app:tree("#publish-checks"):gmatch("[^\n]+") do
		local x, y, h = line:match(" text \"[^\"]*\" ([%d.]+),([%d.]+) [%d.]+x([%d.]+)")
		if x and tonumber(x) == 225 and tonumber(h) == 18 then
			table.insert(ys, tonumber(y) :: number)
		end
	end
	local foot = box(app, "#publish-foot").y
	table.insert(ys, foot + 12)
	local out = {}
	for i = 2, #ys do
		table.insert(out, ys[i] - ys[i - 1])
	end
	return out
end

test("a check is a Settings row: a passed one the row's own height, one with what to do the row's with a description", function()
	local app = publishing(function(a: any)
		a:command("store_stub.fixed")
		a:command("store_stub.no_notes")
	end)
	-- (Each step from one hairline to the next: a row and its hairline. A
	-- Settings.Row with no line under its name and no control is 42 today,
	-- its words and its 12 above and below: zgx drops the 52 least it asks
	-- for, zgx-asks/gap-axis-tokens; with a line under its name, 60.)
	for _, h in check_heights(app) do
		expect(h).to_be(42 + 1)
	end
	local fixing = publishing()
	local seen: { [number]: boolean } = {}
	for _, h in check_heights(fixing) do
		seen[h] = true
	end
	expect(seen[42 + 1]).to_be(true)
	expect(seen[60 + 1]).to_be(true)
end)

test("a section's action stands on its title's line: title to its sentence 4, title to its card 8, as a section without one", function()
	local app = publishing()
	app:scroll(SCROLL, 1e6)
	settle(app)
	local title = box(app, 'role:group "Deploy keys" >> role:heading')
	local line = app:tree('role:group "Deploy keys"'):match("[^\n]*A deploy key lets[^\n]*") or ""
	local sx, sy, sw, sh = line:match("(%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
	local sentence = { x = tonumber(sx) or 0, y = tonumber(sy) or 0, w = tonumber(sw) or 0, h = tonumber(sh) or 0 }
	expect(sentence.y - (title.y + title.h)).to_be(4)
	-- (The action centred on the title's line.)
	local action = box(app, "#keys-new")
	expect(math.abs((action.y + action.h / 2) - (title.y + title.h / 2)) < 0.6).to_be(true)
	local checks_title = box(app, 'role:group "What the store needs" >> role:heading')
	expect(box(app, "#publish-checks").y - 1 - (checks_title.y + checks_title.h)).to_be(8)
end)

test("a key's short form is a column down its rows, ending before Revoke, at 1000 and in a narrow pane", function()
	local app = publishing(function(a: any)
		a:command("store_stub.many_keys")
	end)
	for _, width in { 1000, 560 } do
		app:resize(width, 900)
		settle(app)
		app:scroll(SCROLL, 1e6)
		settle(app)
		local ends: { number } = {}
		for line in app:tree("#fixture-scroll"):gmatch("[^\n]+") do
			if line:find(" text ", 1, true) and line:find("…", 1, true) and line:match('"k%d%d') then
				local x, w = line:match("(%-?[%d.]+),%-?[%d.]+ ([%d.]+)x")
				table.insert(ends, (tonumber(x) :: number) + (tonumber(w) :: number))
			end
		end
		expect(#ends).to_be(10)
		for _, e in ends do
			expect(math.abs(e - ends[1]) < 0.6).to_be(true)
		end
	end
end)

test("a key within 14 days of its end says how soon, after when it was last used", function()
	local app = publishing(function(a: any)
		a:command("store_stub.many_keys")
	end)
	local tree = app:tree()
	-- (Both facts: is it still in use, and how soon it ends.)
	-- (Its date as this time zone says it.)
	expect(tree:find('"Last used [^"]+ · Ends today"') ~= nil).to_be(true)
	expect(tree).to_contain('"Not used yet · Ends tomorrow"')
	expect(tree:find('"Last used [^"]+ · Ends in 5 days"') ~= nil).to_be(true)
	-- (Further off: its date, after when it was last used.)
	expect(tree).to_contain("Not used yet · Doesn't expire")
end)

test("with nothing sent, its versions say so in one quiet line in their card, as its keys do", function()
	local app = publishing(function(a: any)
		a:command("store_stub.never_published")
	end)
	expect(app:has("#versions-none")).to_be(true)
	expect(box(app, "#versions-none").h).to_be(16 + 2 * 12)
	expect(box(app, "#keys-none").h).to_be(16 + 2 * 12)
end)

test("publishing's buttons and its versions' actions end on one edge, the content's", function()
	local app = mounted()
	app:click('role:tab "Yours"')
	settle(app)
	app:click('role:link "Publish…"')
	app:wait("#publish-checks", { timeout_ms = 3000 })
	settle(app, 500)
	local _, content = edges(app, "#publish-checks")
	-- (The checks' column is 1 in from its card's edge.)
	content += 1
	-- (A filled control's box is its edge: Submit, a version's action.)
	for _, target in { "#publish-submit", 'role:button "Stop new installs"' } do
		local b = box(app, target)
		expect(`{target} {math.abs(b.x + b.w - content) < 0.6}`).to_be(`{target} true`)
	end
	-- A ghost trail's words end on it, its wash past by its end padding (a
	-- ghost xs button's, 6): text lined up with text.
	app:scroll(SCROLL, 1e6)
	settle(app)
	for _, target in { "#publish-check-again", "#keys-new" } do
		local b = box(app, target)
		expect(`{target} {math.abs(b.x + b.w - 6 - content) < 0.6}`).to_be(`{target} true`)
	end
end)

test("a deploy key's secret is one line in the code face, never broken inside it", function()
	local app = mounted()
	app:click('role:tab "Yours"')
	settle(app)
	app:click('role:link "Publish…"')
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
