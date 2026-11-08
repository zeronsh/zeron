-- The window's one composer, wired: it travels between a new chat (lifted
-- to a little over the page's middle, opened out, its menus below) and a
-- session (docked at the foot, its menus above), on one critically damped
-- spring, as the gallery's App shell has it (Zeron's composer_dock.rs). It
-- is made once and follows where the shell is: the draft and what it
-- carries are each chat's own (screens/chat/shared), its model, effort
-- and access the session's (changed there, they change the session) or,
-- on a new chat, those the last new chat used.
--
-- In a session it sends to that session (queued while a turn runs, which
-- Stop ends), and shows what the agent asks (an approval, a question) in
-- the pill, the queue, the plan and what runs behind it in its trays. On a
-- new chat it starts a chat in the route's project and opens it.
--
-- `Composer()` is placed once over the page pane (absolute, filling it):
-- it lays itself out.

local ds = require("@ds")
local motion = require("@ds/motion")
local T = require("@ds/transcript")
local Quote = require("@ds/quote")
local Comments = require("@app/screens/chat/comments")
local Annotation = require("@ds/annotation")
local prefs = require("@app/screens/settings/prefs")
local store = require("@app/screens/chat/store")
local route = require("@app/screens/shell/route")
local art = require("@app/screens/shell/art")
local shared = require("@app/screens/chat/shared")
local sessions = require("@account/sessions")
local LinkNotice = require("@app/screens/chat/link_notice")
local files = require("@app/screens/files/api")
local Where = require("@app/screens/chat/where")
local DropOverlay = require("@ds/drop_overlay")
local mentions = require("@ds/composer_mentions")
local Invocations = require("@app/screens/chat/invocations")
local ZeronCommands = require("@app/screens/chat/zeron_commands")
-- (Loaded with the chat page: captures land on its composer.)
local Appshots = require("@app/screens/chat/appshots")

