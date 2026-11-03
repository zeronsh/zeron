-- A session's lanes (screens/chat/lanes, ds/session_lanes) and their panel
-- tabs (screens/chat/lane_tab): the side chats newest first, the subagents
-- from the transcript's spawn chips (running first, the finished folded),
-- each opening a tab of its own (a side chat with a composer, a subagent to
-- read), and the shipping app's New side chat and Fork chat. On the page in
-- ui/lanes_fixture.luau. (Its panel's home lists the same lanes under the
-- same names, tests/panel_home.test.luau: the card's are named by
-- `#session-lanes`.)

local function open(): any
	local app = mount("lanes_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	return app
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

-- A node's top in the window, by its kind and label.
local function top(app: any, what: string): number?
	for line in app:tree("#session-lanes"):gmatch("[^\n]+") do
		if line:find(what, 1, true) then
			return tonumber(line:match(" %-?%d+,(%-?%d+) "))
		end
	end
	return nil
end

test("side chats list newest first, subagents running first with the finished folded", function()
	local app = open()
	expect(app:has('#session-lanes >> "Side chats"')).to_be(true)
	local newer, older = top(app, 'button "Open side chat Looking at the tests"'), top(app, 'button "Open side chat New side chat"')
	expect(newer ~= nil and older ~= nil and newer < older).to_be(true)
	-- With side chats, New and Fork are the heading's buttons.
	expect(app:has('#session-lanes >> role:button "Fork chat"')).to_be(true)
	expect(app:has('#session-lanes >> "Subagents"')).to_be(true)
	expect(app:has('#session-lanes >> role:button \"Open subagent scan the repo"')).to_be(true)
	-- The finished one, folded.
	expect(app:has('#session-lanes >> role:button \"Open subagent read the docs"')).to_be(false)
	expect(app:has('role:button "Expand 1 finished subagents"')).to_be(true)
	app:click('role:button "Expand 1 finished subagents"')
	app:frames(20)
	expect(app:has('#session-lanes >> role:button \"Open subagent read the docs"')).to_be(true)
	expect(app:text("#subagent-count")).to_be("2")
end)

test("a subagent finishing folds into the finished, its row kept until then", function()
	local app = open()
	app:command("finish")
	app:frames(20)
	expect(app:has('#session-lanes >> role:button \"Open subagent scan the repo"')).to_be(false)
	expect(app:has('role:button "Expand 2 finished subagents"')).to_be(true)
end)

test("a side chat opens as a tab with its transcript and a composer of its own", function()
	local app = open()
	app:click('#session-lanes >> role:button "Open side chat Looking at the tests"')
	app:frames(30)
	expect(calls(app)).to_contain("watch:side-a")
	expect(app:tree()).to_contain("This is side-a")
	expect(app:has(".side-chat-tab")).to_be(true)
	app:type('.side-chat-tab >> role:textbox', "hello there")
	app:key("Enter")
	app:frames(10)
	expect(calls(app)).to_contain("send:side-a:hello there")
	-- Opened again: the same tab.
	app:click('#session-lanes >> role:button "Open side chat Looking at the tests"')
	app:frames(20)
	local n = 0
	for _ in calls(app):gmatch("watch:side%-a") do
		n += 1
	end
	expect(n).to_be(1)
end)

test("a subagent opens as a tab to read, with no composer", function()
	local app = open()
	app:click('#session-lanes >> role:button \"Open subagent scan the repo"')
	app:frames(30)
	expect(calls(app)).to_contain("watch:chat-1--sub--1")
	expect(app:tree()).to_contain("This is chat-1--sub--1")
	expect(app:has(".subagent-tab")).to_be(true)
	expect(app:has(".subagent-tab >> role:textbox")).to_be(false)
end)

test("New side chat opens at once and is made by its first message, not before", function()
	local app = open()
	app:click('#session-lanes >> role:button "New side chat"')
	app:frames(30)
	expect(app:has(".side-chat-tab")).to_be(true)
	-- Nothing written, nothing watched yet.
	expect(calls(app)).never.to_contain("create:")
	expect(calls(app)).never.to_contain("watch:")
	app:type('.side-chat-tab >> role:textbox', "first words")
	app:key("Enter")
	app:frames(30)
	local made = calls(app):match("create:chat%-1:([%w%-]+)")
	expect(made).never.to_be_nil()
	expect(calls(app)).to_contain(`send:{made}:first words:fresh`)
	expect(calls(app)).to_contain(`watch:{made}`)
end)

test("Fork chat opens a Forking… tab at once, then its chat", function()
	local app = open()
	app:command("fork_hold")
	app:click('#session-lanes >> role:button "Fork chat"')
	app:frames(30)
	expect(app:tree()).to_contain("Forking…")
	local id = calls(app):match("fork:chat%-1:([%w%-]+)")
	expect(id).never.to_be_nil()
	expect(calls(app)).never.to_contain("watch:")
	app:command("fork_settle")
	app:frames(30)
	expect(app:tree()).never.to_contain("Forking…")
	expect(calls(app)).to_contain(`watch:{id}`)
end)

test("a fork that fails says why in its tab, and Retry is the same fork again", function()
	local app = open()
	app:command("fork_fails")
	app:click('#session-lanes >> role:button "Fork chat"')
	app:frames(30)
	expect(app:tree()).to_contain("Couldn't fork this chat")
	expect(app:tree()).to_contain("Wait for a completed response before starting a side chat")
	local id = calls(app):match("fork:chat%-1:([%w%-]+)")
	app:command("fork_ok")
	app:click('role:button "Retry"')
	app:frames(30)
	local n = 0
	for _ in calls(app):gmatch(`fork:chat%-1:{id:gsub("%-", "%%-")}`) do
		n += 1
	end
	expect(n).to_be(2)
	expect(calls(app)).to_contain(`watch:{id}`)
end)

test("a finished subagent is read from its upload; a running one watched live", function()
	local app = open()
	app:click('#session-lanes >> role:button "Open subagent scan the repo"')
	app:frames(30)
	expect(calls(app)).to_contain("watch:chat-1--sub--1")
	expect(calls(app)).never.to_contain("watch:chat-1--sub--1:frozen")
	app:click('role:button "Expand 1 finished subagents"')
	app:frames(20)
	app:click('#session-lanes >> role:button "Open subagent read the docs"')
	app:frames(30)
	expect(calls(app)).to_contain("watch:chat-1--sub--2:frozen")
end)

test("a spawn chip in the transcript opens its subagent's tab", function()
	local app = open()
	expect(app:has('#fixture-chips >> role:button "Open subagent scan the repo"')).to_be(true)
	app:click('#fixture-chips >> role:button "Open subagent scan the repo"')
	app:frames(30)
	expect(calls(app)).to_contain("watch:chat-1--sub--1")
	expect(app:has(".subagent-tab")).to_be(true)
	-- Any other tool row opens as before (no lane).
	expect(app:has('#fixture-chips >> role:button "Open subagent a.rs"')).to_be(false)
end)

test("a long list shows 8, then the rest a page at a time", function()
	local app = open()
	app:command("many")
	app:frames(20)
	-- 12 side chats: 8 shown.
	expect(app:has('#session-lanes >> role:button "Open side chat Side 7"')).to_be(false)
	expect(app:has('#session-lanes >> role:button "Show 4 more"')).to_be(true)
	app:click('#session-lanes >> role:button "Show 4 more"')
	app:frames(20)
	expect(app:has('#session-lanes >> role:button "Open side chat Side 10"')).to_be(true)
	expect(app:has('#session-lanes >> role:button "Show 4 more"')).to_be(false)
end)

-- Where a node's top is in the window.
local function y_of(app: any, target: string): number?
	for line in app:tree(target):gmatch("[^\n]+") do
		local y = line:match(" %-?%d+,(%-?%d+) %d+x%d+")
		if y then
			return tonumber(y)
		end
	end
	return nil
end

test("a new side chat starts on its parent's agent and model, and its first message goes with them", function()
	local app = open()
	app:click('#session-lanes >> role:button "New side chat"')
	app:frames(30)
	expect(app:has('.side-chat-tab >> role:button "Model" >> "Opus 5.5"')).to_be(true)
	app:type('.side-chat-tab >> role:textbox', "first words")
	app:key("Enter")
	app:frames(30)
	local made = calls(app):match("create:chat%-1:([%w%-]+)")
	expect(made).never.to_be_nil()
	expect(calls(app)).to_contain(`send:{made}:first words:fresh`)
	expect(calls(app)).to_contain(`pick:{made}:claude-code/opus-5.5:claude-code:high`)
	expect(app:has('.side-chat-tab >> "Your message didn\'t send"')).to_be(false)
end)

test("a side chat's own model shows in its picker; a changed pick goes with its next message", function()
	local app = open()
	app:click('#session-lanes >> role:button "Open side chat Looking at the tests"')
	app:frames(30)
	-- (Its own: Codex's GPT-6.1, never its parent's.)
	expect(app:has('.side-chat-tab >> role:button "Model" >> "GPT-6.1"')).to_be(true)
	app:click('.side-chat-tab >> role:button "Model"')
	app:frames(30)
	-- (It lists the side chat's agent's models.)
	expect(app:has('role:option "GPT-6.1 mini"')).to_be(true)
	app:key("Escape")
	app:frames(10)
	-- Another picked (the picker's pick, kept by the chat).
	app:set_state("chat_lane_model_side-a", "codex/gpt-6.1-mini")
	app:frames(20)
	-- (Its label rolls in a letter at a time: read whole.)
	local label = {}
	for t in app:tree('.side-chat-tab >> role:button "Model"'):gmatch('text "([^"]*)"') do
		table.insert(label, t)
	end
	expect(table.concat(label)).to_be("GPT-6.1 mini")
	app:type('.side-chat-tab >> role:textbox', "again")
	app:key("Enter")
	app:frames(20)
	expect(calls(app)).to_contain("pick:side-a:codex/gpt-6.1-mini:codex:medium")
end)

test("a message that didn't go says so above the side chat's composer, never over it, and keeps its words", function()
	local app = open()
	app:command("send_fails")
	app:click('#session-lanes >> role:button "Open side chat Looking at the tests"')
	app:frames(30)
	app:type('.side-chat-tab >> role:textbox', "lost words")
	app:key("Enter")
	app:wait('.side-chat-tab >> "Your message didn\'t send"', { timeout_ms = 3000 })
	app:frames(10)
	expect(app:has('.side-chat-tab >> "Your message didn\'t send"')).to_be(true)
	-- Its line above the composer's box, not a toast over it.
	expect(calls(app)).never.to_contain("toast:")
	local said = y_of(app, '.side-chat-tab >> "Your message didn\'t send"') :: number
	local box = y_of(app, '.side-chat-tab >> role:textbox') :: number
	expect(said < box - 20).to_be(true)
	expect(app:text('.side-chat-tab >> role:textbox')).to_contain("lost words")
end)

-- The side chat composer's footer, read whole (labels roll in a letter at
-- a time).
local function footer_words(app: any): string
	local out = {}
	for t in app:tree(".side-chat-tab"):gmatch('text "([^"]*)"') do
		table.insert(out, t)
	end
	return table.concat(out)
end

test("a new side chat's footer is the chat page's: access, where it works, the model, all as its parent", function()
	local app = open()
	app:click('#session-lanes >> role:button "New side chat"')
	app:frames(40)
	-- The parent's access, and where it works as a picker (it isn't made yet).
	expect(app:has('.side-chat-tab >> role:button "Access"')).to_be(true)
	expect(footer_words(app)).to_contain("Supervised")
	expect(app:has('.side-chat-tab >> role:button "Branch"')).to_be(true)
	expect(footer_words(app)).to_contain("atlas")
	expect(footer_words(app)).to_contain("main")
	-- In the chat page's order: access, place, then the model.
	local function x(target: string): number
		return tonumber((app:tree(target):match(" (%-?%d+),%-?%d+ %d+x%d+"))) or -1
	end
	local a, b, m = x('.side-chat-tab >> role:button "Access"'), x('.side-chat-tab >> role:button "Branch"'), x('.side-chat-tab >> role:button "Model"')
	expect(a < b and b < m).to_be(true)
	-- Sent as it stands: made where its parent is (no place of its own), with
	-- the parent's access.
	app:type('.side-chat-tab >> role:textbox', "hi")
	app:key("Enter")
	app:frames(30)
	local made = calls(app):match("create:chat%-1:([%w%-]+)") :: string
	expect(calls(app)).never.to_contain(`place:{made}`)
	expect(calls(app)).to_contain(`pick:{made}:claude-code/opus-5.5:claude-code:high:supervised`)
end)

