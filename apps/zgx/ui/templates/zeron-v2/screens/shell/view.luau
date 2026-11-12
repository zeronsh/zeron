-- How the main window is laid out, as chosen: the sidebar collapsed away
-- (kept across launches), and each session's panel open or not. The
-- frame reads them; the title bar's toggles, the keys and the palette flip
-- them. Kept here, outside the screens, so a screen made again (its module
-- saved or replaced) comes back as it was.
--
-- A session's panel is its own, as in the shipping app (crates/ui
-- `SessionPanels`): open in one chat, the others stay as they were; back
-- to a chat, it's as you left it. Closed until opened, and remembered for
-- the app's run (a relaunch starts every chat closed). A new chat shows
-- none. Its width is one for all of them, kept across launches (the
-- shipping app's `right_pane_width`: screens/shell/page).
--
--   view.files:get()       whether the session open now has its panel open
--   view.set_files(on)     open or close it, for the session open now
--   view.toggle_files()
--   view.expanded:get()    whether that panel is expanded over the chat
--   view.toggle_expanded() (the panel's Expand button; per session, as
--                          the shipping app's `right_expanded`; closing
--                          the panel leaves it)

local route = require("@app/screens/shell/route")

local M = {}

-- The collapsed sidebar's rail (ds/rail and the sidebar's flyouts: its
-- actions and group tiles in a column at the window's bottom left). Off:
-- collapsed, the sidebar folds away entirely, as Zeron's does, and the
-- title bar's toggle (⌘B) brings it back. `true` mounts it again.
M.rail = false

M.collapsed = ui.persist("shell_collapsed", false)
-- Each session's panel, open (by session id; absent, closed).
M.panels = ui.state("shell_panel_open", {} :: { [string]: boolean })
-- Each session's panel, expanded over its chat (by session id).
M.expanded_panels = ui.state("shell_panel_expanded", {} :: { [string]: boolean })
-- Whose panel the window shows: the session it shows a chat of (settings
-- over it keeps it), or a plugin's page's (`page:<plugin>/<id>`: a page has
-- a panel of its own).
local function owner(c: any): string?
	if c.name == "plugin" and c.plugin and c.page then
		return `page:{c.plugin}/{c.page}`
	end
	return if c.name == "session" then c.session else nil
end
local function session(): string?
	return owner(route.chat:get())
end
M.files = ui.computed(function(): boolean
	local id = session()
	return id ~= nil and M.panels:get()[id] == true
end)
function M.set_files(on: boolean)
	local id = owner(route.chat:peek())
	if not id then
		return
	end
	local now = table.clone(M.panels:peek())
	now[id] = if on then true else nil
	M.panels:set(now)
	-- (Closed, it isn't expanded: reopening over a chat gone from view
	-- reads as a broken chat, as the shipping app found.)
	if not on and M.expanded_panels:peek()[id] then
		M.expanded_panels:set_key(id, nil)
	end
end
function M.toggle_files()
	M.set_files(not M.files:peek())
end
M.expanded = ui.computed(function(): boolean
	local id = session()
	return id ~= nil and M.panels:get()[id] == true and M.expanded_panels:get()[id] == true
end)
function M.toggle_expanded()
	local id = owner(route.chat:peek())
	if not id or not M.files:peek() then
		return
	end
	M.expanded_panels:set_key(id, if M.expanded:peek() then nil else true)
end
-- The sidebar's width as it shows, each frame as it folds or comes back
-- (its column, the window's gap before its card included).
M.sidebar_shown = ui.state("shell_sidebar_shown", 0)
-- Counts the sidebar's edge double-clicked (its width put back).
M.seam_reset = ui.state("shell_seam_reset", 0)
-- The command palette (⌘K), open.
M.palette = ui.state("shell_palette_open", false)
-- New project's palette, open (screens/shell/add_project).
M.add_project = ui.state("shell_add_project_open", false)

return M