-- The title bar's height (Zeron's): a new chat centres under it.
local TITLE_H = 38
-- Zeron's dock spring (ω ≈ 27).
local DOCK: SpringOptions = { stiffness = 730, damping = 54 }
-- Its gap to the window's foot, docked. Its width, rising and docked: the
-- transcript's column (Settings' "Conversation width"), as Zeron's
-- composer lines up with the conversation over it (its edges on the
-- column's).
local FOOT = require("@app/screens/chat/dock").GAP

-- What a new chat starts with: what the last one used.
local defaults = ui.persist("chat_new_defaults", {
	model = nil :: string?,
	effort = nil :: string?,
	fast = false,
	access = "full" :: string?,
	plan = false,
	checkout = "worktree" :: string?,
	device = nil :: string?,
})

local FIELDS = { "model", "effort", "fast", "options", "access", "plan" }
-- Those a session takes only with a message sent (shared.picks keeps them
-- meanwhile); the rest change it as they're picked.
local DRAFTED: { [string]: boolean } = { model = true, effort = true, fast = true, options = true }
-- Whether two choices are the same: the options' picks (tables) by their
-- entries, none and an empty table alike; the rest by value.
local function equal(a: any, b: any): boolean
	if type(a) == "table" or type(b) == "table" then
		local x: { [string]: any } = if type(a) == "table" then a else {}
		local y: { [string]: any } = if type(b) == "table" then b else {}
		for k, v in x do
			if y[k] ~= v then
				return false
			end
		end
		for k, v in y do
			if x[k] ~= v then
				return false
			end
		end
		return true
	end
	return a == b
end
-- The tray open behind the composer (its todos, tasks or queue), kept
-- across a reload.
local TRAY = ui.state("chat_tray", nil :: ("todos" | "tasks" | "queue")?)

local function Composer(): Node
	-- The session shown, or nil on a new chat.
	local session_id = ui.computed(function(): string?
		local now = route.current:get()
		return if now.name == "session" then now.session else nil
	end)
	local function new_chat(): boolean
		return session_id:get() == nil
	end
	-- (Set as it sends from a new chat: the move to the session that
	-- follows slides, as the prompt glides; see `place`.)
	local slide_next = false
	-- Its form (a new chat's, opened out; a session's): the place it shows
	-- at's, changing with its place (see `place`).
	local form_new = ui.state("chat_composer_form_new", session_id:peek() == nil)
	form_new:set(session_id:peek() == nil)
	-- Moved as it switches between them (ds.Composer's `snap`): its form and
	-- size there at once, not eased.
	local snap = ui.state("chat_composer_snap", 0)
	-- Whether its model picker is open: the composer is held where it is
	-- while it is (the picker hangs from its chip; a new chat's composer,
	-- re-centring as its draft grows or shrinks, moved the chip and the
	-- picker with it on every frame of its glide), and goes to its place
	-- as the picker closes.
	local model_open = ui.state("chat_model_open", false)
	local function shown_new(): boolean
		return form_new:get()
	end
	-- The open session, once it is the one shown.
	local function open(): store.Session?
		local s = store.session:get()
		local id = session_id:get()
		if s and id and s.id == id and s.state == "ready" then
			return s
		end
		return nil
	end

	-- The choices, bound to the composer, following the session (or a new
	-- chat's defaults) and changing it.
	local model = ui.state("chat_model", nil :: string?)
	local effort = ui.state("chat_effort", nil :: string?)
	local fast = ui.state("chat_fast", false)
	-- The choices made of the model's options (Devin Fusion's lead and
	-- sidekick), by option id.
	local options = ui.state("chat_options", {} :: { [string]: string })
	local access = ui.state("chat_access", "full" :: string?)
	local plan = ui.state("chat_plan", false)
	local device = ui.state("chat_device", nil :: string?)
	local checkout = ui.state("chat_checkout", "worktree" :: string?)
	local branch = ui.state("chat_branch", nil :: string?)
	local project = ui.state("chat_project", nil :: string?)
	local locals: { [string]: State<any> } =
		{ model = model, effort = effort, fast = fast, options = options, access = access, plan = plan }

	-- In: what the session says (or the defaults, arriving on a new chat).
	-- `applied`: what the choices were set to from the session, so only a
	-- choice made here goes out to it (a session whose config isn't known,
	-- or has no model, is never handed this composer's).
	local applied: { [string]: any } = {}
	-- The chat it last followed ("?" before any): kept, so a reload isn't
	-- an arrival (a new chat's choices put back to its defaults).
	local was = ui.state("chat_composer_was", "?" :: string?)
	ui.effect(function()
		local id = session_id:get()
		local s = open()
		ui.batch(function()
			if id == nil then
				if was:peek() ~= nil then
					local d = defaults:peek()
					for _, k in FIELDS do
						local v = (d :: any)[k]
						if v ~= nil and not equal(locals[k]:peek(), v) then
							locals[k]:set(v)
						end
					end
					checkout:set(d.checkout)
					-- (A device asked of this new chat, a device's "+" in the
					-- sidebar, over the one last used.)
					device:set(route.take_device() or d.device)
					project:set(route.current:peek().project)
				end
			elseif s then
				-- (Its own pick, unsent, over what the session runs with.)
				local pick = shared.picks:peek()[id] or {}
				for _, k in FIELDS do
					local v = (s :: any)[k]
					if DRAFTED[k] and pick[k] ~= nil then
						v = pick[k]
					end
					if v ~= nil and not equal(locals[k]:peek(), v) then
						locals[k]:set(v)
					end
					applied[k] = if v ~= nil then v else locals[k]:peek()
				end
				if s.device and device:peek() ~= s.device then
					device:set(s.device)
				end
				if s.project and project:peek() ~= s.project then
					project:set(s.project)
				end
			end
		end)
		if was:peek() ~= id then
			was:set(id)
		end
	end)
	-- A new chat's project follows the route's: as the route comes home, or
	-- its project changes there. (Kept, so a reload, the route as it was,
	-- leaves a project picked in the composer alone.)
	local route_was = ui.state("chat_composer_route", "")
	ui.effect(function()
		local now = route.current:get()
		local key = `{now.name}:{now.project or ""}`
		if route_was:peek() == key then
			return
		end
		route_was:set(key)
		if now.name == "home" and project:peek() ~= now.project then
			project:set(now.project)
		end
	end)
	-- A device asked of a new chat already showing (a device's "+" in the
	-- sidebar, the composer here): picked, with the route's project. (One
	-- asked on the way here from a session is the arrival's, above.)
	ui.effect(function()
		local pick = route.device_pick:get()
		if pick == nil or session_id:peek() ~= nil or was:peek() ~= nil then
			return
		end
		local d = route.take_device()
		ui.batch(function()
			-- (None: the device's own pick, screens/chat/where.)
			local p = route.current:peek().project
			if project:peek() ~= p then
				project:set(p)
			end
			if d and device:peek() ~= d then
				device:set(d)
			end
		end)
	end)
	-- Out: a choice made here changes the session, or is what the next new
	-- chat starts with.
	ui.effect(function()
		local picked: { [string]: any } = {}
		for _, k in FIELDS do
			picked[k] = locals[k]:get()
		end
		local where: { [string]: any } = { checkout = checkout:get(), device = device:get() }
		local id = session_id:peek()
		if id == nil then
			local d = table.clone(defaults:peek()) :: any
			local changed = false
			for k, v in picked do
				if not equal(d[k], v) then
					d[k] = v
					changed = true
				end
			end
			for k, v in where do
				if d[k] ~= v then
					d[k] = v
					changed = true
				end
			end
			if changed then
				defaults:set(d)
			end
			return
		end
		local s = store.session:peek()
		if not s or s.id ~= id or s.state ~= "ready" then
			return
		end
		-- The agent, model and effort: kept as this chat's pick (nothing of
		-- the session changes until a message goes with them); the pick gone
		-- once it is what the session runs with again.
		local pick: { [string]: any } = {}
		local picking = false
		for k in DRAFTED do
			local v = picked[k]
			local own = (s :: any)[k]
			-- (Fast off is what a session with none set runs with.)
			if v ~= nil and not equal(own, v) and not (v == false and own == nil) then
				pick[k] = v
				picking = true
			end
		end
		local all = shared.picks:peek()
		local was_pick = all[id]
		local same = (was_pick == nil) == not picking
		if same and was_pick then
			for k in DRAFTED do
				if not equal(was_pick[k], pick[k]) then
					same = false
				end
			end
		end
		if not same then
			local next = table.clone(all)
			next[id] = if picking then pick else nil
			shared.picks:set(next)
		end
		local patch: { [string]: any } = {}
		local any_change = false
		for k, v in picked do
			if not DRAFTED[k] and v ~= nil and applied[k] ~= v and (s :: any)[k] ~= v then
				patch[k] = v
				applied[k] = v
				any_change = true
			end
		end
		if any_change then
			-- A model of another agent takes its agent with it.
			if patch.model then
				for _, m in store.catalog:peek().models do
					if m.value == patch.model then
						patch.harness = m.harness
					end
				end
			end
			store.configure(id, patch)
		end
	end)

	-- The device it runs on (the session's, or where a new chat is made):
	-- the model picker lists that device's agents and models, as the
	-- shipping app's does. (nil: this one, or not known yet.)
	local catalog_device = ui.state("chat_catalog_device", nil :: string?)
	ui.effect(function()
		local d = device:get()
		if d == nil and session_id:get() == nil then
			local p = route.project:get()
			d = if p then p.device else nil
		end
		if d ~= catalog_device:peek() then
			catalog_device:set(d)
			store.catalog_on(d)
		end
	end)
	-- A new chat moved onto Cloud runs only the agents Cloud has accounts
	-- for: a model it doesn't run gives way to Cloud's first, as the
	-- shipping app drops one on the switch.
	ui.effect(function()
		local cat = store.catalog:get()
		if session_id:get() ~= nil or device:get() ~= Where.CLOUD or (cat :: any).device ~= Where.CLOUD then
			return
		end
		local picked = model:peek()
		for _, m in cat.models do
			if m.value == picked then
				return
			end
		end
		local first = cat.models[1]
		if first then
			model:set(first.value)
			if first.effort then
				effort:set(first.effort)
			end
		end
	end)
	-- Where a new chat works (screens/chat/where): the devices, the chosen
	-- one's projects, its refs, kept in step and remembered; how it's sent.
	local where = Where.bind {
		device = device,
		project = project,
		checkout = checkout,
		branch = branch,
		active = new_chat,
	}

	local running = ui.computed(function(): boolean
		local s = open()
		return s ~= nil and s.running == true
	end)
	-- What the agent asks, and which of a question's options are chosen
	-- (fresh for each question).
	local ask = ui.computed(function(): store.Ask?
		local s = open()
		return if s then s.ask else nil
	end)
	local question = ui.computed(function(): any
		local a = ask:get()
		return if a and a.kind == "question" then a.question else nil
	end)
	local approval = ui.computed(function(): any
		local a = ask:get()
		return if a and a.kind == "approval" then a.approval else nil
	end)
	local chosen = shared.chosen
	-- The request the options were picked for: kept, so a reload keeps them.
	local asked = ui.state("chat_asked", nil :: string?)
	ui.effect(function()
		local a = ask:get()
		local r = if a then a.request else nil
		if r ~= asked:peek() then
			asked:set(r)
			chosen:set({})
		end
	end)
	local queue = ui.computed(function(): { any }
		local s = open()
		return if s and s.queue then s.queue else {}
	end)
	local todos = ui.computed(function(): { any }
		local s = open()
		return if s and s.todos then s.todos else {}
	end)
	local tasks = ui.computed(function(): { any }
		local s = open()
		return if s and s.tasks then s.tasks else {}
	end)
	-- A line above it: one of its own (a send that failed), else the
	-- session's.
	local own_notice = ui.state("chat_own_notice", nil :: any)
	-- Its own goes as the chat changes (kept: a reload isn't a change).
	local notice_for = ui.state("chat_notice_for", "?")
	ui.effect(function()
		local id = session_id:get() or ""
		if notice_for:peek() ~= id then
			notice_for:set(id)
			own_notice:set(nil)
		end
	end)
	local notice = ui.computed(function(): any
		local mine = own_notice:get()
		if mine then
			-- (Its words kept as a catalogue key, said in the language shown.)
			if mine.key then
				local said = table.clone(mine)
				said.text = ui.t(mine.key):get()
				return said
			end
			return mine
		end
		-- A new chat that can't start where it's set to: why.
		local why = where.blocked:get()
		if why then
			return { tone = "warning", text = why }
		end
		local s = open()
		if s and s.notice then
			return s.notice
		end
		-- The connection: this device's, then the chat's host.
		local now = sessions.now:get()
		local host_unreachable, host = false, nil
		if s and s.device and s.device ~= now.here then
			for _, d in now.devices or {} do
				if d.id == s.device then
					host_unreachable, host = d.online == false, d.name
				end
			end
		end
		return LinkNotice.of {
			connection = now.connection,
			host_unreachable = host_unreachable,
			host = host,
			intent = string.find(shared.text:get(), "%S") ~= nil or #shared.cargo:get() > 0 or #queue:get() > 0,
		}
	end)

	-- The models looked up once: not again on each reload after a lookup
	-- that failed (the composer made again).
	local looked_up = ui.state("chat_catalog_asked", false)
	ui.on_mount(function()
		if store.catalog:peek().state ~= "ready" and not looked_up:peek() then
			looked_up:set(true)
			store.load_catalog()
		end
	end)
	-- A new chat with no model yet: the catalog's first. (Never a session's:
	-- its model is its own.)
	ui.effect(function()
		local c = store.catalog:get()
		if session_id:peek() == nil and c.state == "ready" and model:peek() == nil and #c.models > 0 then
			model:set(c.models[1].value)
			effort:set(c.models[1].effort)
		end
	end)

	-- What a sent message shows of what it carries.
	local function shown_attachments(list: { any }, quotes: { Quote.Quote }?, picks: { { [string]: any } }?, comments: { Comments.Comment }?): { any }
		local out = {}
		for _, a in list do
			table.insert(out, {
				kind = if a.kind == "image" then "image" else "file",
				name = a.name,
				src = if a.kind == "image" then a.path else nil,
			})
		end
		-- And each passage it carries, as a chip (as the host's copy shows
		-- them: chat.rs `split_quotes`).
		for _, q in quotes or {} do
			table.insert(out, { kind = "file", icon = "cursor", name = Quote.where(q), meta = Quote.excerpt(q.text) })
		end
		-- And each element picked on a page (chat.rs `split_annotations`).
		for _, chip in Annotation.shown(picks or {}) do
			table.insert(out, chip)
		end
		-- And its review comments, one chip (chat.rs `comments_chip`).
		local said = comments or {}
		if #said > 0 then
			local places: { string } = {}
			for _, c in said do
				table.insert(places, Comments.place(c))
			end
			table.insert(out, {
				kind = "file",
				icon = "chat-round-line",
				name = ui.t("chat.composer.comments", { n = #said }):get(),
				meta = table.concat(places, ", "),
			})
		end
		return out
	end

	-- What `@` offers: the files of where it works (the session's folder,
	-- or a new chat's project), searched as the mention is typed. (`/` and
	-- `$` offer nothing yet.)
	local mention_sources = ui.state("chat_mentions", { ["/"] = {}, ["$"] = {} } :: any)
	-- The agent the chat runs (its model's): whose completion settings the
	-- composer follows.
	local function agent(): string?
		local chosen = model:get()
		for _, m in store.catalog:get().models do
			if m.value == chosen then
				return m.harness
			end
		end
		return nil
	end
	-- What each trigger lists, as the agent's settings have it: with
	-- "Separate / commands" off, `/` lists the skills after the commands
	-- (Zeron's composer.rs `completion_trigger`).
	-- Where the agent works, for its commands and skills: the session's
	-- folder (on its device), or a new chat's project.
	local function invocation_target(): Invocations.Target?
		local id = session_id:get()
		if id then
			local s = store.session:get()
			return { chat = id, device = if s and s.id == id then s.device else nil }
		end
		local p = route.project:get()
		if not p then
			return nil
		end
		local target: Invocations.Target = { space = p.id :: string, device = p.device :: string? }
		return target
	end
	local function invocation_key(): string?
		return Invocations.key(agent(), invocation_target())
	end
	local mention_lists = ui.computed(function(): any
		local given = table.clone(mention_sources:get())
		-- `/` the agent's commands, `$` its skills (nil while they come).
		local catalog = Invocations.catalog(invocation_key())
		given["/"] = Invocations.items(catalog, "command")
		given["$"] = Invocations.items(catalog, "skill")
		-- No agent to ask: Zeron's own alone. Its list still coming: nil.
		if agent() == nil then
			given["/"] = {}
		elseif given["/"] == nil then
			return given
		end
		local slash = table.clone(given["/"] or {})
		local taken: { [string]: boolean } = {}
		for _, c in slash do
			taken[c.value] = true
		end
		if not prefs.completion(agent()).separate then
			for _, skill in given["$"] or {} do
				table.insert(slash, skill)
			end
		end
		-- Then Zeron's own (named clear of the agent's).
		for _, row in ZeronCommands.rows(taken, session_id:get() ~= nil) do
			table.insert(slash, row)
		end
		given["/"] = slash
		return given
	end)
	-- A `/` or `$` opening afresh reads the agent's list again (the last
	-- one showing meanwhile), as does another agent or place while it's
	-- open. Kept, so a reload with it open reads nothing.
	local invocations_open = ui.state("chat_invocations_open", nil :: string?)
	ui.effect(function()
		local text = shared.text:get()
		local t = mentions.token(text, #text)
		local wanted = t ~= nil and (t.trigger == "/" or (t.trigger == "$" and prefs.completion(agent()).dollar))
		local key = if wanted then invocation_key() else nil
		if key == invocations_open:peek() then
			return
		end
		invocations_open:set(key)
		local harness, target = agent(), invocation_target()
		if key and harness and target then
			Invocations.load(harness, target)
		end
	end)
	-- What was last searched for: kept, so a reload doesn't search again.
	local searched = ui.state("chat_mention_searched", nil :: string?)
	ui.effect(function()
		local text = shared.text:get()
		local t = mentions.token(text, #text)
		if not t or t.trigger ~= "@" then
			return
		end
		local id = session_id:peek()
		local p = route.project:peek()
		local target: any = if id then { chat = id } elseif p then { space = p.id, device = p.device } else nil
		if not target then
			return
		end
		local key = `{id or (p and p.id) or ""}:{t.query}`
		if key == searched:peek() then
			return
		end
		-- (The run before's timer went as this one began: an effect's run
		-- owns what it makes. Written once it searches, so a run that
		-- stopped it before then doesn't count it as searched.)
		ui.after(120, function()
			searched:set(key)
			files.search(target, t.query):after(function(found: any)
				if searched:peek() ~= key then
					return
				end
				local items = {}
				for _, f in found or {} do
					local name = string.match(f.path, "([^/]+)/?$") or f.path
					local folder = string.match(f.path, "^(.*)/[^/]+/?$")
					table.insert(items, {
						value = f.path,
						label = name,
						detail = folder,
						file = not f.dir,
						dir = f.dir,
						-- Sent as Zeron's file mention (screens/chat/shared).
						link = shared.file_link(f.path, f.dir),
					})
				end
				mention_sources:set { ["@"] = items, ["/"] = {}, ["$"] = {} }
			end, function()
				mention_sources:set { ["@"] = {}, ["/"] = {}, ["$"] = {} }
			end)
		end)
	end)

	-- A queued message being edited in the composer: which, and the draft
	-- it set aside (put back when the edit ends).
	local editing = ui.state("chat_editing", nil :: { item: string, aside: string }?)
	local function end_edit(action: string)
		local e = editing:peek()
		local id = session_id:peek()
		if not e or not id then
			return
		end
		local words = shared.text:peek()
		ui.batch(function()
			editing:set(nil)
			shared.text:set(e.aside)
		end)
		store.queue_finish(id, e.item, action, if action == "commit" then words else nil):after(function() end, function(err: any)
			local why = err and err.message
			own_notice:set {
				tone = "danger",
				key = if why == "conflict" then "chat.composer.edit_conflict" else "chat.composer.edit_not_saved",
				dismissable = true,
			}
		end)
	end
	-- Moving to another chat: an edit under way is left as it was (its
	-- displaced draft back in place), then the draft shown is put away
	-- under the chat left and the next chat's own comes out (shared.show),
	-- as Zeron's composer swaps drafts on navigation.
	-- (Only a real move: a reload, the same chat shown, keeps the edit.)
	ui.effect(function()
		local key = session_id:get() or ""
		if shared.showing() == key then
			return
		end
		if editing:peek() then
			end_edit("cancel")
		end
		shared.show(key)
	end)

	local function send(text: string)
		-- Editing a queued message: Send saves it.
		if editing:peek() then
			end_edit("commit")
			return
		end
		-- A draft that is only one of Zeron's own commands runs it (its
		-- token goes; anything carried stays).
		local zeron = ZeronCommands.of_text(shared.text:peek(), (mention_lists:peek() or {})["/"] or {})
		if zeron then
			shared.text:set("")
			ZeronCommands.run(zeron, session_id:peek())
			return
		end
		local typed, carried = shared.take()
		local attachments = {}
		-- Passages attached from the conversation: a block after the words
		-- (Quote.with_quotes), each with who said it and when.
		local quotes: { Quote.Quote } = {}
		-- Elements picked on a page in the panel's browser: each a token
		-- after the words (Annotation.with_annotations), which the engine
		-- expands for the agent.
		local picks: { { [string]: any } } = {}
		-- Review comments left on files: a block after the words
		-- (screens/chat/comments), each its file and line.
		local comments: { Comments.Comment } = {}
		for _, item in carried do
			if item.kind == "comment" then
				table.insert(comments, { file = item.file, line = item.line, body = item.body, to = item.to, side = item.side, snippet = item.snippet })
			elseif item.path then
				-- (A project's file: on the session's computer already.)
				-- (An Appshot goes as its picture; what it shows of the app, below.)
				table.insert(attachments, { kind = if item.kind == "appshot" then "image" else item.kind, path = item.path, name = item.name, rel = item.rel, on_host = item.on_host })
			elseif item.kind == "annotation" and item.quote then
				table.insert(quotes, { text = item.quote, who = item.name, from = item.from })
			elseif item.kind == "annotation" and item.annotation then
				table.insert(picks, item)
			elseif item.kind == "text" and item.text then
				-- A pasted text carried as a chip: its words go in the text.
				typed = typed .. "\n\n" .. item.text
			elseif item.kind == "chat" and item.chat then
				-- Another chat mentioned (a session dropped from the sidebar):
				-- its link in the words, the engine's to read out to the agent.
				local link = shared.chat_link(item.name, item.chat)
				typed = if typed ~= "" then `{typed} {link}` elseif text ~= "" then `{text} {link}` else link
			end
		end
		-- (Each command or skill picked: its canonical reference.)
		local words = Invocations.encode(if typed ~= "" then typed else text)
		local text_out = Comments.with_comments(Annotation.with_annotations(Quote.with_quotes(words, quotes), picks), comments)
		local id = session_id:peek()
		own_notice:set(nil)
		if id then
			-- The prompt shows at once, rising to the view's top with its
			-- reply's room under it (ds/transcript's send); the engine's copy
			-- arrives as the same row.
			-- (While a turn runs it's queued instead: it shows in the queue.)
			-- The chat's unsent pick goes with it (the run takes that agent,
			-- model and effort, and the session with them); queued, it waits
			-- for the turn it will run in.
			local pick = shared.picks:peek()[id]
			local req: { [string]: any } = { session = id, text = text_out, attachments = attachments, appshots = Appshots.of_cargo(carried) }
			if pick and not running:peek() then
				for k, v in pick do
					req[k] = v
				end
				if pick.model then
					for _, m in store.catalog:peek().models do
						if m.value == pick.model then
							req.harness = m.harness
						end
					end
				end
			end
			local function deliver(message_id: string?)
				req.message_id = message_id
				store.send(req :: any):after(function()
					-- Sent with it: the session has it now; no pick left.
					if req.model or req.effort or req.fast ~= nil or req.options then
						local all = table.clone(shared.picks:peek())
						if all[id] == pick then
							all[id] = nil
							shared.picks:set(all)
						end
					end
				end, function(e: any)
					own_notice:set { tone = "danger", key = "chat.composer.didnt_send", dismissable = true }
					if shared.text:peek() == "" then
						shared.text:set(text_out)
					end
				end)
			end
			if running:peek() then
				deliver(nil)
				return
			end
			-- Handed to the host as the prompt joins the transcript (ds/
			-- transcript's send steps clear of the end first, a frame
			-- before): nothing of the host's (its copy of the prompt, the
			-- engine's) can reach the list before the glide is ready, and
			-- the prompt is there once, whoever adds it first.
			local mid = store.new_id()
			T.send(function()
				local here = store.messages:index_of("id", mid) ~= nil
				if not here then
					store.messages:push {
						id = mid,
						role = "user",
						text = words,
						attachments = shown_attachments(attachments, quotes, picks, comments),
						-- On its way from this moment: the line under it
						-- says so at once.
						waiting = "sending",
					}
				end
				deliver(mid)
			end, mid)
			return
		end
		local req: store.Request = {
			text = text_out,
			attachments = attachments,
			appshots = Appshots.of_cargo(carried),
			project = project:peek(),
			device = device:peek(),
			checkout = checkout:peek(),
			branch = branch:peek(),
			model = model:peek(),
			effort = effort:peek(),
			fast = fast:peek(),
			options = options:peek(),
			access = access:peek(),
			plan = plan:peek(),
		}
		for _, m in store.catalog:peek().models do
			if m.value == req.model then
				req.harness = m.harness
			end
		end
		-- Nowhere it can start (a project no longer there): the line above
		-- says why, and the draft stays.
		if where.blocked:peek() then
			return
		end
		local plan = where.plan()
		own_notice:set { key = if plan.cloud then "chat.composer.starting_cloud" else "chat.composer.starting", spinner = true }
		-- (`why`: what to say instead, a Cloud chat's reason in words.)
		local failed = function(why: string?)
			own_notice:set { tone = "danger", text = why, key = if why then nil else "chat.composer.didnt_start", dismissable = true }
			if shared.text:peek() == "" then
				shared.text:set(text_out)
			end
		end
		-- Made where the project is (or on the device chosen), then its first
		-- message run there, as the shipping app starts one (the send sets
		-- its config: the model, effort, access and plan chosen here): in
		-- the project's folder, the worktree its ref is checked out in, or a
		-- new worktree off that ref (screens/chat/where's plan). On Cloud, made
		-- there: the run goes in its machine's checkout, named as it's made.
		req.project = plan.project
		req.device = plan.device
		req.branch = plan.branch
		sessions
			.create({
				project = plan.project,
				device = if plan.project then nil else plan.device,
				branch = plan.branch,
				cwd = if plan.worktree then nil else plan.cwd,
				cloud = plan.cloud,
			})
			:after(function(made: any)
				local id = if plan.cloud and type(made) == "table" then made.id else made
				if type(id) ~= "string" then
					failed()
					return
				end
				if plan.cloud then
					plan.cwd = if type(made.cwd) == "string" and made.cwd ~= "" then made.cwd else nil
				end
				req.session = id
				req.fresh = true
				-- Sent from here: its turn holds the runway as it opens.
				req.message_id = store.new_id()
				T.sent_here(req.message_id :: string)
				req.cwd = plan.cwd
				req.worktree = plan.worktree
				-- Opened first, then sent: the host shows a prompt at once
				-- only for the chat it has open, so the new page has your
				-- message from its first frame, sending, rather than nothing
				-- until the engine's first word. (Sent after it, it waited
				-- for the transcript, an empty page meanwhile.) A send that
				-- fails then says so on that message, with Retry, as any
				-- chat's does.
				own_notice:set(nil)
				store.open(id)
				-- (Down with its prompt: the dock's one slide.)
				slide_next = true
				route.open_session(id)
				store.send(req):after(function() end, function(_e: any) end)
			end, function(e: any)
				-- (A Cloud chat's reason is said in words; an outcome word isn't.)
				local why = if plan.cloud and type(e) == "table" and type(e.message) == "string" and string.find(e.message, " ", 1, true)
					then e.message
					else nil
				failed(why)
			end)
	end

	-- Where the open chat works, live: git's current branch in its own
	-- folder (an agent may have made or switched one since it began; the
	-- branch named on the chat is only where it started), and whether that
	-- folder is a worktree of its project's. Asked again as each turn ends.
	local live_branch = ui.state("chat_live_branch", { key = "", name = nil :: string? })
	ui.effect(function()
		local s = open()
		if not s or not s.cwd then
			return
		end
		local key = `{s.id}|{s.cwd}|{if s.running then "run" else "idle"}`
		if live_branch:peek().key == key then
			return
		end
		local cwd, id = s.cwd, s.id
		local known = if live_branch:peek().key:sub(1, #id + 1) == `{id}|` then live_branch:peek().name else nil
		live_branch:set { key = key, name = known or (s :: any).branch }
		local here = sessions.now:peek().here
		local ok, answer = pcall(store.refs, cwd, if s.device ~= here then s.device else nil)
		if not ok or answer == nil then
			return
		end
		answer:after(function(list: any)
			if live_branch:peek().key ~= key then
				return
			end
			for _, ref in list or {} do
				if ref.current then
					live_branch:set { key = key, name = ref.name }
				end
			end
		end, function() end)
	end)
	local accounts = shared.model_accounts(function(): string?
		return device:get()
	end)
	local function branch_fixed(): ds.ComposerFixed?
		local s = open()
		if not s then
			return nil
		end
		local path: string?, project: string? = nil, nil
		for _, p in sessions.now:get().projects do
			if p.id == s.project then
				path, project = p.path, p.name
			end
		end
		local lb = live_branch:get()
		local name = if lb.key:sub(1, #s.id + 1) == `{s.id}|` then lb.name else (s :: any).branch
		local on: string? = nil
		for _, d in sessions.now:get().devices do
			if d.id == (s :: any).device then
				on = d.name
			end
		end
		return {
			device = on,
			name = name,
			worktree = path ~= nil and s.cwd ~= nil and s.cwd ~= path,
			project = project,
			hue = if project and s.project then (route.look(s.project) or { hue = route.hue(s.project) }).hue else nil,
			src = if project and s.project then (route.look(s.project) or {}).src else nil,
			emoji = if project and s.project then (route.look(s.project) or {}).emoji else nil,
		}
	end

	local function composer(): Node
		return ds.Composer {
			-- Its measures, fold and open tray kept across a reload, so it
			-- is laid out as it was from the first frame.
			keep = "chat",
			toolbar_id = "composer-toolbar",
			open_model = shared.open_model,
			model_open = model_open,
			tray = TRAY,
			-- ("Scroll to end", while it shows: the trays' pill moves aside.)
			aside = function(): number?
				return shared.jump_width:get()
			end,
			on_pill_width = function(w: number?)
				if shared.pill_width:peek() ~= w then
					shared.pill_width:set(w)
				end
			end,
			value = shared.text,
			placeholder = ui.t("chat.composer.placeholder"),
			expanded = shown_new,
			snap = snap,
			menus_below = shown_new,
			running = running,
			access = access,
			plan = plan,
			model = model,
			effort = effort,
			fast = fast,
			options = options,
			-- The catalog, followed in place as it comes and changes.
			harnesses = function(): { ds.Harness }
				return store.catalog:get().harnesses
			end,
			models = function(): { ds.Model }
				return store.catalog:get().models
			end,
			accounts = accounts.accounts,
			on_account = accounts.on_account,
			on_accounts_open = accounts.on_accounts_open,
			-- What its context holds (the ring shows once a window is
			-- reported, its card saying the numbers).
			usage = function(): ds.ComposerUsage?
				local s = open()
				if not s or (s.context_tokens == nil and s.context_window == nil) then
					return nil
				end
				return { tokens = s.context_tokens, window = s.context_window }
			end,
			-- A new chat stands over its wallpaper (Settings → Appearance):
			-- the footer's chips take their backing.
			over_art = function(): boolean
				return shown_new() and art.showing:get()
			end,
			project = project,
			projects = where.projects,
			-- (Work in's "New project": the shell's New project palette.)
			on_new_project = function()
				local add: any = require("@app/screens/shell/boundary").load("@app/screens/shell/add_project")
				if add then
					add.open()
				end
			end,
			device = device,
			devices = where.devices,
			checkout = checkout,
			branch = branch,
			branches = where.branches,
			branches_empty = where.branches_empty,
			branch_fixed = branch_fixed,
			cargo = shared.cargo,
			on_remove_cargo = shared.remove_cargo,
			on_attach = function()
				ui.open_files({ title = ui.t("chat.composer.attach_files"):get(), multiple = true }):after(function(files: any)
					if files and #files > 0 then
						shared.attach_files(files)
					end
				end, function() end)
			end,
			-- (A drop on the pill: the chat's overlay goes with it.)
			on_files = function(files: { any })
				shared.dropping:set({})
				shared.attach_files(files)
			end,
			-- A picture pasted (a screenshot): written out by the host, then
			-- carried as an image, as one dropped is, named as Zeron names
			-- it (attachments.rs `stage_clipboard_image`).
			on_paste_image = function(): boolean
				store.paste_image():after(function(path: string?)
					if type(path) == "string" and path ~= "" then
						shared.attach_files({ { name = "image.png", path = path } })
					end
				end, function() end)
				return true
			end,
			drop_overlay = false,
			on_send = send,
			on_stop = function()
				local id = session_id:peek()
				if id then
					store.interrupt(id)
				end
			end,
			-- The shipping app's settings: what sends, and whether Escape
			-- stops the agent.
			send_with = function(): string
				return prefs.send_with:get()
			end,
			on_escape = store.escape,
			queue = queue,
			can_steer = function(): boolean
				return running:get()
			end,
			on_queue_send = function(item: string)
				local id = session_id:peek()
				if id then
					store.queue(id, { op = "send", item = item })
				end
			end,
			on_queue_edit = function(item: string)
				local id = session_id:peek()
				if not id or editing:peek() then
					return
				end
				store.queue_edit(id, item):after(function(got: any)
					editing:set { item = item, aside = shared.text:peek() }
					shared.text:set(if got then got.text else "")
					shared.focus:add(1)
				end, function(err: any)
					own_notice:set {
						tone = "warning",
						key = if err and err.message == "locked" then "chat.composer.edit_locked" else "chat.composer.edit_unavailable",
						dismissable = true,
					}
				end)
			end,
			on_queue_save = function()
				end_edit("commit")
			end,
			on_queue_cancel = function()
				end_edit("cancel")
			end,
			on_queue_remove = function(item: string)
				local id = session_id:peek()
				if id then
					store.queue(id, { op = "remove", item = item })
				end
			end,
			on_queue_reorder = function(from: number, to: number)
				local id = session_id:peek()
				local list = queue:peek()
				if id and list[from] then
					store.queue(id, { op = "move", item = list[from].id, to = to })
				end
			end,
			todos = todos,
			tasks = tasks,
			-- Stop on a background process: its host stops it, and its row
			-- goes as the session stops listing it (production's tray).
			on_stop_task = function(task: string)
				local id = session_id:peek()
				if not id then
					return
				end
				store.stop_task(id, task):after(function() end, function()
					own_notice:set { tone = "danger", key = "chat.composer.couldnt_stop_task", dismissable = true }
				end)
			end,
			question = question,
			chosen = chosen,
			on_pick = function(value: string)
				local q = question:peek()
				local now = table.clone(chosen:peek())
				if q and q.multi then
					now[value] = not now[value] or nil
				else
					now = { [value] = true }
				end
				chosen:set(now)
			end,
			on_answer = function(answer: any)
				local id, a = session_id:peek(), ask:peek()
				if id and a then
					store.answer(id, a.request, answer)
				end
			end,
			on_close_question = function()
				local id, a = session_id:peek(), ask:peek()
				if id and a then
					store.deny(id, a.request)
				end
			end,
			-- Back a page: it shows again with what was picked on it.
			on_back = function()
				local id, a = session_id:peek(), ask:peek()
				if id and a then
					store.question_back(id, a.request):after(function(picked: any)
						local now: { [string]: boolean } = {}
						for _, v in picked or {} do
							now[v] = true
						end
						chosen:set(now)
					end, function() end)
				end
			end,
			approval = approval,
			-- The engine allows once, each time it's asked.
			can_always = false,
			on_approve = function(scope: string)
				local id, a = session_id:peek(), ask:peek()
				if id and a then
					store.approve(id, a.request, scope)
				end
			end,
			on_deny = function()
				local id, a = session_id:peek(), ask:peek()
				if id and a then
					store.deny(id, a.request)
				end
			end,
			notice = notice,
			on_dismiss_notice = function()
				own_notice:set(nil)
			end,
			disabled = function(): boolean
				local s = store.session:get()
				local id = session_id:get()
				return id ~= nil and s ~= nil and s.id == id and (s.state == "missing" or s.state == "offline")
			end,
			focus = shared.focus,
			mentions = mention_lists,
			-- A command, skill or file picked: what its token stands for; one of
			-- Zeron's own runs (its token gone).
			on_mention = function(item: any, trigger: string, token: string)
				if item.zeron then
					ZeronCommands.run(item.zeron, session_id:peek())
				elseif trigger == "@" then
					Invocations.remember(item, trigger, token)
				else
					Invocations.remember(item, trigger)
				end
			end,
			-- The agent's completion settings: `$` opens the skills only
			-- while "Use $ for skills" is on (else it's ordinary text).
			mention_enabled = function(trigger: string): boolean
				if trigger == "$" then
					return prefs.completion(agent()).dollar
				end
				return true
			end,
			-- With "Separate / commands" off, `/` lists the skills too.
			mention_empty = setmetatable({}, {
				__index = function(_, trigger: string): string?
					if trigger == "/" and not prefs.completion(agent()).separate then
						return ui.t("chat.composer.commands_or_skills"):get()
					end
					return nil
				end,
			}) :: any,
		}
	end

	local page = ui.ref()
	-- The page's height as last laid out, kept (a reload keeps state): a
	-- composer made again reads it until its own page is measured, so its
	-- spring starts where it rests instead of rising into place again.
	local page_h = ui.state("chat_page_h", 0)
	local composer_zone = shared.drop_zone("composer")
	-- There to open the model picker in (the shell's ⌘/), while it is.
	ui.on_mount(function()
		shared.composer_shown:set(true)
	end)
	ui.on_cleanup(function()
		shared.composer_shown:set(false)
	end)
	local dock = ui.ref()
	-- How tall a new chat's composer is with nothing in it (what it's placed
	-- by, so what's written in it doesn't move its top).
	local rest_tall = ui.state("chat_composer_rest_tall", nil :: number?)
	-- (A draft just deleted, it's empty while still easing back, and that
	-- height would move its top: taken once that's over.)
	local REST_SETTLE_MS = 500
	local had_draft = false
	local rest_settles_at = 0
	ui.effect(function()
		local tall = dock.height or 0
		if shared.text:get() ~= "" or #shared.cargo:get() > 0 or next(shared.quotes:get()) ~= nil then
			had_draft = true
			return
		end
		if had_draft then
			had_draft = false
			rest_settles_at = ui.now() + REST_SETTLE_MS
		end
		if tall <= 0 or not new_chat() then
			return
		end
		local function take()
			if math.abs((rest_tall:peek() or 0) - tall) >= 0.5 then
				rest_tall:set(tall)
			end
		end
		local left = rest_settles_at - ui.now()
		if left > 0 and rest_tall:peek() ~= nil then
			ui.after(left, take)
		else
			take()
		end
	end)
	-- Where it stands: mid-page on a new chat (centred under the title bar),
	-- else docked at the foot (0), as laid out now.
	local function target_y(): number
		if not new_chat() then
			return 0
		end
		-- (Measured, else as last measured: a node made again reads 0 until
		-- laid out.)
		local h = page.height or 0
		if h <= 0 then
			h = page_h:get()
		end
		local tall = dock.height or 0
		if tall <= 0 then
			tall = shared.dock_h:get()
		end
		-- (Its top stays as a draft grows: it's placed by its empty height,
		-- and grows down from there. Only a draft that would run off the
		-- page's foot moves it, up by as much.)
		local rest = rest_tall:get() or tall
		local hero = TITLE_H + (h - TITLE_H - 96) / 2
		local docked = h - FOOT - rest / 2
		return math.min(hero - docked, rest - tall)
	end
	-- Moving between a new chat and a session: its own send from a new chat
	-- slides it down with the prompt (`slide_next`); any other move (another
	-- chat opened, a new chat) puts it there at once, in its new form and
	-- size, no fade, no slide, nothing growing (and as that size is
	-- measured, it follows at once too). In the page, as it grows or the window resizes, it
	-- moves there as the dock's spring did. Motion reduced, it's there.
	local GLIDE = { duration = 260, ease = "out(3)" }
	-- How long after a switch its place follows its size at once (the
	-- form's size, there at once too (`snap`), is measured a frame later).
	local SNAP_MS = 400
	-- (Kept state, as this file's others: the composer is made in a plain
	-- function, not a component.)
	local place = ui.state("chat_composer_place", 0)
	-- (Held where it is while its model picker is open: `model_open`.)
	local was_new: boolean? = nil
	-- (Its model picker open on a new chat: how far its top gives way so its
	-- chip stays, the picker hanging from it, as what's written grows.)
	local hold = ui.state("chat_composer_hold", 0)
	local held_tall: number? = nil
	local gliding: any = nil
	local snap_until = 0
	local function glide_to(y: number)
		if gliding then
			gliding:cancel()
			gliding = nil
		end
		if motion.reduced_now() or ui.now() < snap_until or math.abs(place:peek() - y) < 0.5 then
			place:set(y)
			return
		end
		gliding = anime.animate(place, { value = y, duration = GLIDE.duration, ease = GLIDE.ease })
	end
	-- Made again (as the window opens on its last session, the composer is
	-- built twice), a glide begun here must not land after: `place` is kept,
	-- and it would put a session's composer where a new chat's stood.
	ui.on_cleanup(function()
		if gliding then
			gliding:cancel()
			gliding = nil
		end
	end)
	ui.effect(function()
		local y = target_y()
		local nc = new_chat()
		local switched = was_new ~= nil and nc ~= was_new and not slide_next
		was_new = nc
		slide_next = false
		if switched then
			snap_until = ui.now() + SNAP_MS
			snap:set(snap:peek() + 1)
		end
		form_new:set(nc)
		-- On a new chat its place is the layout's (see the row below), so the
		-- page resizing moves it with the page; this is only where a send
		-- slides from.
		if nc and not switched then
			if gliding then
				gliding:cancel()
				gliding = nil
			end
			place:set(y)
			local tall = dock.height or 0
			if model_open:get() then
				held_tall = held_tall or tall
				hold:set(held_tall - tall)
			elseif held_tall then
				held_tall = nil
				anime.animate(hold, { value = 0, duration = GLIDE.duration, ease = GLIDE.ease })
			end
			return
		end
		held_tall = nil
		hold:set(0)
		glide_to(y)
	end)
	-- Over the whole pane, the pointer passing through it to the page
	-- (its wheel too) but for the composer itself. (Under settings the shell
	-- hides the chat frame, its state kept: nothing here is made again.)
	return ui.overlay {
		ref = page,
		-- Not seen over a plugin's page (the chat's is kept unseen under it),
		-- nor anything placed round the composer: the page takes the pointer.
		opacity = function(): number
			return if route.current:get().name == "plugin" then 0 else 1
		end,
		on_resize = function(size: { w: number, h: number })
			if math.abs(page_h:peek() - size.h) >= 1 then
				page_h:set(size.h)
			end
		end,
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		pointer_events = "none",
		ui.row {
			absolute = true,
			pointer_events = "none",
			left = 0,
			right = 0,
			-- On a new chat by its top, from the page's middle: it moves with
			-- the page as the page lays out, and what's written grows it down.
			-- Docked, by its foot.
			top = function(): any
				return if new_chat() then "50%" else nil
			end,
			bottom = function(): any
				return if new_chat() then nil else FOOT
			end,
			px = require("@app/screens/chat/dock").SIDES,
			justify = "center",
			-- Mid-page on a new chat, docked on a session: sliding there as it
			-- sends from a new chat (with its prompt), there at once otherwise
			-- (another chat, a new chat: `place`).
			id = "chat-composer-dock",
			y = function(): number
				if not new_chat() then
					return place:get()
				end
				local tall = dock.height or 0
				if tall <= 0 then
					tall = shared.dock_h:get()
				end
				local rest = rest_tall:get() or tall
				local at = TITLE_H / 2 - 48 - rest / 2
				-- (Only a draft that would run off the page's foot moves it.)
				local h = page.height or 0
				if h > 0 then
					at = math.min(at, h / 2 - FOOT - tall)
				end
				return at + hold:get()
			end,
			ui.div {
				ref = dock,
				id = "chat-composer",
				hook = "chat.composer",
				-- (Over the page, not in it: the session it sends to, as the
				-- session page provides it; none on a new chat.)
				provide = { session = session_id },
				w_full = true,
				pointer_events = "auto",
				max_w = ui.spring(
					ui.computed(function(): number
						return prefs.conversation_width:get()
					end),
					DOCK
				),
				on_resize = function(size: { w: number, h: number })
					if math.abs(shared.dock_h:peek() - size.h) > 0.01 then
						shared.dock_h:set(size.h)
					end
				end,
				-- Files carried over the composer itself: the chat's drop,
				-- as over the page under it.
				on_file_hover = composer_zone.on_file_hover,
				on_file_drop = composer_zone.on_file_drop,
				drop_when = composer_zone.drop_when,
				on_drag_over = composer_zone.on_drag_over,
				on_drag_leave = composer_zone.on_drag_leave,
				on_hover = composer_zone.on_hover,
				on_drop = composer_zone.on_drop,
				-- (A new chat with nowhere to work: its page asks for a project in
				-- its place, as Zeron's first launch does. Hidden here, inside
				-- the pill's own box: a `hidden` on the row above, which spans
				-- the pane, left a box over the transcript taking its wheel.)
				-- (And not over a plugin's page: the chat's page is kept unseen
				-- under it, and so is this.)
				ui.show(function(): boolean
					return not route.needs_project:get() and route.current:get().name ~= "plugin"
				end, composer()),
			},
		},
		-- Files carried over the chat (the page or the composer): all of it
		-- dimmed under "Drop to attach", as Zeron's conversation shows it
		-- (shell.rs `attachment_drop_overlay`: the scrim, the words in the
		-- text's ink, at once and gone at once; no accent).
		DropOverlay { shown = shared.carrying, id = "chat-drop" },
		-- (A sidebar session carried: dropped, a mention; production's words.)
		DropOverlay { shown = shared.carrying_session, id = "chat-drop-mention", label = ui.t("chat.composer.drop_to_mention") :: any },
	}
end

return Composer
