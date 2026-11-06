-- Where the main window is: a new chat (`home`), a session
-- (`session/:id`) or a page of settings (`settings/:page`), in one
-- `ui.router` with the history behind and ahead (the title bar's arrows,
-- the mouse's buttons and a trackpad's swipe over the page, ⌘[ and ⌘]).
-- The other screens read where it is from here and move it from here:
--
--   route.make()            the router made (the shell, as it's built)
--   route.can_back()        a step back to take (`can_forward`, ahead)
--   route.current:get()     { name = "home" | "session" | "settings",
--                             project?, session?, page? } (ids)
--   route.project:get()     the project worked in: { id, device, path, name },
--                           nil for none (a new chat's choice, or the open
--                           session's)
--   route.session:get()     the session open: { id, device, cwd?, title,
--                           project? }, nil on a new chat or settings
--   route.device:get()      the device it runs on: the session's, the
--                           project's, else this Mac's (an id)
--   route.projects:get()    the projects a new chat can work in, as
--                           ds.Composer takes them ({ value, label, hue? })
--   route.open_session(id)  to that session
--   route.new_chat(project?, device?) to a new chat (in that project, a
--                           space id; "none" for none; nil, the one chosen
--                           last), on that device if given (a device id):
--                           the composer's device picked for it (a project
--                           not there: its namesake there, else the
--                           composer's pick there; `placed`)
--   route.device_pick:get() that device, until the new chat's composer takes
--                           it ({ device }; `take_device()` reads and clears)
--   route.settings(page?)   to settings (a page's id; "general" first)
--   route.leave_settings()  back where the chats were
--   route.plugin_page(plugin, page)  to a plugin's page (`plugin`, `page`:
--                           ids); the chats stay as they were under it
--   route.chat:get()        the chat route (as `current`), held while
--                           settings shows: `project` and `session` follow it
--   route.choose_project(id?) the project a new chat starts in
--
-- What it shows changes in place: each state above keeps its table while
-- what it says is the same, so a session's state moving (working, done)
-- reaches the sidebar's row, not everything that reads where you are.
-- The place survives a reload of the scripts (kept in a named state), and
-- a relaunch too: the window opens where it was left (a session, a new
-- chat in the project chosen, a plugin's page), as it is saved with each
-- move (`M.launched`).

local sessions = require("@account/sessions")
local look = require("@app/screens/shell/project_look")

local M = {}

export type Current = { name: string, project: string?, session: string?, page: string?, plugin: string? }
export type Project = { id: string, device: string?, path: string?, name: string? }
export type Session = { id: string, device: string?, cwd: string?, title: string?, project: string? }
export type Choice = { value: string, label: string, hue: string?, src: string? }

local ROUTES = {
	home = "/",
	session = "/session/:id",
	settings = "/settings/:page",
	plugin = "/plugin/:plugin/:page",
}
-- The router, made by the shell as it's built (`M.make`, from the app's
-- script, which runs again on every reload) rather than as this module
-- loads: a router declares commands and a deep-link handler, which a
-- module kept across a reload couldn't carry. Until it's made, where it
-- is reads the kept path, and a move writes it (the router starts there).
type Route = { name: string, params: { [string]: string }, path: string }
local router: any = nil
-- Bumped as a router is made: what reads where it is follows the new one.
local made = ui.state("shell_route_made", 0)

-- `path` as the router would read it (one it doesn't know is home's).
local function parse(path: string): Route
	local id = string.match(path, "^/session/([^/]+)$")
	if id then
		return { name = "session", params = { id = id }, path = path }
	end
	local page = string.match(path, "^/settings/([^/]+)$")
	if page then
		return { name = "settings", params = { page = page }, path = path }
	end
	local plugin, ppage = string.match(path, "^/plugin/([^/]+)/([^/]+)$")
	if plugin then
		return { name = "plugin", params = { plugin = plugin, page = ppage }, path = path }
	end
	return { name = "home", params = {}, path = "/" }
end
-- Where it was left, across a relaunch: the chats' route as a path (a
-- session, a new chat, a plugin's page; never settings, which is over the
-- chats: their place under it stands), saved as it moves. Nil before any.
local last = ui.persist("shell_last_route", nil :: string?)
-- Where the launch started: where it was left (nil, never saved), read
-- once a launch (a named state: a reload keeps the launch's), so the launch
-- can tell whether it has moved since (`M.launched`).
local function saved(): string?
	local p = last:peek()
	if type(p) ~= "string" then
		return nil
	end
	local r = parse(p)
	return if r.name == "settings" then nil else r.path
end
local start = ui.state("shell_launch_start", saved() or "")
-- Where it is, across a reload of the scripts; a launch starts where it
-- was left, so a session opens on it at once, never on another first.
local kept = ui.state("shell_route_path", if start:peek() ~= "" then start:peek() else "/")
-- Where it is, tracked (`route:get()`), or not (`peek`).
local function here(): Route
	made:get()
	return if router then router.route:get() else parse(kept:get())
end
local function at(): Route
	return if router then router.route:peek() else parse(kept:peek())
end
-- The history behind and ahead, as the router keeps it, kept across a
-- reload of the scripts too (`ui.router` starts again from one place; its
-- history is replayed into the one made after a reload: `M.make`). Each
-- move the router makes is followed here: a step back or forward (the
-- title bar's arrows, ⌘[ ⌘], a swipe), a new place (a path past where it
-- stands, what was ahead dropped), or one in place of where it stands.
type History = { entries: { string }, at: number }
local history = ui.state("shell_route_history", { entries = { kept:peek() }, at = 1 } :: History)
-- The longest it keeps (the oldest let go first).
local HISTORY_MAX = 100
-- (Set as the next move replaces where it stands; and while a history is
-- replayed, its moves aren't new ones.)
local replacing = false
local replaying = false

-- To route `name` (`replace`: in place of where it is).
local function go(name: string, params: { [string]: string }?, replace: boolean?)
	if router then
		if replace then
			replacing = true
			router.replace(name, params)
		else
			router.navigate(name, params)
		end
		return
	end
	local path = (ROUTES :: any)[name] :: string
	for k, v in params or {} do
		path = string.gsub(path, `:{k}`, v)
	end
	kept:set(path)
end

-- The window's router, made as the shell is built (the app's script runs
-- it on every load): history behind and ahead, ⌘[ and ⌘], `zeron://` links.
function M.make()
	-- (The history it had, made again: from its first place, through each
	-- after it, then back to where it stood, so what was ahead is too.)
	local h = history:peek()
	local entries = h.entries
	if #entries == 0 or entries[h.at] ~= kept:peek() then
		entries, h = { kept:peek() }, { entries = { kept:peek() }, at = 1 }
		history:set(h)
	end
	router = ui.router {
		routes = ROUTES,
		initial = entries[1],
		scheme = "zeron",
	}
	replaying = true
	ui.batch(function()
		for i = 2, #entries do
			router.open(entries[i])
		end
		for _ = h.at + 1, #entries do
			router.back()
		end
	end)
	replaying = false
	made:set(made:peek() + 1)
end
-- Each move the router makes, followed.
ui.effect(function()
	local path = here().path
	if not router or replaying then
		return
	end
	local h = history:peek()
	local entries, at = table.clone(h.entries), h.at
	if entries[at] == path then
		return
	end
	if replacing then
		replacing = false
		entries[at] = path
	elseif at > 1 and entries[at - 1] == path and router.can_forward:peek() then
		at -= 1
	elseif at < #entries and entries[at + 1] == path then
		at += 1
	else
		-- A new place: past where it stands, what was ahead dropped.
		for i = #entries, at + 1, -1 do
			table.remove(entries, i)
		end
		table.insert(entries, path)
		at = #entries
		while #entries > HISTORY_MAX do
			table.remove(entries, 1)
			at -= 1
		end
	end
	history:set({ entries = entries, at = at })
end)
-- Its place kept across a reload of the scripts.
ui.effect(function()
	local path = here().path
	if kept:peek() ~= path then
		kept:set(path)
	end
end)
-- The title bar's arrows.
function M.can_back(): boolean
	made:get()
	return router ~= nil and router.can_back:get()
end
function M.can_forward(): boolean
	made:get()
	return router ~= nil and router.can_forward:get()
end

-- The project a new chat starts in (a space id; "none", none).
local chosen = ui.persist("shell_project", nil :: string?)

-- `next`, or `last` where they say the same (so what reads it hears
-- nothing).
local function same(a: any, b: any): boolean
	if type(a) ~= "table" or type(b) ~= "table" then
		return a == b
	end
	for k, v in a do
		if not same(v, b[k]) then
			return false
		end
	end
	for k in b do
		if a[k] == nil then
			return false
		end
	end
	return true
end
local function steady<T>(f: () -> T): any
	local last: any = nil
	return ui.computed(function(): T
		local next = f()
		if last ~= nil and same(last, next) then
			return last
		end
		last = next
		return next
	end)
end

local function chat_of(id: string?): sessions.Chat?
	if not id then
		return nil
	end
	local now = sessions.now:get()
	for _, c in now.chats do
		if c.id == id then
			return c
		end
	end
	-- (An archived one opens too: from Settings, or the palette.)
	for _, c in now.archived or {} do
		if c.id == id then
			return c
		end
	end
	return nil
end
local function project_of(id: string?): sessions.Project?
	if not id or id == "none" then
		return nil
	end
	for _, p in sessions.projects:get() do
		if p.id == id then
			return p
		end
	end
	return nil
end

-- The route's ids, read once a change.
local where = steady(function(): Current
	local r = here()
	if r.name == "session" then
		local c = chat_of(r.params.id)
		return { name = "session", session = r.params.id, project = if c then c.project else nil }
	elseif r.name == "settings" then
		return { name = "settings", page = r.params.page }
	elseif r.name == "plugin" then
		return { name = "plugin", plugin = r.params.plugin, page = r.params.page }
	end
	local p = chosen:get()
	-- A chosen project since deleted: none chosen.
	if p and p ~= "none" and sessions.state:get() == "ready" and not project_of(p) then
		p = nil
	end
	return { name = "home", project = p }
end)
M.current = where

-- Where the chats are: the route's, or, while settings shows (a page of
-- its own over the window), the chat route it was opened from, held, so
-- what shows a chat (the page, the composer, the files pane) keeps it. A
-- plugin's page is a route of its own, as a chat is: no chat is open on it.
-- (Held in a named state, so a reload of the scripts in settings still
-- has the chats where they were under it.)
local held = ui.state("shell_chat_route", { name = "home" } :: Current)
ui.effect(function()
	local now = where:get()
	if now.name ~= "settings" and not same(now, held:peek()) then
		held:set(now)
	end
end)
M.chat = steady(function(): Current
	local now = where:get()
	if now.name ~= "settings" then
		return now
	end
	return held:get()
end)

-- The plugin's page shown (`{ plugin, id }`), as the chats' route has it
-- (held under settings too); none on a chat. Its panel's tabs are its own.
M.page = steady(function(): { plugin: string, id: string }?
	local c = M.chat:get()
	if c.name ~= "plugin" or not c.plugin or not c.page then
		return nil
	end
	return { plugin = c.plugin, id = c.page }
end)

-- What a new chat's composer has picked: where its panel works. (Not `chosen`: that's
-- what someone chose, which a first launch with no projects asks them for.)
M.new_chat_project = ui.state("shell_new_chat_project", nil :: string?)
M.project = steady(function(): Project?
	local c = M.chat:get()
	local p = project_of(c.project or (if c.name == "home" then M.new_chat_project:get() else nil))
	if not p then
		return nil
	end
	return { id = p.id, device = p.device, path = p.path, name = p.name }
end)