test("a side chat's access and place picks stick per chat and go with its first message; then it's fixed", function()
	local app = open()
	app:click('#session-lanes >> role:button "New side chat"')
	app:frames(40)
	local chat = ""
	for key, v in ((app:state("chat_lanes_unsent") :: any) or {}) do
		if v == "chat-1" then
			chat = key
		end
	end
	expect(chat).never.to_be("")
	-- Picked: auto-accept edits, the other project, a new worktree off feature.
	app:set_state(`chat_lane_access_{chat}`, "edits")
	app:set_state(`chat_lane_project_{chat}`, "p2")
	app:frames(20)
	app:set_state(`chat_lane_checkout_{chat}`, "worktree")
	app:set_state(`chat_lane_branch_{chat}`, "feature")
	app:frames(20)
	expect(footer_words(app)).to_contain("Auto-accept edits")
	expect(footer_words(app)).to_contain("zephyr")
	-- Kept by the chat: its tab closed to the panel's home and back, the same.
	app:command("panel_home")
	app:frames(10)
	app:command("panel_back")
	app:frames(30)
	expect(footer_words(app)).to_contain("Auto-accept edits")
	expect(footer_words(app)).to_contain("zephyr")
	app:type('.side-chat-tab >> role:textbox', "over there")
	app:key("Enter")
	app:frames(30)
	expect(calls(app)).to_contain(`place:{chat}:p2:d:feature:/zephyr:`)
	expect(calls(app)).to_contain(`pick:{chat}:claude-code/opus-5.5:claude-code:high:edits:worktree`)
	-- Made: where it works is said, no longer picked.
	expect(app:has('.side-chat-tab >> role:button "Branch"')).to_be(false)
	expect(app:has('.side-chat-tab >> "Branch feature"')).to_be(true)
end)

