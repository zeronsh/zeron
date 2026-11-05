-- The window's fit (screens/shell/fit_policy): Zeron's policy (crates/ui
-- shell/panel_layout.rs `horizontal_panel_fit`, shell.rs `content_split`,
-- `right_pane_max_width`), its cases as Zeron's own tests have them
-- (shell.rs `right_pane_ceiling_preserves_the_chat_floor`,
-- `right_pane_takeover_consumes_the_chat_column`; panel_interaction_tests
-- `panels_share_space_…`: their widths and which column a narrow window
-- hides).

local fit = require("../screens/shell/fit_policy")

local function col(open: boolean, at: number): fit.Column
	return { open = open, at = at }
end
local CLOSED = col(false, 0)

test("the panel's ceiling keeps the chat's least", function()
	expect(fit.right_pane_max_width(1200, 256, fit.CHAT_PANEL_MIN)).to_be(644)
	expect(1200 - 256 - 644).to_be(fit.CHAT_PANEL_MIN)
	-- The chat's least wins over the panel's own when the window is narrow.
	expect(fit.right_pane_max_width(800, 256, fit.CHAT_PANEL_MIN)).to_be(244)
end)

test("expanded, the panel takes all of the chat's room", function()
	expect(fit.right_pane_takeover_width(1200, 256)).to_be(944)
end)

