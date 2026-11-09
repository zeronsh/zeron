-- Zeron's panel fit, the policy alone (no state): its widths and which
-- columns a window has room for. screens/shell/fit holds the window's and
-- reads it; see there. (crates/ui settings.rs, shell/panel_layout.rs
-- `horizontal_panel_fit`, shell.rs `content_split`, `right_pane_max_width`,
-- `right_pane_takeover_width`.)

local M = {}

-- Zeron's widths (crates/ui settings.rs), in points. (The sidebar's default
-- a step wider than Zeron's 256: Gaël's.)
M.SIDEBAR_MIN, M.SIDEBAR_MAX, M.SIDEBAR_DEFAULT = 242, 400, 272
M.FILES_PANEL_MIN, M.FILES_PANEL_DEFAULT, M.FILES_PANEL_MAX = 220, 286, 440
M.RIGHT_PANE_MIN, M.RIGHT_PANE_DEFAULT = 360, 520
M.CHAT_PANEL_MIN = 300
-- A chat's card at the page's least: the window's three gaps round it (each
-- side, and before the panel) come out of CHAT_PANEL_MIN. What a split's panes
-- each want beside the two sides: it leaves a draft 93 points of the
-- composer's line beside its controls (45 at 240, 5 at 200, then the line
-- opens out with its controls overlapping) and the reply a 240-point column.
M.PANE_MIN = 288
-- What stands round a split's panes beside the sidebar and the panel: the window's
-- edge each side (5) and the gaps after the sidebar and before the panel (4).
M.PAGE_EDGES = 18

export type Column = { open: boolean, at: number }
export type Fit = { sidebar: boolean, right: boolean, files: boolean, sidebar_limit: number }

-- What the page wants: `across` panes side by side (panes stacked in a column
-- are one across), each at its least, the gaps between them and the window's
-- round them. One across is the chat's least.
function M.page_floor(across: number, gap: number): number
	if across <= 1 then
		return M.CHAT_PANEL_MIN
	end
	return M.PAGE_EDGES + across * M.PANE_MIN + (across - 1) * gap
end

-- What the page asks of the sidebar: its panes no narrower than the panel
-- beside them. The sidebar gives way before they are, though they can narrow
-- to their least (`PANE_MIN`) and past it.
function M.page_comfort(page_floor: number): number
	if page_floor <= M.CHAT_PANEL_MIN then
		return page_floor
	end
	-- (`gap` is small beside a pane: the count comes out whole.)
	local across = math.round((page_floor - M.PAGE_EDGES) / M.PANE_MIN - 0.02)
	return page_floor + across * (M.RIGHT_PANE_MIN - M.PANE_MIN)
end

-- Whether the page has what it wants beside columns this wide.
function M.panes_fit(viewport: number, page_floor: number, sidebar: number, panel: number): boolean
	return page_floor + sidebar + panel <= viewport
end

-- The policy: which of the columns show in a window `viewport` wide, the
-- sidebar preferring `sidebar_width`, the panel expanded or not. (`files_min`:
-- Files' least, Zeron's unless an app's Files column is wider.)
function M.panels(
	viewport: number,
	sidebar_width: number,
	sidebar: Column,
	right: Column,
	files: Column,
	expanded: boolean,
	files_min: number?
): Fit
	local FILES = files_min or M.FILES_PANEL_MIN
	local names = { "sidebar", "right", "files" }
	local open = { sidebar.open, right.open, files.open }
	local at = { sidebar.at, right.at, files.at }
	viewport = math.max(viewport, 0)
	-- Expanded spends only the chat's width on the panel: the other columns
	-- fit as ever. Below the chat's and the panel's least, the panel alone
	-- may still take over.
	local narrow_takeover = expanded and right.open and viewport < M.CHAT_PANEL_MIN + M.RIGHT_PANE_MIN
	if narrow_takeover then
		open[1], open[3] = false, false
	end
	while true do
		local chat_floor = if open[2] and narrow_takeover then 0 else M.CHAT_PANEL_MIN
		local minimum = chat_floor
			+ (if open[1] then M.SIDEBAR_MIN else 0)
			+ (if open[2] then M.RIGHT_PANE_MIN else 0)
			+ (if open[3] then FILES else 0)
		if minimum <= viewport then
			return {
				sidebar = open[1],
				right = open[2],
				files = open[3],
				sidebar_limit = if open[1]
					then math.min(
						viewport
							- chat_floor
							- (if open[2] then M.RIGHT_PANE_MIN else 0)
							- (if open[3] then FILES else 0),
						sidebar_width
					)
					else 0,
			}
		end
		-- The least recently opened goes; an expanded panel after the rest.
		local victim: number? = nil
		local best: { number }? = nil
		for i = 1, 3 do
			if open[i] then
				local key = { if expanded and names[i] == "right" then 1 else 0, at[i] }
				if best == nil or key[1] < best[1] or (key[1] == best[1] and key[2] < best[2]) then
					victim, best = i, key
				end
			end
		end
		if victim == nil then
			return { sidebar = false, right = false, files = false, sidebar_limit = 0 }
		end
		open[victim] = false
	end
end

-- The panel's share of `available` (what the tools leave the chat and the
-- panel): half, the panel keeping its least and the page as much of what it
-- wants (`page_floor`, one chat's least unless it's split) as fits.
function M.content_split(available: number, page_floor: number?): number
	return math.min(math.max(available / 2, M.RIGHT_PANE_MIN), math.max(available - (page_floor or M.CHAT_PANEL_MIN), M.RIGHT_PANE_MIN))
end

-- The most the panel's part beside its tree may take: what leaves the page
-- what it wants, but never under the panel's least. Past that the panes narrow.
function M.panel_room(viewport: number, sidebar: number, tree: number, page_floor: number): number
	return math.max(viewport - sidebar - tree - page_floor, M.RIGHT_PANE_MIN - tree)
end

-- The widest the panel may be dragged: the chat keeps its least.
function M.right_pane_max_width(viewport: number, sidebar: number, chat_floor: number): number
	return math.max(viewport - sidebar - chat_floor, 0)
end

-- The panel expanded: all of the chat's room.
function M.right_pane_takeover_width(viewport: number, sidebar: number): number
	return math.max(viewport - sidebar, 0)
end

return M
