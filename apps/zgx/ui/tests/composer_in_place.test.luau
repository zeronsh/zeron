-- The composer's notice follows what it says in place: an upload counting
-- up, then failing with Retry, is one line throughout, its words, tone and
-- action changing on it. (It was keyed by everything it said, so each step
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

test("a notice that moves on stays one line", function()
	local app = mount("composer_fixture.luau")
	app:resize(700, 500)
	settle(app)
	app:set_state("test_composer_notice", { text = "Uploading 1 of 3", spinner = true })
	settle(app)
	local line = node(app, "status")
	local words = node(app, "text", "Uploading 1 of 3")
	expect(line).never.to_be_nil()

	app:set_state("test_composer_notice", { text = "Uploading 2 of 3", spinner = true })
	settle(app)
	expect(`line @{node(app, "status")}`).to_be(`line @{line}`)
	expect(`words @{node(app, "text", "Uploading 2 of 3")}`).to_be(`words @{words}`)

	app:set_state("test_composer_notice", { text = "One file didn't upload", tone = "danger", action = "Retry", dismissable = true })
	settle(app)
	expect(`line @{node(app, "status")}`).to_be(`line @{line}`)
	expect(app:has('role:button "Retry"')).to_be(true)
	expect(app:has('role:button "Dismiss"')).to_be(true)

	-- Gone: the line goes.
	app:set_state("test_composer_notice", nil)
	expect(app:wait("role:status", { gone = true, timeout_ms = 2000 })).to_be(true)
end)
