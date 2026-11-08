-- The model picker's accounts, as Zeron's composer has them: an Account row and
-- a usage ring on the chip, for the composer's device (this one or another),
-- reading the fuller window (Cursor's, the pool of the model chosen); no usage
-- reported, no ring; one that can't be switched to, not offered.

local function window(label: string, used: number): any
	return { label = label, used = used }
end

local function accounts(): { any }
	return {
		{ id = "a1", email = "wing@example.test", plan = "Max", active = true, can_switch = true, windows = { window("5h", 0.34), window("Week", 0.12) } },
		{ id = "a2", email = "work@example.test", plan = "Pro", active = false, can_switch = true, windows = { window("5h", 0.05), window("Week", 0.91) } },
	}
end

local function with_accounts(on_device: { [string]: any }): any
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:frames(30)
	app:set_state("private_text_hidden", false)
	app:set_state("agents.on_device", on_device)
	app:frames(10)
	return app
end

local function here(list: { any }): { [string]: any }
	return { [""] = { ["claude-code"] = list } }
end

test("an agent with accounts has the row, the address and how much is used", function()
	local app = with_accounts(here(accounts()))
	expect(app:has("#model-picker-usage")).to_be(true)
	app:click('role:button "Model"')
	app:frames(30)
	local row = app:tree("#model-picker-account")
	expect(row).to_contain("wing@example.test")
	expect(row).to_contain("34%")
end)

test("Hide email addresses hides them in the row and the list, and a press still chooses", function()
	local app = with_accounts(here(accounts()))
	app:set_state("private_text_hidden", true)
	app:click('role:button "Model"')
	app:frames(30)
	app:click("#model-picker-account")
	app:frames(30)
	local all = app:tree()
	expect(string.find(all, "wing@example.test", 1, true)).to_be(nil)
	expect(string.find(all, "work@example.test", 1, true)).to_be(nil)
	-- (Its usage still says, and no switch to show it takes the press.)
	expect(app:has('~"Pro · 5h 5% · Week 91%"')).to_be(true)
	expect(app:has('role:switch "Show email"')).to_be(false)
	app:click('~"Pro · 5h 5% · Week 91%"')
	app:frames(30)
	expect(app:state("calls")).to_contain("switch_on::claude-code:a2")
end)

test("the fullest window is the figure shown, whichever and however many", function()
	local list = accounts()
	list[1].windows = { window("5h", 0.2), window("Opus", 0.8), window("Week", 0.3) }
	local app = with_accounts(here(list))
	app:click('role:button "Model"')
	app:frames(30)
	expect(app:tree("#model-picker-account")).to_contain("80%")
end)

test("a chat on another computer reads that computer's accounts", function()
	local app = with_accounts({
		[""] = { ["claude-code"] = accounts() },
		box = {
			["claude-code"] = {
				{ id = "b1", email = "box@example.test", plan = "Max", active = true, can_switch = true, windows = { window("Week", 0.77) } },
				{ id = "b2", email = "spare@example.test", plan = "Pro", active = false, can_switch = true, windows = {} },
			},
		},
	})
	app:set_state("chat_device", "box")
	app:frames(10)
	expect(app:has("#model-picker-usage")).to_be(true)
	app:click('role:button "Model"')
	app:frames(30)
	local row = app:tree("#model-picker-account")
	expect(row).to_contain("box@example.test")
	expect(row).to_contain("77%")
	expect(app:state("calls")).to_contain("watch_device:box")
	app:click("#model-picker-account")
	app:frames(30)
	app:click('~"spare@example.test"')
	app:frames(30)
	expect(app:state("calls")).to_contain("switch_on:box:claude-code:b2")
end)

test("a computer whose accounts haven't come has neither, and is asked", function()
	local app = with_accounts(here(accounts()))
	app:set_state("chat_device", "somewhere-else")
	app:frames(10)
	expect(app:has("#model-picker-usage")).to_be(false)
	expect(app:state("calls")).to_contain("watch_device:somewhere-else")
end)

test("opening the panel asks after the usage again", function()
	local app = with_accounts(here(accounts()))
	app:click('role:button "Model"')
	app:frames(30)
	expect(app:state("calls")).to_contain("refresh_device:")
end)

test("no usage reported: the account shows, no percentage and no ring", function()
	local list = accounts()
	list[1].windows = {}
	local app = with_accounts(here(list))
	expect(app:has("#model-picker-usage")).to_be(false)
	app:click('role:button "Model"')
	app:frames(30)
	local row = app:tree("#model-picker-account")
	expect(row).to_contain("wing@example.test")
	expect(string.find(row, "%", 1, true)).to_be(nil)
end)

test("one not in use says what's wrong with it; one that can't be switched to isn't offered", function()
	local list = accounts()
	list[2].error = "Signed out — sign in again"
	list[2].can_switch = false
	local app = with_accounts(here(list))
	app:click('role:button "Model"')
	app:frames(30)
	app:click("#model-picker-account")
	app:frames(30)
	expect(app:has('~"Pro · Signed out — sign in again"')).to_be(true)
	app:click('~"work@example.test"')
	app:frames(30)
	expect(string.find(app:state("calls"), "switch_on", 1, true)).to_be(nil)
end)

-- How many words the chip's model is drawn as: one at rest, one a letter
-- while it rolls.
local function letters(app: any): number
	local _, n = string.gsub(app:tree('role:button "Model"'), "\n", "")
	return n
end

test("the chip's model doesn't roll for a change from elsewhere, and does for a pick", function()
	local app = with_accounts({})
	app:click('role:button "Model"')
	app:frames(30)
	app:key("Escape")
	app:frames(60)
	local at_rest = letters(app)
	-- (As moving to a chat on another model, or refocusing a pane, does.)
	app:set_state("chat_model", "sonnet")
	app:frames(3)
	expect(letters(app)).to_be(at_rest)
	expect(app:tree('role:button "Model"')).to_contain("Sonnet")
	app:frames(60)
	-- Picked in its panel: it rolls.
	app:click('role:button "Model"')
	app:frames(30)
	app:set_state("chat_model", "opus")
	app:frames(3)
	expect(letters(app) > at_rest).to_be(true)
end)

test("an agent with no accounts has neither", function()
	local app = with_accounts(here({}))
	expect(app:has("#model-picker-usage")).to_be(false)
	app:click('role:button "Model"')
	app:frames(30)
	expect(app:has("#model-picker-account")).to_be(false)
end)
