-- The composer's Running tray (what a session runs in the background), on
-- ui/running_tray_fixture.luau. It stays folded to its header until opened,
-- and a list published whole each time (as the host does) changes its rows
-- in place: none goes and comes back. (The host once listed the processes
-- only while a turn ran, so the tray emptied as each turn ended and filled
-- again as the next began, opening itself each time it came back.)

-- `n` processes, `b1`…: a fresh table each time, as the host publishes.
local function snapshot(n: number, word: string): { any }
	local list = {}
	for i = 1, n do
		table.insert(list, { id = `b{i}`, kind = "process", name = `job {i}`, detail = `{word} {i}`, since = os.time() - 60 * i })
	end
	return list
end

-- Each row's Stop, by its node: `{ ["job 1"] = "@12", … }`, and how many.
local function rows(app: any): ({ [string]: string }, number)
	local out, count = {}, 0
	for line in app:tree():gmatch("[^\n]+") do
		local id, name = line:match('@(%d+) button "Stop (job %d+)"')
		if id then
			out[name] = id
			count += 1
		end
	end
	return out, count
end

test("the Running tray stays folded to its header until opened", function()
	local app = mount("running_tray_fixture.luau")
	app:resize(700, 600)
	app:frames(10)
	app:set_state("test_running_tasks", snapshot(20, "first"))
	app:frames(30)
	-- Its header is there, folded, and says how many.
	expect(app:has('role:button "Open Running"')).to_be(true)
	expect(app:has('role:tab "Fold Running"')).to_be(false)
	expect(app:has('~"20"')).to_be(true)
	expect(app:state("test_running_tray")).to_be(nil)
	-- More arriving keeps it folded.
	app:set_state("test_running_tasks", snapshot(21, "second"))
	app:frames(30)
	expect(app:state("test_running_tray")).to_be(nil)
	expect(app:has('role:button "Open Running"')).to_be(true)
	-- Opened, its rows show, and Stop reaches the host.
	app:click('role:button "Open Running"')
	app:frames(30)
	expect(app:state("test_running_tray")).to_be("tasks")
	expect(app:has('role:tab "Fold Running"')).to_be(true)
	local _, open = rows(app)
	expect(open).to_be(21)
	app:click('role:button "Stop job 3"')
	app:frames(5)
	expect(app:state("calls")).to_contain("stop:b3")
	expect(app:error()).to_be(nil)
end)

test("alternating snapshots of the same processes keep every row in place", function()
	local app = mount("running_tray_fixture.luau")
	app:resize(700, 900)
	app:frames(10)
	app:set_state("test_running_tasks", snapshot(20, "first"))
	app:set_state("test_running_tray", "tasks")
	app:frames(40)
	local before, count = rows(app)
	expect(count).to_be(20)
	-- Snapshots a moment apart: the same twenty, each a new table with its
	-- words changed, one more arriving and going, back and forth.
	for round = 1, 6 do
		local list = if round % 2 == 1 then snapshot(21, `round {round}`) else snapshot(20, `round {round}`)
		app:set_state("test_running_tasks", list)
		-- Every frame of the change: no row of the twenty leaves, or is
		-- made again.
		for _ = 1, 8 do
			app:frames(1)
			local now = rows(app)
			for i = 1, 20 do
				local name = `job {i}`
				expect(`{name} @{now[name]}`).to_be(`{name} @{before[name]}`)
			end
		end
		expect(app:state("test_running_tray")).to_be("tasks")
	end
	expect(app:error()).to_be(nil)
end)

-- Subagents as the chat lists them: running first, then the finished.
local function agents(running: number, done: number): { any }
	local list = {}
	for i = 1, running do
		table.insert(list, { id = `a{i}`, kind = "agent", name = `agent {i}` })
	end
	for i = 1, done do
		table.insert(list, { id = `f{i}`, kind = "agent", name = `old agent {i}`, ended = if i % 3 == 0 then "failed" else "done", failed = i % 3 == 0 or nil })
	end
	return list
