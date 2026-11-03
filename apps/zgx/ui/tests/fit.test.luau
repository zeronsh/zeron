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
