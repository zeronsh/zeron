--!strict
-- Zeron's helpers for a plugin. A copy sits in every plugin Zeron makes
-- (`zeron plugin new`, New plugin, or an agent's create_plugin), and it is
-- part of what the plugin's signature covers. Use it from init.luau:
--
--   local zeron = require("./zeron")("my-plugin")   -- the plugin's own id
--
-- Where your plugin shows, by selector: any element of Zeron's by its id, its
-- class or its role and name (`describe_elements`, one of Zeron's MCP tools,
-- lists them, and what each part of the window gives to what sits in it):
--
--   zeron.place { at = ".session-row", after = Badge }
--   zeron.place { at = "#session-header", before = Timer }
--   zeron.place { at = 'role:button "Send"', after = Hint }
--   zeron.place { at = "#chat-home", replace = Welcome }   (needs "ui.change"
--                                                           on that selector)
--   zeron.place { at = "slot:git.branch", after = PrButton }  (a slot another
--                                                           plugin offers:
--                                                           zeron.slot("branch"))
--
-- `Badge` is a function that builds a node (`function() return ui.text { … }
-- end`): it's built where each match is, once per match (a badge per session
-- row), as a component of its own (`ui.use_state` works in it), and reads
-- what that place gives it:
--
--   zeron.context("session")   the session there, or nil where there's none
--                              (in a tool: the one open in the window)
--   zeron.need("session")      the same, or: draw nothing here, and Settings
--                              → Plugins says "Focus timer needs a session,
--                              but it's in the title bar"
--
-- What's given: `session` (a sidebar row; the session page and all in it),
-- `sessions` (the sidebar), `message` (a message), `file` (a file's row or
-- tab), `panel` (a panel's tab), `code` (a reply's code block: { lang,
-- source }; `.md-code.lang-mermaid` is one in that language), `tex` (its
-- math), `tab` and `page` (your own tabs and pages), and everywhere
-- `profile`, `appearance`, `route`. A node that replaces another stands
-- where it stood and reads what's given above it: Zeron gives each of
-- these on what holds the element you replace. A record carries only what you were allowed (fields you weren't
-- are absent, not an error):
--
--   session   { id }  + title, status, project, device, updated, created
--                       with "sessions.read"
--   message   { id }  + role, text with "messages.read"
--   profile   { }     + id, name, username, avatar with "profile.read",
--                       email with "account.email" (unless you hide emails)
--   file      { id }  + path, name with "files.read" (else an id of its own)
--   appearance "light" | "dark";  route { name, session?, page? }
--
-- What else each part needs, in zeron-plugin.json's `permissions`:
--
--   zeron.place                  "ui.add" (adding), "ui.change" (replacing)
--   zeron.tool { name, run }
--       a tool agents can call; `run(args, call)` returns the answer, or
--       runs as a task and returns it later; error() fails the call
--       (the tool is listed in the manifest's "tools") ("tools")
--   zeron.menu { kind, id, label, run }
--       an item in Zeron's menus: kind "session" (a session's row; run
--       gets { id }), "file" (a file's row; { path, session }), or a menu
--       another plugin offers ("git.commit": a History row's) ("ui.add")
--   zeron.menu_entries(name, context)
--       the items others added to a menu this plugin offers ("<id>.<name>"),
--       as ContextMenu entries, run in their plugin with `context`
--   zeron.send(to, data)
--       data (anything JSON, 64 KB at most) to the same plugin on another of
--       your devices (a device id), or to all of them ("*"); a promise of
--       what happened on each: delivered, not_installed, not_allowed,
--       not_open or offline                                  ("devices")
--   zeron.on_message(function(message) end)
--       what arrives: { from = { id, name }, data, at }      ("devices")
--   zeron.open(route)    "home", { session = id }, { settings = page } ("navigate")
--   zeron.compose(text)  text into the message box, for you to send   ("compose")
--   zeron.compose_comment { file, line, note, to?, side?, snippet?, key?, session? }
--                        a review comment chip on a session's draft, sent as
--                        Zeron's review comments block (file:line, the note,
--                        the code under it)                              ("compose")
--   zeron.composed()     in a build: your comments still on a draft, by key
--                        ({ [key] = { session, hovered } }), reactive    ("compose")
--   zeron.toast(text)    a short note at the window's foot   (always)
--   zeron.notify { title, body?, session? }  a notification, as Zeron's
--                        own (Settings → Notifications)     ("notifications")
--   zeron.open_url(url)  a web link, where you open links (Settings →
--                        Links: the browser beside the chat, or yours)
--                                                            ("links.open")
--   zeron.need("panel").open_tab { kind, title, icon?, data? }
--       a tab of your own in the open chat's panel (on a page of yours,
--       the page's own panel), kept with its tabs
--                                                            ("panel.tabs")
--   zeron.tab(kind, function(tab) return <node> end)
--       how a kind of your tabs draws; `tab` is { kind, title, data }
--                                                            ("panel.tabs")
--   zeron.page { id, title, icon?, build = function(page) return <node> end }
--       a page of your own, listed in the sidebar           ("pages.add")
--       (What a place gives, `page` and `tab` among it, is read-only: hand
--       a component props of its own, `Board { page = page }`, not `page`.)
--   zeron.store(key, default)  what it keeps between runs: :get(), :set(v)
--   zeron.db.exec(sql, params) / zeron.db.query(sql, params)  its own
--       SQLite database (no permission; a promise)
--   zeron.run(program, args, { cwd, stdin, timeout, env })  a program its
--       manifest's "commands" names, as you: { status, stdout, stderr } ("commands")
--   zeron.settings()           its settings' values by id (declared in
--       zeron-plugin.json's "settings"; Zeron draws their page in Settings,
--       under Plugins, while the plugin is on), followed where read
--   zeron.setting(id)          one of them: :get() (followed), :peek()
--   zeron.on_setting(function(id, value, was) end)  told as one changes
--   zeron.set_status(id, { badge, tone?, text? })  what Zeron says of one
--       beside its control: a short badge in a tone ("neutral", "success",
--       "warning", "danger") and words in its description's place, plain
--       text; nil clears it
--   zeron.every(seconds, fn)   now and then, 30 seconds at least
--   zeron.github.watch_pr(session_id)  in a build function: a function
--       reading that session's pull request as it changes ("github"; your
--       GitHub sign-in stays Zeron's: no token reaches the plugin)
--
-- zgx's own plugin API works too (ui.contribute, ui.filter, ui.override);
-- zeron.place is ui.contribute with what's above around it. Some elements
-- are never reachable, whatever the selector: signing in, allowing plugins,
-- plugin and template settings.

