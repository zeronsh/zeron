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
-- The chats side by side in the page's split (screens/shell/split
-- keeps the list). Their panel is one: open for any of them, it stays open
-- as the focus moves between them, at its one width, so nothing shifts;
-- opened or closed, it is for all of them.
local split_grouped = ui.shared("split.grouped", {} :: { string })
-- The sessions whose panel is this one's: its split's, else it alone.
local function sharing(id: string, peek: boolean?): { string }
	local g = if peek then split_grouped:peek() else split_grouped:get()
	if table.find(g, id) then
		return g
	end
	return { id }
end
M.files = ui.computed(function(): boolean
	local id = session()
	if id == nil then
		return false
	end
	local open = M.panels:get()
	for _, member in sharing(id) do
		if open[member] == true then
			return true
		end
	end
	return false
end)
function M.set_files(on: boolean)
	local id = owner(route.chat:peek())
	if not id then
		return
	end
	local members = sharing(id, true)
	local now = table.clone(M.panels:peek())
	for _, member in members do
		now[member] = if on then true else nil
	end
	M.panels:set(now)
	-- (Closed, it isn't expanded: reopening over a chat gone from view
	-- reads as a broken chat, as the shipping app found.)
	if not on then
		for _, member in members do
			if M.expanded_panels:peek()[member] then
				M.expanded_panels:set_key(member, nil)
			end
		end
	end
end
function M.toggle_files()
	M.set_files(not M.files:peek())
end
-- (Open by its split's panel counts: expanding a chat whose own was
-- never opened, its split's being, works as its own would.)
M.expanded = ui.computed(function(): boolean
	local id = session()
	return id ~= nil and M.files:get() and M.expanded_panels:get()[id] == true
end)
function M.toggle_expanded()
	local id = owner(route.chat:peek())
	if not id or not M.files:peek() then
		return
	end
	M.expanded_panels:set_key(id, if M.expanded:peek() then nil else true)
end
-- The command palette (⌘K), open.
M.palette = ui.state("shell_palette_open", false)
-- New project's palette, open (screens/shell/add_project).
M.add_project = ui.state("shell_add_project_open", false)

return M
