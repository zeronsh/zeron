-- A chat that's a thread of a paused card (screens/chat/card_hold, on
-- ui/chat_fixture.luau): its queue says it's held, with Resume (the card's
-- `resumeItem`, into Now); said with nothing queued too, and gone once the
-- card is resumed.

local function open(app: any, id: string)
	app:command(`go_{id}`)
	app:frames(30)
end

local function showing(app: any, sel: string): boolean
	if not app:has(sel) then
		return false
	end
	local first = string.match(app:tree(sel), "[^\n]+") or ""
	return string.find(first, " hidden", 1, true) == nil
end

local function open_held(app: any)
	if app:has('role:button "Open Held"') then
		app:click('role:button "Open Held"')
	else
		app:click('role:tab "Open Held"')
	end
	app:wait("#never-shown", { timeout_ms = 320 })
	app:frames(10)
end

local LINE = '.composer-held >> ~"Held while its card is paused"'

test("card hold: a paused card's thread says its queue is held; Resume resumes the card", function()
	local app = mount("chat_fixture.luau")
	open(app, "s2")
	app:command("queue_one")
	app:frames(10)
	-- (Not paused: its queue as ever.)
	expect(showing(app, LINE)).to_be(false)
	app:command("card_paused")
	app:frames(20)
	expect(app:has('role:button "Open Held"')).to_be(true)
	open_held(app)
	expect(showing(app, LINE)).to_be(true)
	expect(showing(app, 'role:listitem ~"Queued words"')).to_be(true)
	app:click("#composer-held-action")
	app:frames(20)
	expect(app:state("calls")).to_contain("orch:resumeItem:it-held:")
	-- Resumed: the line goes, the queue stays.
	expect(showing(app, LINE)).to_be(false)
	expect(app:has('role:button "Open Held"') or app:has('role:tab "Open Held"')).to_be(false)
end)

test("card hold: with nothing queued, a paused card's thread still says it's held", function()
	local app = mount("chat_fixture.luau")
	open(app, "s2")
	app:command("card_paused")
	app:frames(20)
	open_held(app)
	expect(showing(app, LINE)).to_be(true)
	app:click("#composer-held-action")
	app:frames(20)
	expect(app:state("calls")).to_contain("orch:resumeItem:it-held:")
	expect(showing(app, LINE)).to_be(false)
end)

test("card hold: another chat's card paused says nothing here", function()
	local app = mount("chat_fixture.luau")
	open(app, "s1")
	app:command("card_paused")
	app:frames(10)
	open(app, "s2")
	app:command("queue_one")
	app:frames(20)
	expect(app:has('role:button "Open Held"')).to_be(false)
	expect(showing(app, LINE)).to_be(false)
end)

test("card hold: a Resume refused keeps the line and says it didn't resume", function()
	local app = mount("chat_fixture.luau")
	open(app, "s2")
	app:command("card_paused")
	app:set_state("orch_refuse", true)
	app:frames(20)
	open_held(app)
	app:click("#composer-held-action")
	app:frames(20)
	expect(app:state("calls")).to_contain("orch:resumeItem:it-held:")
	-- (Said in the deck's alerts.)
	expect(app:has('role:tab "Open Alerts"')).to_be(true)
	app:click('role:tab "Open Alerts"')
	app:wait("#never-shown", { timeout_ms = 320 })
	app:frames(10)
	expect(app:has(`~"Couldn't resume its card"`)).to_be(true)
	-- (Still held: its tab says so.)
	expect(app:has('role:tab "Open Held"')).to_be(true)
end)
