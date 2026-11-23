-- Capture's receipt when parts share words (ds/orch_receipt, ds/orch_data):
-- a sentence routed to two places is underlined in both colours, striped;
-- either chip lights it in its own colour; the pointer on it lights both
-- chips; parts that overlap in part each keep their own words and share
-- the overlap. On ui/capture_routes_fixture.luau (the dump "dp-shared":
-- rt-a and rt-b on the first sentence, rt-c and rt-d overlapping in the
-- second). In Capture's history, opened from its bar.

local data = require("@ds/orch_data")

local function open(): any
	local app = mount("capture_routes_fixture.luau")
	app:resize(1440, 1200)
	app:frames(30)
	app:click("#control-history-open")
	app:frames(30)
	app:wait("#no-such-node", { timeout_ms = 500 })
	app:frames(30)
	return app
end

local function joined(list: { any }): string
	local out = {}
	for _, x in list do
		table.insert(out, tostring(x))
	end
	return table.concat(out, ",")
end

-- The routes each piece of `dump`'s marks names, and the words with them.
local function marks(app: any): { string }
	return (app:state("orch_receipt_marks") or {})["dp-shared"] or {}
end
local function washed(app: any): { string }
	return (app:state("orch_receipt_washed") or {})["dp-shared"] or {}
end
local function routes_in(list: { string }, words: string): { [string]: boolean }
	local out: { [string]: boolean } = {}
	for _, m in list do
		local rid, piece = string.match(m, "^([^:]+):(.*)$")
		if rid and piece and string.find(words, piece, 1, true) then
			out[rid] = true
		end
	end
	return out
end

local SHARED = "in cloud tab in settings i switched cloud machines from small to large, so you can try use them for testing and building now, should work way better."
local OVERLAP = "not selectable in the chat page"
local ONLY_C = "i think it's an issue that cloud machine size is "
local ONLY_D = ", and not selectable with zeron mcp when you create one."

test("one span routed to two places is one piece naming both routes", function()
	local text = "alpha beta gamma delta"
	local segs = data.segments(text, {
		{ span = { from = 6, to = 16 } },
		{ span = { from = 6, to = 16 } },
	})
	local whole = {}
	for _, s in segs do
		table.insert(whole, s.text)
	end
	expect(table.concat(whole)).to_be(text)
	expect(#segs).to_be(3)
	expect(segs[2].text).to_be("beta gamma")
	expect(joined(segs[2].routes)).to_be("1,2")
	-- (The first, for callers that want one.)
	expect(segs[2].route).to_be(1)
	expect(#segs[1].routes).to_be(0)
end)

test("parts overlapping in part each keep their own words and share the overlap", function()
	local text = "one two three four five"
	--            0   4   8     14   19
	local segs = data.segments(text, {
		{ span = { from = 4, to = 13 } }, -- "two three"
		{ span = { from = 8, to = 18 } }, -- "three four"
	})
	local got = {}
	for _, s in segs do
		table.insert(got, `{s.text}=[{joined(s.routes)}]`)
	end
	expect(table.concat(got, "|")).to_be("one =[]|two =[1]|three=[1,2]| four=[2]| five=[]")
end)

test("words shared by routes are striped: each route's colour by turns, nothing lost", function()
	local text = "in cloud tab in settings i switched cloud machines"
	local s2 = data.stripes(text, 2)
	expect(table.concat(s2)).to_be(text)
	expect(#s2 >= 2).to_be(true)
	for _, piece in s2 do
		expect(#piece >= data.STRIPE / 2).to_be(true)
	end
	-- One word, two routes: its letters halved.
	local one = data.stripes("Moscow", 2)
	expect(joined(one)).to_be("Mos,cow")
	-- One route: whole.
	expect(#data.stripes(text, 1)).to_be(1)
end)

test("a sentence routed to two places is underlined in both their colours", function()
	local app = open()
	expect(app:error()).to_be_nil()
	local shared = routes_in(marks(app), SHARED)
	expect(shared["rt-a"]).to_be(true)
	expect(shared["rt-b"]).to_be(true)
	expect(shared["rt-c"]).to_be_nil()
	-- Every route has its chip.
	expect(app:has('#orch-chips >> role:button "Orchestrators, Context, Delivered"')).to_be(true)
	expect(app:has('#orch-chips >> role:button "Exnomic VPN, Context, Delivered"')).to_be(true)
end)

test("either chip of a shared sentence lights it, in that chip's colour", function()
	local app = open()
	app:hover('#orch-chips >> role:button "Exnomic VPN, Context, Delivered"')
	app:frames(5)
	expect(app:state("orch_receipt_hovered")).to_be("rt-b")
	expect(joined(washed(app))).to_be(`rt-b:{SHARED}`)
	app:hover('#orch-chips >> role:button "Orchestrators, Context, Delivered"')
	app:frames(5)
	expect(app:state("orch_receipt_hovered")).to_be("rt-a")
	expect(joined(washed(app))).to_be(`rt-a:{SHARED}`)
end)

test("the pointer on a shared sentence lights both its chips", function()
	local app = open()
	app:hover('#control-capture >> .orch-part >> "switched "')
	app:frames(20)
	expect(app:state("orch_receipt_hovered")).to_be("rt-a,rt-b")
	-- Off it: neither.
	app:hover('#control-history-close')
	app:frames(20)
	expect(app:state("orch_receipt_hovered")).to_be("")
end)

test("parts overlapping in part: both underline the overlap, each its own words, both chips light", function()
	local app = open()
	-- The second sentence's pieces, in order: rt-c's own words, the
	-- overlap striped rt-c then rt-d, rt-d's own words.
	local second = {}
	for _, m in marks(app) do
		if string.match(m, "^rt%-[cd]:") then
			table.insert(second, m)
		end
	end
	expect(table.concat(second, "|")).to_be(`rt-c:{ONLY_C}|rt-c:not selectable |rt-d:in the chat page|rt-d:{ONLY_D}`)
	-- The later part's chip lights its own words and the overlap.
	app:hover('#orch-chips >> role:button "Exnomic VPN, Task, Delivered"')
	app:frames(5)
	expect(joined(washed(app))).to_be(`rt-d:{OVERLAP},rt-d:{ONLY_D}`)
	-- The pointer on the overlap lights both chips; on its own words, one.
	app:hover('#control-capture >> .orch-part >> "chat "')
	app:frames(20)
	expect(app:state("orch_receipt_hovered")).to_be("rt-c,rt-d")
	-- (Away first: the word's card hangs over the line above, "zeron"'s.)
	app:hover("#control-history-close")
	app:wait("#no-such-node", { timeout_ms = 600 })
	app:hover('#control-capture >> .orch-part >> "zeron "')
	app:frames(20)
	expect(app:state("orch_receipt_hovered")).to_be("rt-d")
end)
