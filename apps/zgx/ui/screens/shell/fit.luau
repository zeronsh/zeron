-- Which columns the window has room for, Zeron's way (crates/ui
-- shell/panel_layout.rs `horizontal_panel_fit`, shell.rs `content_split`,
-- `right_pane_max_width`, the minima in settings.rs). Zeron's three columns
-- beside the chat are the sidebar, its docked Files explorer and its
-- surface host ("Right"). Here the explorer lives inside the session's
-- panel (its file tab's tree, docked beside the file): the panel is Right,
-- and Files is its docked tree, open whenever the panel is and ranked just
-- before it. The panel's width is the explorer's allowance (while the fit
-- shows Files) and Right's share beside it; Files hidden for room, the
-- panel narrows to Right's share and the tree floats over the file
-- instead (ds/files_pane's drawer), hidden, not closed.
--
-- When the open columns' least widths don't fit beside the chat's least,
-- the one opened longest ago hides first (an expanded panel last: its
-- expansion is the person's choice to give it the chat's room). Hidden
-- isn't closed: it keeps its open flag (a session's panel per session), its
-- tabs and its width, and comes back as the window widens. Asking for a
-- hidden one (its toggle, ⌘B, ⌘R) brings it back, the least recent of
-- the others hiding for it, instead of closing it. Before the sidebar
-- hides, it narrows to what's left (`sidebar_limit`).
--
-- A split's panes want their least (`page_floor`). Three columns at most
-- (Wing's cap): when the panes wouldn't have it with both sides open, the
-- side asked for longest ago gives way, hidden by the fit, not closed, and
-- back by itself as soon as there's room again (the other side closed, a
-- pane gone, the window wider), as Gaël asked. What the person closed stays
-- closed. A window too narrow for them: the panel's width gives way first,
-- to its least (screens/shell/page), then the sidebar hides before the panes
-- go narrower than the panel, coming back with the room. The panel never
-- grows by itself beside a split: new room is the panes'.
--
--   fit.current:get()     { sidebar, right, files, sidebar_limit }, now
--   fit.page_floor:get()  what the page wants beside the columns
--   fit.toggle_sidebar()  fit.toggle_panel()
--   fit.panels(viewport, sidebar_width, sidebar, right, files, expanded)
--                         the policy alone, each column { open, opened_at }

local route = require("@app/screens/shell/route")
local view = require("@app/screens/shell/view")

local M = {}

-- The policy and its widths (screens/shell/fit_policy), here too.
local policy = require("@app/screens/shell/fit_policy")
M.SIDEBAR_MIN, M.SIDEBAR_MAX, M.SIDEBAR_DEFAULT = policy.SIDEBAR_MIN, policy.SIDEBAR_MAX, policy.SIDEBAR_DEFAULT
M.FILES_PANEL_MIN, M.FILES_PANEL_DEFAULT, M.FILES_PANEL_MAX = policy.FILES_PANEL_MIN, policy.FILES_PANEL_DEFAULT, policy.FILES_PANEL_MAX
M.RIGHT_PANE_MIN, M.RIGHT_PANE_DEFAULT = policy.RIGHT_PANE_MIN, policy.RIGHT_PANE_DEFAULT
M.CHAT_PANEL_MIN = policy.CHAT_PANEL_MIN
M.panels = policy.panels
M.floor_of = policy.page_floor
M.panel_room = policy.panel_room
M.content_split = policy.content_split
M.right_pane_max_width = policy.right_pane_max_width
M.right_pane_takeover_width = policy.right_pane_takeover_width
export type Column = policy.Column
export type Fit = policy.Fit

-- What a file tab's tree adds to the panel's width: nothing. It stands beside the
-- file inside the panel, whatever the panel's width.
M.TREE = 0

-- When each column was last opened (asked for): an order, newest highest.
-- (Kept across launches, so what gave way for room before a relaunch is
-- what gives way after it.)
local sequence = ui.persist("shell_panel_sequence", 1)
M.sidebar_at = ui.persist("shell_sidebar_opened_at", 1)
M.panel_at = ui.persist("shell_panel_opened_at", {} :: { [string]: number })
-- Whose panel: the session's, or a plugin's page's (`page:<plugin>/<id>`).
local function session(): string?
	local c = route.chat:peek()
	if c.name == "plugin" then
		return `page:{c.plugin}/{c.page}`
	end
	if c.name == "home" then
		return "home"
	end
	return if c.name == "session" then c.session else nil
end
local function stamp(): number
	local n = sequence:peek() + 1
	sequence:set(n)
	return n
end
-- `column` asked for now.
function M.record(column: "sidebar" | "right")
	if column == "sidebar" then
		M.sidebar_at:set(stamp())
	else
		local id = session()
		if id then
			M.panel_at:set_key(id, stamp())
		end
	end
end

-- The window's width (the shell's root, as it lays out), and the sidebar's
-- preferred width (set by the shell's sidebar: its own, kept across
-- launches).
M.viewport = ui.state("shell_viewport_w", 0)
-- The window laid out at a new width (the shell's root). The columns
-- follow a resize at once, as a window edge dragged should: each knows it
-- by the width having moved since it last looked (screens/shell/page,
-- init's `sidebar_direct`), never by when.
function M.set_viewport(w: number)
	if math.abs(M.viewport:peek() - w) > 0.5 then
		M.viewport:set(w)
	end
end
-- (Registering moves `registered`, which the fit reads: a sidebar that
-- says its width after the fit was first worked out (a reload of either,
-- or the shell asking first) is heard at once. A plain local wasn't: the
-- fit kept capping the sidebar at the default, or at an old module's
-- width, until the route moved, so it jumped on a session's click.)
local preferred: (() -> number)? = nil
local registered = ui.state("shell_fit_sidebar_registered", 0)
function M.sidebar_width(read: () -> number)
	preferred = read
	registered:set(registered:peek() + 1)
end
-- What the page wants beside the columns: said by the page that lays it out
-- (a template's without a split says nothing: one chat's least).
local page_floor_of: (() -> number)? = nil
function M.page_holds(read: () -> number)
	page_floor_of = read
	registered:set(registered:peek() + 1)
end
M.page_floor = ui.computed(function(): number
	registered:get()
	return if page_floor_of then page_floor_of() else M.CHAT_PANEL_MIN
end)
-- The window's width where the sidebar was asked for though a split's panes
-- then narrow: a toggle always shows it. It gives way again only under this
-- width; room for it all forgets it.
local sidebar_kept = ui.state("shell_fit_sidebar_kept", nil :: number?)
-- The fit now. (Nothing hides before the window's width is known.)
M.current = ui.computed(function(): Fit
	local vw = M.viewport:get()
	registered:get()
	local open_sidebar = not view.collapsed:get()
	local width = if preferred then preferred() else M.SIDEBAR_DEFAULT
	local c = route.chat:get()
	-- (A plugin's page's panel is its own, ranked by its key.)
	local id = if c.name == "session" then c.session elseif c.name == "plugin" then `page:{c.plugin}/{c.page}` elseif c.name == "home" then "home" else nil
	-- (Held under settings: the panel stands beside them as it was, in v3.)
	local on_session = c.name == "session"
	local on_page = c.name == "plugin"
	local on_home = c.name == "home"
	local open_panel = (on_session or on_page or on_home) and view.files:get()
	-- (A page has no files to dock a tree for.)
	local open_tree = on_session and view.files:get()
	if vw <= 0 then
		return { sidebar = open_sidebar, right = open_panel, files = false, sidebar_limit = width }
	end
	local floor = M.page_floor:get()
	local panel_at = if id then M.panel_at:get()[id] or 0 else 0
	-- Too little room for a split's panes as wide as the panel beside both
	-- sides: the side asked for longest ago gives way, hidden, not closed, and
	-- comes back with the room. The sidebar asked for last keeps its place
	-- while the panes have room beside it alone (or it was asked for here
	-- even so: `sidebar_kept`), the panel giving way; otherwise the sidebar
	-- gives way, as before the panes narrow past it. (Not over an expanded
	-- panel.)
	if
		open_sidebar
		and floor > M.CHAT_PANEL_MIN
		and not view.expanded:get()
		and not policy.panes_fit(vw, policy.page_comfort(floor), width, if open_panel then M.RIGHT_PANE_MIN else 0)
	then
		local kept = sidebar_kept:get()
		local keep = kept ~= nil and vw >= kept
		local sidebar_last = M.sidebar_at:get() > panel_at
		if open_panel and sidebar_last and (keep or policy.panes_fit(vw, policy.page_comfort(floor), width, 0)) then
			open_panel = false
		elseif not keep or open_panel then
			open_sidebar = false
		end
	end
	-- (Beside a split its tree floats: docking it as the window widened grew the
	-- panel by its width, out of the room that's the panes'.)
	if floor > M.CHAT_PANEL_MIN then
		open_tree = false
	end
	return M.panels(
		vw,
		width,
		{ open = open_sidebar, at = M.sidebar_at:get() },
		{ open = open_panel, at = panel_at },
		-- (Its docked tree first of all: it gives way by floating over the
		-- file, not by closing, so it never costs the sidebar its place.
		-- Ranked by when it opened, as Zeron's separate Files column is, it
		-- hid the sidebar in windows with room for sidebar, chat and panel,
		-- and the panel then opened at the sidebar's share too.)
		{ open = open_tree, at = -math.huge },
		view.expanded:get(),
		M.TREE
	)
end)

-- The panel's room for its docked tree (Files' allowance), while the fit
-- shows it; and the panel's width from its share: that and Right's share
-- of what the sidebar and the tree leave the chat and it.
function M.files_part(f: Fit): number
	return if f.files then M.TREE else 0
end
function M.panel_share(f: Fit, viewport: number, sidebar: number): number
	local files = M.files_part(f)
	return files + M.content_split(math.max(viewport - sidebar - files, 0), M.page_floor:peek())
end
-- The widest the panel may be (dragged, or kept from before).
function M.panel_max(f: Fit, viewport: number, sidebar: number): number
	return M.files_part(f) + M.panel_room(viewport, sidebar, M.files_part(f), M.page_floor:peek())
end

-- The width the sidebar lays out at: its limit while it shows; while it
-- folds or the fit hides it, the last it had (its edge clips it as it
-- goes, nothing in it laid out again: Zeron's `fit_exit_content`).
local held_room: number? = nil
M.sidebar_room = ui.computed(function(): number
	local f = M.current:get()
	if f.sidebar then
		held_room = f.sidebar_limit
		return f.sidebar_limit
	end
	return held_room or (if preferred then preferred() else M.SIDEBAR_DEFAULT)
end)

-- The sidebar asked for at a width: forgotten once its panes have room, so it
-- gives way the next time they don't.
ui.effect(function()
	local kept = sidebar_kept:get()
	if kept == nil then
		return
	end
	local width = if preferred then preferred() else M.SIDEBAR_DEFAULT
	if policy.panes_fit(M.viewport:get(), policy.page_comfort(M.page_floor:get()), width, if view.files:get() then M.RIGHT_PANE_MIN else 0) then
		sidebar_kept:set(nil)
	end
end)

-- Asked for while hidden for room: ranked newest, so it comes back (another
-- hiding for it); true when it does. Still hidden (no room even so), its
-- rank stays and the caller closes it as asked.
local function promote(column: "sidebar" | "right"): boolean
	local shown = function(): boolean
		local f = M.current:peek()
		return if column == "sidebar" then f.sidebar else f.right
	end
	if shown() then
		return false
	end
	local was_seq, was_at = sequence:peek(), if column == "sidebar" then M.sidebar_at:peek() else nil
	local id = session()
	local was_panel = if id then M.panel_at:peek()[id] else nil
	M.record(column)
	if shown() then
		return true
	end
	sequence:set(was_seq)
	if column == "sidebar" then
		M.sidebar_at:set(was_at :: number)
	elseif id then
		M.panel_at:set_key(id, was_panel)
	end
	return false
end

-- The panel opened by anything (`view.set_files`: its toggle, a chat's file
-- link, ⌘E): ranked newest if it's hidden, as its toggle does, so what asks
-- for it sees it, the other side giving way for it if it must (hidden by
-- the fit, back once it closes). (`was_open`, showing: nothing's opened.)
function M.panel_opened(was_open: boolean)
	if was_open and M.current:peek().right then
		return
	end
	-- (Asked for after the sidebar: the sidebar kept at this width gives way.)
	sidebar_kept:set(nil)
	promote("right")
end

-- The sidebar's toggle (the title bar's, ⌘B): hidden for room, brought
-- back; else folded or opened, opening it ranked newest.
function M.toggle_sidebar()
	if not view.collapsed:peek() then
		-- Hidden for a split's panes: asked for, it shows, the panel giving
		-- way if it must (hidden, back once the sidebar folds), and the panes
		-- narrow if that's not room enough.
		if not M.current:peek().sidebar and M.page_floor:peek() > M.CHAT_PANEL_MIN and not view.expanded:peek() then
			M.record("sidebar")
			sidebar_kept:set(M.viewport:peek())
			return
		end
		if promote("sidebar") then
			return
		end
	end
	local open = view.collapsed:peek()
	view.collapsed:set(not open)
	if open then
		M.record("sidebar")
		sidebar_kept:set(M.viewport:peek())
	end
end

-- The session's panel's toggle (the title bar's Panel, ⌘R): the same.
function M.toggle_panel()
	if view.files:peek() and promote("right") then
		sidebar_kept:set(nil)
		return
	end
	local open = not view.files:peek()
	view.set_files(open)
	if open then
		M.record("right")
	end
end

return M