type Call = { id: string, kind: string, tool: string?, item: string?, args: any, chat: string? }
type Message = { seq: number, from: { id: string, name: string }, data: any, at: number }
type Tool = { name: string, run: (args: any, call: { chat: string? }) -> any }
type Item = { kind: string, id: string, label: string, run: (context: any) -> () }

local made: { [string]: any } = {}

return function(id: string): any
	-- In the plugin's service (service.luau, run headless by the engine on
	-- each device), the engine's own helpers: zeron.serve, zeron.tool,
	-- zeron.every, zeron.on_stop, zeron.emit, zeron.run, zeron.fetch,
	-- zeron.notify, zeron.sleep, zeron.task, zeron.store, zeron.json,
	-- zeron.device (see the guide's "Services").
	local service = (_G :: any).zeron_service
	if service then
		return service(id)
	end
	if made[id] then
		return made[id]
	end
	local prefix = `plugin.{id}`
	local M = {}

	-- What each place gives, read where a node of this plugin is built: the
	-- app's `provide` above it (zgx carries it into a plugin's node), read
	-- as the build function runs. Outside a placed node (a tool, a menu
	-- item) it's the window's: its route, its look, you, and the session
	-- open in it. `M.read` is how, and a test may replace it.
	--
	-- Read what you need at the top of a build function and close over it:
	-- a function run later (a reactive prop) is outside the build, so it
	-- reads the window's. A record that changes (a session's status) is
	-- read in such a function by its id: `zeron.context_of("session", id)`;
	-- a list that moves (the sidebar's `sessions`) by `zeron.live(name)`.
	local building: string? = nil
	local spent: (at: string, ms: number) -> ()
	-- What the node being built has read of its place, by name: the
	-- provided value or state, kept so the node is built again as a state
	-- changes (a header's session as you switch).
	local capturing: { [string]: any }? = nil
	local NONE = {}
	-- A provide is a value or a state of one (a computed): its value, followed.
	local function unwrap(v: any): any
		-- (`type`, not `typeof`: a state's typeof is its own name.)
		local t = type(v)
		if t == "userdata" or t == "table" then
			local ok, get = pcall(function()
				return v.get
			end)
			if ok and type(get) == "function" then
				return v:get()
			end
		end
		return v
	end
	function M.read(name: string): any
		if building ~= nil then
			local raw: any = if capturing then capturing[name] else nil
			if raw == nil then
				local ok, value = pcall(ui.use_context, name, nil)
				raw = if ok and value ~= nil then value else NONE
				if capturing then
					capturing[name] = raw
				end
			end
			return if raw == NONE then nil else unwrap(raw)
		end
		if name == "route" then
			return ui.shared("zeron.route", { name = "home" }):get()
		elseif name == "appearance" then
			return ui.shared("zeron.appearance", "dark"):get()
		elseif name == "session" then
			return ui.shared("zeron.page_session", nil :: any):get()
		end
		return nil
	end

	-- What you were allowed of each, kept by Zeron for this plugin alone.
	local records = ui.shared(`{prefix}.records`, {} :: any)

	-- An id of this plugin's own for a file: the same file, the same id,
	-- for this plugin alone (another plugin's id for it differs).
	local function own_id(path: string): string
		local h = 5381
		for i = 1, #id do
			h = (h * 33 + string.byte(id, i)) % 2147483647
		end
		for i = 1, #path do
			h = (h * 33 + string.byte(path, i)) % 2147483647
		end
		return string.format("f%08x", h)
	end

	-- What's behind a token a place gave (a file's, a block's), as this
	-- plugin may read it: nil when it may not.
	local function behind(kind: string, token: any): any
		if type(token) ~= "string" then
			return nil
		end
		local ok, found = pcall(ui.call_sync, `{prefix}.{kind}`, token)
		return if ok then found else nil
	end

	function M.context(name: string): any
		-- You, as you allowed it (your account's id and name with
		-- "profile.read"): never from a place, which gives no account.
		if name == "profile" then
			return table.clone((records:get() or {}).profile or {})
		end
		local value = M.read(name)
		if name == "panel" then
			if type(value) == "table" and type(value.page) == "table" then
				-- A plugin's page's own panel (the page provides it).
				value = { page = { plugin = value.page.plugin, id = value.page.id } }
			else
				-- The panel of the session where this is (a tab of it, inside
				-- the panel; else the session there, or the window's).
				local session = M.read("session") or ui.shared("zeron.page_session", nil :: any):get()
				if session == nil then
					return nil
				end
				value = { id = value, session = session }
			end
		end
		if value == nil then
			return nil
		end
		local r = records:get() or {}
		if name == "session" or name == "message" then
			local found = (r[name .. "s"] or {})[value]
			local out = { id = value }
			for k, v in found or {} do
				out[k] = v
			end
			return out
		elseif name == "sessions" then
			local out = {}
			for _, sid in value do
				table.insert(out, M.context_of("session", sid))
			end
			return out
		elseif name == "panel_branch" or name == "panel_details" then
			-- The session, by its id: its device, branch and folder as its
			-- record holds them ("sessions.read"; the folder "files.read"'s).
			local sid = if type(value) == "table" then value.session else nil
			if type(sid) ~= "string" then
				return nil
			end
			local s = (r.sessions or {})[sid] or {}
			return { session = sid, device = s.device, cwd = s.cwd, branch = s.branch }
		elseif name == "code" or name == "tex" then
			-- A reply's block ("messages.read", or a plugin allowed to change
			-- how such blocks show): `{ lang, source }`, or the TeX.
			return behind("block", value)
		elseif name == "panel" then
			-- Plain data, and what can be done with it: a tab of this
			-- plugin's own ("panel.tabs").
			local panel = value
			-- (A session's panel, or a page's: a plugin's page has its own.)
			local session, page = panel.session, panel.page
			panel.open_tab = function(tab: { kind: string, title: string, icon: string?, data: any? })
				ui.invoke(`{prefix}.open_tab`, {
					session = session,
					page = page,
					kind = tab.kind,
					title = tab.title,
					icon = tab.icon,
					data = if tab.data == nil then ui.null else tab.data,
				})
			end
			return panel
		elseif name == "file" then
			-- A token: its path with "files.read", else an id of this
			-- plugin's own (the same file, the same id, this run).
			local f = if r.files_read then behind("file", value) else nil
			if type(f) == "table" and type(f.path) == "string" then
				return { id = f.path, path = f.path, name = f.name or (string.match(f.path, "([^/]+)$") or f.path) }
			end
			return { id = own_id(tostring(value)) }
		end
		return value
	end

	-- What a place gives, as it changes, for a reactive function: called in
	-- a build function, it returns a function reading `name` there as
	-- `zeron.context` does, live (the sidebar's `sessions` as they move).
	function M.live(name: string): () -> any
		local at, held = building, capturing
		M.read(name)
		return function(): any
			local was, was_capturing = building, capturing
			building, capturing = at, held
			local ok, value = pcall(M.context, name)
			building, capturing = was, was_capturing
			return if ok then value else nil
		end
	end

	-- `name`'s record for `value` (a session in a list of them).
	function M.context_of(name: string, value: any): any
		local found = ((records:get() or {})[name .. "s"] or {})[value]
		local out = { id = value }
		for k, v in found or {} do
			out[k] = v
		end
		return out
	end

	-- What a node needs, or it draws nothing (and Settings says why).
	local NEED = {}
	function M.need(name: string): any
		local found = M.context(name)
		if found == nil then
			error(setmetatable({ need = name }, NEED), 0)
		end
		return found
	end

	-- A node placed by selector: built at each match by `before`, `after` or
	-- `replace` (a function that builds it, run inside a component of its
	-- own, so `ui.use_state` works there), kept to itself: what fails in it
	-- draws nothing, and is said once, as this plugin's.
	type Build = (props: any) -> Node
	-- `fill`: what it puts there fills the room the element it replaces
	-- had (a panel's tab, a page), rather than taking only what it needs.
	type Place = { at: string, before: Build?, after: Build?, replace: Build?, fill: boolean? }
	-- What building this plugin's nodes costs, by place: how many builds,
	-- their time in all and at most, and the most time in one frame (a
	-- frame: builds within 16 ms of each other). Told to Zeron at most twice
	-- a second, so Settings can say when a plugin is slow.
	local costs: { [string]: { builds: number, total: number, most: number } } = {}
	local frame = { at = -1, ms = 0, most = 0 }
	local telling = false
	local function tell()
		telling = false
		local places = {}
		for at, c in costs do
			table.insert(places, { at = at, builds = c.builds, mean_ms = c.total / c.builds, max_ms = c.most })
		end
		ui.invoke(`{prefix}.cost`, { places = places, frame_ms = frame.most })
	end
	function spent(at: string, ms: number)
		local c = costs[at] or { builds = 0, total = 0, most = 0 }
		c.builds += 1
		c.total += ms
		c.most = math.max(c.most, ms)
		costs[at] = c
		local now = ui.now()
		if now - frame.at > 16 then
			frame.at, frame.ms = now, 0
		end
		frame.ms += ms
		frame.most = math.max(frame.most, frame.ms)
		if not telling then
			telling = true
			ui.after(500, tell)
		end
	end

	-- Each place this plugin made, and the ref its nodes carry: where they
	-- are, for an agent asking (`plugin_status`).
	local placed: { { at: string, how: string, ref: any } } = {}
	function M.place(p: Place)
		local inner = p.before or p.after or p.replace
		assert(inner ~= nil, "zeron.place needs before, after or replace")
		local here = ui.ref()
		table.insert(placed, { at = p.at, how = if p.before then "before" elseif p.after then "after" else "replace", ref = here })
		local Guard = ui.component(function(props: any): Node
			local captured: { [string]: any } = {}
			local function build(): Node
				local was, was_capturing = building, capturing
				building, capturing = p.at, captured
				local started = os.clock()
				local ok, node = pcall(inner, props)
				spent(p.at, (os.clock() - started) * 1000)
				building, capturing = was, was_capturing
				if ok then
					return ui.div { ref = here, w_full = p.fill, h_full = p.fill, node }
				end
				if type(node) == "table" and getmetatable(node) == NEED then
					ui.invoke(`{prefix}.problem`, { need = node.need, at = p.at })
				else
					ui.invoke(`{prefix}.problem`, { error = tostring(node), at = p.at })
				end
				return ui.div {}
			end
			local first = build()
			-- The states it read that name one thing (an id: the session in
			-- a header as you switch): built again as one changes, a new
			-- thing in its place. A list or a record is never a reason to
			-- build again (its node's state would go each time it moved):
			-- `zeron.live(name)` reads those as they change.
			local states = {}
			for _, raw in captured do
				local t = type(raw)
				if raw ~= NONE and (t == "userdata" or t == "table") then
					local ok, get = pcall(function()
						return raw.get
					end)
					if ok and type(get) == "function" then
						local now = raw:get()
						local kind = type(now)
						if kind == "string" or kind == "number" or kind == "boolean" or now == nil then
							table.insert(states, raw)
						end
					end
				end
			end
			if #states == 0 then
				return first
			end
			local initial = true
			return ui.presence(function(): string
				local key = {}
				for _, st in states do
					table.insert(key, tostring(st:get()))
				end
				return table.concat(key, "\0")
			end, function(_key: string): Node
				if initial then
					initial = false
					return first
				end
				return build()
			end)
		end)
		-- Another plugin's slot (`slot:<plugin>.<name>`, which that plugin
		-- offers with `zeron.slot`): added to it, in plugin order, reading
		-- what that plugin provides there.
		local slot = string.match(p.at, "^slot:(.+)$")
		if slot then
			if p.replace then
				ui.invoke(`{prefix}.problem`, { error = "a slot is added to, not replaced", at = p.at })
				return
			end
			local ok, why = pcall(ui.contribute, slot, Guard {})
			if not ok then
				ui.invoke(`{prefix}.problem`, { error = tostring(why), at = p.at })
			end
			return
		end
		do
			local spec: { [string]: any } = { target = p.at }
			if p.before then
				spec.before = Guard {}
			elseif p.after then
				spec.after = Guard {}
			else
				spec.replace = Guard {}
			end
			-- A place zgx refuses (a list's row, say) is said as this
			-- plugin's problem; its other places stand.
			local ok, why = pcall(ui.contribute, spec)
			if not ok then
				ui.invoke(`{prefix}.problem`, { error = tostring(why), at = p.at })
			end
		end
	end

	-- A place of this plugin's own where other plugins add things: a
	-- column holding what each adds (`zeron.place { at = "slot:<id>.<name>",
	-- after = … }`, "ui.add"), in plugin order, each reading what's
	-- provided around the slot (`provide = { … }` on an element above it:
	-- plain data, or a state of it, never a function). Named under this
	-- plugin's id, so no other plugin can offer the same one; `props` style
	-- the column.
	function M.slot(name: string, props: { [string]: any }?): Node
		return ui.slot(`{id}.{name}`, props or {})
	end

	-- Tabs of this plugin's own in a chat's panel ("panel.tabs"): one opens
	-- with `zeron.need("panel").open_tab { kind, title, icon?, data? }`, and
	-- Zeron keeps it with the panel's own tabs (closing, reordering, as the
	-- app restarts). `build` draws it: `tab` is { kind, title, data }.
	local tab_kinds: { [string]: (tab: any) -> Node } = {}
	local tabs_placed = false
	function M.tab(kind: string, build: (tab: any) -> Node)
		tab_kinds[kind] = build
		if tabs_placed then
			return
		end
		tabs_placed = true
		M.place {
			at = `.plugin-tab.plugin-{id}`,
			fill = true,
			replace = function(): Node
				local tab = M.need("tab")
				local draw = tab_kinds[tab.kind]
				if not draw then
					error(`{id} has no tab kind {tab.kind}`)
				end
				return draw(tab)
			end,
		}
	end

	-- A page of this plugin's own, listed in the sidebar ("pages.add").
	-- `build` draws it: `page` is { id, title }.
	local pages: { [string]: (page: any) -> Node } = {}
	local pages_placed = false
	-- (`title` a string, or a function of one: `zeron.t("page.title")`, its
	-- sidebar entry then following the language, as it changes, by being
	-- listed again under the same id.)
	function M.page(page: { id: string, title: string | () -> string, icon: string?, build: (page: any) -> Node })
		pages[page.id] = page.build
		if type(page.title) == "function" then
			local title = page.title :: () -> string
			local said: string? = nil
			ui.effect(function()
				local now = title()
				if now ~= said then
					said = now
					ui.invoke(`{prefix}.add_page`, { id = page.id, title = now, icon = page.icon })
				end
			end)
		else
			ui.invoke(`{prefix}.add_page`, { id = page.id, title = page.title, icon = page.icon })
		end
		if pages_placed then
			return
		end
		pages_placed = true
		M.place {
			at = `.plugin-page.plugin-{id}`,
			fill = true,
			replace = function(): Node
				local shown = M.need("page")
				local draw = pages[shown.id]
				if not draw then
					error(`{id} has no page {shown.id}`)
				end
				return draw(shown)
			end,
		}
	end

	-- GitHub, through Zeron's sign-in ("github"): the plugin never holds a
	-- token. `zeron.github.watch_pr(session_id)`, in a build function, keeps
	-- that session's pull request watched while the node shows (the engine
	-- pushes changes; nothing polls) and returns a function reading it:
	-- `{ number, title, url, state = "open" | "merged" | "closed", base,
	-- head }`, or nil for none (or not heard yet). It follows that session
	-- alone: another's moving tells it nothing.
	local github_heard = ui.shared(`{prefix}.github`, {} :: any)
	local watched: { [string]: number } = {}
	local asking = false
	local function ask_github()
		asking = false
		local list = {}
		for session, n in watched do
			if n > 0 then
				table.insert(list, session)
			end
		end
		table.sort(list)
		ui.invoke(`{prefix}.github_watch`, list)
	end
	local function soon()
		if not asking then
			asking = true
			ui.after(0, ask_github)
		end
	end
	M.github = {}
	-- A pull request itself, by its address on GitHub: a promise of what
	-- GitHub says of it (its title, body, state, author, branches, checks,
	-- files, comments and reviews, as Zeron's own viewer reads them), or of
	-- its diff. `refresh` asks GitHub again rather than what Zeron holds.
	function M.github.pr(url: string, refresh: boolean?): any
		return ui.call(`{prefix}.github_call`, { op = "pr", url = url, refresh = refresh == true })
	end
	-- A file's place in a pull request's changes on GitHub: a promise of
	-- its link (for `zeron.open_url`).
	function M.github.file_url(url: string, path: string): any
		return ui.call(`{prefix}.github_call`, { op = "file_url", url = url, path = path })
	end
	-- A comment on a pull request, as you ("github.write"): a promise.
	function M.github.comment(url: string, body: string): any
		return ui.call(`{prefix}.github_call`, { op = "comment", url = url, body = body })
	end
	-- (`at`: { head, base } commit ids, the diff at those revisions.)
	function M.github.diff(url: string, refresh: boolean?, at: { head: string?, base: string? }?): any
		return ui.call(`{prefix}.github_call`, {
			op = "diff",
			url = url,
			refresh = refresh == true,
			headRefOid = if at and at.head then at.head else ui.null,
			baseRefOid = if at and at.base then at.base else ui.null,
		})
	end
	-- The same calls for a session's pull request, asked on the device its
	-- checkout lives on: `zeron.github.at(session_id).pr(url)`, and as you
	-- ("github.write"): `.line_comment(url, { path, line, side = "RIGHT" |
	-- "LEFT", commit }, body)`, `.reply(url, thread_id, body)`,
	-- `.resolve(url, thread_id, resolved)`.
	-- (`project`: for a pull request opened from a project's list, not a
	-- session; asked on the project's device.)
	function M.github.at(session: string?, project: string?): any
		local function ask(fields: { [string]: any }): any
			fields.session = session or ui.null
			fields.project = project or ui.null
			return ui.call(`{prefix}.github_call`, fields)
		end
		local on = {}
		function on.pr(url: string, refresh: boolean?): any
			return ask({ op = "pr", url = url, refresh = refresh == true })
		end
		function on.diff(url: string, refresh: boolean?, at: { head: string?, base: string? }?): any
			return ask({
				op = "diff",
				url = url,
				refresh = refresh == true,
				headRefOid = if at and at.head then at.head else ui.null,
				baseRefOid = if at and at.base then at.base else ui.null,
			})
		end
		function on.comment(url: string, body: string): any
			return ask({ op = "comment", url = url, body = body })
		end
		function on.line_comment(url: string, where: { path: string, line: number, side: string?, commit: string }, body: string): any
			return ask({
				op = "line_comment",
				url = url,
				body = body,
				path = where.path,
				line = where.line,
				side = where.side or "RIGHT",
				commitId = where.commit,
			})
		end
		function on.reply(url: string, thread: string, body: string): any
			return ask({ op = "reply", url = url, thread = thread, body = body })
		end
		function on.resolve(url: string, thread: string, resolved: boolean): any
			return ask({ op = "resolve", url = url, thread = thread, resolved = resolved })
		end
		function on.file_url(url: string, path: string): any
			return ask({ op = "file_url", url = url, path = path })
		end
		return on
	end

	-- A project's repository on GitHub (`owner/name`, or nil), found from
	-- its folder where it lives (the folder stays Zeron's), and that
	-- repository's open pull requests a page at a time: `filter` is
	-- "authored", "reviewing" or "all"; `after`, the last page's
	-- `nextCursor`; `cached`, the first page as Zeron last kept it.
	function M.github.repository(project: string): any
		return ui.call(`{prefix}.github_call`, { op = "repository", project = project })
	end
	function M.github.list(project: string?, repository: string, opts: { filter: string?, after: string?, refresh: boolean?, cached: boolean? }?): any
		local o = opts or {}
		return ui.call(`{prefix}.github_call`, {
			op = "list",
			project = project or ui.null,
			repository = repository,
			filter = o.filter or "authored",
			after = o.after or ui.null,
			refresh = o.refresh == true,
			cached = o.cached == true,
		})
	end

	-- Your sessions by id (each as `zeron.context_of("session", id)` reads
	-- it; "sessions.read"), as they change.
	function M.sessions(): { [string]: any }
		return (records:get() or {}).sessions or {}
	end

	-- Your projects by id (`{ name, device }`; "sessions.read"), as they change.
	function M.projects(): { [string]: any }
		return (records:get() or {}).projects or {}
	end

	function M.github.watch_pr(session: string): () -> any
		watched[session] = (watched[session] or 0) + 1
		soon()
		pcall(ui.on_cleanup, function()
			local n = (watched[session] or 1) - 1
			watched[session] = if n > 0 then n else nil
			soon()
		end)
		-- (That session's entry alone: a pull request moving for another
		-- session changes nothing the caller reads, so its node isn't told.)
		local mine = ui.computed(function(): any
			local pr = (github_heard:get() or {})[session]
			return if pr == false then nil else pr
		end)
		return function(): any
			return mine:get()
		end
	end

	-- Every watched session's pull request as last heard (`{ [session] =
	-- pr | false }`), read as a state is: for the plugin's own bookkeeping
	-- (what to notify of), never for a row's drawing (`watch_pr`'s, its own).
	function M.github.heard(): any
		return github_heard:get() or {}
	end

	-- A program its manifest names (`"commands": ["gh"]`, "commands"), run
	-- as you on this Mac, with `args` passed as they are (no shell):
	--   zeron.run("gh", { "api", "user" }):after(function(r) … r.stdout … end)
	-- A promise of { status, stdout, stderr }; `opts`: { cwd, stdin,
	-- timeout } (seconds, 60 unless said, 300 at most).
	function M.run(program: string, args: { string }?, opts: { cwd: string?, stdin: string?, timeout: number?, env: { [string]: string }? }?): any
		local o = opts or {}
		return ui.call(`{prefix}.run`, { program = program, args = args or {}, cwd = o.cwd, stdin = o.stdin, timeout = o.timeout, env = o.env })
	end

	-- This plugin's own database (SQLite; no permission, removed with the
	-- plugin, 100 MB at most). Parameters are bound, never put in the SQL:
	--   zeron.db.exec("CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT)")
	--   zeron.db.exec("INSERT OR REPLACE INTO kv VALUES (?, ?)", { k, v })
	--   zeron.db.query("SELECT v FROM kv WHERE k = ?", { k }):after(function(rows) … end)
	-- `exec` answers { changes, last_id } (no params: a script of several
	-- statements); `query`, its rows, each { column = value }.
	M.db = {
		exec = function(sql: string, params: { any }?): any
			return ui.call(`{prefix}.db`, { op = "exec", sql = sql, params = params or {} })
		end,
		query = function(sql: string, params: { any }?): any
			return ui.call(`{prefix}.db`, { op = "query", sql = sql, params = params or {} })
		end,
	}

	-- What this plugin keeps between runs, for it alone (Zeron keeps it, a
	-- megabyte at most; removing the plugin removes it):
	--   local seen = zeron.store("seen", {})
	--   seen:get()      (followed where it's read)   seen:set(value)
	local kept = ui.shared(`{prefix}.store`, {} :: any)
	function M.store(key: string, default: any): any
		local box = {}
		function box.get(_self: any): any
			local v = (kept:get() or {})[key]
			return if v == nil then default else v
		end
		function box.peek(_self: any): any
			local v = (kept:peek() or {})[key]
			return if v == nil then default else v
		end
		function box.set(_self: any, value: any)
			ui.invoke(`{prefix}.store_set`, { key = key, value = if value == nil then ui.null else value })
		end
		return box
	end

	-- Its own words, in the language Zeron shows (Settings → General →
	-- Language): `locales/<locale>.json` beside zeron-plugin.json, `en.json`
	-- the source, nested keys, ICU messages (`{name}`, `{n, plural, one {#
	-- item} other {# items}}`, `{x, select, … other {…}}`):
	--   ui.text { zeron.t("timer.start") }          follows the language
	--   ui.text { zeron.t("timer.left", { n = 3 }) }
	--   tooltip = zeron.s("timer.hint")             the text now, a string
	-- A language the plugin lacks, or a key, falls back to its English (then
	-- the key itself). Without a locales folder it all says the key: use
	-- plain strings instead. (Zeron's own words, the design system's, follow
	-- the language by themselves.)
	local words = ui.shared(`{prefix}.locales`, {} :: any)
	-- (The locale Zeron shows, as it shares it.)
	local locale_shown = ui.shared("zeron.locale", "en")
	local function shown(): string
		local locale = locale_shown:get()
		return if type(locale) == "string" then locale else "en"
	end
	local formats: ((string, { [string]: any }?) -> string)? = nil
	local function format(msg: string, args: { [string]: any }?): string
		if not formats then
			local ok, locale = pcall(require, "@ds/locale")
			formats = if ok and type(locale) == "table" and locale.format then locale.format else function(m: string): string
				return m
			end
		end
		return (formats :: any)(msg, args)
	end
	function M.s(key: string, args: { [string]: any }?): string
		local all = words:get() or {}
		-- The locale shown, its parents (zh-Hans, then zh), then English.
		local locale: string? = shown()
		while locale and locale ~= "" do
			local these = all[locale]
			if these and these[key] then
				return format(these[key], args)
			end
			locale = string.match(locale, "^(.*)%-[^-]*$")
		end
		local english = all.en
		if english and english[key] then
			return format(english[key], args)
		end
		return key
	end
	function M.t(key: string, args: { [string]: any }?): () -> string
		return function(): string
			return M.s(key, args)
		end
	end

	-- Its settings: declared in zeron-plugin.json's "settings", each
	-- `{ id, kind, label, description?, default? }` (kinds: switch, choice,
	-- text, number, shortcut, folder). Zeron draws their page and keeps
	-- what the person sets, for this plugin alone; a setting never touched
	-- reads its default. Read only here: only the person sets them.
	--   zeron.settings().tone      every value, followed where it's read
	--   zeron.setting("tone"):get()
	--   zeron.on_setting(function(id, value, was) … end)
	local settings_now = ui.shared(`{prefix}.settings`, {} :: any)
	function M.settings(): { [string]: any }
		return table.clone(settings_now:get() or {})
	end
	function M.setting(key: string): any
		local box = {}
		function box.get(_self: any): any
			return (settings_now:get() or {})[key]
		end
		function box.peek(_self: any): any
			return (settings_now:peek() or {})[key]
		end
		return box
	end
	-- Told of each change as it comes (not of the values it starts with).
	local setting_listeners: { (id: string, value: any, was: any) -> () } = {}
	local setting_seen: { [string]: any }? = nil
	local function tell_setting(key: string, value: any, was: any)
		for _, listen in setting_listeners do
			local ok, why = pcall(listen, key, value, was)
			if not ok then
				print(`{id}: a setting's listener failed: {why}`)
			end
		end
	end
	-- A status for one of its settings, drawn by Zeron beside its control
	-- (gone when the plugin is off): `{ badge = "Ready", tone = "success" }`,
	-- `text` in the setting's description's place; nil clears it.
	function M.set_status(key: string, status: { badge: string?, tone: string?, text: string? }?)
		ui.invoke(`{prefix}.setting_status`, { id = key, status = if status == nil then ui.null else status })
	end
	function M.on_setting(listen: (id: string, value: any, was: any) -> ())
		table.insert(setting_listeners, listen)
		if setting_seen ~= nil then
			return
		end
		setting_seen = table.clone(settings_now:peek() or {})
		ui.effect(function()
			local now = settings_now:get() or {}
			local was = setting_seen or {}
			setting_seen = table.clone(now)
			-- (Its values arriving at first are what it starts with.)
			if next(was) == nil then
				return
			end
			for key, value in now do
				if was[key] ~= value then
					tell_setting(key, value, was[key])
				end
			end
			for key, value in was do
				if now[key] == nil then
					tell_setting(key, nil, value)
				end
			end
		end)
	end

	-- Something to do now and then, with no window part (a check that has
	-- nothing to push it): every `seconds`, 30 at least; what fails in it is
	-- said, and it goes on. Returns a function that stops it.
	function M.every(seconds: number, run: () -> ()): () -> ()
		return ui.every(math.max(30, seconds) * 1000, function()
			local ok, why = pcall(run)
			if not ok then
				print(`{id}: an every() run failed: {why}`)
			end
		end)
	end

	local tools: { [string]: Tool } = {}
	local items: { [string]: Item } = {}

	local function answer(call: Call, ok: boolean, value: any)
		if ok then
			ui.invoke(`{prefix}.result`, { id = call.id, ok = true, value = if value == nil then ui.null else value })
		else
			ui.invoke(`{prefix}.result`, { id = call.id, ok = false, error = tostring(value) })
		end
	end

	-- Where this plugin's nodes are now: each place, how many it matched,
	-- and their boxes in the window.
	local function where(): any
		local out = {}
		for _, p in placed do
			local regions = {}
			local ok, elements = pcall(function()
				return p.ref:elements()
			end)
			for _, e in (if ok then elements else {}) :: { any } do
				table.insert(regions, { x = e.x, y = e.y, w = e.width, h = e.height })
			end
			table.insert(out, { at = p.at, how = p.how, count = #regions, regions = regions })
		end
		return { placements = out }
	end

	local function run(call: Call)
		if call.kind == "where" then
			answer(call, true, where())
			return
		end
		if call.kind == "menu" then
			local item = items[call.item or ""]
			if item then
				local ok, why = pcall(item.run, call.args)
				if not ok then
					print(`{id}: {call.item} failed: {why}`)
				end
			end
			answer(call, true, nil)
			return
		end
		local tool = tools[call.tool or ""]
		if not tool then
			answer(call, false, `{id} has no tool {call.tool}`)
			return
		end
		-- A task, so a tool can await (a fetch) before it answers.
		ui.task(function()
			local ok, value = pcall(tool.run, call.args or {}, { chat = call.chat })
			answer(call, ok, value)
		end)
	end

	-- Calls waiting for this plugin (tools, menu items): each run once, as
	-- it arrives. The host takes a call off the list once it's answered.
	local calls = ui.shared(`{prefix}.calls`, {} :: { Call })
	-- (Those started are kept across a reload of the plugin, so a call
	-- still running isn't run again.)
	local running = ui.state(`{prefix}.started`, {} :: { [string]: boolean })
	ui.effect(function()
		local now = calls:get()
		local started = table.clone(running:peek())
		local waiting: { [string]: boolean } = {}
		local fresh: { Call } = {}
		for _, call in now do
			waiting[call.id] = true
			if not started[call.id] then
				started[call.id] = true
				table.insert(fresh, call)
			end
		end
		for seen in started do
			if not waiting[seen] then
				started[seen] = nil
			end
		end
		running:set(started)
		for _, call in fresh do
			run(call)
		end
	end)

	function M.tool(tool: Tool)
		tools[tool.name] = tool
	end

	local function publish_items()
		local list = {}
		for _, item in items do
			table.insert(list, { kind = item.kind, id = item.id, label = item.label })
		end
		table.sort(list, function(a, b)
			return a.id < b.id
		end)
		ui.invoke(`{prefix}.menus`, list)
	end

	function M.menu(item: Item)
		items[item.id] = item
		publish_items()
	end

	-- Menus this plugin offers others (`<id>.<name>`: a row's own menu):
	-- what other plugins added to `name` (`zeron.menu { kind =
	-- "<id>.<name>", … }`), as ContextMenu entries under each plugin's
	-- name, run there with `context` (plain data) as chosen. Read as the
	-- menu opens.
	local offered = ui.shared(`{prefix}.menu_items`, {} :: { any })
	function M.menu_entries(name: string, context: { [string]: any }?): { any }
		local kind = `{id}.{name}`
		local out: { any } = {}
		local heading: string? = nil
		for _, item in offered:peek() or {} do
			if item.kind == kind then
				if heading ~= item.plugin then
					heading = item.plugin
					table.insert(out, { heading = item.plugin_name })
				end
				local plugin, item_id = item.plugin, item.id
				table.insert(out, {
					label = item.label,
					on_select = function()
						ui.invoke(`{prefix}.menu_run`, { plugin = plugin, id = item_id, kind = kind, context = if context == nil then ui.null else context })
					end,
				})
			end
		end
		return out
	end

	-- Its service's events (`zeron.emit` in service.luau), by device
	-- (`@here`: this device's) and event.
	local service_listeners: { [string]: { [string]: { (any, any) -> () } } } = {}
	local function heard_event(message: any)
		local keys = { message.from and message.from.id or "" }
		if message.here then
			table.insert(keys, "@here")
		end
		for _, key in keys do
			local by_event = service_listeners[key]
			for _, listen in (by_event and by_event[message.event]) or {} do
				local ok, why = pcall(listen, message.data, message)
				if not ok then
					print(`{id}: a service event's listener failed: {why}`)
				end
			end
		end
	end

	-- Messages from your other devices, each heard once, in order.
	local inbox = ui.shared(`{prefix}.inbox`, {} :: { Message })
	local listeners: { (Message) -> () } = {}
	local heard = 0
	ui.effect(function()
		local last = heard
		for _, message in inbox:get() do
			if message.seq > heard then
				heard = message.seq
				if (message :: any).event ~= nil then
					heard_event(message)
					continue
				end
				for _, listen in listeners do
					local ok, why = pcall(listen, message)
					if not ok then
						print(`{id}: a message's listener failed: {why}`)
					end
				end
			end
		end
		if heard > last then
			ui.invoke(`{prefix}.ack`, heard)
		end
	end)

	function M.on_message(listen: (Message) -> ())
		table.insert(listeners, listen)
	end

	function M.send(to: string, data: any): any
		return ui.call(`{prefix}.send`, { to = to, data = data })
	end

	function M.devices(): any
		return ui.shared("zeron.devices", {} :: { any })
	end

	-- The window it's drawn in: whether it shows (not hidden, minimised or
	-- covered by another) and whether it's the one in front. Read as a
	-- state is (an effect or computed reading them runs again as they
	-- change); true where the app doesn't say. Leave work no one would see
	-- for when it shows. (`peek`: read as `:peek()` reads, followed by
	-- nothing.)
	function M.window(peek: boolean?): { shown: boolean, active: boolean }
		local ok, shown, active = pcall(function(): (any, any)
			local s, a = ui.shared("zgx.window.shown", true), ui.shared("zgx.window.active", true)
			if peek then
				return s:peek(), a:peek()
			end
			return s:get(), a:get()
		end)
		if not ok then
			return { shown = true, active = true }
		end
		return { shown = shown ~= false, active = active ~= false }
	end

	-- Its service ("service": "service.luau", with the "service"
	-- permission): on this device (no device), or on another of yours by
	-- its id (`zeron.devices()`). Each device runs it only if it allowed
	-- the plugin there, in the same version.
	--   local git = zeron.service(session.device)
	--   git:call("status", { cwd = session.worktree }):after(function(s) … end,
	--       function(err) if zeron.reason(err) == "offline" then … end end)
	--   git:on("progress", function(data) … end)   -- its zeron.emit("progress", …)
	-- `:call(method, args, { timeout })` is a promise of what the method
	-- returned (30 s unless said, 300 at most); a refusal fails it with
	-- "<reason>: <words>" (`zeron.reason(err)` reads the reason).
	local services: { [string]: any } = {}
	function M.service(device: string?): any
		local key = device or "@here"
		if services[key] then
			return services[key]
		end
		local service = {}
		function service.call(_self: any, method: string, args: any?, opts: { timeout: number? }?): any
			return ui.call(`{prefix}.service_call`, {
				device = device or ui.null,
				method = method,
				args = if args == nil then ui.null else args,
				timeout = (opts and opts.timeout) or ui.null,
			})
		end
		-- `fn(data, event)` for each `zeron.emit(event, data)` it makes;
		-- returns a function that stops listening.
		function service.on(_self: any, event: string, fn: (any, any) -> ()): () -> ()
			local by_event = service_listeners[key] or {}
			service_listeners[key] = by_event
			local list = by_event[event] or {}
			by_event[event] = list
			table.insert(list, fn)
			if device then
				-- (Another device's events reach this one while it's watched.)
				ui.call(`{prefix}.service_call`, { device = device, method = "@watch", args = ui.null, timeout = ui.null })
					:after(nil, function(why)
						print(`{id}: watching {device}'s service: {why}`)
					end)
			end
			return function()
				local i = table.find(list, fn)
				if i then
					table.remove(list, i)
				end
			end
		end
		services[key] = service
		return service
	end

	-- A failed service call's reason: "offline", "not_installed",
	-- "not_approved", "off", "version_mismatch", "timeout", "failed", …
	function M.reason(err: any): string
		local text = tostring(err)
		return string.match(text, "^([%a_]+):") or string.match(text, "^([%a_]+)$") or "failed"
	end

	function M.open(route: any)
		ui.invoke("zeron.open", route)
	end

	-- A notification, as Zeron's own banners show ("notifications"): under
	-- Settings → Notifications (off, or held while Zeron is in front), a few
	-- a minute at most; `session`, the session a click opens. A promise of
	-- what came of it: "posted", "off", "held", "paced" or "unavailable".
	function M.notify(n: { title: string, body: string?, session: string? }): any
		return ui.call(`{prefix}.notify`, { title = n.title, body = n.body or "", session = n.session or ui.null })
	end

	function M.open_url(url: string)
		ui.invoke(`{prefix}.open_url`, url)
	end

	function M.compose(text: string)
		ui.invoke("zeron.compose.insert", text)
	end

	-- A review comment on a file or a diff, as a chip on a session's draft
	-- (the one shown without `session`): `file` the project's path, `line`
	-- (and a range's last, `to`), `side` "L" (the original file's line) or
	-- "R" (the changed one's) on a diff, `snippet` the code it's on, `note`
	-- what you say. Sent as Zeron's review comments block. `key`: yours, to
	-- know it by in `composed()` while it's carried.
	function M.compose_comment(c: { file: string, line: number, note: string, to: number?, side: string?, snippet: string?, key: string?, session: string? })
		ui.invoke(`{prefix}.compose_comment`, {
			file = c.file,
			line = c.line,
			note = c.note,
			to = c.to or ui.null,
			side = c.side or ui.null,
			snippet = c.snippet or ui.null,
			key = c.key or ui.null,
			session = c.session or ui.null,
		})
	end

	-- Your comments still on a draft (not sent, not taken off), by their
	-- `key`: the session's draft each is on, and whether its chip is under
	-- the pointer. Read in a build: it changes as they do.
	local composed = ui.shared(`{prefix}.composed`, {} :: { [string]: any })
	function M.composed(): { [string]: { session: string?, hovered: boolean } }
		return composed:get() or {}
	end

	function M.toast(text: string)
		ui.invoke(`{prefix}.toast`, text)
	end

	made[id] = M
	return M
end
