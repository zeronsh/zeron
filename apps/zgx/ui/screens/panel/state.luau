-- The panel's tabs, per session, kept here (module states, made as the
-- module loads) so they outlive the panel's views: switching sessions,
-- a reload of the scripts or a template swapped in finds each session's
-- tabs, its active one and the order they were visited in, as they were.
-- A tab's own state (a file tab's file, a terminal's session) is its
-- kind's, by the tab's id.
--
--   state.of(key)                 a session's { tabs, active?, visited }
--   state.open(key, kind, extra?) a new tab at the end, active: its id
--   state.close(key, id)          gone; the last visited open one becomes
--                                 active (else the first, else the empty page)
--   state.activate(key, id?)      that tab (nil: the empty page)
--   state.move(key, id, to)       to place `to` (from 1)
--
-- `key`: the session's ("chat:<id>"), a project's with none open, or a
-- plugin's page's ("page:<plugin>/<id>": a page has a panel of its own).

local M = {}

export type Kind = "files" | "terminal" | "plugin" | "browser" | "side_chat" | "subagent"
export type Tab = {
	id: string,
	kind: Kind,
	-- A file tab's first file (opened as the tab is made).
	path: string?,
	-- A browser tab's address, as it was last navigated (its page lives in
	-- the host by the tab's id, screens/panel/browser).
	url: string?,
	-- A terminal's number, for its title ("Terminal 2"); the engine's
	-- terminal it shows, on which device, and the command zgx runs for it.
	n: number?,
	terminal: string?,
	device: string?,
	command: string?,
	-- A new terminal's claim: where its bridge leaves the engine's id.
	claim: string?,
	-- A run action's terminal (the panel home's): the project, device,
	-- chat and action it runs ({ space, device, chat, action }), run once
	-- as the tab first shows; its name is the tab's `title`.
	run: { [string]: any }?,
	-- A plugin's tab: which plugin, its own kind, its title and icon, and
	-- what it carries (screens/plugins opens it; the plugin draws it).
	plugin: string?,
	plugin_kind: string?,
	title: string?,
	icon: string?,
	data: any?,
	-- A lane's (a side chat, a subagent: screens/chat/lane_tab): the chat
	-- it shows (a side chat's id, a subagent's doc).
	chat: string?,
}
export type Session = { tabs: { Tab }, active: string?, visited: { string } }

M.sessions = ui.state("panel_sessions", {} :: { [string]: Session })
local next_id = ui.state("panel_next_id", 1)

local EMPTY: Session = { tabs = {}, visited = {} }

-- A new chat with no project's tabs (v3's page asks for it): a panel of its own all the
-- same, with what needs no folder.
M.NO_PROJECT = "no-project"

-- Which session's tabs: its id's, or its project's; a plugin's page's,
-- on one (`page`: `{ plugin, id }`).
function M.key(session: any, project: any, page: any?): string?
	if type(page) == "table" and type(page.plugin) == "string" and type(page.id) == "string" then
		return `page:{page.plugin}/{page.id}`
	end
	if type(session) == "string" then
		return `chat:{session}`
	elseif type(session) == "table" and session.id then
		return `chat:{session.id}`
	elseif type(project) == "string" then
		return `project:{project}`
	elseif type(project) == "table" and project.id then
		return `project:{project.id}`
	end
	return nil
end

-- A session's tabs (tracked).
function M.of(key: string?): Session
	if not key then
		return EMPTY
	end
	return M.sessions:get()[key] or EMPTY
end

local function peek(key: string): Session
	return M.sessions:peek()[key] or EMPTY
end

local function write(key: string, s: Session)
	M.sessions:set_key(key, s)
end

local function copy(s: Session): Session
	return { tabs = table.clone(s.tabs), active = s.active, visited = table.clone(s.visited) }
end

-- (Visited last first, each once.)
local function visit(s: Session, id: string?)
	if not id then
		return
	end
	local i = table.find(s.visited, id)
	if i then
		table.remove(s.visited, i)
	end
	table.insert(s.visited, 1, id)
end

-- The keyboard to a terminal tab a person just opened or turned to: its
-- terminal takes it as it shows (the shipping app's `set_right_active` →
-- `request_focus`, claimed once the body mounts). One showing already
-- takes it at once; one not yet drawn, as it's drawn, within a while.
-- Nothing of this is kept: a reload, or the panel coming back with a
-- session, asks for nothing.
local focusers: { [string]: () -> () } = {}
local wanted: { [string]: number } = {}
local asks = 0
local WANT_MS = 10000

