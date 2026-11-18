-- An orchestrator's crew (ds/orch_crew, ui/orch_crew_fixture.luau): a card
-- a thread, the one that needs you first; each says what it's doing, how
-- far through its todo and what it last reported; a card opens its thread;
-- a thread's activity moving changes its words in place.

local CREW = "#orch-crew-fixture"

-- A node's identity in the tree, by a pattern on its line: `@id`.
local function node(app: any, pattern: string): string?
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(pattern, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

-- The cards' titles, top to bottom then left to right, as the tree lists them.
local function titles(app: any): { string }
	local out = {}
	for line in app:tree(CREW):gmatch("[^\n]+") do
		local t = line:match('button "Open ([^"]+)"')
		if t then
			table.insert(out, t)
		end
	end
	return out
end

test("orch crew: a card a thread, the one that needs you first", function()
	local app = mount("orch_crew_fixture.luau")
	app:resize(460, 900)
	app:frames(10)
	expect(app:count(`{CREW} >> .orch-crew-card`)).to_be(4)
	local order = titles(app)
	expect(order[1]).to_be("Orchestrator screen")
	-- Then the three working, the latest moved first.
	expect(order[2]).to_be("Engine: reports + MCP tools")
	expect(order[3]).to_be("Engine: orchestration state")
	expect(order[4]).to_be("Control room UI")
	expect(app:error()).to_be(nil)
end)

test("orch crew: what each is doing, how far, what it last said", function()
	local app = mount("orch_crew_fixture.luau")
	app:resize(460, 900)
	app:frames(10)
	expect(app:has(`{CREW} >> ~"cargo test -p zeron-engine --lib orchestration"`)).to_be(true)
	expect(app:has(`{CREW} >> ~"5/8"`)).to_be(true)
	expect(app:has(`{CREW} >> ~"WatchOrchestration stream"`)).to_be(true)
	expect(app:has(`{CREW} >> ~"Needs a decision: tabs or split"`)).to_be(true)
	expect(app:has(`{CREW} >> .orch-crew-report-state >> ~"Blocked"`)).to_be(true)
	-- Its role, machine and model.
	expect(app:has(`{CREW} >> ~"engine state · desk · opus 5.5"`)).to_be(true)
	-- Waiting on you, it says nothing of an activity.
	expect(app:count(`{CREW} >> .orch-crew-activity-label`)).to_be(3)
end)

test("orch crew: a card opens its thread", function()
	local app = mount("orch_crew_fixture.luau")
	app:resize(460, 900)
	app:frames(10)
	app:click(`{CREW} >> ~"Control room UI"`)
	app:frames(2)
	expect(app:state("opened")).to_be("th-ui-control")
	app:click(`{CREW} >> ~"Needs a decision"`)
	app:frames(2)
	expect(app:state("opened")).to_be("th-ui-orch")
end)

test("orch crew: an activity moving changes its words in place", function()
	local app = mount("orch_crew_fixture.luau")
	app:resize(460, 900)
	app:frames(10)
	local card = node(app, 'button "Open Engine: orchestration state"')
	local title = node(app, 'text "Engine: orchestration state"')
	expect(card).never.to_be(nil)
	local members = app:state("crew_members") :: { any }
	local changed = {}
	for i, m in members do
		if m.member.chat_id == "th-engine-state" then
			local chat = table.clone(m.chat)
			chat.activity = { kind = "pushing", label = "git push -u origin HEAD", since = chat.activity.since }
			chat.todo = { done = 6, total = 8, current = "Ops" }
			changed[i] = { member = m.member, chat = chat }
		else
			changed[i] = m
		end
	end
	app:set_state("crew_members", changed)
	-- (A status or an activity shows 300 ms at least: ds/orch_hold.)
	app:wait(`{CREW} >> ~"git push -u origin HEAD"`)
	expect(app:has(`{CREW} >> ~"git push -u origin HEAD"`)).to_be(true)
	expect(app:has(`{CREW} >> ~"cargo test -p zeron-engine"`)).to_be(false)
	expect(app:has(`{CREW} >> ~"6/8"`)).to_be(true)
	expect(`card @{node(app, 'button "Open Engine: orchestration state"')}`).to_be(`card @{card}`)
	expect(`title @{node(app, 'text "Engine: orchestration state"')}`).to_be(`title @{title}`)
end)

test("orch crew: one column narrow, two wide, drawn and resized", function()
	local app = mount("orch_crew_fixture.luau")
	for _, size in { { 460, 900 }, { 1100, 800 }, { 420, 700 }, { 1100, 800 } } do
		app:resize(size[1], size[2])
		app:frames(10)
		expect(app:count(`{CREW} >> .orch-crew-card`)).to_be(4)
		expect(app:error()).to_be(nil)
	end
end)

test("orch crew: none yet, a quiet line", function()
	local app = mount("orch_crew_fixture.luau")
	app:resize(460, 900)
	app:frames(5)
	app:set_state("crew_members", {})
	app:frames(5)
	expect(app:count(`{CREW} >> .orch-crew-card`)).to_be(0)
	expect(app:has(`{CREW} >> ~"No crew yet"`)).to_be(true)
end)
