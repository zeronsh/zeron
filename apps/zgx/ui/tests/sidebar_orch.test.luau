-- The sidebar's Control room and Orchestrators section, on orchestration's
-- sample world (ui/sidebar_orch_fixture.luau): what needs you counted, a row
-- per orchestrator with its crew working of all of it, orchestrators' own
-- chats never listed as sessions, and what its menus and "+" ask for.

local SIDE = "#shell-sidebar"
local ROW = `{SIDE} >> .orchestrator-row`

local function open(): any
	local app = mount("sidebar_orch_fixture.luau")
	app:resize(1000, 800)
	app:frames(30)
	return app
end

local function tree(app: any, target: string): string
	return app:tree(target) :: string
end

-- The lines of the row named `name` (a 30-high button) and what it holds,
-- in `target`'s tree ("": none).
local function row_of(app: any, name: string, target: string?): string
	local out: { string } = {}
	local inside: number? = nil
	for line in tree(app, target or SIDE):gmatch("[^\n]+") do
		local indent = #(line:match("^(%s*)") :: string)
		if inside then
			if indent <= inside then
				break
			end
			table.insert(out, line)
		elseif line:find(`button "{name}"`, 1, true) and line:find("x30", 1, true) then
			inside = indent
			table.insert(out, line)
		end
	end
	return table.concat(out, "\n")
end

test("sidebar orch: the Control room counts what needs you", function()
	local app = open()
	-- (The board's Needs you: the orchestrator screen's card (ak-1, its
	-- thread awaiting input counted with it), the Moscow reboot's (ak-2),
	-- and sccache's, blocked with nothing asked: ds/orch_data `needs`.)
	expect(row_of(app, "Control room")).to_contain('text "3"')
	app:click(`{SIDE} >> role:button "Control room"`)
	app:frames(10)
	expect(app:state("orch_route")).to_be("control:")
end)

test("sidebar orch: a row per orchestrator, the Brain dump not among them", function()
	local app = open()
	expect(app:has(`{SIDE} >> role:button "Brain dump"`)).to_be(false)
	-- Orchestrators: three of four at work; ak-1 open and a thread asking.
	local orch = row_of(app, "Orchestrators")
	expect(orch).to_contain('text "3/4"')
	expect(orch).to_contain('text "2"')
	-- Exnomic VPN: one of two at work, its approval waiting.
	local vpn = row_of(app, "Exnomic VPN")
	expect(vpn).to_contain('text "1/2"')
	expect(vpn).to_contain('text "1"')
	-- CI speed: paused, its mark in place of its counts.
	local ci = row_of(app, "CI speed")
	expect(ci).to_contain('img "Paused"')
	expect(ci).never.to_contain('text "0/1"')
end)

test("sidebar orch: orchestrators' chats aren't sessions", function()
	local app = open()
	local list = tree(app, `{SIDE} >> #sidebar`)
	expect(list).to_contain('button "Linux titlebar toggle"')
	for _, name in { "Orchestrators", "Exnomic VPN", "CI speed", "Brain dump" } do
		expect(list).never.to_contain(`button "{name}"`)
	end
end)

test("sidebar orch: a row opens its page", function()
	local app = open()
	app:click(`{ROW} >> ~"3/4"`)
	app:frames(10)
	expect(app:state("orch_route")).to_be("orchestrator:orc-orch")
end)

test("sidebar orch: the section's + opens New orchestrator", function()
	local app = open()
	expect(app:state("orch_new_open")).to_be(false)
	app:click(`{SIDE} >> role:button "New orchestrator"`)
	app:frames(5)
	expect(app:state("orch_new_open")).to_be(true)
end)

test("sidebar orch: a row's menu pauses and resumes it", function()
	local app = open()
	app:right_click(`{ROW} >> ~"1/2"`)
	app:frames(20)
	app:click('role:menuitem "Pause"')
	app:frames(10)
	expect(app:state("orch_calls")).to_contain("updateOrchestrator:id=orc-vpn,paused=true")
	app:right_click(`{ROW} >> role:img "Paused"`)
	app:frames(20)
	app:click('role:menuitem "Resume"')
	app:frames(10)
	expect(app:state("orch_calls")).to_contain("updateOrchestrator:id=orc-ci,paused=false")
end)

test("sidebar orch: Make orchestrator on a session's menu", function()
	local app = open()
	app:right_click(`{SIDE} >> role:button "Linux titlebar toggle"`)
	app:frames(20)
	app:click('role:menuitem "Make orchestrator"')
	app:frames(10)
	expect(app:state("orch_calls")).to_contain("promoteChat:chatId=th-titlebar,name=Linux titlebar toggle")
	expect(app:state("orch_route")).to_be("orchestrator:th-titlebar")
end)

test("sidebar orch: Add to board on a session's menu, while it has no card not done", function()
	local app = open()
	-- Its card is done: it can go on the board again.
	app:right_click(`{SIDE} >> role:button "Linux titlebar toggle"`)
	app:frames(20)
	app:click('role:menuitem "Add to board"')
	app:frames(10)
	expect(app:state("orch_calls")).to_contain("addToBoard:chatId=th-titlebar")
end)

test("sidebar orch: the rail, drawn and resized, folded and open", function()
	local app = open()
	local rail = tree(app, "#orch-rail")
	expect(rail).to_contain('button "Control room"')
	expect(rail).to_contain('button "Exnomic VPN"')
	expect(rail).never.to_contain('button "Brain dump"')
	for _, size in { { 1400, 900 }, { 700, 600 }, { 1000, 800 } } do
		app:resize(size[1], size[2])
		app:frames(20)
		expect(app:has(`{SIDE} >> role:button "Control room"`)).to_be(true)
	end
	-- The section folded by its title, and back.
	app:click(`{SIDE} >> .orchestrators-header`)
	app:frames(20)
	expect(row_of(app, "Exnomic VPN")).to_be("")
	app:click(`{SIDE} >> .orchestrators-header`)
	app:frames(20)
	expect(row_of(app, "Exnomic VPN")).never.to_be("")
	-- The open sidebar folded away, and back.
	app:set_state("orch_sidebar_collapsed", true)
	app:frames(40)
	app:set_state("orch_sidebar_collapsed", false)
	app:frames(40)
	expect(app:has(`{SIDE} >> role:button "Control room"`)).to_be(true)
end)
