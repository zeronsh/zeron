-- A row's trailing items (its PR badge, the subagents pill, the corner) in
-- a cluttered, narrow sidebar: the corner is one width on every row
-- whether a title is cut short or not, and when a row has no room left at
-- all (a five-digit PR and a "99+" pill at 200 points wide) the corner
-- gives up the room it holds back rather than leave the row. On the
-- design system's sidebar in ui/sidebar_squeeze_fixture.luau.

local function open(narrow: boolean): any
	local app = mount("sidebar_squeeze_fixture.luau")
	app:resize(900, 700)
	app:frames(30)
	if narrow then
		app:command("narrow")
		app:frames(30)
	end
	return app
end

type Box = { text: string, x: number, w: number }

-- What a row has in it that shows (not what its second line keeps
-- hidden), each with where it stands.
local function inside(app: any, title: string): (Box, { Box })
	local row: Box? = nil
	local out: { Box } = {}
	for line in app:tree(`role:button "{title}"`):gmatch("[^\n]+") do
		local x, w = line:match(" (%-?%d+),%-?%d+ (%d+)x%d+")
		if x == nil then
			continue
		end
		if row == nil then
			row = { text = line, x = tonumber(x) :: number, w = tonumber(w) :: number }
		elseif not line:find(" hidden", 1, true) then
			table.insert(out, { text = line, x = tonumber(x) :: number, w = tonumber(w) :: number })
		end
	end
	assert(row, `no row "{title}"`)
	return row :: Box, out
end

-- Where a row's PR badge ends.
local function badge_end(app: any, title: string): number
	local _, boxes = inside(app, title)
	for _, b in boxes do
		if b.text:find("Pull request", 1, true) then
			return b.x + b.w
		end
	end
	error(`no PR badge on "{title}"`)
end

test("the corner's column holds, a title cut short or not", function()
	local app = open(false)
	local at = badge_end(app, "Docs sweep")
	expect(badge_end(app, "Tidy the tests")).to_be(at)
	expect(badge_end(app, "Cache invalidation for the image pipeline")).to_be(at)
end)

test("nothing of a row is drawn past its end at 200 points, a five-digit PR and a 99+ pill with it", function()
	local app = open(true)
	for _, title in {
		"Migrate the build to the new runner and keep the cache warm",
		"Docs sweep",
		"Cache invalidation for the image pipeline",
	} do
		local row, boxes = inside(app, title)
		for _, b in boxes do
			expect(b.x + b.w <= row.x + row.w).to_be(true)
		end
	end
end)