end

-- Whether a button is on screen (a folded tray keeps its rows laid out of
-- the way, hidden), and the height of the tray's list as it is laid out.
local function shown(app: any, label: string): boolean
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(`button "{label}"`, 1, true) then
			return not line:find(" hidden", 1, true) and not line:find(" 0x0", 1, true)
		end
	end
	return false
end
local function list_height(app: any): number
	for line in app:tree():gmatch("[^\n]+") do
		local h = line:match("scroll %-?%d+,%-?%d+ %d+x(%d+)$")
		if h then
			return tonumber(h) :: number
		end
	end
	return 0
end

test("a long list of subagents stays one folded line, then one list that scrolls", function()
	local app = mount("running_tray_fixture.luau")
	app:resize(700, 500)
	app:frames(10)
	app:set_state("test_running_tasks", agents(17, 33))
	app:frames(30)
	-- Folded: one pill, whatever the count, with the running ones counted.
	expect(app:has('role:button "Open Subagents"')).to_be(true)
	expect(app:has('role:button "Open Running"')).to_be(false)
	expect(shown(app, "Open agent 1")).to_be(false)
	app:click('role:button "Open Subagents"')
	app:frames(30)
	-- The running ones are rows; the 33 finished stay behind one.
	expect(shown(app, "Open agent 1")).to_be(true)
	expect(app:has('role:button "Show 33 finished"')).to_be(true)
	expect(app:has('role:button "Open old agent 1"')).to_be(false)
	-- However many, the open tray keeps to its height (the list scrolls in
	-- it), so the conversation above is never pushed far.
	local cap = list_height(app)
	expect(cap > 0 and cap <= 216).to_be(true)
	app:click('role:button "Open agent 2"')
	expect(app:state("calls")).to_contain("open:a2")
	-- (Past the cap: scrolled to.)
	app:scroll('role:button "Open agent 5"', 2000)
	app:frames(10)
	app:click('role:button "Show 33 finished"')
	app:frames(30)
	expect(app:has('role:button "Hide 33 finished"')).to_be(true)
	expect(shown(app, "Open old agent 1")).to_be(true)
	expect(list_height(app)).to_be(cap)
	expect(app:error()).to_be(nil)
end)

test("with nothing running the tray stays for whoever has it open, and goes when folded", function()
	local app = mount("running_tray_fixture.luau")
	app:resize(700, 500)
	app:frames(10)
	app:set_state("test_running_tasks", agents(2, 3))
	app:set_state("test_running_tray", "agents")
	app:frames(30)
	expect(app:has('role:tab "Fold Subagents"')).to_be(true)
	-- The last two finish: the tab, open, stays and lists what finished.
	app:set_state("test_running_tasks", agents(0, 5))
	app:frames(30)
	expect(app:has('role:tab "Fold Subagents"')).to_be(true)
	expect(app:has('role:button "Open old agent 1"')).to_be(true)
	-- Folded, it goes: nothing runs to keep it.
	app:click('role:tab "Fold Subagents"')
	app:frames(30)
	expect(app:state("test_running_tray")).to_be(nil)
	-- And finished subagents alone never raise it.
	expect(app:has('role:button "Open Subagents"')).to_be(false)
	expect(app:error()).to_be(nil)
end)

test("a command of several lines shows on one line in its row", function()
	local app = mount("running_tray_fixture.luau")
	app:resize(700, 600)
	app:frames(10)
	app:set_state("test_running_tasks", {
		{ id = "b1", kind = "process", name = "Check the build", detail = "cd repo\n  cargo check\ncargo test", since = os.time() - 5 },
	})
	app:frames(20)
	app:click('role:button "Open Running"')
	app:frames(30)
	expect(app:has('~"cd repo cargo check cargo test"')).to_be(true)
	expect(app:has('~"cargo check\ncargo test"')).to_be(false)
	expect(app:error()).to_be(nil)
end)
