-- Hover cards with actions in the Control room (ds/hover_card Anchored): a
-- part's card hangs from its chip (or its answer line, or the word it's
-- on), never from the pointer, so the pointer reaches it and what's in it;
-- it stays as the pointer crosses into it, a click on the chip pins it, and
-- Escape or a click outside lets it go (under its anchor, or over it where
-- there's no room under it). In Capture's history, opened from
-- its bar, on ui/control_page_fixture.luau (orchestration_sample).

local function open(): any
	local app = mount("control_page_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	app:set_state("control_history_panel", true)
	app:frames(30)
	return app
end

local CHIP = '#control-capture >> role:button "Exnomic VPN, Question, Answered"'
local OTHER = '#control-capture >> role:button "CI speed, Priority, Delivered"'
-- The card as the pointer shows it (zgx's tooltip panel round it), and
-- what's in it.
local HOVER = "hover"
local PINNED = "pinned"
local IN_HOVER = 'role:dialog "Tooltip"'

-- A node's box from the tree (x, y, w, h).
local function box(app: any, target: string): (number?, number?, number?, number?)
	if not app:has(target) then
		return nil, nil, nil, nil
	end
	local line = app:tree(target):match("[^\n]+") or ""
	local x, y, w, h = line:match(" (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
	return tonumber(x), tonumber(y), tonumber(w), tonumber(h)
end
-- The card's panel, showing (drawn, of a size): on hover, zgx's tooltip
-- dialog; pinned, the popover's (named by its chip, so no name of its own).
local function card_box(app: any, kind: string): (number?, number?, number?, number?)
	for line in app:tree():gmatch("[^\n]+") do
		if not line:find("hidden", 1, true) then
			local x, y, w, h
			if kind == HOVER then
				x, y, w, h = line:match(' dialog "Tooltip" (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)')
			else
				x, y, w, h = line:match(" dialog (%-?[%d%.]+),(%-?[%d%.]+) ([%d%.]+)x([%d%.]+)")
			end
			if x and tonumber(w) > 0 and tonumber(h) > 0 then
				return tonumber(x), tonumber(y), tonumber(w), tonumber(h)
			end
		end
	end
	return nil, nil, nil, nil
end
local function shown(app: any, kind: string): boolean
	local x = card_box(app, kind)
	return x ~= nil
end
-- `ms` of the clock, frames delivered meanwhile (a hover card's wait, its
-- grace and its fade run on it).
local function pause(app: any, ms: number)
	app:wait("#no-such-node", { timeout_ms = ms })
end
-- Until `target` shows, faded in (a hover card waits for the pointer to
-- rest), or is gone; 3 s at most.
local function until_shown(app: any, target: string): boolean
	for _ = 1, 150 do
		if shown(app, target) then
			pause(app, 200)
			return shown(app, target)
		end
		pause(app, 20)
	end
	return false
end
local function until_gone(app: any, target: string): boolean
	for _ = 1, 150 do
		if not shown(app, target) then
			return true
		end
		pause(app, 20)
	end
	return false
end
-- A card that hangs from an anchor whose top is `top` and foot `foot`: 6
-- points under it, or (no room under it, as at the foot of the history over
-- Capture's bar) 6 points over it.
local function hangs(y: number?, h: number?, top: number, foot: number): boolean
	if not y then
		return false
	end
	return math.abs(y - (foot + 6)) <= 1 or math.abs(y + (h or 0) + 6 - top) <= 1
end
local function route_at(app: any): string
	return tostring(app:state("shell_route_path") or "")
end

test("hover card: on a route chip, it hangs under the chip, from its start", function()
	local app = open()
	local cx, cy, _, ch = box(app, CHIP)
	expect(cx ~= nil).to_be(true)
	app:hover(CHIP)
	expect(until_shown(app, HOVER)).to_be(true)
	expect(app:error()).to_be_nil()
	-- Its words: where it went, how it stands, the way in.
	expect(app:has(`{IN_HOVER} >> "Exnomic VPN"`)).to_be(true)
	expect(app:has(`{IN_HOVER} >> .orch-part-open`)).to_be(true)
	local x, y, w, h = card_box(app, HOVER)
	expect(w).to_be(480)
	expect(math.abs((x :: number) - (cx :: number)) <= 1).to_be(true)
	expect(hangs(y, h, cy :: number, (cy :: number) + (ch :: number))).to_be(true)
end)

test("hover card: the pointer crosses into it and it stays, where it was", function()
	local app = open()
	app:hover(CHIP)
	expect(until_shown(app, HOVER)).to_be(true)
	local x0, y0 = card_box(app, HOVER)
	-- Off the chip, onto the card (past the gap between them), and about
	-- in it: it stays, and doesn't move.
	app:hover(`{IN_HOVER} >> .orch-part-message`)
	pause(app, 400)
	expect(shown(app, HOVER)).to_be(true)
	local x1, y1 = card_box(app, HOVER)
	expect(x1).to_be(x0)
	expect(y1).to_be(y0)
	app:hover(`{IN_HOVER} >> .orch-part-open`)
	pause(app, 400)
	local x2, y2 = card_box(app, HOVER)
	expect(x2).to_be(x0)
	expect(y2).to_be(y0)
	-- Off both: it goes.
	app:hover("#control-history-toggle")
	expect(until_gone(app, HOVER)).to_be(true)
end)

test("hover card: Open opens where the part went, and the card goes", function()
	local app = open()
	app:hover(CHIP)
	expect(until_shown(app, HOVER)).to_be(true)
	app:click(`{IN_HOVER} >> .orch-part-open`)
	app:frames(20)
	expect(app:error()).to_be_nil()
	expect(route_at(app)).to_be("/orchestrator/orc-vpn")
	expect(shown(app, HOVER)).to_be(false)
	expect(shown(app, PINNED)).to_be(false)
end)

test("hover card: a click on the chip pins it; the pointer leaving or on another chip doesn't take it", function()
	local app = open()
	local _, cy, _, ch = box(app, CHIP)
	app:click(CHIP)
	expect(until_shown(app, PINNED)).to_be(true)
	expect(app:state("hover_card_pinned") ~= "").to_be(true)
	local x0, y0, _, h0 = card_box(app, PINNED)
	expect(hangs(y0, h0, cy :: number, (cy :: number) + (ch :: number))).to_be(true)
	-- The pointer away: it stays.
	app:hover("#control-history-toggle")
	pause(app, 600)
	expect(shown(app, PINNED)).to_be(true)
	-- On another chip: still this one, and no card of that one over it.
	app:hover(OTHER)
	pause(app, 600)
	expect(shown(app, PINNED)).to_be(true)
	expect(shown(app, HOVER)).to_be(false)
	local x1, y1 = card_box(app, PINNED)
	expect(x1).to_be(x0)
	expect(y1).to_be(y0)
	-- Its way in, with the keys: focus is in the card; Tab reaches Open.
	app:key("Tab")
	app:key("Enter")
	app:frames(20)
	expect(route_at(app)).to_be("/orchestrator/orc-vpn")
	expect(shown(app, PINNED)).to_be(false)
	expect(app:state("hover_card_pinned")).to_be("")
end)

test("hover card: pinned, Escape, a click outside, or the chip again lets it go", function()
	local app = open()
	-- Escape.
	app:click(CHIP)
	expect(until_shown(app, PINNED)).to_be(true)
	app:key("Escape")
	expect(until_gone(app, PINNED)).to_be(true)
	expect(app:state("hover_card_pinned")).to_be("")
	-- A click outside.
	app:click(CHIP)
	expect(until_shown(app, PINNED)).to_be(true)
	app:click("#control-history-toggle")
	expect(until_gone(app, PINNED)).to_be(true)
	expect(app:state("hover_card_pinned")).to_be("")
	-- The chip again.
	app:click("#control-history-toggle")
	app:frames(10)
	app:click(CHIP)
	expect(until_shown(app, PINNED)).to_be(true)
	app:click(CHIP)
	expect(until_gone(app, PINNED)).to_be(true)
	expect(app:state("hover_card_pinned")).to_be("")
	expect(app:error()).to_be_nil()
end)

test("hover card: on an answer line, it hangs under the line and its Open works", function()
	local app = open()
	local LINE = "#control-capture >> .orch-answer [1]"
	-- The line's box, from what's on it: its name 14 in (its dot and the
	-- gap), its way in centred on the 24-point row.
	local tx = box(app, `{LINE} >> "Orchestrator screen"`)
	local _, oy = box(app, "#control-capture >> .orch-answer-open [1]")
	expect(tx ~= nil and oy ~= nil).to_be(true)
	local lx, bottom = (tx :: number) - 14, (oy :: number) - 1 + 24
	app:hover(LINE)
	expect(until_shown(app, HOVER)).to_be(true)
	local x, y, _, h = card_box(app, HOVER)
	expect(math.abs((x :: number) - lx) <= 1).to_be(true)
	expect(hangs(y, h, bottom - 24, bottom)).to_be(true)
	app:hover(`{IN_HOVER} >> .orch-part-open`)
	pause(app, 400)
	local x1, y1 = card_box(app, HOVER)
	expect(x1).to_be(x)
	expect(y1).to_be(y)
	app:click(`{IN_HOVER} >> .orch-part-open`)
	app:frames(20)
	expect(route_at(app) ~= "/").to_be(true)
	expect(shown(app, HOVER)).to_be(false)
end)

test("hover card: in the text, it hangs from the word the pointer came to, not from the pointer", function()
	local app = open()
	-- "rebooted" on the second line; under it, on the third, "registry",
	-- another part's: the card hangs from the first over the second.
	local WORD = '#control-capture >> .orch-part >> "rebooted "'
	local wx, wy = box(app, WORD)
	expect(wx ~= nil).to_be(true)
	app:hover(WORD)
	-- (The word's anchor comes under a pointer at rest: the pointer moves
	-- onto it, as a person's does.)
	app:hover("#control-history-toggle")
	app:hover("#control-capture >> .orch-part-anchor")
	expect(until_shown(app, HOVER)).to_be(true)
	local x, y, _, ch = card_box(app, HOVER)
	-- (Over the word, its start under the word's, 6 points clear: under
	-- it, the card would cover the chip the word scrolled into view.)
	expect(math.abs((x :: number) - (wx :: number)) <= 1).to_be(true)
	expect(math.abs((y :: number) + (ch :: number) + 6 - (wy :: number)) <= 1).to_be(true)
	-- On its way to the card, the pointer crosses another part's word: the
	-- card stays where it hangs, and the pointer reaches it.
	app:hover('#control-capture >> .orch-part >> "thing "')
	app:hover(`{IN_HOVER} >> .orch-part-message`)
	pause(app, 500)
	expect(shown(app, HOVER)).to_be(true)
	local x1, y1 = card_box(app, HOVER)
	expect(x1).to_be(x)
	expect(y1).to_be(y)
	app:hover(`{IN_HOVER} >> .orch-part-meta`)
	pause(app, 400)
	local x2, y2 = card_box(app, HOVER)
	expect(x2).to_be(x)
	expect(y2).to_be(y)
	expect(app:error()).to_be_nil()
end)
