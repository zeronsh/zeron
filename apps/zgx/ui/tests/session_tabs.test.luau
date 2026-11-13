-- The sessions' tabs on the top bar: they start just past the sidebar's "…" (with
-- the sidebar shut, past Back and Forward), and the one showing is never left in
-- the overflow. And a new chat's page stays a card with the sidebar closed. On
-- the shell in ui/shell_fixture.luau.

local CHATS = { "Fix the build", "Write the docs", "Tidy the tests", "Ship the release", "Answer the review" }
local IDS = { "a1", "a2", "a3", "b1", "b2" }

local function box(app: any, target: string): { x: number, w: number, y: number }
	local line = app:tree(target):match("[^\n]+") :: string
	local x, y, w = line:match(" (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x[%d%.]+")
	return { x = tonumber(x) :: number, y = tonumber(y) :: number, w = tonumber(w) :: number }
end

local function open(app: any, title: string)
	app:click(`#shell-sidebar >> role:button "{title}"`)
	app:frames(30)
end

-- A tab for each chat, each composed with a partner (a chat alone has none), the
-- last showing.
local function all_open(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	app:command("partner_chats")
	app:frames(10)
	open(app, CHATS[#CHATS])
	local tabs = {}
	for i, id in IDS do
		table.insert(tabs, { kind = "row", key = id, children = { { kind = "leaf", id = id }, { kind = "leaf", id = `p{i}` } } })
	end
	app:set_state("inset_split_trees", tabs)
	app:frames(30)
	return app
end

-- Wholly in the strip's room.
local function in_view(app: any, id: string)
	local room, tab = box(app, "#session-tabs-scroll"), box(app, `#session-tab-{id}`)
	expect(tab.x >= room.x - 0.5).to_be(true)
	expect(tab.x + tab.w <= room.x + room.w + 0.5).to_be(true)
end

test("the tabs start past the sidebar's menu, and past Back and Forward with it shut", function()
	local app = all_open()
	local menu = box(app, 'role:button "Sidebar view options"')
	local open_x = box(app, "#session-tabs").x
	expect(open_x >= menu.x + menu.w).to_be(true)
	-- (All the way left: over the sidebar, not from the page's card.)
	expect(open_x < box(app, "#shell-page").x).to_be(true)

	app:click('#titlebar >> "Toggle sidebar"')
	app:frames(60)
	local forward = box(app, '#titlebar >> "Forward"')
	local shut_x = box(app, "#session-tabs").x
	expect(shut_x >= forward.x + forward.w).to_be(true)
	expect(shut_x < open_x).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("the sidebar's menu still takes a click with the tabs along the top bar", function()
	local app = all_open()
	app:click('role:button "Sidebar view options"')
	app:frames(20)
	expect(app:wait('"Compact"', { timeout_ms = 5000 })).to_be(true)
	expect(app:error()).to_be_nil()
end)

-- Nothing laid over the page takes its clicks: the composer takes the keyboard, the
-- model picker opens.
local function page_takes_clicks(app: any)
	app:click('#chat-composer-dock >> role:textbox "Message"')
	app:frames(10)
	local focused = ""
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(" focused", 1, true) then
			focused = line
		end
	end
	expect(focused).to_contain("textbox")
	app:click('#chat-composer-dock >> role:button "Model"')
	expect(app:wait(".dropdown-fold", { timeout_ms = 5000 })).to_be(true)
	app:key("Escape")
	app:frames(30)
end

test("the page takes clicks: a new chat, a chat alone, and panes side by side", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	page_takes_clicks(app)

	open(app, "Fix the build")
	page_takes_clicks(app)

	app:set_state("inset_split_trees", { { kind = "row", children = { { kind = "leaf", id = "a1" }, { kind = "leaf", id = "a2" } } } })
	app:frames(30)
	page_takes_clicks(app)
	-- (Each pane, under the same layers, takes the focus at a click: the composer
	-- goes to it.)
	app:click("#split-pane-a1")
	app:frames(30)
	local at_a1 = box(app, "#chat-composer-dock").x
	app:click("#split-pane-a2")
	app:frames(30)
	expect(box(app, "#chat-composer-dock").x ~= at_a1).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("settings stand under the top bar: the tabs still there, the list and page cards", function()
	local app = all_open()
	app:command("shell.settings")
	app:frames(30)
	expect(app:has("#session-tab-b2")).to_be(true)
	local settings = box(app, "#shell-settings")
	expect(settings.y).to_be(42)
	-- A tab goes back to its chat.
	app:click("#session-tab-a1")
	app:frames(30)
	expect(app:has("#shell-settings")).to_be(false)
	expect(app:state("shell_route_path")).to_be("/session/a1")
	expect(app:error()).to_be_nil()
end)

test("a new chat's page has a panel, for the project its composer has chosen", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	app:click('#titlebar >> role:button "Panel"')
	app:frames(60)
	expect(box(app, "#shell-files").w > 300).to_be(true)
	expect(app:wait("#panel-open-terminal", { timeout_ms = 5000 })).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("with no project chosen, a new chat's panel still has what needs no folder", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	app:set_state("chat_project", "none")
	app:frames(30)
	app:click('#titlebar >> role:button "Panel"')
	app:frames(60)
	expect(app:wait("#panel-open-terminal", { timeout_ms = 5000 })).to_be(true)
	expect(app:has("#panel-open-browser")).to_be(true)
	expect(app:has("#panel-open-files")).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("an open panel stands beside settings, and its toggle closes it there", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	open(app, "Fix the build")
	app:click('#titlebar >> role:button "Panel"')
	app:frames(60)
	local panel = box(app, "#shell-files")
	app:command("shell.settings")
	app:frames(60)
	expect(box(app, "#shell-files").w).to_be(panel.w)
	local settings = box(app, "#settings")
	expect(settings.x + settings.w <= panel.x).to_be(true)
	-- Expanded, it has what's past the list, not under it.
	app:click("#panel-expand")
	app:frames(60)
	local nav, wide = box(app, "#settings-nav"), box(app, "#shell-files")
	expect(wide.w > panel.w).to_be(true)
	expect(nav.w > 200).to_be(true)
	expect(nav.x + nav.w <= wide.x).to_be(true)
	app:click("#panel-expand")
	app:frames(60)
	app:command("shell.toggle_files")
	app:frames(60)
	expect(box(app, "#settings").w > settings.w).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("typing a trigger in a split opens one menu, not one for the hidden composer too", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	open(app, "Fix the build")
	app:set_state("inset_split_trees", { { kind = "row", children = { { kind = "leaf", id = "a1" }, { kind = "leaf", id = "a2" } } } })
	app:frames(30)
	app:type('#chat-composer-dock >> role:textbox "Message"', "/")
	app:frames(30)
	local open_menus = 0
	for line in app:tree():gmatch("[^\n]+") do
		if line:find("#mention-popover", 1, true) or line:find("#mention-panel", 1, true) then
			if not line:find(" hidden", 1, true) and line:find("#mention-panel", 1, true) then
				open_menus += 1
			end
		end
	end
	expect(open_menus).to_be(1)
	expect(app:error()).to_be_nil()
end)

test("each tab opened is in view, with the panel open and the strip narrow", function()
	local app = all_open()
	app:click('#titlebar >> role:button "Panel"')
	app:frames(60)
	-- (By the keys: with two panes and the panel, the sidebar has stepped aside.)
	local function go(id: string)
		for _ = 1, 12 do
			if tostring(app:state("shell_route_path")):find(id, 1, true) then
				return
			end
			app:command("shell.next_session")
			app:frames(30)
		end
		error(`never reached {id}`)
	end
	for i, id in IDS do
		if i < #IDS then
			go(id)
			in_view(app, id)
		end
	end
	-- Back along them, each hidden in turn by the one before.
	for i = #IDS, 1, -1 do
		go(IDS[i])
		in_view(app, IDS[i])
	end
	expect(app:error()).to_be_nil()
end)

test("the panel opening keeps the tab showing in view as the strip narrows", function()
	local app = all_open()
	in_view(app, "b2")
	app:click('#titlebar >> role:button "Panel"')
	app:frames(60)
	in_view(app, "b2")
	expect(app:error()).to_be_nil()
end)

test("a new chat's page stays a card with the sidebar closed", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	local before = box(app, "#shell-page")
	app:click('#titlebar >> "Toggle sidebar"')
	-- (No flat page on the way: the same set in throughout its slide.)
	for _ = 1, 8 do
		app:frames(5)
		expect(box(app, "#shell-page").y).to_be(before.y)
	end
	app:frames(30)
	expect(box(app, "#shell-page").x > 0).to_be(true)
	expect(box(app, "#shell-page").y).to_be(before.y)
	expect(app:error()).to_be_nil()
end)

-- The tabs as they stand, by chat.
local function tabs_shown(app: any): { [string]: boolean }
	local out: { [string]: boolean } = {}
	for id in app:tree("#session-tabs"):gmatch("#session%-tab%-([%w_]+)") do
		out[id] = true
	end
	return out
end

local function choose(app: any, tab: string, item: string)
	app:right_click(`#session-tab-{tab}`)
	expect(app:wait(`role:menuitem "{item}"`, { timeout_ms = 5000 })).to_be(true)
	app:click(`role:menuitem "{item}"`)
	app:frames(30)
end

test("a chat tab's menu has the panel tabs' choices", function()
	local app = all_open()
	app:right_click("#session-tab-a2")
	for _, item in { "Close Tab", "Close Other Tabs", "Close Tabs to the Left", "Close Tabs to the Right" } do
		expect(app:wait(`role:menuitem "{item}"`, { timeout_ms = 5000 })).to_be(true)
	end
	expect(app:error()).to_be_nil()
end)

test("Close Tab closes that tab, and the chat showing stays if it was another", function()
	local app = all_open()
	choose(app, "a2", "Close Tab")
	local now = tabs_shown(app)
	expect(now.a2).to_be_nil()
	expect(now.a1).to_be(true)
	expect(tostring(app:state("shell_route_path")):find("b2", 1, true) ~= nil).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("Close Other Tabs leaves that tab, and goes to it when the chat showing was closed", function()
	local app = all_open()
	choose(app, "a2", "Close Other Tabs")
	local now = tabs_shown(app)
	expect(now.a2).to_be(true)
	for _, id in { "a1", "a3", "b1", "b2" } do
		expect(now[id]).to_be_nil()
	end
	expect(tostring(app:state("shell_route_path")):find("a2", 1, true) ~= nil).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("Close Tabs to the Left and to the Right close only that side", function()
	local app = all_open()
	-- (b2 shows, to the right: left of a3 goes, and the chat showing stays.)
	choose(app, "a3", "Close Tabs to the Left")
	local now = tabs_shown(app)
	expect(now.a1).to_be_nil()
	expect(now.a2).to_be_nil()
	expect(now.a3).to_be(true)
	expect(now.b2).to_be(true)
	expect(tostring(app:state("shell_route_path")):find("b2", 1, true) ~= nil).to_be(true)
	-- (Right of a3 takes the chat showing with it: a3 opens.)
	choose(app, "a3", "Close Tabs to the Right")
	now = tabs_shown(app)
	expect(now.a3).to_be(true)
	expect(now.b1).to_be_nil()
	expect(now.b2).to_be_nil()
	expect(tostring(app:state("shell_route_path")):find("a3", 1, true) ~= nil).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("a tab still opens its chat on a click, and reorders by a drag, with its menu on", function()
	local app = all_open()
	app:click("#session-tab-a1")
	app:frames(20)
	expect(tostring(app:state("shell_route_path")):find("a1", 1, true) ~= nil).to_be(true)
	expect(box(app, "#session-tab-a1").x < box(app, "#session-tab-a2").x).to_be(true)
	-- (To the tab past the next: its leading edge passes the next one's middle
	-- on the way.)
	app:drag("#session-tab-a1", "#session-tab-a3")
	app:frames(40)
	expect(box(app, "#session-tab-a2").x < box(app, "#session-tab-a1").x).to_be(true)
	expect(box(app, "#session-tab-b1").x > box(app, "#session-tab-a1").x).to_be(true)
	expect(app:error()).to_be_nil()
end)

-- Carried off the strip onto a pane's edge, a chat's tab lands in that split,
-- as a sidebar row does: the split's panes show where only once it's off.
test("a session tab carried onto a split pane's edge joins that split", function()
	local app = all_open()
	expect(app:has("#split-pane-a1")).to_be(false)
	-- (Onto the pane's right edge: its transcript's rail stands there.)
	app:drag("#session-tab-a1", "#split-pane-p5 >> #transcript-rail")
	app:frames(40)
	expect(app:has("#split-pane-a1")).to_be(true)
	expect(app:has("#split-pane-p5")).to_be(true)
	expect(app:error()).to_be_nil()
end)

-- A tab carried off the strip and let go over nothing flies back into its
-- place: one tab throughout, none left behind in the strip while it's away.
test("a tab let go off the strip goes back to its place", function()
	local app = all_open()
	app:frames(20)
	local was = box(app, "#session-tab-a2").x
	app:drag("#session-tab-a2", "#star-banner")
	app:frames(60)
	expect(math.abs(box(app, "#session-tab-a2").x - was) <= 0.5).to_be(true)
	expect(box(app, "#session-tab-a1").x < was).to_be(true)
	expect(box(app, "#session-tab-a3").x > was).to_be(true)
	expect(app:error()).to_be_nil()
end)

-- Those chats split side by side, the focus on the first.
local function split(app: any, chats: { string })
	local children = {}
	for _, id in chats do
		table.insert(children, { kind = "leaf", id = id })
	end
	app:set_state("inset_split_trees", { { kind = "row", children = children } })
	app:frames(30)
end

local function letter(app: any, tab: string, at: number): string?
	local line = (app:tree(`#session-tab-{tab}-tile-{at}`) :: string):match("[^\n]+") or ""
	return line:match('"(.-)"')
end

test("a chat alone has one tab, replaced as you navigate, until chats are composed", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	open(app, "Fix the build")
	expect(tabs_shown(app).a1).to_be(true)
	open(app, "Write the docs")
	local now = tabs_shown(app)
	expect(now.a1).to_be_nil()
	expect(now.a2).to_be(true)
	-- Composed: its tab stays, and the next chat alone has one beside it.
	split(app, { "a1", "a2" })
	open(app, "Ship the release")
	now = tabs_shown(app)
	expect(now.a1).to_be(true)
	expect(now.b1).to_be(true)
	-- That one's replaced, where it stands; the composed one's left alone.
	local at = box(app, "#session-tab-b1").x
	open(app, "Tidy the tests")
	app:frames(30)
	now = tabs_shown(app)
	expect(now.a1).to_be(true)
	expect(now.b1).to_be_nil()
	expect(now.a3).to_be(true)
	expect(box(app, "#session-tab-a3").x).to_be(at)
	expect(app:error()).to_be_nil()
end)

test("a tab left with one chat is the tab for a chat alone, and ⌘W on it goes to a new chat", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	open(app, "Fix the build")
	split(app, { "a1", "a2" })
	app:command("shell.close")
	app:frames(40)
	expect(#(app:state("inset_split_trees") :: any)).to_be(1)
	expect(app:state("shell_route_path") ~= "/").to_be(true)
	-- With a split there to go to, still a new chat.
	app:set_state("inset_split_trees", {
		{ kind = "leaf", id = "a1", key = "solo" },
		{ kind = "row", key = "pair", children = { { kind = "leaf", id = "a3" }, { kind = "leaf", id = "b1" } } },
	})
	app:frames(30)
	open(app, "Fix the build")
	app:command("shell.close")
	app:frames(40)
	expect(app:state("shell_route_path")).to_be("/")
	local now = tabs_shown(app)
	expect(now.solo).to_be_nil()
	expect(now.pair).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("a tab's panes stack, three at most, the pane focused at the far left", function()
	local app = all_open()
	split(app, { "a1", "b1" })
	expect(app:wait("#session-tab-a1-tile-2", { timeout_ms = 2000 })).to_be(true)
	expect(app:wait("#session-tab-a1-tile-3", { timeout_ms = 200 })).to_be(false)

	split(app, { "a1", "b1", "a2", "a3" })
	expect(app:wait("#session-tab-a1-tile-3", { timeout_ms = 2000 })).to_be(true)
	expect(app:wait("#session-tab-a1-tile-4", { timeout_ms = 200 })).to_be(false)
	app:frames(30)
	local front, middle, back = box(app, "#session-tab-a1-tile-1"), box(app, "#session-tab-a1-tile-2"), box(app, "#session-tab-a1-tile-3")
	-- (The pane focused at the far left, the others behind it to the right.)
	expect(middle.x > front.x).to_be(true)
	expect(back.x > middle.x).to_be(true)
	expect(front.x < box(app, "#session-tab-a1").x + 8).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("the pane focused comes to the front of the stack, the one before it behind it", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	open(app, "Fix the build")
	open(app, "Ship the release")
	split(app, { "a1", "b1" })
	open(app, "Fix the build")
	app:frames(40)
	expect(letter(app, "a1", 1)).to_be("A")
	expect(letter(app, "a1", 2)).to_be("B")
	open(app, "Ship the release")
	app:frames(40)
	expect(letter(app, "a1", 1)).to_be("B")
	expect(letter(app, "a1", 2)).to_be("A")
	expect(app:error()).to_be_nil()
end)

local function route_has(app: any, id: string): boolean
	return tostring(app:state("shell_route_path")):find(id, 1, true) ~= nil
end

local function panes_of(app: any, key: string): { string }
	for _, t in app:state("inset_split_trees") :: any do
		if (t.key or "") == key or (not t.key and (t.id == key)) then
			local out: { string } = {}
			local function walk(n: any)
				if n.kind == "leaf" then
					table.insert(out, n.id)
				else
					for _, c in n.children do
						walk(c)
					end
				end
			end
			walk(t)
			return out
		end
	end
	return {}
end

-- A chat alone in a tab, and two in another.
local function two_tabs(app: any)
	app:set_state("inset_split_trees", {
		{ kind = "leaf", id = "a1", key = "solo" },
		{ kind = "row", key = "pair", children = { { kind = "leaf", id = "a2" }, { kind = "leaf", id = "a3" } } },
		{ kind = "leaf", id = "b1", key = "other" },
	})
	app:frames(30)
end

test("a chat kept in two tabs stays in the first", function()
	local app = all_open()
	app:set_state("inset_split_trees", {
		{ kind = "row", key = "first", children = { { kind = "leaf", id = "a1" }, { kind = "leaf", id = "a2" } } },
		{ kind = "row", key = "both", children = { { kind = "leaf", id = "a1" }, { kind = "leaf", id = "a3" }, { kind = "leaf", id = "b1" } } },
	})
	app:frames(30)
	expect(table.concat(panes_of(app, "first"), ",")).to_be("a1,a2")
	expect(table.concat(panes_of(app, "both"), ",")).to_be("a3,b1")
	expect(app:error()).to_be_nil()
end)

test("a closed split's tab stays closed: a chat of its opened again takes the tab for a chat alone", function()
	local app = all_open()
	app:set_state("inset_split_trees", {
		{ kind = "row", key = "both", children = { { kind = "leaf", id = "a1" }, { kind = "leaf", id = "b1" } } },
		{ kind = "leaf", id = "a2", key = "other" },
	})
	app:frames(30)
	open(app, "Write the docs")
	choose(app, "both", "Close Tab")
	expect(tabs_shown(app).both).to_be_nil()
	open(app, "Fix the build")
	app:frames(30)
	expect(tabs_shown(app).both).to_be_nil()
	local kept = app:state("inset_split_trees") :: any
	expect(#kept).to_be(1)
	expect(kept[1].kind).to_be("leaf")
	expect(kept[1].id).to_be("a1")
	-- Nor is the other chat kept anywhere.
	expect(app:has("#split-pane-b1")).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("tabs kept from before keep their order, a split where its first chat stood", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	app:set_state("inset_split_trees", { { kind = "row", children = { { kind = "leaf", id = "a2" }, { kind = "leaf", id = "a3" } } } })
	app:set_state("shell_session_tabs", { "a1", "a2", "b1", "a3" })
	app:frames(30)
	-- (Of the chats alone, one tab: the last.)
	local kept = app:state("inset_split_trees") :: any
	expect(#kept).to_be(2)
	expect(kept[1].kind).to_be("row")
	expect(kept[2].id).to_be("b1")
	expect(#(app:state("shell_session_tabs") :: any)).to_be(0)
	expect(app:error()).to_be_nil()
end)

local function has(app: any, target: string, ms: number?): boolean
	return app:wait(target, { timeout_ms = ms or 2000 })
end

test("a tab's corner says a chat awaits you over one at work, and its × comes under the pointer", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	open(app, "Fix the build")
	-- (Fix the build is at work; Ship the release awaits you; Write the docs, neither.)
	split(app, { "a1", "a2" })
	expect(has(app, "#session-tab-a1-state")).to_be(true)
	expect(app:tree("#session-tab-a1-state"):find("Working", 1, true) ~= nil).to_be(true)

	-- In a split with one awaiting you, that's what it says.
	split(app, { "a1", "b1" })
	expect(has(app, "#session-tab-a1-state")).to_be(true)
	local both = app:tree("#session-tab-a1-state") :: string
	expect(both:find("Working", 1, true)).to_be_nil()

	app:hover("#session-tab-a1")
	app:frames(10)
	expect(has(app, '#session-tab-a1 >> role:button "Close tab"')).to_be(true)
	expect(has(app, "#session-tab-a1-state", 200)).to_be(false)
	expect(app:error()).to_be_nil()
end)

test("a chat opened with no tab replaces the tab for a chat alone", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	open(app, "Fix the build")
	open(app, "Write the docs")
	open(app, "Tidy the tests")
	local kept = app:state("inset_split_trees") :: any
	expect(#kept).to_be(1)
	expect(kept[1].id).to_be("a3")
	expect(app:error()).to_be_nil()
end)

test("a chat opened from a split gets a new tab, the split left as it is", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	open(app, "Fix the build")
	app:set_state("inset_split_trees", { { kind = "row", key = "pair", children = { { kind = "leaf", id = "a1" }, { kind = "leaf", id = "a2" } } } })
	app:frames(30)
	open(app, "Ship the release")
	app:frames(30)
	expect(table.concat(panes_of(app, "pair"), ",")).to_be("a1,a2")
	local kept = app:state("inset_split_trees") :: any
	expect(#kept).to_be(2)
	expect(kept[2].id).to_be("b1")
	expect(app:error()).to_be_nil()
end)

test("a chat in another tab goes to that tab, replacing nothing", function()
	local app = all_open()
	open(app, "Fix the build")
	app:frames(30)
	expect(#(app:state("inset_split_trees") :: any)).to_be(5)
	expect(route_has(app, "a1")).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("from a new chat's page a chat opened replaces the tab for a chat alone too", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	open(app, "Fix the build")
	app:click('#shell-sidebar >> role:button "New chat"')
	app:frames(30)
	open(app, "Write the docs")
	app:frames(30)
	local kept = app:state("inset_split_trees") :: any
	expect(#kept).to_be(1)
	expect(kept[1].id).to_be("a2")
	expect(app:error()).to_be_nil()
end)

test("a split turned from a row to a column has a divider that goes up and down", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	open(app, "Fix the build")
	app:set_state("inset_split_trees", { { kind = "row", key = "p", children = { { kind = "leaf", id = "a1" }, { kind = "leaf", id = "a2" } } } })
	app:frames(30)
	expect(app:wait("#split-divider-row:/1", { timeout_ms = 2000 })).to_be(true)
	app:set_state("inset_split_trees", { { kind = "col", key = "p", children = { { kind = "leaf", id = "a1" }, { kind = "leaf", id = "a2" } } } })
	app:frames(30)
	expect(app:wait("#split-divider-col:/1", { timeout_ms = 2000 })).to_be(true)
	expect(app:wait("#split-divider-row:/1", { timeout_ms = 200 })).to_be(false)
	expect(app:error()).to_be_nil()
end)

-- The chosen tab's wash is one pill, gliding under whichever tab is chosen.
test("the chosen tab's wash glides to the tab chosen", function()
	local app = all_open()
	app:click("#session-tab-a2")
	app:frames(40)
	local pill, a2 = box(app, "#session-tab-chosen"), box(app, "#session-tab-a2")
	expect(math.abs(pill.x - a2.x) <= 0.5).to_be(true)
	expect(math.abs(pill.w - a2.w) <= 0.5).to_be(true)
	app:click("#session-tab-b1")
	app:frames(2)
	-- (On its way: past where it was, short of where it goes.)
	local b1 = box(app, "#session-tab-b1")
	local mid = box(app, "#session-tab-chosen")
	expect(mid.x > a2.x and mid.x < b1.x).to_be(true)
	app:frames(40)
	expect(math.abs(box(app, "#session-tab-chosen").x - b1.x) <= 0.5).to_be(true)
	expect(app:error()).to_be_nil()
end)

-- The tab for a chat alone, replaced as you navigate by one just as wide (both
-- titles as long as a tab's may show), still stands on the chosen tab's wash.
test("a chat alone's tab replaced by one as wide still has the chosen tab's wash", function()
	local app = mount("shell_fixture.luau")
	app:resize(1400, 800)
	app:frames(30)
	app:command("long_titles")
	app:frames(10)
	open(app, "Desktop app performance investigation, first pass")
	expect(app:wait("#session-tab-chosen", { timeout_ms = 2000 })).to_be(true)
	local first = box(app, "#session-tab-a1")
	open(app, "Desktop app performance investigation, second pass")
	app:frames(30)
	local now = tabs_shown(app)
	expect(now.a1).to_be_nil()
	expect(now.a2).to_be(true)
	local tab = box(app, "#session-tab-a2")
	expect(tab.w).to_be(first.w)
	expect(app:wait("#session-tab-chosen", { timeout_ms = 2000 })).to_be(true)
	local pill = box(app, "#session-tab-chosen")
	expect(math.abs(pill.x - tab.x) <= 0.5).to_be(true)
	expect(math.abs(pill.w - tab.w) <= 0.5).to_be(true)
	expect(app:error()).to_be_nil()
end)

-- The arrows go from tab to tab, choosing each, the keyboard going with them;
-- Home and End go to the ends.
test("the arrows go from tab to tab, choosing each", function()
	local app = all_open()
	-- (Arriving on a1 gives its composer the keys; a press on the tab
	-- already chosen gives them to the tab.)
	app:click("#session-tab-a1")
	app:frames(20)
	app:click("#session-tab-a1")
	app:frames(20)
	app:key("ArrowRight")
	app:frames(20)
	expect(tostring(app:state("shell_route_path")):find("a2", 1, true) ~= nil).to_be(true)
	expect(app:tree("#session-tab-a2")).to_contain("focused")
	app:key("End")
	app:frames(20)
	expect(tostring(app:state("shell_route_path")):find("b2", 1, true) ~= nil).to_be(true)
	app:key("ArrowLeft")
	app:frames(20)
	expect(app:tree("#session-tab-b1")).to_contain("focused")
	expect(app:error()).to_be_nil()
end)
