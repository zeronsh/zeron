-- Rows read one by one (account/rows): a session push changing one chat
-- wakes the reader of that chat alone, however the push hands the rest
-- back (here, every row a new table: the whole list written again).

local function open(): any
	local app = mount("rows_fixture.luau")
	app:frames(5)
	app:command("rows_reset")
	return app
end

-- The readers that ran since the last reset: { [id] = runs }.
local function ran(app: any): { [string]: number }
	app:command("rows_read")
	local out = {}
	for k, v in app:state("rows_counts") or {} do
		local id = string.match(k, "^row%.(.+)$")
		if id then
			out[id] = v
		end
	end
	return out
end

local function only(r: { [string]: number }): { string }
	local out = {}
	for id in r do
		table.insert(out, id)
	end
	table.sort(out)
	return out
end

test("rows keyed: one chat's status change wakes its reader alone", function()
	local app = open()
	app:command("rows_status")
	app:frames(3)
	expect(only(ran(app))).to_equal({ "c7" })
	expect(app:has('"c7: Chat 7 working"')).to_be(true)
	expect(app:has('"c8: Chat 8 done"')).to_be(true)
end)

test("rows keyed: a rename wakes the renamed chat's reader alone", function()
	local app = open()
	app:command("rows_rename")
	app:frames(3)
	expect(only(ran(app))).to_equal({ "c13" })
	expect(app:has('"c13: Renamed done"')).to_be(true)
end)

test("rows keyed: a chat added is read, the others not woken", function()
	local app = open()
	app:command("rows_add")
	app:frames(3)
	expect(only(ran(app))).to_equal({ "c41" })
	expect(app:has('"c41: Chat 41 working"')).to_be(true)
end)

test("rows keyed: a chat removed wakes its reader alone, which finds it gone", function()
	local app = open()
	app:command("rows_remove")
	app:frames(3)
	expect(only(ran(app))).to_equal({ "c21" })
	expect(app:has('"c21: gone"')).to_be(true)
	expect(app:has('"c22: Chat 22 done"')).to_be(true)
end)

test("rows keyed: the same list pushed again wakes nobody", function()
	local app = open()
	app:command("rows_same")
	app:frames(3)
	expect(#only(ran(app))).to_be(0)
end)
