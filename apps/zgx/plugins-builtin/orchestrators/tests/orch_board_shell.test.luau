-- The board's frame (ds/orch_board, ui/orch_board_places_fixture.luau):
-- with no card open the columns reach the board's right edge; compact (a
-- narrow page) with a card open, its detail takes the board's place and
-- Back returns to it; whose a card is, its dot, is named.

-- A target's first box shown: x, y, w, h.
local function box(app: any, target: string): (number, number, number, number)
	for line in app:tree(target, 1):gmatch("[^\n]+") do
		if not string.find(line, " hidden", 1, true) then
			local x, y, w, h = string.match(line, " (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
			if x then
				return tonumber(x) :: number, tonumber(y) :: number, tonumber(w) :: number, tonumber(h) :: number
			end
		end
	end
	return 0, 0, 0, 0
end

local function open(w: number, h: number): any
	local app = mount("orch_board_places_fixture.luau")
	app:resize(w, h)
	app:frames(30)
	return app
end

test("board shell: with no card open, the columns reach the board's right edge", function()
	local app = open(1500, 1000)
	local bx, _, bw = box(app, "#orch-board")
	local cx, _, cw = box(app, "#orch-columns")
	expect(math.abs((bx + bw) - (cx + cw)) < 1).to_be(true)
	-- A card open: the detail beside the columns, a gap between.
	app:click("#orch-card-it-chat")
	app:frames(20)
	local cx2, _, cw2 = box(app, "#orch-columns")
	local dx = box(app, "#orch-board-detail")
	expect(dx > cx2 + cw2).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("board shell: compact with a card open, the detail takes the board's place; Back returns", function()
	local app = open(1000, 1400)
	app:set_state("compact", true)
	app:frames(20)
	local _, card_y = box(app, "#orch-card-it-chat")
	app:set_state("board_selected", "it-chat")
	app:frames(20)
	expect(app:error()).to_be_nil()
	-- The detail the board's width; the columns not shown (never 0 wide
	-- beside it).
	local _, _, bw = box(app, "#orch-board")
	local _, _, dw = box(app, "#orch-board-detail")
	expect(dw > 0).to_be(true)
	expect(math.abs(dw - bw) < 1).to_be(true)
	local _, _, cw = box(app, "#orch-columns")
	expect(cw).to_be(0)
	-- Back: the board as it was, the card where it stood.
	expect(app:has("#orch-detail-back")).to_be(true)
	app:click("#orch-detail-back")
	app:frames(20)
	expect(app:state("board_selected")).to_be_nil()
	expect(app:has("#orch-board-detail")).to_be(false)
	local _, _, cw2 = box(app, "#orch-columns")
	expect(cw2 > 0).to_be(true)
	local _, card_y2 = box(app, "#orch-card-it-chat")
	expect(card_y2).to_be(card_y)
	expect(app:error()).to_be_nil()
end)

test("board shell: a card's and a row's owner dot are named for its orchestrator", function()
	local app = open(1500, 1000)
	expect(app:has('#orch-card-it-merge >> role:img "Zeron fixes"')).to_be(true)
	expect(app:has('#orch-card-it-admin >> role:img "Exnomic VPN"')).to_be(true)
	app:set_state("board_view", "list")
	app:frames(20)
	expect(app:has('#orch-row-it-merge >> role:img "Zeron fixes"')).to_be(true)
	expect(app:error()).to_be_nil()
end)
