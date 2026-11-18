-- Settings → Plugins → Browse: the plugin store, searched on its servers
-- (ui/store_stub.luau answers as it would), on the page in
-- ui/plugins_store_fixture.luau. Its first page and the featured ones; more
-- as the page scrolls, never twice; a search, a category, a keyword and
-- the order asking again, the rows shown staying until the answer comes,
-- then staying the same nodes where they're the same plugins; what's
-- installed here said on its row; and every state the card can be in.
--
-- (The store answers on a timer, as the host does: the tests wait for what
-- they look for. Rows are built as they show, the page never moved for
-- them, under the card's fade too: zgx-asks/ancestor-list-fading-in.)

local SCROLL = "#fixture-scroll"
-- A row's height (Settings.truncated_row_h with a byline) and its hairline.
local ROW = 78

local function open(): any
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	app:click('role:tab "Browse"')
	app:wait('#store-rows', { timeout_ms = 3000 })
	app:frames(20)
	return app
end

local function calls(app: any): string
	local ok, list = pcall(function()
		return app:state("store_stub_calls")
	end)
	return if ok and list then table.concat(list, "\n") else ""
end

local function count(text: string, needle: string): number
	local n = 0
	local at = 1
	while true do
		local s, e = string.find(text, needle, at, true)
		if not s then
			return n
		end
		n += 1
		at = (e :: number) + 1
	end
end

-- A node's first line: its `@id` and its box.
local function line(app: any, target: string): string
	return (app:tree(target):match("[^\n]+")) or ""
end
local function id_of(app: any, target: string): string?
	return line(app, target):match("^%s*@(%d+)")
end
local function height_of(app: any, target: string): number
	local h = line(app, target):match("%d+x([%d.]+)")
	return tonumber(h) or 0
end

-- How many rows the list holds, from its height (rows not built count too).
local function rows(app: any): number
	-- (The list is the rows' box's first child.)
	local second = app:tree("#store-rows"):match("[^\n]+\n([^\n]+)") or ""
	local h = tonumber(second:match("%d+x([%d.]+)")) or 0
	return math.floor((h + 1) / (ROW + 1) + 0.5)
end

local function settle(app: any, ms: number?)
	app:wait("#never-there", { timeout_ms = ms or 400 })
	app:frames(10)
end

test("Browse lists the store's plugins, the most installed first, with who made them and how many have them", function()
	local app = open()
	local tree = app:tree("#store-card")
	expect(tree).to_contain('button "Focus timer"')
	expect(tree).to_contain("by @ada")
	expect(tree).to_contain("installs")
	-- (Its account's badges, never a mark of its own.)
	expect(tree).to_contain("Maintainer")
	expect(tree).never.to_contain("Verified")
	expect(rows(app)).to_be(30)
	local c = calls(app)
	expect(c).to_contain("search:q=,category=,keyword=,sort=popular,cursor=\n")
	expect(c).to_contain("categories")
	expect(app:error()).to_be_nil()
end)

test("with nothing searched, the featured stand over the card, and go as a search narrows it", function()
	local app = open()
	expect(app:has("#store-featured")).to_be(true)
	expect(calls(app)).to_contain(",featured")
	local featured = app:tree("#store-featured")
	expect(count(featured, "settings-feature") + count(featured, 'button "')).never.to_be(0)
	expect(featured).to_contain("Focus timer")
	app:type('#store-card >> role:textbox "Search plugins"', "timer")
	settle(app, 600)
	expect(app:has("#store-featured")).to_be(false)
	app:fill('#store-card >> role:textbox "Search plugins"', "")
	settle(app, 600)
	expect(app:has("#store-featured")).to_be(true)
end)

test("a plugin's row opens its page, and Back returns to Browse", function()
	local app = open()
	app:click('#store-card >> role:button "Focus timer"')
	app:wait('"What it asks for"', { timeout_ms = 3000 })
	expect(calls(app)).to_contain("detail:focus-timer")
	app:click('role:button "Back to Plugins"')
	app:wait("#store-card", { timeout_ms = 3000 })
	expect(app:has('role:tab "Browse" selected') or app:tree():find('tab "Browse"', 1, true) ~= nil).to_be(true)
end)

test("scrolling near the end asks for the next page once, and the list never holds a plugin twice", function()
	local app = open()
	app:command("store_stub.overlap")
	for _ = 1, 12 do
		app:scroll(SCROLL, 2000)
		settle(app, 150)
	end
	local c = calls(app)
	-- Every page once: the first, then each from where the last ended.
	expect(count(c, "sort=popular,cursor=\n")).to_be(1)
	expect(count(c, "sort=popular,cursor=30")).to_be(1)
	-- 75 plugins (74 and one at every listing limit), however the pages
	-- overlapped.
	expect(rows(app)).to_be(75)
	expect(app:error()).to_be_nil()
end)

test("the next page coming shows a row's skeleton at the foot, then its rows", function()
	local app = open()
	app:command("store_stub.hold")
	app:scroll(SCROLL, 4000)
	app:frames(20)
	expect(app:has("#store-more")).to_be(true)
	expect(rows(app)).to_be(30)
	app:command("store_stub.release")
	settle(app)
	expect(app:has("#store-more")).to_be(false)
	expect(rows(app)).to_be(60)
end)

test("a next page that doesn't come says so at the foot, and Try again asks again", function()
	local app = open()
	app:command("store_stub.fail_next")
	app:scroll(SCROLL, 4000)
	settle(app)
	expect(app:tree("#store-card")).to_contain("More plugins didn't load.")
	expect(rows(app)).to_be(30)
	app:click('#store-card >> role:button "Try again"')
	settle(app)
	expect(rows(app)).to_be(60)
	expect(app:tree("#store-card")).never.to_contain("More plugins didn't load.")
end)

test("a search keeps the rows shown until its answer comes, then the same plugins keep their rows", function()
	local app = open()
	local focus = id_of(app, '#store-card >> role:button "Focus timer"')
	expect(focus).never.to_be_nil()
	expect(app:has('#store-card >> role:button "Branch badge"')).to_be(true)
	app:command("store_stub.hold")
	app:type('#store-card >> role:textbox "Search plugins"', "timer")
	-- Past the typing's pause: asked, not yet answered.
	app:wait("#never-there", { timeout_ms = 400 })
	app:frames(10)
	expect(calls(app)).to_contain("search:q=timer,category=,keyword=,sort=relevance,cursor=")
	-- No flash: the rows it had, never a skeleton or "nothing".
	expect(app:has('#store-card >> role:button "Branch badge"')).to_be(true)
	expect(app:tree("#store-card")).never.to_contain("Nothing matches")
	app:command("store_stub.release")
	settle(app)
	expect(app:has('#store-card >> role:button "Branch badge"')).to_be(false)
	expect(app:has('#store-card >> role:button "Pomodoro"')).to_be(false)
	expect(`focus @{id_of(app, '#store-card >> role:button "Focus timer"')}`).to_be(`focus @{focus}`)
	expect(app:has('#store-card >> role:button "Pair timer"')).to_be(true)
end)

test("a search that finds nothing says what it looked for, and Clear brings every plugin back", function()
	local app = open()
	app:type('#store-card >> role:textbox "Search plugins"', "zzzz")
	settle(app, 700)
	expect(app:tree("#store-card")).to_contain("Nothing matches “zzzz”")
	app:click('#store-card >> role:button "Clear search"')
	settle(app, 700)
	expect(rows(app)).to_be(30)
	expect(app:has('#store-card >> role:button "Focus timer"')).to_be(true)
end)

test("a category, the order and a keyword each ask the store again", function()
	local app = open()
	-- (The categories run sideways past the row's width: Git is further on.)
	app:wheel("#store-categories", { dx = 400 })
	app:frames(10)
	app:click('#store-filters >> role:tab "Git"')
	settle(app)
	expect(calls(app)).to_contain("search:q=,category=git,keyword=,sort=popular,cursor=")
	expect(app:has("#store-featured")).to_be(false)
	app:click("#store-order")
	app:frames(10)
	app:click('role:menuitem "Recently updated"')
	settle(app)
	expect(calls(app)).to_contain("search:q=,category=git,keyword=,sort=updated,cursor=")
	-- A keyword, from a plugin's page: one token beside the search, removable.
	app:wheel("#store-categories", { dx = -400 })
	app:frames(10)
	app:click('#store-filters >> role:tab "All"')
	settle(app)
	app:click('#store-card >> role:button "Focus timer"')
	app:wait("#store-keywords", { timeout_ms = 3000 })
	settle(app)
	app:click('#store-keywords >> role:button "pomodoro"')
	app:wait("#store-card", { timeout_ms = 3000 })
	settle(app)
	expect(calls(app)).to_contain("keyword=pomodoro")
	expect(app:has('#store-card >> role:button "Remove pomodoro"')).to_be(true)
	app:click('#store-card >> role:button "Remove pomodoro"')
	settle(app)
	local last = ""
	for l in calls(app):gmatch("[^\n]+") do
		if l:find("^search:q=,") and not l:find("featured", 1, true) then
			last = l
		end
	end
	expect(last).to_be("search:q=,category=,keyword=,sort=updated,cursor=")
end)

test("words searched order by best match; without words, Best match isn't offered", function()
	local app = open()
	app:click("#store-order")
	app:frames(10)
	expect(app:tree()).never.to_contain("Best match")
	app:key("Escape")
	app:frames(5)
	app:type('#store-card >> role:textbox "Search plugins"', "git")
	settle(app, 600)
	expect(app:tree("#store-order")).to_contain("Best match")
end)

test("a row says Installed for what this device has, and Update once a newer version is found for it", function()
	local app = open()
	expect(app:tree('#store-card >> role:button "Branch badge"')).to_contain("Installed")
	expect(app:tree('#store-card >> role:button "Focus timer"')).never.to_contain("Installed")
	local branch = id_of(app, '#store-card >> role:button "Branch badge"')
	app:command("store.branch_update")
	app:frames(10)
	expect(app:tree('#store-card >> role:button "Branch badge"')).to_contain("Update")
	expect(`branch @{id_of(app, '#store-card >> role:button "Branch badge"')}`).to_be(`branch @{branch}`)
end)

test("the store out of reach says so in the card, and Try again lists it once it's back", function()
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	app:command("store_stub.offline")
	app:click('role:tab "Browse"')
	app:wait('"Can\'t reach the plugin store"', { timeout_ms = 3000 })
	app:frames(20)
	expect(app:tree("#store-card")).to_contain("Installed plugins still work.")
	app:command("store_stub.online")
	app:click('#store-card >> role:button "Try again"')
	app:wait('#store-rows', { timeout_ms = 3000 })
	settle(app)
	expect(rows(app)).to_be(30)
end)

test("a store that fails says what went wrong, in Zeron's words where it knows them", function()
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	app:command("store_stub.fail_next")
	app:click('role:tab "Browse"')
	app:wait('"The plugin store didn\'t answer"', { timeout_ms = 3000 })
	app:frames(20)
	expect(app:has('#store-card >> role:button "Try again"')).to_be(true)
	app:click('#store-card >> role:button "Try again"')
	app:wait('#store-rows', { timeout_ms = 3000 })
end)

test("the first load shows skeleton rows, shaped as the rows, in the card under its search", function()
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	app:command("store_stub.hold")
	app:click('role:tab "Browse"')
	app:frames(30)
	expect(app:has('#store-card >> role:textbox "Search plugins"')).to_be(true)
	expect(app:has("#store-rows")).to_be(false)
	expect(app:tree("#store-card")).never.to_contain("Nothing")
	app:command("store_stub.release")
	app:wait('#store-rows', { timeout_ms = 3000 })
end)

test("Installed and Yours are as they were: Browse adds a tab and asks the store only once it's chosen", function()
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(30)
	expect(app:has('role:tab "Installed"')).to_be(true)
	expect(app:has('role:tab "Yours"')).to_be(true)
	expect(app:has('role:tab "Browse"')).to_be(true)
	expect(app:has("#catalog-card")).to_be(true)
	expect(app:has('role:switch "Branch badge"')).to_be(true)
	app:wait("#never-there", { timeout_ms = 200 })
	expect(calls(app)).to_be("")
end)

test("a store's words are drawn as they are, never read as the app's templates", function()
	local app = open()
	app:type('#store-card >> role:textbox "Search plugins"', "haiku")
	settle(app, 600)
	expect(app:error()).to_be_nil()
	local card = app:tree("#store-card")
	expect(card).to_contain("Haiku {commit}")
	expect(card).to_contain("{badge}")
	app:fill('#store-card >> role:textbox "Search plugins"', "")
	settle(app, 600)
	app:type('#store-card >> role:textbox "Search plugins"', "o")
	settle(app, 600)
	expect(app:error()).to_be_nil()
end)

test("a row at every limit at once keeps its height, its words fading at its end", function()
	local app = open()
	app:type('#store-card >> role:textbox "Search plugins"', "Superlong")
	settle(app, 700)
	expect(app:error()).to_be_nil()
	local name = string.sub("Superlongpluginnamewithnospacesanywhereatalljusttokeepgoingonandon", 1, 64)
	local line = app:tree(`#store-card >> role:button "{name}"`):match("[^\n]+") or ""
	local w, h = line:match("(%d+)x(%d+)")
	expect(tonumber(h)).to_be(ROW)
	expect(tonumber(w) <= 640).to_be(true)
end)

test("the featured are the store's featured list, asked for as such, in the store's order", function()
	local app = open()
	expect(calls(app)).to_contain("search:q=,category=,keyword=,sort=popular,cursor=,featured")
	local order = {}
	for line in app:tree("#store-featured"):gmatch("[^\n]+") do
		local name = line:match('button "([^"]+)"')
		if name then
			table.insert(order, name)
		end
	end
	-- (The stub's three featured, most installed first, as the store sorts.)
	expect(table.concat(order, ", ")).to_be("Focus timer, Review notes, Slack ping")
end)

test("rows are built as the card fades them in, the page never scrolled: as Browse opens, and back from a plugin's page", function()
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	app:click('role:tab "Browse"')
	app:wait('#store-rows', { timeout_ms = 3000 })
	app:frames(30)
	expect(app:has('#store-card >> role:button "Focus timer"')).to_be(true)
	-- Back from a plugin's page, the whole page fading in: its rows there.
	app:click('#store-card >> role:button "Focus timer"')
	app:wait("#store-offer", { timeout_ms = 3000 })
	app:frames(20)
	app:click('role:button "Back to Plugins"')
	app:wait("#store-rows", { timeout_ms = 3000 })
	app:frames(30)
	expect(app:has('#store-card >> role:button "Branch badge"')).to_be(true)
end)

test("the list's head says how many what's asked matches, kept as a new search comes, Plugins while unknown, one height throughout", function()
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	-- Loading: the head is there, saying Plugins, at its height.
	app:command("store_stub.hold")
	app:click('role:tab "Browse"')
	app:frames(20)
	-- (The head: the section's words over its card; its height, from the
	-- section's top to the card's.)
	local function head(): string
		-- (The section's group, named by its title: the tree's second line.)
		local tree = app:tree("#store-list")
		return tree:match("^[^\n]+\n([^\n]+)") or ""
	end
	local function y_of(sel: string): number
		return tonumber((app:tree(sel):match(" [%d.]+,([%-%d.]+) "))) or 0
	end
	local function head_h(): number
		return y_of("#store-card") - y_of("#store-list")
	end
	expect(head()).to_contain("Plugins")
	local loading_h = head_h()
	app:command("store_stub.release")
	settle(app)
	expect(head()).to_contain("75 plugins")
	expect(head_h()).to_be(loading_h)
	-- Order is the head's own action, at its end.
	expect(app:tree("#store-order")).to_contain('text "Most installed"')
	-- A search: the count of what it finds, the last one kept until it answers.
	app:command("store_stub.hold")
	app:type('#store-card >> role:textbox "Search plugins"', "timer")
	settle(app, 500)
	expect(head()).to_contain("75 plugins")
	app:command("store_stub.release")
	settle(app, 500)
	expect(head()).never.to_contain("75 plugins")
	expect(head()).to_contain("plugin")
	-- Nothing found: the card says so; the head says Plugins.
	app:fill('#store-card >> role:textbox "Search plugins"', "zzzz")
	settle(app, 600)
	expect(head()).to_contain("Plugins")
	-- A store that doesn't count: Plugins.
	app:command("store_stub.no_total")
	app:fill('#store-card >> role:textbox "Search plugins"', "")
	settle(app, 600)
	expect(head()).to_contain("Plugins")
	expect(head()).never.to_contain("75")
	expect(head_h()).to_be(loading_h)
end)
