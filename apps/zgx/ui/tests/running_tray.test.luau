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
	expect(app:has('role:button "Fold Running"')).to_be(false)
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
	expect(app:has('role:button "Fold Running"')).to_be(true)
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