test("a side chat that's made shows where it works, fixed, and its own access", function()
	local app = open()
	app:click('#session-lanes >> role:button "Open side chat Looking at the tests"')
	app:frames(30)
	expect(app:has('.side-chat-tab >> role:button "Branch"')).to_be(false)
	expect(app:has('.side-chat-tab >> "Branch feature"')).to_be(true)
	expect(footer_words(app)).to_contain("Auto-accept edits")
	app:type('.side-chat-tab >> role:textbox', "next")
	app:key("Enter")
	app:frames(20)
	expect(calls(app)).to_contain("pick:side-a:codex/gpt-6.1:codex:medium:edits")
end)

test("a side chat's composer has the chat page's context ring, from its own numbers", function()
	local app = open()
	-- New, it has run nothing: no ring.
	app:click('#session-lanes >> role:button "New side chat"')
	app:frames(30)
	expect(app:has('.side-chat-tab >> #composer-context')).to_be(false)
	-- One that has run: its ring ("Context used"), at the footer's end
	-- after the model, as the chat page's.
	app:click('#session-lanes >> role:button "Open side chat Looking at the tests"')
	app:frames(30)
	expect(app:has('.side-chat-tab >> #composer-context')).to_be(true)
	expect(app:has('.side-chat-tab >> "Context used"')).to_be(true)
	local function x(target: string): number
		return tonumber((app:tree(target):match(" (%-?%d+),%-?%d+ %d+x%d+"))) or -1
	end
	expect(x('.side-chat-tab >> #composer-context') > x('.side-chat-tab >> role:button "Model"')).to_be(true)
end)

test("a side chat's tab has the sidebar's menu for its chat: rename, copy, archive, delete; no pin or move", function()
	local app = open()
	app:click('#session-lanes >> role:button "Open side chat Looking at the tests"')
	app:frames(30)
	app:set_state("test_menu_of", "side-a")
	app:command("tab_menu")
	local labels = tostring(app:state("test_menu_labels"))
	for _, item in { "Rename", "Copy", "Archive", "Delete…" } do
		expect(labels).to_contain(item)
	end
	expect(labels).never.to_contain("Pin")
	expect(labels).never.to_contain("Move to")
	app:set_state("test_menu_pick", "Archive")
	app:command("tab_menu_pick")
	app:frames(20)
	expect(calls(app)).to_contain("archive:side-a")
end)

test("a new side chat's tab, not made yet, has only the tab's own menu", function()
	local app = open()
	app:click('#session-lanes >> role:button "New side chat"')
	app:frames(30)
	local made = ""
	for key in ((app:state("chat_lanes_unsent") :: any) or {}) do
		made = key
	end
	app:set_state("test_menu_of", made)
	app:command("tab_menu")
	expect(tostring(app:state("test_menu_labels"))).to_be("")
end)
