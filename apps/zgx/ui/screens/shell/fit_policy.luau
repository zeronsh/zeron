-- Zeron's panel fit, the policy alone (no state): its widths and which
-- columns a window has room for. screens/shell/fit holds the window's and
-- reads it; see there. (crates/ui settings.rs, shell/panel_layout.rs
-- `horizontal_panel_fit`, shell.rs `content_split`, `right_pane_max_width`,
-- `right_pane_takeover_width`.)

local M = {}

-- Zeron's widths (crates/ui settings.rs), in points.
M.SIDEBAR_MIN, M.SIDEBAR_MAX, M.SIDEBAR_DEFAULT = 242, 400, 256
M.FILES_PANEL_MIN, M.FILES_PANEL_DEFAULT, M.FILES_PANEL_MAX = 220, 286, 440
M.RIGHT_PANE_MIN, M.RIGHT_PANE_DEFAULT = 360, 520
M.CHAT_PANEL_MIN = 300

export type Column = { open: boolean, at: number }
export type Fit = { sidebar: boolean, right: boolean, files: boolean, sidebar_limit: number }

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
-- panel): half, the panel keeping its least and the chat as much of its own
-- as fits.
function M.content_split(available: number): number
	return math.min(math.max(available / 2, M.RIGHT_PANE_MIN), math.max(available - M.CHAT_PANEL_MIN, M.RIGHT_PANE_MIN))
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
