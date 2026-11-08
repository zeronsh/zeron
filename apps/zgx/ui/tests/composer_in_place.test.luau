-- The composer's notice (an alert of the deck's) follows what it says in
-- place: an upload counting up, then failing with Retry, is one row
-- throughout, its words, tone and action changing on it. (It was keyed by everything it said, so each step
-- faded the line out and in again.) On the composer in
-- ui/composer_fixture.luau.

-- A node's identity, by its kind and label (or text): `@id`.
local function node(app: any, kind: string, label: string?): string?
	local pattern = if label then ` {kind} "{label}"` else ` {kind} `
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(pattern, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

local function settle(app: any)
	app:frames(30)
end

test("a notice that moves on stays one row of the alerts", function()
	local app = mount("composer_fixture.luau")
	app:resize(700, 500)
	settle(app)
	app:set_state("test_composer_notice", { text = "Uploading 1 of 3", spinner = true })
	settle(app)
	-- In the pill, opened by the person.
	expect(app:has('role:button "Open Alerts"')).to_be(true)
	app:click('role:button "Open Alerts"')
	settle(app)
	local row = node(app, "listitem")
	local words = node(app, "text", "Uploading 1 of 3")
	expect(row).never.to_be_nil()
	-- Not to be let go: it says what's going on.
	expect(app:has('role:button "Dismiss Uploading 1 of 3"')).to_be(false)

	app:set_state("test_composer_notice", { text = "Uploading 2 of 3", spinner = true })
	settle(app)
	expect(`row @{node(app, "listitem")}`).to_be(`row @{row}`)
	expect(`words @{node(app, "text", "Uploading 2 of 3")}`).to_be(`words @{words}`)

	app:set_state("test_composer_notice", { text = "One file didn't upload", tone = "danger", action = "Retry", dismissable = true })
	settle(app)
	expect(`row @{node(app, "listitem")}`).to_be(`row @{row}`)
	expect(app:has('role:button "Retry"')).to_be(true)
	expect(app:has('role:button "Dismiss One file didn\'t upload"')).to_be(true)

	-- Gone: its tab goes.
	app:set_state("test_composer_notice", nil)
	settle(app)
	expect(app:has('role:button "Open Alerts"')).to_be(false)
	expect(app:has('role:tab "Fold Alerts"')).to_be(false)
	expect(app:error()).to_be(nil)
end)