M.session = steady(function(): Session?
	local c = chat_of(M.chat:get().session)
	if not c then
		return nil
	end
	return { id = c.id, device = c.device, cwd = c.cwd, title = c.title, project = c.project }
end)

M.device = steady(function(): string?
	local s = M.session:get()
	if s and s.device then
		return s.device
	end
	local p = M.project:get()
	if p and p.device then
		return p.device
	end
	return sessions.here:get()
end)

-- A new chat with nowhere to work yet (Zeron's first launch): no project,
-- and none not chosen ("No project"). Its page asks for one in the
-- composer's place.
M.needs_project = ui.computed(function(): boolean
	return sessions.state:get() == "ready" and #sessions.projects:get() == 0 and M.chat:get().name == "home" and chosen:get() ~= "none"
end)

-- A project's hue: one of the palette's, the same for it every time (a
-- hue its people chose aside: `M.look`).
M.hue = look.hue
-- A project's tile, as it is now: { hue, emoji?, src? } (screens/shell/
-- project_look), nil for none.
M.look = look.by_id

M.projects = steady(function(): { Choice }
	local out: { Choice } = {}
	for _, p in sessions.projects:get() do
		local l = look.of(p)
		table.insert(out, { value = p.id, label = p.name, hue = l.hue, src = l.src, emoji = l.emoji })
	end
	return out
end)