test("the panel's share is half of what the tools leave, within the least", function()
	-- (Zeron's lifecycle test: 1600 wide, sidebar 300, Files 377 / 286.)
	expect(fit.content_split(1600 - 377)).to_be(611.5)
	expect(fit.content_split(1600 - 300 - 377)).to_be(461.5)
	expect(fit.content_split(1600 - 300 - 286)).to_be(507)
	-- 1000 wide: half would be under the panel's least.
	expect(fit.content_split(1000 - 300)).to_be(360)
end)

test("everything open fits a wide window, the sidebar at its own width", function()
	local f = fit.panels(1600, 300, col(true, 1), col(true, 2), col(true, 3), false)
	expect(f.sidebar and f.right and f.files).to_be(true)
	expect(f.sidebar_limit).to_be(300)
end)

test("a narrow window hides the column opened longest ago, not the others", function()
	-- Right opened, then Files, then the sidebar: Files is the oldest of
	-- those that must give way (Zeron's 1000-wide step).
	local f = fit.panels(1000, 300, col(true, 5), col(true, 4), col(true, 3), false)
	expect(f.sidebar).to_be(true)
	expect(f.right).to_be(true)
	expect(f.files).to_be(false)
	-- Files asked for (ranked newest): it comes back and the panel goes.
	f = fit.panels(1000, 300, col(true, 5), col(true, 4), col(true, 6), false)
	expect(f.files).to_be(true)
	expect(f.right).to_be(false)
	expect(f.sidebar).to_be(true)
end)

test("short of room, the sidebar narrows to what's left before it hides", function()
	-- (Zeron's 1130-wide step: 1130 - 300 - 360 - 220.)
	local f = fit.panels(1130, 300, col(true, 1), col(true, 2), col(true, 3), false)
	expect(f.sidebar).to_be(true)
	expect(f.sidebar_limit).to_be(250)
	-- Never wider than its own width.
	f = fit.panels(2000, 260, col(true, 1), col(true, 2), CLOSED, false)
	expect(f.sidebar_limit).to_be(260)
end)

test("an expanded panel goes last, whatever its age", function()
	-- 300 + 242 + 360 = 902 > 900: one must go; the panel is older but
	-- expanded, so the sidebar hides.
	local f = fit.panels(900, 256, col(true, 5), col(true, 1), CLOSED, true)
	expect(f.right).to_be(true)
	expect(f.sidebar).to_be(false)
	-- Not expanded, the older one goes.
	f = fit.panels(900, 256, col(true, 5), col(true, 1), CLOSED, false)
	expect(f.right).to_be(false)
	expect(f.sidebar).to_be(true)
end)

test("expanded in a window too narrow for chat and panel, the panel alone shows", function()
	local f = fit.panels(600, 256, col(true, 5), col(true, 1), col(true, 2), true)
	expect(f.right).to_be(true)
	expect(f.sidebar).to_be(false)
	expect(f.files).to_be(false)
	-- Narrower than the panel's own least: nothing.
	f = fit.panels(300, 256, col(true, 5), col(true, 1), CLOSED, true)
	expect(f.right).to_be(false)
end)

test("closed columns take no room, and nothing open hides nothing", function()
	local f = fit.panels(700, 256, col(true, 1), CLOSED, CLOSED, false)
	expect(f.sidebar).to_be(true)
	expect(f.sidebar_limit).to_be(256)
	f = fit.panels(500, 256, CLOSED, CLOSED, CLOSED, false)
	expect(f.sidebar or f.right or f.files).to_be(false)
	-- Too narrow for the sidebar beside the chat's least: it hides.
	f = fit.panels(500, 256, col(true, 1), CLOSED, CLOSED, false)
	expect(f.sidebar).to_be(false)
end)

-- A split's panes (screens/shell/fit, `other_gives_way`): what they want.
local GAP = 4
local function floor(n: number): number
	return fit.page_floor(n, GAP)
end

test("the page wants its panes' least and the gaps; one pane is the chat's least", function()
	expect(floor(1)).to_be(fit.CHAT_PANEL_MIN)
	expect(floor(2)).to_be(598)
	expect(floor(3)).to_be(890)
	expect(floor(4)).to_be(1182)
	expect(floor(5)).to_be(1474)
	expect(floor(6)).to_be(1766)
	-- What each of two or more has at that: what stands round them and the gaps
	-- between them out of it, shared.
	for n = 2, 6 do
		expect((floor(n) - fit.PAGE_EDGES - (n - 1) * GAP) / n).to_be(fit.PANE_MIN)
	end
end)

test("both sides open: 1 to 6 panes at every window, they fit or one side must go", function()
	local PANEL = fit.RIGHT_PANE_MIN
	for n = 1, 6 do
		for _, w in { 760, 900, 1100, 1280, 1440, 1728, 2000, 2560 } do
			local want = floor(n) + fit.SIDEBAR_DEFAULT + PANEL <= w
			expect(`{n} at {w}: {fit.panes_fit(w, floor(n), fit.SIDEBAR_DEFAULT, PANEL)}`).to_be(`{n} at {w}: {want}`)
		end
	end
	-- The reported windows.
	expect(fit.panes_fit(1100, floor(2), 256, PANEL)).to_be(false)
	expect(fit.panes_fit(1440, floor(2), 256, PANEL)).to_be(true)
	expect(fit.panes_fit(1440, floor(3), 256, PANEL)).to_be(false)
end)

test("the fit itself takes no account of panes: a window narrowing keeps its old rule", function()
	-- Everything still fits a chat's least: nothing hides, whatever the split.
	local f = fit.panels(1000, 256, col(true, 1), col(true, 2), CLOSED, false)
	expect(f.sidebar and f.right).to_be(true)
end)

test("the panel's share leaves the panes what they want, and keeps its own least", function()
	-- 1100, no sidebar: half of 812 would leave them under 598.
	expect(fit.content_split(812, floor(2))).to_be(360)
	-- Plenty: half.
	expect(fit.content_split(2000, floor(2))).to_be(1000)
	-- Between: what the panes leave it.
	expect(fit.content_split(1100, floor(2))).to_be(502)
	-- One pane is the chat's.
	expect(fit.content_split(700, floor(1))).to_be(fit.content_split(700))
	-- Too little for the panes: its least, and they narrow.
	expect(fit.content_split(812, floor(3))).to_be(360)
end)

test("the most the panel takes leaves the panes what they want, down to its least", function()
	expect(fit.panel_room(1440, 256, 0, floor(2))).to_be(586)
	expect(fit.panel_room(1100, 0, 0, floor(2))).to_be(502)
	-- Too little: its least, whatever the panes want; its tree counted in it.
	expect(fit.panel_room(1100, 0, 0, floor(3))).to_be(360)
	expect(fit.panel_room(1100, 0, 288, floor(3))).to_be(72)
end)
