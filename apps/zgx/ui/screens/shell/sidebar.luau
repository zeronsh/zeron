-- The main window's sidebar: the design system's (ds.Sidebar) on the
-- engine's sessions (@account/sessions). What it shows follows them in
-- place: a session's state, title or time moving reaches its row, one
-- made or archived moves the rest aside. Where sessions are put by hand
-- (dragged to Pinned, into a section of your own) goes to the engine, and
-- comes back as every device has it.
--
-- `Sidebar { collapsed = state, island = bool? }`: the sidebar, or
-- (`island`) its rail. The two share their view options, their places and
-- the session chosen, which is the route's (screens/shell/route).

local ds = require("@ds")
local sessions = require("@account/sessions")
local project_calls = require("@account/projects")
-- The project editor (screens/shell/project_editor, its emoji picker and
-- the emoji's data): loaded as it's first opened, not with the window, and
-- its dialog placed from then on (`project_editor_used`, kept across a
-- reload, so a dialog open then is there again).
local project_editor_used = ui.state("shell_project_editor_used", false)
local function project_editor(): any
	return require("@app/screens/shell/project_editor")
end
local auth = require("@account/auth")
local PrivateText = require("@ds/private_text")
local status = require("@ds/status")
local route = require("@app/screens/shell/route")
local view = require("@app/screens/shell/view")
local fit = require("@app/screens/shell/fit")
local motion = require("@ds/motion")
local boundary = require("@app/screens/shell/boundary")
local control = require("@ds/control")
local locale = require("@ds/locale")
local ds_session_row = require("@ds/session_row")
local activity = require("@app/screens/shell/activity")

export type Props = {
	collapsed: State<boolean>,
	-- The most it may take, and whether a change follows at once (the
	-- window's fit: screens/shell/fit).
	limit: (() -> number)?,
	direct: (() -> boolean)?,
	island: boolean?,
	-- Room left at its top for the title bar laid over it.
	top: number?,
}

-- What the sidebar and its rail share: the view options (Zeron's Organize,
-- Sort and Show), its width, where sessions are put, and the session open.
-- Kept across launches, but the session open (the route's) and the places
-- (the engine's).
local shared = {
	compact = ui.persist("shell_sidebar_compact", true),
	-- How it groups, as levels in order (`{ "project", "device" }`: by
	-- project, then device; none, one list). By project, then device, until
	-- changed. Kept as one choice before ("project"): read and kept as its
	-- levels from now (below).
	organize = ui.persist("shell_sidebar_organize", { "project", "device" } :: any),
	-- Both levels as the view menu's rows stand (dragged by their handles):
	-- Organize is those on, in this order.
	organize_order = ui.persist("shell_sidebar_organize_order", { "project", "device" }),
	sort = ui.persist("shell_sidebar_sort", "updated"),
	width = ui.persist("shell_sidebar_width", fit.SIDEBAR_DEFAULT),
	details = {
		branch = ui.persist("shell_sidebar_show_branch", true),
		pr = ui.persist("shell_sidebar_show_pr", true),
		harness = ui.persist("shell_sidebar_show_harness", true),
		project_icon = ui.persist("shell_sidebar_show_project_icon", true),
		location = ui.persist("shell_sidebar_show_location", true),
		-- (Zeron's "Show archived": off until asked for.)
		archived = ui.persist("shell_sidebar_show_archived", false),
		-- (Zeron's project filter over the list: off until asked for.)
		project_filter = ui.persist("shell_sidebar_show_project_filter", false),
		-- The projects with none but archived sessions, folded into one row
		-- at the projects' end: listed there (closed until it's opened), or
		-- not at all (until asked for).
		empty_projects = ui.persist("shell_sidebar_show_empty_projects", false),
		-- A collapsed group still lists its sessions working, asking or done
		-- unseen: on, so folding a group away never hides what needs you.
		keep_active = ui.persist("shell_sidebar_keep_active", true),
	},
	-- The groups showing all their sessions ("Show N more"): until the app
	-- quits (a reload keeps them).
	expanded = ui.state("shell_sidebar_expanded", {} :: { [string]: boolean }),
	-- The project the list is narrowed to (nil: all of them), while the
	-- filter shows.
	project = ui.persist("shell_sidebar_project", nil :: string?),
	places = ui.state("shell_places", { pinned = {}, sections = {} } :: sessions.Places),
	selected = ui.state("shell_selected", nil :: string?),
	-- Its view, kept here so a sidebar made again comes back as it was.
	offset = ui.state("shell_sidebar_offset", 0),
	folded = ui.persist("shell_sidebar_folded", {} :: { [string]: boolean }),
}

-- Scroll probe (for now: "it jumps back a step every few steps"): the
-- list's offset moving back within a moment of moving down is printed
-- (`sidebar-probe:` on the app's stderr), with how often the sidebar was
-- built and the sessions changed. It reads and prints; it writes nothing.
local probe = { last = 0, at = 0, dir = 0, builds = 0, built_at = 0, sessions_at = 0 }
ui.effect(function()
	sessions.now:get()
	probe.sessions_at = ui.now()
end)
ui.effect(function()
	local offset = shared.offset:get()
	local t = ui.now()
	local d = offset - probe.last
	if d < -0.5 and probe.dir > 0 and t - probe.at < 300 then
		print(
			`sidebar-probe: back {math.floor(-d + 0.5)} pt to {math.floor(offset)} ({math.floor(t - probe.at)} ms after a step down);`
				.. ` sidebar built {probe.builds}x, last {math.floor(t - probe.built_at)} ms ago;`
				.. ` sessions changed {math.floor(t - probe.sessions_at)} ms ago`
		)
	end
	if math.abs(d) > 0.5 then
		probe.dir = if d > 0 then 1 else -1
	end
	probe.last, probe.at = offset, t
end)

-- The same, field by field.
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

-- The places, as the sidebar edits them (no fold of the engine's).
local function plain(p: sessions.Places): sessions.Places
	local sections = {}
	for _, s in p.sections do
		table.insert(sections, { id = s.id, name = s.name, sessions = table.clone(s.sessions) })
	end
	return { pinned = table.clone(p.pinned), sections = sections }
end

-- The engine's places reach the sidebar; the sidebar's edits reach the
-- engine (the host sends only what differs from what it has, in order).
-- Made with the module, once for the window (the script owns them, not
-- whichever sidebar or rail came first).
--
-- An edit made here stands until the engine says it back: its places as
-- they were (a session's state moving republishes them), or half way
-- there (the host's changes land one by one), never undo it, or a dropped
-- row would jump back and forward again. What the engine says otherwise
-- wins once it has had time to answer (another device's edit).
local heard: sessions.Places? = nil
local sent: sessions.Places? = nil
local sent_at = 0
local ANSWER_MS = 5000
ui.effect(function()
	local theirs = plain(sessions.places:get())
	-- (Their places as they were: nothing to take.)
	if heard and same(theirs, heard) then
		return
	end
	heard = theirs
	if sent then
		if same(theirs, sent) then
			sent = nil
		elseif ui.now() - sent_at < ANSWER_MS then
			return
		else
			sent = nil
		end
	end
	if not same(theirs, plain(shared.places:peek())) then
		shared.places:set(theirs)
	end
end)
-- (Sent once the sessions are there, whether or not the engine has
-- heard its places yet: what goes is only what differs from what it
-- said, so nothing it hasn't said is ever taken out.)
-- (Nothing sent that the engine said already, or that went last: an echo
-- coming back, or the scripts reloading, sends nothing.)
local last_placed: sessions.Places? = nil
ui.effect(function()
	local mine = plain(shared.places:get())
	if sessions.now:peek().state ~= "ready" then
		return
	end
	if (heard and same(mine, heard)) or (last_placed and same(mine, last_placed)) then
		return
	end
	sent, sent_at, last_placed = mine, ui.now(), mine
	sessions.place(mine)
end)
-- Your sections' folds are the engine's, on every device: what it says
-- folds them here, and folding one here tells it.
ui.effect(function()
	local sections = sessions.places:get().sections
	local mine = shared.folded:peek()
	local next: { [string]: boolean }? = nil
	for _, s in sections do
		local folded = s.collapsed == true
		if (mine[s.id] == true) ~= folded then
			next = next or table.clone(mine)
			;(next :: any)[s.id] = if folded then true else nil
		end
	end
	if next then
		shared.folded:set(next)
	end
end)
ui.effect(function()
	local mine = shared.folded:get()
	for _, s in sessions.now:peek().places.sections do
		local folded = mine[s.id] == true
		if folded ~= (s.collapsed == true) then
			sessions.collapse(s.id, folded)
		end
	end
end)
-- The session chosen is the route's: a choice moves the route, and the
-- route (back, forward, a new chat) moves the choice.
-- (Only a choice moves it: as the sidebar is made, after a reload say,
-- the session kept chosen is the route's already, or settings is open
-- over it, and nothing should move.)
local made = false
ui.effect(function()
	local id = shared.selected:get()
	if not made then
		made = true
		return
	end
	if id and id ~= route.chat:peek().session then
		route.open_session(id)
	end
end)
ui.effect(function()
	-- (The chat route's: settings over the window leaves it chosen.)
	local s = route.chat:get().session
	if shared.selected:peek() ~= s then
		shared.selected:set(s)
	end
end)

-- Device names, by id; this Mac's.
local function devices(): ({ [string]: string }, string?)
	local names: { [string]: string } = {}
	local here: string? = nil
	local here_id = sessions.here:get()
	for _, d in sessions.devices:get() do
		names[d.id] = d.name
		if d.here or d.id == here_id then
			here = d.name
		end
	end
	return names, here
end

-- A device's id by the name its group shows (ds.Sidebar groups by name):
-- the project's own device where it's named so, else the first of yours
-- named so, else one of Cloud's; nil for none (Unknown device).
local function device_named(name: string, project: string?): string?
	local now = sessions.now:peek()
	local own: string? = nil
	for _, p in now.projects do
		if p.id == project then
			own = p.device
		end
	end
	local first, cloud = nil :: string?, nil :: string?
	for _, d in now.devices do
		if d.name == name then
			if d.id == own then
				return d.id
			elseif d.cloud then
				cloud = cloud or d.id
			else
				first = first or d.id
			end
		end
	end
	return first or cloud
end

-- The sessions as the sidebar groups them: under each project (in the
-- engine's order, empty ones too: a new chat starts in one), those in none
-- under Chats.
--
-- A project is empty when every session it has had is archived: none
-- listed, one or more on the Archived shelf. The sidebar folds those into
-- one row at the projects' end. One with no sessions at all (just added)
-- isn't: it's where a new chat is about to start, listed with the rest, until
-- Show → Empty projects is off, which hides it too (it's still in the project
-- selector). Counted on all of them,
-- not what a filter shows, so narrowing never moves a project in or out
-- (the status filter hides every group with none anyway); the project the
-- project filter is narrowed to is never folded away. Archived sessions
-- stay on the Archived shelf, as everyone's do, so Show → Archived changes
-- nothing here.
local NO_PROJECT = "none"
-- What an agent asks of you, by chat (the chat page's, `chat.asks`): it
-- says which of "input" a session waiting on you is, an approval or a
-- question.
local asks = ui.shared("chat.asks", {} :: { [string]: string })
local ASKED: { [string]: string } = { approval = "approval", question = "input" }

-- Which sessions show, as a plugin allowed to change how the sidebar shows
-- asks (`sidebar.filter`, the plugin system's to grant): nil, all of them;
-- or `{ statuses = { "working", … } }`, those in these states ("idle", in
-- none). Written by a plugin, so read with care: what isn't that shape is
-- nil, and a word it doesn't know counts for nothing. Who's narrowing it is
-- the host's word (`sidebar.filter_by`: the name of the plugin allowed to,
-- src/plugins), never the plugin's own `by`, which could claim to be Zeron.
local KNOWN = { working = true, waiting = true, input = true, approval = true, failed = true, done = true, woke = true, idle = true }
local filter = ui.shared("sidebar.filter", nil :: any)
local filter_by = ui.shared("sidebar.filter_by", nil :: any)
type Narrow = { statuses: { [string]: boolean }, words: { string }, by: string }
local narrow = ui.computed(function(): Narrow?
	local f = filter:get()
	if type(f) ~= "table" or type(f.statuses) ~= "table" then
		return nil
	end
	local statuses: { [string]: boolean } = {}
	local words: { string } = {}
	for _, w in f.statuses do
		if type(w) == "string" and KNOWN[w] and not statuses[w] then
			statuses[w] = true
			table.insert(words, w)
		end
	end
	if #words == 0 then
		return nil
	end
	local named = filter_by:get()
	local by = if type(named) == "string" and named ~= "" then string.sub(named, 1, 40) else locale.s("shell.sidebar.filter.a_plugin")
	return { statuses = statuses, words = words, by = by }
end)
-- How the notice names each state (shell.sidebar.filter.state.<word>).
local function say(word: string): string
	return locale.s(`shell.sidebar.filter.state.{word}`)
end

-- The Archived shelf's sessions (the most recently moved first), as the
-- list's rows read them.
local shelf = ui.computed(function(): { any }
	local names = devices()
	local out = {}
	for _, c in sessions.archived:get() do
		table.insert(out, {
			id = c.id,
			title = c.title,
			time = c.updated,
			created = c.created,
			agent = c.agent,
			harness = c.harness,
			agent_icon = c.agent_icon,
			device = names[c.device] or locale.s("shell.sidebar.unknown_device"),
			branch = c.branch,
		})
	end
	return out
end)

-- The project the list is narrowed to (the filter showing, and the
-- project still there), else nil.
local function only_project(): string?
	if not shared.details.project_filter:get() then
		return nil
	end
	local id = shared.project:get()
	if not id then
		return nil
	end
	for _, p in sessions.projects:get() do
		if p.id == id then
			return id
		end
	end
	return nil
end
-- (The filter put away: it shows everything again, as Zeron's clears it.)
ui.effect(function()
	if not shared.details.project_filter:get() and shared.project:peek() ~= nil then
		shared.project:set(nil)
	end
end)

-- Each session's row as last made: kept while it says the same, so the
-- row (ds/sidebar follows its session's own table) stays asleep as others
-- change.
local made_rows: { [string]: { [string]: any } } = {}
local function kept(row: { [string]: any }): { [string]: any }
	local was = made_rows[row.id]
	if was then
		local same = true
		for k, v in row do
			if was[k] ~= v then
				same = false
				break
			end
		end
		if same then
			for k in was do
				if row[k] == nil then
					same = false
					break
				end
			end
		end
		if same then
			return was
		end
	end
	made_rows[row.id] = row
	return row
end

-- `p` and the other checkouts of its repository, on any device, the oldest first
-- (its name and picture stand for them all, the same on every device). A project
-- with no repository is its own.
local function checkouts_of(p: any, all: { any }): { any }
	local out: { any } = {}
	for _, q in all do
		if q.id == p.id or (p.repository and q.repository == p.repository) then
			table.insert(out, q)
		end
	end
	table.sort(out, function(a: any, b: any): boolean
		local x, y = a.created or math.huge, b.created or math.huge
		if x ~= y then
			return x < y
		end
		return a.id < b.id
	end)
	return out
end
local function oldest_checkout(p: any, all: { any }): any
	return checkouts_of(p, all)[1]
end
-- Where a new chat in project `id`'s repository starts: its clone on this device,
-- else `id`.
local function checkout_here(id: string?): string?
	local now = sessions.now:peek()
	for _, p in now.projects do
		if p.id == id then
			for _, q in checkouts_of(p, now.projects) do
				if q.device == now.here then
					return q.id
				end
			end
		end
	end
	return id
end

-- peeked, not tracked: read from a shortcut or a click, never a computed
route.narrowed_project(function(): string?
	if not shared.details.project_filter:peek() then
		return nil
	end
	local id = shared.project:peek()
	for _, p in sessions.now:peek().projects do
		if p.id == id then
			return checkout_here(id)
		end
	end
	return nil
end)

local projects = ui.computed(function(): { ds.SidebarProject }
	local now = sessions.now:get()
	local asked = asks:get()
	local only = narrow:get()
	local just = only_project()
	local names = devices()
	local out: { ds.SidebarProject } = {}
	local by: { [string]: ds.SidebarProject } = {}
	-- One group for each repository, whichever device its clones are on: it stands
	-- where the first of them does, and reads as the oldest of them.
	local of_repository: { [string]: ds.SidebarProject } = {}
	for _, p in now.projects do
		local group = if p.repository then of_repository[p.repository] else nil
		if not group then
			local first = oldest_checkout(p, now.projects)
			local l = route.look(first.id) or { hue = route.hue(first.id) }
			group = { id = first.id, name = first.name, hue = l.hue, src = l.src, emoji = l.emoji, sessions = {} }
			table.insert(out, group :: any)
			if p.repository then
				of_repository[p.repository] = group :: any
			end
		end
		by[p.id] = group :: any
	end
	local loose: ds.SidebarProject = { id = NO_PROJECT, name = locale.s("shell.sidebar.chats"), icon = "side-chat", sessions = {} }
	-- (Which have sessions listed, and which archived ones.)
	-- (By group: a repository isn't empty while any clone of it has a session.)
	local just_group = if just then by[just] else nil
	local listed: { [any]: boolean } = {}
	for _, c in now.chats do
		local group = if c.project then by[c.project] else nil
		if group then
			listed[group] = true
		end
	end
	local archived: { [any]: boolean } = {}
	for _, c in now.archived or {} do
		local group = if c.project then by[c.project] else nil
		if group then
			archived[group] = true
			if not listed[group] and group ~= just_group then
				group.empty = true
			end
		end
	end
	-- (And which have never had one: gone with the empty ones when those are off.)
	for _, group in out do
		if not listed[group] and not archived[group] and group ~= just_group then
			group.unused = true
		end
	end
	for _, c in now.chats do
		if just and not (just_group and c.project and by[c.project] == just_group) then
			continue
		end
		local group = (c.project and by[c.project]) or loose
		-- (Waiting on you beats working: an approval, then a question.)
		local status = ASKED[asked[c.id] or ""] or c.status
		-- (The Activity view's: woken from sleep and not looked at since, the
		-- alarm, unless it's asking for you or under way. Which shelf it's
		-- on, and its place; asleep, its corner says when it wakes. Off, none
		-- of it.)
		local act = activity.on:get()
		if act and c.woke and (status == nil or status == "done") then
			status = "woke"
		end
		if only and not only.statuses[status or "idle"] then
			continue
		end
		local on_shelf = activity.shelf(c, status)
		local rank = if act then activity.rank(c, on_shelf) else nil
		table.insert(group.sessions, kept({
			id = c.id,
			title = c.title,
			status = status :: any,
			time = if on_shelf == "asleep" and c.sleep_until then activity.wakes_in(c.sleep_until) else c.updated,
			shelf = on_shelf,
			rank = rank,
			created = c.created,
			agent = c.agent :: any,
			device = names[c.device] or locale.s("shell.sidebar.unknown_device"),
			branch = c.branch,
			-- (Its side chats working count as its subagents do, as its
			-- lanes' badge counts them: screens/chat/lanes.)
			subagents = (c.running_subagents or 0) + (c.working_side_chats or 0),
		}) :: any)
	end
	if #loose.sessions > 0 then
		table.insert(out, loose)
	end
	-- Narrowed: the groups left with none fold away.
	if only then
		local kept: { ds.SidebarProject } = {}
		for _, g in out do
			if #g.sessions > 0 then
				table.insert(kept, g)
			end
		end
		return kept
	end
	return out
end)

-- The project filter (Zeron's, its view options' "Project filter"): all
-- projects, or one, searched for, the list narrowed to it.
local ALL = "::all"
local function project_filter(): Node
	local chosen = ui.computed(function(): string
		return only_project() or ALL
	end)
	local value = {
		get = function(_): string
			return chosen:get()
		end,
		peek = function(_): string
			return chosen:peek()
		end,
		set = function(_, v: string)
			shared.project:set(if v == ALL then nil else v)
		end,
	}
	return ui.show(
		function(): boolean
			return shared.details.project_filter:get()
		end,
		ui.div {
			id = "sidebar-project-filter",
			w_full = true,
			shrink = 0,
			ds.Dropdown {
				value = value :: any,
				icon = "folder",
				search = function(): string
					return locale.s("shell.sidebar.project_filter.search")
				end,
				-- (Zeron's, pinned under the projects.)
				actions = {
					{
						label = ui.t("shell.sidebar.project_filter.new_project") :: any,
						icon = "plus",
						on_click = function()
							local add: any = boundary.load("@app/screens/shell/add_project")
							if add then
								add.open()
							end
						end,
					},
				},
				items = function(): { any }
					local out: { any } = { { value = ALL, label = locale.s("shell.sidebar.project_filter.all_projects"), icon = "folder-with-files" } }
					-- (A repository once, as the list has it.)
					local all = sessions.projects:get()
					local seen: { [string]: boolean } = {}
					for _, p in all do
						local first = oldest_checkout(p, all)
						if not seen[first.id] then
							seen[first.id] = true
							local l = route.look(first.id) or { hue = route.hue(first.id) }
							table.insert(out, { value = first.id, label = first.name, hue = l.hue, src = l.src, emoji = l.emoji, project = first.id })
						end
					end
					return out
				end,
			},
		}
	)
end

-- What the filter shows, said once at the list's top, and the way back to
-- all of it: "Focus is showing working sessions only · Show all".
local function filter_notice(): Node
	local function says(): string
		local only = narrow:get()
		if not only then
			return ""
		end
		local names: { string } = {}
		for _, w in only.words do
			table.insert(names, say(w))
		end
		local list = if #names == 1
			then names[1]
			else locale.s("shell.sidebar.filter.list_last", {
				rest = table.concat(names, locale.s("shell.sidebar.filter.list_sep"), 1, #names - 1),
				last = names[#names],
			})
		return locale.s("shell.sidebar.filter.notice", { by = only.by, list = list })
	end
	return ui.show(
		function(): boolean
			return narrow:get() ~= nil
		end,
		ui.row {
			id = "sidebar-filter",
			w_full = true,
			shrink = 0,
			px = "$space.2",
			gap = "$space.2",
			items = "start",
			-- (Said whole, over two lines if it needs them: it's why the
			-- list is short.)
			ui.text { says, grow = 1, min_w = 0, wrap = true, text_size = "$font.size.xs", color = "$text.tertiary" },
			ds.Link {
				ui.t("shell.sidebar.filter.show_all") :: any,
				tone = "quiet",
				size = "sm",
				on_click = function()
					filter:set(nil)
				end,
			},
		}
	)
end

-- You, at the foot: your name, else your email; signed out, the
-- workspace's ("Local", as Zeron's: kept on this device); nothing until the
-- engine has said which.
type User = { id: string?, name: string }
local function user(): User
	local u = auth.user:get()
	if u then
		local name = if u.first_name then (u.first_name :: string):match("^%s*(.-)%s*$") else nil
		-- (No first name: the email, unless addresses are hidden on screen.)
		local fallback = if PrivateText.hidden:get() then locale.s("shell.sidebar.you") else u.email
		-- (Your photo: Settings → Profile's, kept from the last time it was heard.)
		local settings_store: any = boundary.load("@app/screens/settings/store")
		local avatar = if settings_store then settings_store.photo_file(settings_store.photo()) else ""
		return { id = u.id, name = if name and name ~= "" then name else fallback, avatar = if avatar ~= "" then avatar else nil }
	end
	if auth.state:get() == "restoring" then
		return { name = "" }
	end
	return { name = locale.s("shell.sidebar.local") }
end

-- What stands over you at the foot: the engine's update notice (C's
-- screens/engine/update: nothing unless this app's engine is out of date
-- and busy, or someone else's is; mounted always, as an idle engine of
-- ours is restarted from it), then Star on GitHub. (The list's gap between
-- them only while the notice says something: an empty one is no height and
-- takes no gap.)
local star: () -> Node
-- What the connection line says (production's connection pill: no
-- surface, a faint caption; the amber dot when offline or a change wasn't
-- saved, the working ring while reconnecting). Healthy: nothing.
local CONNECTION: { [string]: string } = {
	unsaved = "shell.sidebar.connection.unsaved",
	offline = "shell.sidebar.connection.offline",
	reconnecting = "shell.sidebar.connection.reconnecting",
}
local function connection(): string?
	local c: any = sessions.connection:get()
	return if type(c) == "string" and CONNECTION[c] then c else nil
end
local function connection_line(): Node
	return ui.show(
		function(): boolean
			return connection() ~= nil
		end,
		ui.row {
			id = "connection-line",
			w_full = true,
			shrink = 0,
			-- (Its dot on the foot's left edge: Star on GitHub's glyph's, the
			-- filter's and the empty list's words'. Its words on Star on
			-- GitHub's: the dot or the ring in a slot as wide as the star,
			-- as far from them.)
			px = "$space.2",
			pb = "$space.2",
			gap = "$zeron.space.tight",
			items = "center",
			ui.row {
				shrink = 0,
				w = 12,
				items = "center",
				ui.presence(function(): string
					return if connection() == "reconnecting" then "ring" else "dot"
				end, function(glyph: string): Node
					if glyph == "ring" then
						return ui.div { shrink = 0, status.StatusIcon { kind = "connecting", size = 10 } }
					end
					return ui.div { shrink = 0, w = 5, h = 5, rounded = "$radius.full", bg = "$warning" }
				end),
			},
			ds.FadeText {
				function(): string
					local key = CONNECTION[connection() or ""]
					return if key then locale.s(key) else ""
				end,
				grow = 1,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
		}
	)
end

local function foot_notes(): Node
	local notice_h = ui.use_state(0)
	return ui.column {
		w_full = true,
		shrink = 0,
		connection_line(),
		ui.div {
			w_full = true,
			on_resize = function(size)
				if notice_h:peek() ~= size.h then
					notice_h:set(size.h)
				end
			end,
			boundary.screen(boundary.named("shell.boundary.screen.update_notice"), "@app/screens/engine/update", function(update): Node
				return update.Notice()
			end, { pass = true }),
		},
		ui.div {
			w_full = true,
			mt = function(): any
				return if notice_h:get() > 0 then "$zeron.list.gap" else 0
			end,
			star(),
		},
	}
end

-- "Star on GitHub", over you at the foot, until it's put away (its × or
-- following it), and then never again on this Mac: Zeron's, to where its
-- landing page links.
local REPO = "https://github.com/zeronsh/comet"
local starred = ui.persist("shell_star_banner_dismissed", false)
function star(): Node
	local function away()
		starred:set(true)
	end
	return ui.show(
		function(): boolean
			return not starred:get()
		end,
		-- A chip of the accent, a row's corner: its × as far from the chip's
		-- edge at the end as from its top and foot, concentric.
		ui.row {
			id = "star-banner",
			role = "link",
			label = ui.t("shell.sidebar.star"),
			w_full = true,
			-- As tall as a row (you, under it).
			h = ui.raw(30, "a sidebar row's height (ds/sidebar's SLOT)"),
			shrink = 0,
			pl = "$space.2",
			-- (Its × as far in from the end as from the top: (30 − 22) / 2.)
			pr = "$space.1",
			gap = "$zeron.space.tight",
			items = "center",
			rounded = "$radius.lg",
			bg = "$zeron.tone.accent.wash",
			hover = { bg = "$zeron.tone.accent.edge" },
			color = "$zeron.tone.accent.ink",
			cursor = "pointer",
			on_click = function()
				ui.invoke("open_url", REPO)
				away()
			end,
			-- A link's menu (ds/links): Open, as the click; Copy link.
			context_menu = require("@ds/links").outside_menu(function(): string
				return REPO
			end, function()
				ui.invoke("open_url", REPO)
				away()
			end),
			ui.div { shrink = 0, y = 0.5, ds.Icon { name = "star-bold", size = 12 } },
			ui.text { ui.t("shell.sidebar.star"), grow = 1, min_w = 0, text_size = "$font.size.xs", weight = "$font.weight.medium" },
			ui.div {
				theme = { zeron = { icon_button = { radius = control.nest("$radius.lg", "$space.1") } } },
				ds.IconButton { icon = "close", label = ui.t("common.dismiss"), tooltip = locale.s("common.dismiss"), size = "sm", tone = "quiet", on_click = away },
			},
		}
	)
end

-- Renaming a session: which, and the name being typed.
local renaming = ui.state("shell_renaming", nil :: string?)
local rename_open = ui.state("shell_rename_open", false)
-- The session renamed in its row (Zeron's inline rename), while it is.
local renaming_row = ui.state("shell_renaming_row", nil :: string?)
-- The session asked about deleting, and its dialog.
local deleting = ui.state("shell_deleting", nil :: string?)
local delete_open = ui.state("shell_delete_open", false)
-- What the delete takes with it (its subagents and side chats, theirs in
-- turn), as the host counted them when it was asked.
local delete_subagents = ui.state("shell_delete_subagents", 0)
local delete_sides = ui.state("shell_delete_sides", 0)
-- Sleep → Custom…: the chat, the moment chosen, and whether it's asking.
local sleep_custom = ui.state("shell_sleep_custom", nil :: string?)
local sleep_custom_at = ui.state("shell_sleep_custom_at", nil :: number?)
local sleep_custom_open = ui.state("shell_sleep_custom_open", false)
local new_title = ui.state("shell_rename_title", "")

local function chat(id: string): sessions.Chat?
	local now = sessions.now:peek()
	for _, c in now.chats do
		if c.id == id then
			return c
		end
	end
	-- A side chat (the panel's lists and tabs take this menu too).
	for _, list in (now :: any).children or {} do
		for _, c in list do
			if c.id == id then
				return c :: any
			end
		end
	end
	return nil
end

-- Whether `id` is a side chat (under another): the sidebar lists none, so
-- pinning it or putting it in a section doesn't apply.
local function is_side(id: string): boolean
	for _, c in sessions.now:peek().chats do
		if c.id == id then
			return false
		end
	end
	return chat(id) ~= nil
end

-- Where a session is put by hand: pinned, a section, or back in its group.
local function put(id: string, where: string?)
	local p = plain(shared.places:peek())
	local pinned = {}
	for _, other in p.pinned do
		if other ~= id then
			table.insert(pinned, other)
		end
	end
	if where == "pinned" then
		table.insert(pinned, id)
	end
	for _, s in p.sections do
		local i = table.find(s.sessions, id)
		if i then
			table.remove(s.sessions, i)
		end
		if s.id == where then
			table.insert(s.sessions, id)
		end
	end
	shared.places:set({ pinned = pinned, sections = p.sections })
end

-- What else asks for these (the shell's keys, the palette).
local M = {}

-- You and settings at a sidebar's foot (this one's, and the settings
-- window's: screens/shell/settings): you open your profile, chosen while
-- it's open; settings' own button is a toggle, on while a page of theirs
-- other than your profile is open, a click again leaving them.
M.user = user
M.account = {
	on_user = function()
		route.settings("profile")
	end,
	user_open = function(): boolean
		local c = route.current:get()
		return c.name == "settings" and c.page == "profile"
	end,
	on_settings = function()
		local c = route.current:peek()
		if c.name == "settings" and c.page ~= "profile" then
			route.leave_settings()
		else
			route.settings(if c.name == "settings" then "general" else nil)
		end
	end,
	settings_open = function(): boolean
		local c = route.current:get()
		return c.name == "settings" and c.page ~= "profile"
	end,
}


-- The view options, kept across launches (how it's grouped, sorted, …).
M.options = shared
-- The sidebar's view, whole, for other lists of sessions to draw as it
-- draws them (the panel home's side chats and subagents, screens/panel/
-- home, through ds/session_row): `{ compact, organize, sort, details }`,
-- `details` every one of its Show options by name (`ds.SidebarDetails`'s,
-- as ds/sidebar reads them). Made from the options themselves, so one the
-- sidebar gains reaches them with no change here.
local sidebar_view = ui.shared("sidebar.view", {} :: { [string]: any })
-- (Organize kept as one choice before, "project": its levels from now.)
ui.effect(function()
	local v = shared.organize:get()
	if type(v) ~= "table" then
		shared.organize:set(ds_session_row.levels(v))
	end
end)
-- (`same`: the places' field-by-field comparison, above.)
ui.effect(function()
	local details: { [string]: any } = {}
	for name, st in shared.details :: any do
		details[name] = (st :: any):get()
	end
	local v = {
		compact = shared.compact:get(),
		organize = ds_session_row.levels(shared.organize:get()),
		sort = shared.sort:get(),
		details = details,
	}
	if not same(v, sidebar_view:peek()) then
		sidebar_view:set(v)
	end
end)
-- The sessions the list shows, in its order (those folded away, and those
-- past a group's "Show N more", left out): ⌘1–9 go to its first nine.
M.order = ui.state("shell_sidebar_order", {} :: { string })
-- ⌘ held alone: those nine wear their keys (Zeron's jump hints): shown at
-- once, as Zeron's are, kept through the ⌘1–9 they offer, and only while
-- this window is the one typed in and no palette has the keys. Any other
-- key pressed meanwhile is a shortcut (⌘C), not a look at them: they go
-- until the modifiers held change again (`M.key_pressed`, from the shell's
-- root and its commands).
-- ⌘ is the key `cmd+1` is pressed with: Command on macOS, Ctrl elsewhere
-- (zgx's `cmd`), where Super (`meta`) is just another modifier.
local held = ui.modifiers
local mac = string.find(ui.shortcut_text("cmd"), "⌘", 1, true) ~= nil
local primary = if mac then held.meta else held.control
local other = if mac then held.control else held.meta
local pressed = ui.state("shell_jump_hints_pressed", false)
ui.effect(function()
	-- (Every change of what's held starts afresh, as the shipping app's.)
	local _ = { held.meta:get(), held.shift:get(), held.control:get(), held.alt:get() }
	if pressed:peek() then
		pressed:set(false)
	end
end)
-- A key went down (the shell root's `on_key`; a shell command run, its menu
-- key equivalent never reaching `on_key`): the hints go, unless it's the
-- ⌘1–9 they offer (`digit`).
function M.key_pressed(digit: boolean?)
	if primary:peek() and not digit and not pressed:peek() then
		pressed:set(true)
	end
end
M.jump_hints = ui.state("shell_jump_hints", false)
ui.effect(function()
	local alone = primary:get() and not (held.shift:get() or other:get() or held.alt:get())
	local show = alone
		and not pressed:get()
		and ui.window.active:get() ~= false
		and not view.palette:get()
	if not show then
		if M.jump_hints:peek() then
			M.jump_hints:set(false)
		end
		return
	end
	if not M.jump_hints:peek() then
		M.jump_hints:set(true)
	end
end)
-- The session card showing (a row rested on: its id and the row's box in
-- the window), drawn over the window by the shell; nil, none.
export type Card = { id: string, x: number, y: number, w: number, h: number }
M.card = ui.state("shell_session_card", nil :: Card?)
-- (The window's fit narrows it from what it would be: this.)
fit.sidebar_width(function(): number
	return shared.width:get()
end)

function M.archive(id: string)
	if route.current:peek().session == id then
		route.new_chat()
	end
	sessions.archive(id)
end

-- A session's menu, as the gallery's (Zeron's sidebar chat menu): rename,
-- pin, copy, move into one of your sections, archive (kept: Settings →
-- Archived lists it), delete (for good, asked once more); then plugins'
-- items, under their names (screens/plugins/menus: the app's registry of
-- them, read as the menu opens).
-- (Copied, said so, as Zeron's: "Path copied".)
local function copy(text: string?, what: string)
	if text then
		ui.clipboard.write(text)
		ds.toast.show { title = locale.s("shell.sidebar.copied", { what = what }), tone = "success" }
	end
end
-- A string as a URL's part (as production's `encode_component`).
local function encoded(v: string): string
	return (string.gsub(v, "[^%w%-_%.~]", function(c: string): string
		return string.format("%%%02X", string.byte(c))
	end))
end

M.pin = function(id: string)
	local pinned = table.find(shared.places:peek().pinned, id) ~= nil
	put(id, if pinned then nil else "pinned")
end

local function menu(id: string): { any }?
	local c = chat(id)
	if not c then
		return nil
	end
	local p = shared.places:peek()
	local pinned = table.find(p.pinned, id) ~= nil
	local into: { any } = { { heading = locale.s("shell.sidebar.menu.sections") } }
	for _, s in p.sections do
		local here = table.find(s.sessions, id) ~= nil
		table.insert(into, {
			label = s.name,
			icon = "folder",
			checked = function(): boolean
				return here
			end,
			-- (Chosen where it is: back to its project.)
			on_select = function()
				put(id, if here then nil else s.id)
			end,
		})
	end
	table.insert(into, { separator = true })
	table.insert(into, {
		label = locale.s("shell.sidebar.menu.new_section"),
		icon = "plus",
		on_select = function()
			local sid = `section-{os.time()}-{math.random(1, 1e6)}`
			local now = plain(shared.places:peek())
			table.insert(now.sections, { id = sid, name = locale.s("shell.sidebar.menu.new_section"), sessions = {} })
			shared.places:set(now)
			put(id, sid)
		end,
	})
	-- What Copy offers: its folder, its harness's link and id where it has
	-- them (a Codex thread's), and its own id.
	local copies: { any } = {
		{
			label = locale.s("shell.sidebar.menu.path"),
			icon = "folder",
			disabled = if c.cwd then nil else locale.s("shell.sidebar.menu.no_folder"),
			on_select = function()
				copy(c.cwd, locale.s("shell.sidebar.menu.path"))
			end,
		},
	}
	table.insert(copies, {
		label = locale.s("shell.sidebar.menu.zeron_link"),
		icon = "arrow-up-right",
		on_select = function()
			sessions.link(id):after(function(link: any)
				copy(if type(link) == "string" then link else nil, locale.s("shell.sidebar.menu.zeron_link"))
			end, function()
				ds.toast.show { title = locale.s("shell.sidebar.menu.link_not_ready"), tone = "danger" }
			end)
		end,
	})
	if c.harness == "codex" and c.harness_session then
		table.insert(copies, {
			label = locale.s("shell.sidebar.menu.codex_link"),
			icon = "arrow-up-right",
			on_select = function()
				copy(`codex://threads/{encoded(c.harness_session :: string)}`, locale.s("shell.sidebar.menu.codex_link"))
			end,
		})
	end
	if c.harness_session then
		table.insert(copies, {
			label = locale.s("shell.sidebar.menu.harness_session_id"),
			icon = "document",
			on_select = function()
				copy(c.harness_session, locale.s("shell.sidebar.menu.harness_session_id"))
			end,
		})
	end
	table.insert(copies, {
		label = locale.s("shell.sidebar.menu.session_id"),
		icon = "document",
		on_select = function()
			copy(c.id, locale.s("shell.sidebar.menu.session_id"))
		end,
	})
	local side = is_side(id)
	-- The Activity view's Sleep (its times, each saying when, and one of
	-- your own), or, asleep, Wake.
	local sleep: any = false
	if activity.on:peek() and not side then
		if c.sleep_until then
			sleep = {
				label = locale.s("shell.activity.wake"),
				icon = "sun",
				on_select = function()
					activity.wake(id)
				end,
			}
		else
			local times: { any } = {}
			for _, p in activity.presets() do
				table.insert(times, {
					label = locale.s("shell.activity.sleep.choice", { name = p.label, when = p.detail }),
					on_select = function()
						activity.sleep(id, p.until_at)
					end,
				})
			end
			table.insert(times, { separator = true })
			table.insert(times, {
				label = locale.s("shell.activity.sleep.custom"),
				on_select = function()
					-- (It opens on tomorrow at 9: a time already chosen,
					-- moved from there.)
					local d = os.date("*t") :: any
					sleep_custom:set(id)
					sleep_custom_at:set(activity.local_time({ year = d.year, month = d.month, day = d.day + 1, hour = 9, min = 0, sec = 0 }))
					sleep_custom_open:set(true)
				end,
			})
			sleep = { label = locale.s("shell.activity.sleep.title"), icon = "moon", submenu = times }
		end
	end
	-- Waiting on background work it said it waits on: the person can say
	-- otherwise (a dev server marked by mistake). Its background agents
	-- aren't its say, so there's nothing to take back there.
	local stop_waiting: any = false
	if c.status == "waiting" then
		for _, w in c.waiting or {} do
			if w.kind == "process" then
				stop_waiting = {
					label = locale.s("shell.activity.stop_waiting"),
					icon = "close-circle",
					on_select = function()
						sessions.stop_waiting(id):after(function() end, function()
							ds.toast.show { title = locale.s("shell.activity.stop_waiting_failed"), tone = "danger" }
						end)
					end,
				}
				break
			end
		end
	end
	local entries = {
		{
			label = locale.s("common.rename"),
			icon = "pen",
			on_select = function()
				M.rename(id)
			end,
		},
		if side
			then false
			else {
				label = locale.s(if pinned then "shell.sidebar.menu.unpin" else "shell.sidebar.menu.pin"),
				icon = "pin",
				shortcut = "cmd+shift+p",
				on_select = function()
					M.pin(id)
				end,
			},
		{
			label = locale.s("common.copy"),
			icon = "copy",
			submenu = copies,
		},
		-- (Activity lists no sections: nothing to move into.)
		if side or activity.on:peek() then false else { label = locale.s("shell.sidebar.menu.move_to"), icon = "folder-with-files", submenu = into },
		sleep,
		stop_waiting,
		{ separator = true },
		{
			-- (No key: ⌘⌫ is a text field's, deleting to the line's start.)
			label = locale.s("common.archive"),
			icon = "archive-minimalistic",
			disabled = if c.status == "working" or c.status == "waiting" then locale.s("shell.sidebar.menu.archive_working") else nil,
			on_select = function()
				M.archive(id)
			end,
		},
		{
			label = locale.s("shell.sidebar.menu.delete"),
			icon = "trash-bin-minimalistic",
			danger = true,
			-- (Asked once more, as Zeron's: "Delete session?")
			-- (Once it knows what goes with it, to say so.)
			on_select = function()
				local function ask(subagents: number, sides: number)
					delete_subagents:set(subagents)
					delete_sides:set(sides)
					deleting:set(id)
					delete_open:set(true)
				end
				sessions.deleted_with(id):after(function(with: any)
					ask(tonumber(with and with.subagents) or 0, tonumber(with and with.sides) or 0)
				end, function()
					ask(0, 0)
				end)
			end,
		},
	}
	-- (A side chat's: without what doesn't apply to it.)
	local kept: { any } = {}
	for _, e in entries :: { any } do
		if e ~= false then
			table.insert(kept, e)
		end
	end
	entries = kept :: any
	-- (The registry is the plugin system's: without it, the app's alone.)
	local menus: { extend: ((entries: { any }, kind: string, context: { [string]: any }) -> { any })? }? =
		boundary.load("@app/screens/plugins/menus")
	local extend = menus and menus.extend
	if extend then
		return extend(entries, "session", { id = id })
	end
	return entries
end

-- The same menu, for a session shown elsewhere (the panel's side chat rows
-- and tabs): a side chat's without Pin and Move to.
M.session_menu = menu

-- The rename dialog, one for the window.
-- A project edited (its name and its icon: screens/shell/project_editor),
-- or removed with its sessions ("Remove project?"), from its header's
-- right click.
local project_acting = ui.state("shell_project_acting", nil :: string?)
local project_remove_open = ui.state("shell_project_remove_open", false)
-- Of a repository on several devices, the checkouts to remove, by id.
local project_remove_chosen = ui.state("shell_project_remove_chosen", {} :: { [string]: boolean })
local function project_of(id: string?): sessions.Project?
	for _, p in sessions.projects:get() do
		if p.id == id then
			return p
		end
	end
	return nil
end
local function project_menu(id: string): { any }?
	local p = project_of(id)
	if not p then
		return nil
	end
	return {
		{
			label = locale.s("shell.sidebar.menu.edit_project"),
			icon = "pen",
			on_select = function()
				M.edit_project(id)
			end,
		},
		{
			label = locale.s("shell.sidebar.menu.remove"),
			icon = "trash-bin-minimalistic",
			danger = true,
			on_select = function()
				-- (Of a repository on several devices: this device's, to begin with.)
				local now = sessions.now:peek()
				local chosen: { [string]: boolean } = {}
				for _, q in checkouts_of(p, now.projects) do
					if q.device == now.here then
						chosen[q.id] = true
					end
				end
				project_remove_chosen:set(chosen)
				project_acting:set(id)
				project_remove_open:set(true)
			end,
		},
	}
end
local function project_dialogs(): Node
	-- The checkouts of the project being removed (one, for most).
	local function acting_checkouts(): { any }
		local p = project_of(project_acting:get())
		return if p then checkouts_of(p, sessions.projects:get()) else {}
	end
	local function device_of(q: any): string
		for _, d in sessions.now:peek().devices do
			if d.id == q.device then
				return d.name
			end
		end
		return locale.s("shell.sidebar.project.its_device")
	end
	local function chats_in(id: string): number
		local n = 0
		for _, c in sessions.now:peek().chats do
			if c.project == id then
				n += 1
			end
		end
		return n
	end
	-- (Its sessions as Zeron counts them: those in it, not archived.)
	local function removing(): string
		local id = project_acting:get()
		local p = project_of(id)
		local name = if p then p.name else locale.s("shell.sidebar.project.this_project")
		local n = 0
		for _, c in sessions.now:get().chats do
			if c.project == id then
				n += 1
			end
		end
		local device = locale.s("shell.sidebar.project.its_device")
		for _, d in sessions.devices:get() do
			if p and d.id == p.device then
				device = d.name
			end
		end
		return locale.s("shell.sidebar.project.removing", { name = name, n = n, device = device })
	end
	return ui.column {
		-- Its name and its icon (Edit project…), once first opened.
		ui.presence(function(): string?
			return if project_editor_used:get() then "editor" else nil
		end, function(): Node
			return project_editor().Dialog()
		end),
		ds.Dialog {
			open = project_remove_open,
			title = ui.t("shell.sidebar.project.remove_title") :: any,
			tone = "danger",
			size = "sm",
			confirm = {
				label = ui.t("common.remove") :: any,
				on_click = function()
					local id = project_acting:peek()
					project_acting:set(nil)
					if not id then
						return
					end
					-- The one project, or the checkouts ticked.
					local going: { string } = { id }
					local list = acting_checkouts()
					if #list > 1 then
						going = {}
						for _, q in list do
							if project_remove_chosen:peek()[q.id] then
								table.insert(going, q.id)
							end
						end
					end
					for _, gone in going do
						-- (Showing it, or one of its sessions: a new chat instead.)
						local shown = route.session:peek()
						if route.chat:peek().project == gone or (shown and shown.project == gone) then
							route.new_chat()
						end
						project_calls.delete(gone):after(function() end, function(err: any)
							ds.toast.show { title = locale.s("shell.sidebar.project.couldnt_remove"), body = if type(err) == "string" then err else nil, tone = "danger" }
						end)
					end
				end,
			},
			ui.presence(function(): string
				return if #acting_checkouts() > 1 then "several" else "one"
			end, function(kind: string): Node
				if kind == "one" then
					return ui.text { removing, w_full = true, wrap = true, text_size = "$font.size.md", line_height = 20, color = "$text.secondary" }
				end
				return ui.column {
					w_full = true,
					gap = "$space.3",
					ui.text {
						function(): string
							local p = project_of(project_acting:get())
							return locale.s("shell.sidebar.project.removing_where", { name = if p then p.name else "" })
						end,
						w_full = true,
						wrap = true,
						text_size = "$font.size.md",
						line_height = 20,
						color = "$text.secondary",
					},
					ui.each(ui.computed(acting_checkouts), function(q: any): Node
						return ds.Checkbox {
							label = device_of(q),
							description = locale.s("shell.sidebar.project.checkout", { n = chats_in(q.id), path = q.path }),
							value = function(): boolean
								return project_remove_chosen:get()[q.id] == true
							end,
							on_change = function(on: boolean)
								local next = table.clone(project_remove_chosen:peek())
								next[q.id] = if on then true else nil
								project_remove_chosen:set(next)
							end,
						}
					end, {
						key = function(q: any): string
							return q.id
						end,
					}),
				}
			end, { w_full = true }),
		},
	}
end

-- Deleting a session, asked once more (Zeron's "Delete session?"): for
-- good, so its red Delete waits on Cancel.
local function delete_dialog(): Node
	local function says(): string
		local id = deleting:get()
		local c = if id then chat(id) else nil
		local t = if c and c.title ~= "" then (string.gsub(c.title, "%s+", " ")) else locale.s("shell.sidebar.session.new_session")
		local will = locale.s("shell.sidebar.session.will_delete", { title = t })
		local subagents, sides = delete_subagents:get(), delete_sides:get()
		local also = if subagents > 0 and sides > 0
			then locale.s("shell.sidebar.session.also_deletes_both", { subagents = subagents, sides = sides })
			elseif subagents > 0 then locale.s("shell.sidebar.session.also_deletes_subagents", { n = subagents })
			elseif sides > 0 then locale.s("shell.sidebar.session.also_deletes_side", { n = sides })
			else nil
		if not also then
			return will
		end
		return locale.s("shell.sidebar.session.will_delete_with", { will = will, also = also })
	end
	return ds.Dialog {
		open = delete_open,
		title = ui.t("shell.sidebar.session.delete_title") :: any,
		tone = "danger",
		size = "sm",
		confirm = {
			label = ui.t("common.delete") :: any,
			on_click = function()
				local id = deleting:peek()
				deleting:set(nil)
				if not id then
					return
				end
				-- (Its title now: refused, the window has already left it.)
				local c = chat(id)
				local title = if c and c.title ~= "" then (string.gsub(c.title, "%s+", " ")) else locale.s("shell.sidebar.session.new_session")
				if route.current:peek().session == id then
					route.new_chat()
				end
				sessions.delete(id):after(function() end, function(err: any)
					ds.toast.show {
						title = locale.s("shell.sidebar.session.couldnt_delete", { title = title }),
						body = if type(err) == "string" and err ~= "" then err else nil,
						tone = "warning",
					}
				end)
			end,
		},
		-- (As its own body line, but following the session.)
		ui.text { says, w_full = true, wrap = true, text_size = "$font.size.md", line_height = 20, color = "$text.secondary" },
	}
end

-- Sleep until a time of your own: a day and a time from a minute ahead.
local function sleep_dialog(): Node
	return ds.Dialog {
		open = sleep_custom_open,
		title = ui.t("shell.activity.sleep.custom_title") :: any,
		-- Under it, the moment chosen, in words: what Sleep will do.
		body = function(): string
			local at = sleep_custom_at:get()
			if not at then
				return ""
			end
			return locale.s("shell.activity.sleep.until_when", {
				day = locale.weekday(at),
				date = locale.month_day(at),
				time = locale.clock(at),
			})
		end,
		size = "sm",
		confirm = {
			label = ui.t("shell.activity.sleep.title") :: any,
			-- (A time that has passed, or none chosen, puts nothing to sleep.)
			on_click = function()
				local id, at = sleep_custom:peek(), sleep_custom_at:peek()
				sleep_custom:set(nil)
				sleep_custom_open:set(false)
				if id and at and at > os.time() then
					activity.sleep(id, at)
				end
			end,
		},
		-- (Built as it opens: its month of days is nothing to keep.)
		build = function(): Node
			return ds.DatePicker {
				id = "sleep-custom",
				value = sleep_custom_at,
				min = function(): number
					return os.time() + 60
				end,
				label = ui.t("shell.activity.sleep.custom_title") :: any,
			}
		end,
	}
end

local function rename_dialog(): Node
	local function done()
		local id = renaming:peek()
		local title = string.match(new_title:peek(), "^%s*(.-)%s*$") or ""
		renaming:set(nil)
		rename_open:set(false)
		if id and title ~= "" then
			sessions.rename(id, string.sub(title, 1, 200))
		end
	end
	return ds.Dialog {
		open = rename_open,
		title = ui.t("shell.sidebar.session.rename_title") :: any,
		size = "sm",
		confirm = { label = ui.t("common.rename") :: any, on_click = done },
		ds.TextField { value = new_title, label = ui.t("shell.sidebar.session.name"), placeholder = function(): string
			return locale.s("shell.sidebar.session.name_placeholder")
		end, select_on_focus = true, w_full = true },
	}
end

M.Sidebar = ui.component(function(props: Props): Node
	probe.builds += 1
	probe.built_at = ui.now()
	-- (Made again, a reload: no row is rested on yet.)
	if not props.island then
		M.card:set(nil)
	end
	local bar = ds.Sidebar {
		compact = shared.compact,
		organize = shared.organize :: any,
		organize_order = shared.organize_order,
		sort = shared.sort :: any,
		details = shared.details,
		places = shared.places :: any,
		width = shared.width,
		default_width = fit.SIDEBAR_DEFAULT,
		shown = if props.island then nil else view.sidebar_shown,
		before = if props.island then nil else 5,
		-- (Its edge double-clicked: the chat and the panel split again, as
		-- Zeron's `reset_panel_widths`.)
		on_reset = function()
			view.seam_reset:set(view.seam_reset:peek() + 1)
		end,
		collapsed = props.collapsed,
		limit = props.limit,
		direct = props.direct,
		loading = function(): boolean
			return sessions.state:get() ~= "ready"
		end,
		island = props.island,
		selected = shared.selected,
		projects = projects,
		here = function(): string?
			local _, here = devices()
			return here
		end :: any,
		user = user,
		top = props.top,
		card = not props.island,
		-- (A device's "+", inside a project's group: on that device too.)
		on_new = function(project: string?, device: string?)
			route.new_chat(if device then project else checkout_here(project), if device then device_named(device, project) else nil)
		end,
		on_archive = M.archive,
		archived = shelf,
		activity = activity.on,
		on_activity_settings = function()
			route.settings("activity")
		end,
		on_unarchive = function(id: string)
			sessions.unarchive(id)
		end,
		pages = ui.shared("plugins.pages", {} :: { any }),
		on_page = function(page: any)
			route.plugin_page(page.plugin, page.id)
		end,
		on_search = function()
			view.palette:set(true)
		end,
		on_user = M.account.on_user,
		user_open = M.account.user_open,
		on_settings = M.account.on_settings,
		settings_open = M.account.settings_open,
		menu = menu,
		project_menu = project_menu,
		narrowed = function(): boolean
			return narrow:get() ~= nil
		end,
		notice = if props.island
			then nil
			else ui.column { w_full = true, gap = "$zeron.list.gap", project_filter(), filter_notice() },
		tip = if props.island then nil else foot_notes(),
		-- (The rail has no list to scroll, and its own folds.)
		offset = if props.island then nil else shared.offset,
		folded = if props.island then nil else shared.folded,
		expanded = if props.island then nil else shared.expanded,
		order =if props.island then nil else M.order,
		jump_hints = if props.island
			then nil
			else function(): boolean
				return M.jump_hints:get()
			end,
		renaming_session = if props.island then nil else renaming_row,
		on_card = if props.island
			then nil
			else function(c: Card?)
				M.card:set(c)
			end,
		on_rename = function(id: string, title: string)
			sessions.rename(id, string.sub(title, 1, 200))
		end,
	}
	if props.island then
		return bar
	end
	return ui.div {
		id = "shell-sidebar",
		hook = "shell.sidebar",
		h_full = true,
		shrink = 0,
		relative = true,
		-- (The window's gap before its card: there as long as the card is,
		-- going only with its last 5 points, so the card's left edge stands
		-- still as it folds and comes back. It once closed on the sidebar's
		-- clock, the whole card sliding 5 points.)
		pl = function(): number
			local shown = view.sidebar_shown:get()
			return ui.raw(math.min(5, shown), "the window's gap before its card")
		end,
		-- (That gap's top moves the window, as the bar beside it does.)
		ui.div { absolute = true, left = 0, top = 0, w = ui.raw(5, "the window's gap"), h = ui.raw(42, "the top bar"), window_drag = true },
		bar,
		rename_dialog(),
		delete_dialog(),
		sleep_dialog(),
		project_dialogs(),
	}
end)

-- A project's name and icon edited (Edit project…), from its header's menu
-- or Settings → Projects: the window's one editor, `start` and `also` as its
-- `open`'s.
function M.edit_project(id: string, start: { [string]: any }?, also: { string }?)
	project_editor().open(id, start, also)
	project_editor_used:set(true)
end

-- `Sidebar { … }` builds it; `sidebar.pin(id)`, `sidebar.archive(id)` and
-- `sidebar.rename(id)` act on a session as its menu does.
function M.rename(id: string)
	-- In its row, where the list shows it (as Zeron's); else (folded away,
	-- filtered out) the dialog.
	if table.find(M.order:peek(), id) then
		renaming_row:set(id)
		return
	end
	local c = chat(id)
	if c then
		new_title:set(c.title)
		renaming:set(id)
		rename_open:set(true)
	end
end
-- Its width, kept: the settings window's list shares it (screens/shell/
-- settings), so the two sidebars are one width, dragged in either.
M.width = shared.width

return setmetatable(M, {
	__call = function(_, props: Props): Node
		return M.Sidebar(props)
	end,
})