function M.open_session(id: string)
	local r = at()
	if r.name == "session" and r.params.id == id then
		return
	end
	go("session", { id = id })
end

-- Launched (not the scripts reloaded), it starts where it was left (see
-- `kept`), and once the chats have come, settles: a session left open
-- that's still there stays open; one archived or gone since, and a first
-- launch with nothing saved, give way to the session most recently active,
-- as Zeron opens on it (shell/tabs.rs); none, a new chat. A session not
-- among the chats yet is given a moment (`GRACE_MS`) to arrive, as an
-- engine just started may list them late. Gone somewhere meanwhile (a
-- `zeron://` link, a click), it stays there. (Kept in a named state, so a
-- reload, which keeps it, never moves the window; a fixture that wants its
-- start as it is sets it first.)
M.launched = ui.state("shell_launch_opened", false)
local GRACE_MS = 2000
-- (The saved session not among the chats as they came, and whether its
-- moment to arrive has passed.)
local awaiting = ui.state("shell_launch_awaiting", false)
local given_up = ui.state("shell_launch_given_up", false)
ui.effect(function()
	if awaiting:get() and not given_up:peek() then
		ui.after(GRACE_MS, function()
			given_up:set(true)
		end)
	end
end)
ui.effect(function()
	-- (The chats read first: an early return keeps following them.)
	local now = sessions.now:get()
	local late = given_up:get()
	if M.launched:peek() then
		return
	end
	if now.state ~= "ready" then
		return
	end
	local from = start:peek()
	-- Moved since it started: it stays.
	if at().path ~= (if from ~= "" then from else "/") then
		M.launched:set(true)
		return
	end
	local r = parse(from)
	if from ~= "" and r.name ~= "session" then
		-- (A new chat, a plugin's page: as it was left.)
		M.launched:set(true)
		return
	end
	if r.name == "session" then
		local id = r.params.id
		for _, c in now.chats do
			if c.id == id then
				M.launched:set(true)
				return
			end
		end
		local archived = false
		for _, c in now.archived or {} do
			if c.id == id then
				archived = true
			end
		end
		if not archived and not late then
			if not awaiting:peek() then
				awaiting:set(true)
			end
			return
		end
	end
	M.launched:set(true)
	local latest: sessions.Chat? = nil
	for _, c in now.chats do
		if latest == nil or c.updated > (latest :: sessions.Chat).updated then
			latest = c
		end
	end
	-- (In place of where it started: no step back to a new chat it never
	-- showed, or a session gone.)
	if latest then
		go("session", { id = latest.id }, true)
	elseif r.name == "session" then
		go("home", nil, true)
	end
end)
-- Where it is, saved as it moves (settings aside: it's over the chats),
-- once the launch has settled (until then it is where it was left, or
-- somewhere it will settle from).
ui.effect(function()
	local r = here()
	if not M.launched:get() or r.name == "settings" then
		return
	end
	if last:peek() ~= r.path then
		last:set(r.path)
	end
end)

-- A device asked of the next new chat (a device's "+" in the sidebar):
-- held until its composer takes it (`take_device`), so it lands whether
-- the composer is there already or arrives with the move home. (A named
-- state: a reload in between keeps it.)
M.device_pick = ui.state("shell_device_pick", nil :: { device: string }?)
function M.take_device(): string?
	local pick = M.device_pick:peek()
	if pick then
		M.device_pick:set(nil)
		return pick.device
	end
	return nil
end

-- Where a new chat in `project` on `device` can start, as the composer's
-- picker allows (a project is a folder on one device): one of Cloud's
-- machines is Cloud (screens/chat/where's CLOUD, which keeps a project
-- Cloud can open); a computer the project isn't on, the same project there
-- (its repository, its name or its path), else none: the composer picks
-- one there (as the panel home's side chats' "+" does).
local function placed(project: string?, device: string): (string?, string)
	local now = sessions.now:peek()
	for _, d in now.devices do
		if d.id == device and d.cloud then
			return project, "cloud"
		end
	end
	local p = project_of(project)
	if p == nil or p.device == device then
		return project, device
	end
	local named: sessions.Project? = nil
	for _, q in now.projects do
		if q.device == device then
			if p.repository and q.repository == p.repository then
				return q.id, device
			elseif p.github and q.github == p.github then
				return q.id, device
			elseif q.name == p.name or q.path == p.path then
				named = named or q
			end
		end
	end
	return if named then named.id else nil, device
end

function M.new_chat(project: string?, device: string?)
	local on: string? = nil
	-- (None of the project's there: none chosen, so the composer picks.)
	local unchosen = false
	if device then
		local asked = project
		project, on = placed(project, device)
		unchosen = asked ~= nil and project == nil
	end
	ui.batch(function()
		if project then
			chosen:set(project)
		elseif unchosen then
			chosen:set(nil)
		end
		if on then
			-- (A fresh table each time, so the same device asked again is
			-- heard.)
			M.device_pick:set({ device = on })
		end
		if at().name ~= "home" then
			go("home")
		end
	end)
end

function M.choose_project(id: string?)
	chosen:set(id)
end

-- To settings (a page's id; the one open, else General). Its pages take
-- one step of the history between them (one replaces another), so a step
-- back from any of them is back to the chats.
function M.settings(page: string?)
	local r = at()
	local to = page or (if r.name == "settings" then r.params.page else nil) or "general"
	if r.name == "settings" then
		if r.params.page ~= to then
			go("settings", { page = to }, true)
		end
		return
	end
	go("settings", { page = to })
end

-- A `zeron://` link the system handed the app (a clicked banner, a chat's
-- link), read by the host (src/links.rs): `{ seq, chat?, settings? }`, `seq`
-- moving with each. Followed once each, never on a reload.
local link = ui.shared("app.link", { seq = 0 } :: { seq: number, chat: string?, settings: string? })
local followed = ui.state("shell_link_followed", 0)
ui.effect(function()
	local l = link:get()
	if l.seq == 0 or l.seq == followed:peek() then
		return
	end
	followed:set(l.seq)
	if l.chat then
		M.open_session(l.chat)
	elseif l.settings then
		M.settings(l.settings)
	end
end)

-- To a plugin's page ("pages.add", listed in the sidebar). One replaces
-- another, so a step back from any is back to the chats.
function M.plugin_page(plugin: string, page: string)
	local r = at()
	if r.name == "plugin" then
		if r.params.plugin ~= plugin or r.params.page ~= page then
			go("plugin", { plugin = plugin, page = page }, true)
		end
		return
	end
	go("plugin", { plugin = plugin, page = page })
end

-- Out of settings, back where the chats were: a step back (so forward
-- opens settings again), or, with none to take (it opened there), to the
-- chat route it holds.
function M.leave_settings()
	if at().name ~= "settings" then
		return
	end
	if router and router.can_back:peek() then
		router.back()
		return
	end
	local c = M.chat:peek()
	if c.name == "session" and c.session then
		go("session", { id = c.session })
	else
		go("home")
	end
end

function M.back()
	if router then
		router.back()
	end
end

function M.forward()
	if router then
		router.forward()
	end
end

return M
