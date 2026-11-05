-- Reduce motion (ds/motion: the system's setting or Zeron's own,
-- `motion_reduce`): what a state change shows is there on the next frame,
-- with no travel after it. The sidebar's fold and the panel's opening land
-- at once.

-- How wide node `id` stands now (its line in the tree).
local function width(app: any, id: string): number
	for line in app:tree("#shell"):gmatch("[^\n]+") do
		if line:find(" " .. id, 1, true) then
			return tonumber(line:match(" %-?%d+,%-?%d+ (%d+)x%d+")) or 0
		end
	end
	return -1
end

local function reduced_shell(): any
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	expect(app:error()).to_be_nil()
	app:set_state("motion_reduce", true)
	app:frames(2)
	return app
end

-- After `act`, two frames (the press's, then the one it shows on), its
-- width as it stays: no frame after those moves it.
local function lands_at_once(app: any, id: string, act: () -> ()): (number, number)
	local before = width(app, id)
	act()
	app:frames(2)
	local landed = width(app, id)
	app:frames(30)
	expect(width(app, id)).to_be(landed)
	return before, landed
end

test("with motion reduced, the sidebar folds and comes back at once", function()
	local app = reduced_shell()
	-- (Its card, at once. The window's 5-point edge before it is a tween, fixed as
	-- it's built: here that was before motion was reduced.)
	local function card(act: () -> ()): (number, number)
		local before = width(app, "#shell-sidebar")
		act()
		app:frames(2)
		local landed = width(app, "#shell-sidebar")
		app:frames(30)
		expect(math.abs(width(app, "#shell-sidebar") - landed) <= 5).to_be(true)
		return before, landed
	end
	local open_w, folded = card(function()
		app:click('#titlebar >> role:button "Toggle sidebar"')
	end)
	expect(folded <= 5).to_be(true)
	local _, back = card(function()
		app:click('#titlebar >> role:button "Toggle sidebar"')
	end)
	expect(math.abs(back - open_w) <= 5).to_be(true)
end)

test("with motion reduced, the panel opens and closes at once", function()
	local app = reduced_shell()
	app:click('#shell-sidebar >> role:button "Write the docs"')
	app:frames(20)
	local shut, opened = lands_at_once(app, "#shell-files", function()
		app:click('#titlebar >> role:button "Panel"')
	end)
	expect(opened > 300).to_be(true)
	local _, closed = lands_at_once(app, "#shell-files", function()
		app:click('#titlebar >> role:button "Panel"')
	end)
	expect(closed).to_be(shut)
end)

test("without it, the sidebar's fold still travels", function()
	local app = mount("shell_fixture.luau")
	app:resize(1200, 800)
	app:frames(30)
	local open_w = width(app, "#shell-sidebar")
	app:click('#titlebar >> role:button "Toggle sidebar"')
	app:frames(2)
	local early = width(app, "#shell-sidebar")
	app:frames(40)
	local folded = width(app, "#shell-sidebar")
	-- Part way two frames in, then on to where it rests.
	expect(early > folded and early <= open_w).to_be(true)
end)

-- A send's prompt, frame by frame from when it joins: its y on each.
local function prompt_ys(app: any, words: string): { number }
	app:type('role:textbox "Message"', words)
	app:key("Enter")
	local id: string? = nil
	local ys: { number } = {}
	for _ = 1, 40 do
		app:wait("#never-shown", { timeout_ms = 4 })
		if not id then
			local ms = app:state("chat.messages")
			local last = ms and ms[#ms]
			if last and last.text == words then
				id = last.id
			end
		end
		if id then
			local first: string = app:tree("#" .. id):match("[^\n]+") or ""
			local y = tonumber(first:match(" %-?%d+,(%-?%d+) "))
			if y then
				table.insert(ys, y)
			end
		end
	end
	return ys
end

test("with motion reduced, a send's prompt lands at the top without gliding", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s1")
	app:frames(30)
	app:set_state("motion_reduce", true)
	app:frames(2)
	local ys = prompt_ys(app, "Straight there")
	expect(#ys > 5).to_be(true)
	-- Where it rests; at most one frame anywhere between where it joined
	-- and there (the frame the jump shows on).
	local rest = ys[#ys]
	local between = 0
	for _, y in ys do
		if y ~= ys[1] and y ~= rest then
			between += 1
		end
	end
	expect(rest < 120).to_be(true)
	expect(between <= 1).to_be(true)
end)
