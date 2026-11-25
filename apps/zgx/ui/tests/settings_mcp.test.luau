-- Settings → MCP: a section with nothing in it (no server added, nothing asked
-- for) takes no room, so the page's sections stand one step of its rhythm
-- apart whichever of them show. On ui/mcp_page_fixture.luau.

local PAGE = "#settings-page"
local NAV = "#settings-nav"
-- From the page's words to its first section, and what a line of text may
-- round to.
local STEP, SLACK = 32, 2

-- A text's top and foot on the page.
local function span(app: any, words: string): (number, number)
	for line in app:tree(PAGE):gmatch("[^\n]+") do
		if line:find(words, 1, true) then
			local y, h = line:match(" %-?[%d.]+,(%-?[%d.]+) [%d.]+x([%d.]+)")
			if y then
				return tonumber(y) :: number, (tonumber(y) :: number) + (tonumber(h) :: number)
			end
		end
	end
	error(`no "{words}" on the page:\n{app:tree(PAGE)}`)
end

-- From the foot of the page's description to the top of a section's title.
local function below(app: any, title: string): number
	local _, foot = span(app, "Tools your agents can use")
	local top = span(app, title)
	return top - foot
end

local function open(): any
	local app = mount("mcp_page_fixture.luau")
	app:resize(1200, 900)
	app:frames(30)
	app:command("settings_stub.sign_in")
	app:command("shell.settings")
	app:frames(20)
	app:click(`{NAV} >> role:tab "MCP"`)
	app:frames(30)
	return app
end

test("MCP: with no server and nothing asked for, the list to add from is right under the page's words", function()
	local app = open()
	expect(app:has(`{PAGE} >> "Your servers"`)).to_be(false)
	local gap = below(app, "Add a server")
	expect(`{math.abs(gap - STEP) <= SLACK} ({gap})`).to_be(`true ({gap})`)
	expect(app:error()).to_be_nil()
end)

test("MCP: a server added, or a change asked for, shows there, and its room goes with it", function()
	local app = open()
	for _, case in { { "mcp_page.add", "Your servers" }, { "mcp_page.ask", "Waiting for you" } } do
		app:command(case[1])
		app:frames(30)
		local gap = below(app, case[2])
		expect(`{case[2]}: {math.abs(gap - STEP) <= SLACK} ({gap})`).to_be(`{case[2]}: true ({gap})`)
		app:command("mcp_page.none")
		app:frames(30)
		expect(app:has(`{PAGE} >> "{case[2]}"`)).to_be(false)
		local back = below(app, "Add a server")
		expect(`{math.abs(back - STEP) <= SLACK} ({back})`).to_be(`true ({back})`)
	end
	expect(app:error()).to_be_nil()
end)