function M.want_keys(key: string, id: string?)
	if not id then
		return
	end
	local tab = M.find(key, id)
	if not tab or tab.kind ~= "terminal" then
		return
	end
	local k = `{key}\0{id}`
	asks += 1
	local mine = asks
	wanted[k] = mine
	-- (After what's asking has run: a panel opening opens first.)
	ui.after(0, function()
		local focus = focusers[k]
		if focus and wanted[k] == mine then
			wanted[k] = nil
			focus()
		end
	end)
	ui.after(WANT_MS, function()
		if wanted[k] == mine then
			wanted[k] = nil
		end
	end)
end

-- A terminal tab's body, drawn: how it takes the keyboard. It takes it now
-- if a person asked for it moments ago. What it gives back lets go.
function M.focuser(key: string, id: string, focus: () -> ()): () -> ()
	local k = `{key}\0{id}`
	focusers[k] = focus
	if wanted[k] then
		wanted[k] = nil
		ui.after(0, focus)
	end
	return function()
		if focusers[k] == focus then
			focusers[k] = nil
		end
	end
end

function M.open(key: string, kind: Kind, extra: { [string]: any }?): string
	local s = copy(peek(key))
	local n = next_id:peek()
	next_id:set(n + 1)
	local id = `tab{n}`
	local tab: Tab = { id = id, kind = kind, path = if extra then extra.path else nil }
	if kind == "plugin" and extra then
		tab.plugin, tab.plugin_kind = extra.plugin, extra.plugin_kind
		tab.title, tab.icon, tab.data = extra.title, extra.icon, extra.data
	end
	if kind == "browser" and extra then
		tab.url = extra.url
	end
	if (kind == "side_chat" or kind == "subagent") and extra then
		tab.chat, tab.title, tab.data = extra.chat, extra.title, extra.data
	end
	if kind == "terminal" and extra then
		-- (A run action's: its name, and what runs it.)
		tab.title, tab.run = extra.title, extra.run
	end
	if kind == "terminal" then
		local count = 0
		for _, t in s.tabs do
			if t.kind == "terminal" then
				count = math.max(count, t.n or 0)
			end
		end
		tab.n = count + 1
	end
	table.insert(s.tabs, tab)
	s.active = id
	visit(s, id)
	write(key, s)
	M.want_keys(key, id)
	return id
end

function M.activate(key: string, id: string?)
	local s = copy(peek(key))
	-- (Turned to again, already in front, as the panel opens on it: the
	-- keyboard all the same.)
	if s.active == id then
		M.want_keys(key, id)
		return
	end
	s.active = id
	visit(s, id)
	ui.batch(function()
		write(key, s)
	end)
	M.want_keys(key, id)
end

-- The tab with `id` (in `key`), if it's still open.
function M.find(key: string, id: string): Tab?
	for _, t in peek(key).tabs do
		if t.id == id then
			return t
		end
	end
	return nil
end

function M.close(key: string, id: string)
	local s = copy(peek(key))
	local at = nil
	for i, t in s.tabs do
		if t.id == id then
			at = i
			break
		end
	end
	if not at then
		return
	end
	table.remove(s.tabs, at)
	local v = table.find(s.visited, id)
	if v then
		table.remove(s.visited, v)
	end
	local was_active = s.active == id
	ui.batch(function()
		if was_active then
			local next_active: string? = nil
			for _, seen in s.visited do
				for _, t in s.tabs do
					if t.id == seen then
						next_active = seen
						break
					end
				end
				if next_active then
					break
				end
			end
			s.active = next_active or (if s.tabs[1] then s.tabs[1].id else nil)
			write(key, s)
		else
			write(key, s)
		end
	end)
	if was_active then
		-- (A terminal in front now: the keyboard, as the shipping app's
		-- `close_right_surface`.)
		M.want_keys(key, s.active)
	end
end

-- A terminal tab's engine terminal, once opened (kept with the tab, so a
-- reload shows the same one at once).
function M.set_terminal(key: string, id: string, terminal: string?, device: string?, command: string?, claim: string?)
	local s = copy(peek(key))
	for i, t in s.tabs do
		if t.id == id then
			local next_tab = table.clone(t)
			next_tab.terminal = terminal
			next_tab.device = device
			next_tab.command = command
			next_tab.claim = claim
			s.tabs[i] = next_tab
		end
	end
	write(key, s)
end

-- A browser tab's address, as its page navigates (kept with the tab, so a
-- reload or a return to the tab finds the page where it went).
function M.set_url(key: string, id: string, url: string?)
	local s = copy(peek(key))
	for i, t in s.tabs do
		if t.id == id and t.url ~= url then
			local next_tab = table.clone(t)
			next_tab.url = url
			s.tabs[i] = next_tab
			write(key, s)
		end
	end
end

function M.move(key: string, id: string, to: number)
	local s = copy(peek(key))
	local from = nil
	for i, t in s.tabs do
		if t.id == id then
			from = i
			break
		end
	end
	if not from then
		return
	end
	local tab = s.tabs[from]
	table.remove(s.tabs, from)
	table.insert(s.tabs, math.clamp(to, 1, #s.tabs + 1), tab)
	write(key, s)
end

return M
