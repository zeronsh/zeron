-- A side chat's transcript and composer read as the chat page's (ui/
-- parity_fixture.luau: the two side by side, the same width, the same
-- conversation): the composer at the same height from the foot, the last
-- row as far above it, the top inset the same, a sent prompt gliding to the
-- same place with its working row the same distance above the composer,
-- and "Jump to latest" as far above the composer. Any of these moving in
-- one and not the other fails here.

local SIDES = { page = "#parity-page", lane = "#parity-lane" }

local function open(): any
	local app = mount("parity_fixture.luau")
	app:resize(1300, 820)
	app:frames(60)
	expect(app:error()).to_be_nil()
	expect(app:wait("#parity-page >> #transcript", { timeout_ms = 15000 })).to_be(true)
	expect(app:wait("#parity-lane >> #transcript", { timeout_ms = 15000 })).to_be(true)
	app:frames(60)
	return app
end

-- A node's box in the window: x, y, w, h (its first line in the tree).
local function box(app: any, target: string): (number, number, number, number)
	local line = app:tree(target):match("[^\n]+") or ""
	local x, y, w, h = line:match(" (%-?%d+),(%-?%d+) (%d+)x(%d+)")
	expect(x ~= nil).to_be(true)
	return tonumber(x) :: number, tonumber(y) :: number, tonumber(w) :: number, tonumber(h) :: number
end

-- Where a side's composer starts (its pill's top), and its transcript.
local COMPOSER = { page = "#chat-composer", lane = "#lane-composer-tab1" }
local function composer_top(app: any, side: string): number
	local _, y = box(app, `{SIDES[side]} >> {COMPOSER[side]}`)
	return y
end
local function transcript_top(app: any, side: string): number
	local _, y = box(app, `{SIDES[side]} >> #transcript`)
	return y
end

-- The bottom of the last text in a side's transcript (its last row), and
-- the top of the text `words`.
local function last_bottom(app: any, side: string): number
	local bottom = -1
	for line in app:tree(`{SIDES[side]} >> #transcript`):gmatch("[^\n]+") do
		if line:find(' text "', 1, true) and not line:find(" hidden", 1, true) then
			local _, y, _, h = line:match(" (%-?%d+),(%-?%d+) (%d+)x(%d+)")
			if y then
				bottom = math.max(bottom, tonumber(y) :: number + tonumber(h) :: number)
			end
		end
	end
	return bottom
end
local function top_of(app: any, side: string, words: string): number?
	for line in app:tree(`{SIDES[side]} >> #transcript`):gmatch("[^\n]+") do
		if line:find(`text "{words}"`, 1, true) then
			local y = line:match(" %-?%d+,(%-?%d+) %d+x%d+")
			return tonumber(y)
		end
	end
	return nil
end

-- (Within a point: the two are laid out in different columns.)
local function same(a: number, b: number, what: string)
	if math.abs(a - b) > 1 then
		error(`{what}: the chat page's {a}, the side chat's {b}`, 2)
	end
end

test("the composer sits as far from the foot, the top inset is the same, the last row as far above the composer", function()
	local app = open()
	local _, page_y, _, page_h = box(app, "#parity-page")
	local _, lane_y, _, lane_h = box(app, "#parity-lane")
	same(page_y + page_h - composer_top(app, "page"), lane_y + lane_h - composer_top(app, "lane"), "the composer's distance from the foot")
	same(transcript_top(app, "page") - page_y, transcript_top(app, "lane") - lane_y, "the transcript's top inset")
	same(composer_top(app, "page") - last_bottom(app, "page"), composer_top(app, "lane") - last_bottom(app, "lane"), "the last row's room above the composer")
end)

test("a sent prompt glides to the same place, its working row as far above the composer", function()
	local app = open()
	for _, side in { "page", "lane" } do
		app:type(`{SIDES[side]} >> role:textbox "Message"`, "Tell me a long story")
		app:key("Enter")
		app:frames(10)
	end
	-- (The glide, and the working row's arrival.)
	for _ = 1, 10 do
		app:wait('"never shown, only waited past"', { timeout_ms = 100 })
		app:frames(10)
	end
	local page_prompt = top_of(app, "page", "Tell me a long story")
	local lane_prompt = top_of(app, "lane", "Tell me a long story")
	expect(page_prompt ~= nil and lane_prompt ~= nil).to_be(true)
	same((page_prompt :: number) - transcript_top(app, "page"), (lane_prompt :: number) - transcript_top(app, "lane"), "the sent prompt's place under the top")
	same(composer_top(app, "page") - last_bottom(app, "page"), composer_top(app, "lane") - last_bottom(app, "lane"), "the working row's room above the composer")
	expect(composer_top(app, "lane") - last_bottom(app, "lane") > 0).to_be(true)
end)

test("Jump to latest floats as far above the composer", function()
	local app = open()
	for _, side in { "page", "lane" } do
		app:scroll(`{SIDES[side]} >> #transcript`, -2000)
		app:frames(10)
		-- (A new gesture for the other: the first one's wheel lets go.)
		app:wait('"never shown, only waited past"', { timeout_ms = 700 })
	end
	for _ = 1, 6 do
		app:wait('"never shown, only waited past"', { timeout_ms = 100 })
		app:frames(10)
	end
	local tops = {}
	for _, side in { "page", "lane" } do
		local target = `{SIDES[side]} >> role:button "Jump to latest"`
		expect(app:has(target)).to_be(true)
		local _, y, _, h = box(app, target)
		tops[side] = composer_top(app, side) - (y + h)
	end
	same(tops.page, tops.lane, "Jump to latest's room above the composer")
	expect(tops.lane > 0).to_be(true)
end)
