-- A subagent in its reply's work group (ds/activity): the side chat's glyph, its
-- loader while it works, and on show with its branch with the group folded.

local function open(): any
	local app = mount("subagent_rows_fixture.luau")
	app:resize(900, 700)
	app:frames(20)
	return app
end

test("folded, a subagent at work stays, and one done doesn't", function()
	local app = open()
	expect(app:has('role:button "Open subagent Read the articles"')).to_be(true)
	expect(app:has('role:button "Open subagent Read the feeds"')).to_be(false)
	expect(app:has('"the docs"')).to_be(false)
end)

test("it goes as it finishes", function()
	local app = open()
	app:set_state("subagent_status", "done")
	app:frames(20)
	expect(app:has('role:button "Open subagent Read the articles"')).to_be(false)
end)

test("pressing it opens its lane", function()
	local app = open()
	app:click('role:button "Open subagent Read the articles"')
	app:frames(5)
	expect(app:state("subagent_opened")).to_be("doc--sub--1")
end)

test("opened, the group lists it once", function()
	local app = open()
	app:click('role:button ~"Searched"')
	app:frames(30)
	local n = 0
	for line in app:tree():gmatch("[^\n]+") do
		if line:find("Open subagent Read the articles", 1, true) then
			n += 1
		end
	end
	expect(n).to_be(1)
	expect(app:has('role:button "Open subagent Read the feeds"')).to_be(true)
end)
