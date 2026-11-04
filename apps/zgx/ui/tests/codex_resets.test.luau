-- Codex's banked resets on Settings → Providers, as the gallery draws them
-- (demos/example_resets): the count on the row after the account, "Use a
-- reset" there once a window is at its limit, the dialog that spends one
-- (the soonest first), and every word Codex can send back, on that row:
-- the limits started over, still banked, gone, or why not, with another
-- go. Only Codex accounts bank anything; none banked, no row. The page
-- changes in place throughout. On ui/resets_fixture.luau, account/agents
-- answered as the host (src/agents.rs) would.

local PAGE = "#page"
local DIALOG = 'role:dialog "Use a Codex reset?"'

local function settle(app: any)
	app:frames(30)
end

local function open(): any
	local app = mount("resets_fixture.luau")
	app:resize(900, 760)
	settle(app)
	return app
end

-- Whether `text` shows anywhere under `scope`.
local function shows(app: any, scope: string, text: string): boolean
	return app:tree(scope):find(text, 1, true) ~= nil
end

-- A node's identity under `scope`, by its kind and label or text: `@id`.
local function node(app: any, scope: string, kind: string, label: string): string?
	for line in app:tree(scope):gmatch("[^\n]+") do
		if line:find(` {kind} "{label}"`, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

-- Whether the dialog is open (a closed one stays in the tree, hidden, as
-- it leaves: a test host's frames pass no time).
local function dialog_open(app: any): boolean
	local line = app:tree():match('[^\n]*dialog "Use a Codex reset%?"[^\n]*')
	return line ~= nil and line:find(" hidden", 1, true) == nil
end

local function calls(app: any): string
	return (app:state("resets_calls") :: any) or ""
end

-- The radios in the dialog, in order, with whether each is chosen and
-- focused.
local function radios(app: any): { { label: string, checked: boolean, focused: boolean } }
	local out = {}
	for line in app:tree(DIALOG):gmatch("[^\n]+") do
		local label = line:match(' radio "([^"]+)"')
		if label then
			table.insert(out, { label = label, checked = line:find(" checked", 1, true) ~= nil, focused = line:find(" focused", 1, true) ~= nil })
		end
	end
	return out
end

-- This computer's clock time for `at` ("3:45 PM"), as ds/locale says it.
local function clock(at: number): string
	local t = os.date("*t", at) :: any
	local h = t.hour % 12
	return string.format("%d:%02d %s", if h == 0 then 12 else h, t.min, if t.hour < 12 then "AM" else "PM")
end

local function use_reset(app: any)
	app:click(`{PAGE} >> role:button "Use a reset"`)
	settle(app)
	expect(dialog_open(app)).to_be(true)
end

test("Codex resets: a Codex account at its limit shows its banked resets and Use a reset; Claude's banks none", function()
	local app = open()
	expect(shows(app, PAGE, "2 resets banked")).to_be(true)
	local until_at = clock(os.time() + 2 * 3600)
	expect(app:has(`{PAGE} >> "One starts the session and week over now, instead of waiting until {until_at}."`)).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Use a reset"`)).to_be(true)
	-- One banked row, the Codex account's: the Claude account at its limit
	-- has none, and only one Use a reset shows.
	local rows = 0
	for _ in app:tree(PAGE):gmatch("resets? banked") do
		rows += 1
	end
	expect(rows).to_be(1)
	expect(shows(app, PAGE, "bo@example.com")).to_be(true)
end)

test("Codex resets: the dialog lists the soonest first, chosen and focused; Use reset spends it and the row says the limits started over", function()
	local app = open()
	local email = node(app, PAGE, "text", "ada@example.com")
	local switch = node(app, PAGE, "switch", "Codex on")
	local count = node(app, PAGE, "text", "2 resets banked")
	use_reset(app)
	expect(app:has(`{DIALOG} >> "The session and week limits for ada@example.com start over now. A reset is spent only if a window needed it."`)).to_be(true)
	local list = radios(app)
	expect(#list).to_be(2)
	expect(list[1].label:find("(soonest)", 1, true) ~= nil).to_be(true)
	expect(list[2].label:find("(soonest)", 1, true) == nil).to_be(true)
	expect(list[1].checked).to_be(true)
	expect(list[1].focused).to_be(true)
	expect(list[2].checked).to_be(false)

	app:click(`{DIALOG} >> role:button "Use reset"`)
	settle(app)
	expect(dialog_open(app)).to_be(false)
	expect(calls(app)).to_contain("use_reset:cx1:c1")
	-- Under way: said on the row, nothing to press.
	expect(shows(app, PAGE, "Using a reset…")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Use a reset"`)).to_be(false)

	app:command("resets.answer")
	settle(app)
	expect(shows(app, PAGE, "Plus · Limits reset just now")).to_be(true)
	expect(shows(app, PAGE, "1 reset banked")).to_be(true)
	expect(shows(app, PAGE, "Kept for the next time a limit is hit.")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Use a reset"`)).to_be(false)
	-- In place: the account, the card's switch and the count are the nodes
	-- they were.
	expect(node(app, PAGE, "text", "ada@example.com")).to_be(email)
	expect(node(app, PAGE, "switch", "Codex on")).to_be(switch)
	expect(node(app, PAGE, "text", "1 reset banked")).to_be(count)

	-- A fresh probe: the real numbers; the row still says what happened.
	app:command("resets.fresh")
	settle(app)
	expect(shows(app, PAGE, '"2%"')).to_be(true)
	expect(shows(app, PAGE, "Plus · Limits reset just now")).to_be(true)
end)

test("Codex resets: choosing the later reset spends that one", function()
	local app = open()
	use_reset(app)
	local list = radios(app)
	app:click(`{DIALOG} >> role:radio "{list[2].label}"`)
	settle(app)
	expect(radios(app)[2].checked).to_be(true)
	app:click(`{DIALOG} >> role:button "Use reset"`)
	settle(app)
	expect(calls(app)).to_contain("use_reset:cx1:c2")
end)

test("Codex resets: Cancel spends nothing", function()
	local app = open()
	use_reset(app)
	app:click(`{DIALOG} >> role:button "Cancel"`)
	settle(app)
	expect(dialog_open(app)).to_be(false)
	expect(calls(app)).to_be("")
	expect(shows(app, PAGE, "2 resets banked")).to_be(true)
end)

test("Codex resets: no window needed it: still banked, said until dismissed", function()
	local app = open()
	app:set_state("resets_next", "nothing")
	use_reset(app)
	app:click(`{DIALOG} >> role:button "Use reset"`)
	settle(app)
	app:command("resets.answer")
	settle(app)
	expect(shows(app, PAGE, "No window needed it, so the reset is still banked.")).to_be(true)
	expect(shows(app, PAGE, "2 resets banked")).to_be(true)
	app:click(`{PAGE} >> role:button "Dismiss"`)
	settle(app)
	expect(calls(app)).to_contain("dismiss_reset:cx1")
	expect(shows(app, PAGE, "No window needed it")).to_be(false)
end)

test("Codex resets: a reset no longer there says so, and the count goes down", function()
	local app = open()
	app:set_state("resets_next", "gone")
	use_reset(app)
	app:click(`{DIALOG} >> role:button "Use reset"`)
	settle(app)
	app:command("resets.answer")
	settle(app)
	expect(shows(app, PAGE, "That reset was no longer available.")).to_be(true)
	expect(shows(app, PAGE, "1 reset banked")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Dismiss"`)).to_be(true)
end)

test("Codex resets: Codex answering for another account: nothing used, why said, another go", function()
	local app = open()
	app:set_state("resets_next", "failed:mismatch")
	use_reset(app)
	app:click(`{DIALOG} >> role:button "Use reset"`)
	settle(app)
	app:command("resets.answer")
	settle(app)
	expect(shows(app, PAGE, "Couldn't use the reset")).to_be(true)
	expect(shows(app, PAGE, "Codex answered for another account. Nothing was used.")).to_be(true)
	-- The account as it was: still at its limit, both still banked.
	expect(shows(app, PAGE, "Limit reached")).to_be(true)
	expect(shows(app, PAGE, "Limits reset just now")).to_be(false)
	-- Try again: the dialog, the soonest chosen again.
	app:click(`{PAGE} >> role:button "Try again"`)
	settle(app)
	expect(dialog_open(app)).to_be(true)
	expect(radios(app)[1].checked).to_be(true)
	app:click(`{DIALOG} >> role:button "Cancel"`)
	settle(app)
	app:click(`{PAGE} >> role:button "Dismiss"`)
	settle(app)
	expect(shows(app, PAGE, "Couldn't use the reset")).to_be(false)
	expect(shows(app, PAGE, "2 resets banked")).to_be(true)
	expect(app:has(`{PAGE} >> role:button "Use a reset"`)).to_be(true)
end)

test("Codex resets: each refusal says why", function()
	local app = open()
	local refusals: { [string]: string } = {
		offline = "Zeron can't reach the device this account is on.",
		signed_out = "Codex couldn't confirm this login. Sign in again first.",
		missing = "This account is no longer saved on its device.",
		failed = "Codex didn't confirm it. Trying again won't use two.",
	}
	for why, words in refusals do
		app:set_state("resets_next", `failed:{why}`)
		app:click(if app:has(`{PAGE} >> role:button "Try again"`) then `{PAGE} >> role:button "Try again"` else `{PAGE} >> role:button "Use a reset"`)
		settle(app)
		expect(dialog_open(app)).to_be(true)
		app:click(`{DIALOG} >> role:button "Use reset"`)
		settle(app)
		app:command("resets.answer")
		settle(app)
		expect(`{why}: {tostring(app:has(`{PAGE} >> "{words}"`))}`).to_be(`{why}: true`)
	end
end)

test("Codex resets: the last reset used, then none banked: no row at all", function()
	local app = open()
	app:command("resets.one")
	settle(app)
	expect(shows(app, PAGE, "1 reset banked")).to_be(true)
	use_reset(app)
	-- One alone: no "(soonest)".
	expect(radios(app)[1].label:find("(soonest)", 1, true) == nil).to_be(true)
	app:click(`{DIALOG} >> role:button "Use reset"`)
	settle(app)
	app:command("resets.answer")
	settle(app)
	expect(shows(app, PAGE, "No resets banked")).to_be(true)
	expect(shows(app, PAGE, "That was the last one banked.")).to_be(true)
	app:command("resets.none")
	settle(app)
	expect(shows(app, PAGE, "banked")).to_be(false)
	expect(app:has(`{PAGE} >> role:button "Use a reset"`)).to_be(false)
end)

test("Codex resets: with addresses hidden the dialog doesn't name the account", function()
	local app = open()
	app:set_state("private_text_hidden", true)
	settle(app)
	use_reset(app)
	expect(app:has(`{DIALOG} >> "The session and week limits for this account start over now. A reset is spent only if a window needed it."`)).to_be(true)
	expect(shows(app, DIALOG, "ada@example.com")).to_be(false)
end)
