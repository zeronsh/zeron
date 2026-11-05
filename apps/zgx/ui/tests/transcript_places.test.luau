-- A chat switched away from and back to opens where it was left: at its
-- end if it was there, else at the message that was at the top of the view
-- (screens/chat/session keeps each chat's place; ds/transcript, made anew
-- for it, puts the view back). On the chat fixture's long chat (s1) and a
-- short one (s2).

-- The questions in view, top to bottom, and the transcript's top.
local function shown(app: any): ({ string }, number)
	local top = 0
	local ids = {}
	for line in app:tree("#transcript"):gmatch("[^\n]+") do
		local y, h = line:match("#transcript scroll %-?%d+,(%-?%d+) %d+x(%d+)")
		if y then
			top = tonumber(y) or 0
		end
		local id, ry, rh = line:match("#(u%d+) article .- %-?%d+,(%-?%d+) %d+x(%d+)")
		if id and (tonumber(ry) :: number) + (tonumber(rh) :: number) > top and (tonumber(ry) :: number) < top + 600 then
			table.insert(ids, id)
		end
	end
	return ids, top
end

-- Real time for the transcript's own waits (it notes its place 150 ms
-- after a scroll, and settles a restored place over 80 ms).
local function settle(app: any)
	app:wait("#never-shown", { timeout_ms = 250 })
	app:frames(10)
end

local function away_and_back(app: any)
	app:command("go_s2")
	app:frames(20)
	settle(app)
	app:command("go_s1")
	app:frames(20)
	settle(app)
end

local function open(): any
	local app = mount("chat_fixture.luau")
	app:resize(960, 700)
	app:frames(40)
	settle(app)
	return app
end

test("a long chat left at its end opens at its end again", function()
	local app = open()
	local before = shown(app)
	expect(table.find(before, "u30") ~= nil).to_be(true)
	away_and_back(app)
	local after = shown(app)
	expect(`{table.concat(after, ",")}: {table.find(after, "u30") ~= nil}`).to_be(`{table.concat(after, ",")}: true`)
end)

test("a long chat scrolled up opens at the same message again", function()
	local app = open()
	app:scroll("#transcript", -2400)
	app:frames(20)
	settle(app)
	local before = shown(app)
	expect(#before > 0).to_be(true)
	expect(table.find(before, "u30")).to_be(nil)
	away_and_back(app)
	local after = shown(app)
	expect(`first {after[1]}`).to_be(`first {before[1]}`)
end)
