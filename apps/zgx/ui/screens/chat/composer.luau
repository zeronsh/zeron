-- The window's one composer, wired: it travels between a new chat (lifted
-- to a little over the page's middle, opened out, its menus below) and a
-- session (docked at the foot, its menus above), on one critically damped
-- spring, as the gallery's App shell has it (Zeron's composer_dock.rs). It
-- is made once and follows where the shell is: the draft and what it
-- carries are each chat's own (screens/chat/shared), its model, effort
-- and access the session's (changed there, they change the session) or,
-- on a new chat, the new chat's own: those last chosen there, never a
-- chat's.
--
-- In a session it sends to that session (queued while a turn runs, which
-- Stop ends), and shows what the agent asks (an approval, a question) in
-- the pill, the queue, the plan and what runs behind it in its trays. On a
-- new chat it starts a chat in the route's project and opens it.
--
-- `Composer()` is placed once over the page pane (absolute, filling it):
-- it lays itself out.

local ds = require("@ds")
local Dictation = require("@app/screens/chat/dictation")
local motion = require("@ds/motion")
local T = require("@ds/transcript")
local prefs = require("@app/screens/settings/prefs")
local store = require("@app/screens/chat/store")
local lanes = require("@app/screens/chat/lanes")
local route = require("@app/screens/shell/route")
local art = require("@app/screens/shell/art")
local shared = require("@app/screens/chat/shared")
local sessions = require("@account/sessions")
local LinkNotice = require("@app/screens/chat/link_notice")
local CloudTray = require("@app/screens/chat/cloud_tray")
local Held = require("@app/screens/chat/held")
local CloudMachine = require("@app/screens/chat/cloud_machine")
local Where = require("@app/screens/chat/where")
local DropOverlay = require("@ds/drop_overlay")
local Asks = require("@app/screens/chat/asks")
local Invocations = require("@app/screens/chat/invocations")
local ZeronCommands = require("@app/screens/chat/zeron_commands")
-- (Loaded with the chat page: captures land on its composer.)
local Appshots = require("@app/screens/chat/appshots")
local Carried = require("@app/screens/chat/carried")
local Completions = require("@app/screens/chat/completions")

-- Zeron's dock spring (ω ≈ 27).
local DOCK: SpringOptions = { stiffness = 730, damping = 54 }
-- Its gap to the window's foot, docked. Its width, rising and docked: the
-- transcript's column (Settings' "Conversation width"), as Zeron's
-- composer lines up with the conversation over it (its edges on the
-- column's).
local Dock = require("@app/screens/chat/dock")
local FOOT = Dock.GAP

-- What a new chat starts with: what was last chosen on the new-chat page
-- (its settings, its own: a chat shown or made never changes them), kept
-- across launches. Each is put back as it was, none included, as the page
-- is come back to.
local defaults = ui.persist("chat_new_defaults", {
	model = nil :: string?,
	effort = nil :: string?,
	fast = false,
	options = nil :: { [string]: string }?,
	access = "full" :: string?,
	plan = false,
	checkout = "worktree" :: string?,
	device = nil :: string?,
	-- Where (nil: none picked yet; screens/chat/where picks one).
	project = nil :: string?,
	branch = nil :: string?,
})
-- A new chat's own value of each choice where its default is none: what
-- the composer shows then.
local NONE: { [string]: any } = { fast = false, options = {}, plan = false }

-- The last default a plugin chose that was taken (its time, ms): one is
-- taken once, across restarts, never again over a pick made since.
local plugin_default_seen = ui.persist("chat_new_defaults_plugin", 0)

local FIELDS = { "model", "effort", "fast", "options", "access", "plan" }
-- Where a new chat is made, kept with them.
local WHERE = { "checkout", "device", "project", "branch" }
-- Those a session takes only with a message sent (shared.picks keeps them
-- meanwhile); the rest change it as they're picked.
local DRAFTED: { [string]: boolean } = { model = true, effort = true, fast = true, options = true }
-- Whether two choices are the same: the options' picks (tables) by their
-- entries, none and an empty table alike; the rest by value.
-- An agent's default model: the row its catalog marks, else its first
-- (harness nil: the first agent listed's).
local function default_of(models: { any }, harness: string?): any
	local first = nil
	for _, m in models do
		local h = harness or m.harness
		harness = h
		if m.harness == h then
			if m.default then
				return m
			end
			first = first or m
		end
	end
	return first
end
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
					-- Its own settings, each as it was left (none too: never
					-- what the chat just shown had).
					local d = defaults:peek()
					for _, k in FIELDS do
						local v = (d :: any)[k]
						if v == nil then
							v = NONE[k]
						end
						if not equal(locals[k]:peek(), v) then
							locals[k]:set(v)
						end
					end
					checkout:set(d.checkout)
					branch:set(d.branch)
					-- (A device asked of this new chat, a device's "+" in the
					-- sidebar, over the one last used; a project asked of it,
					-- a project's "+", over the one last picked here.)
					device:set(route.take_device() or d.device)
					local asked = route.take_project()
					project:set(if asked then asked.project elseif d.project ~= nil then d.project else route.current:peek().project)
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
					elseif v == nil and (k == "model" or k == "effort") and locals[k]:peek() ~= nil then
						-- A session that names none runs its agent's own: never
						-- the last chat's model (another agent's, maybe one
						-- this device doesn't have), sent along as a pick.
						locals[k]:set(nil)
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
	-- What a new chat starts with, as a plugin allowed "models.set" chose it
	-- (screens/plugins publishes it once the engine allowed it): kept as a
	-- pick made here would be, and shown at once on a new chat.
	local plugin_default = ui.shared("plugins.new_chat_model", nil :: any)
	ui.effect(function()
		local asked = plugin_default:get()
		if type(asked) ~= "table" or type(asked.model) ~= "string" then
			return
		end
		local seq = tonumber(asked.seq) or 0
		if seq <= plugin_default_seen:peek() then
			return
		end
		plugin_default_seen:set(seq)
		local d = table.clone(defaults:peek()) :: any
		d.model = asked.model
		d.effort = asked.effort
		defaults:set(d)
		if session_id:peek() == nil then
			ui.batch(function()
				locals.model:set(asked.model)
				if asked.effort ~= nil then
					locals.effort:set(asked.effort)
				end
			end)
		end
	end)
	-- A project asked of a new chat already showing (a project's "+", the
	-- palette's New chat in…): picked. (One asked on the way here is the
	-- arrival's, above; a project picked here is otherwise the page's own,
	-- whatever the route last asked for.)
	ui.effect(function()
		local pick = route.project_pick:get()
		if pick == nil or session_id:peek() ~= nil or was:peek() ~= nil then
			return
		end
		local asked = route.take_project()
		if asked and project:peek() ~= asked.project then
			project:set(asked.project)
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
		local where: { [string]: any } = {
			checkout = checkout:get(),
			device = device:get(),
			project = project:get(),
			branch = branch:get(),
		}
		local id = session_id:peek()
		if id == nil then
			-- (Only once the page's own are out: never the chat left's, on
			-- their way out.)
			if was:peek() ~= nil then
				return
			end
			local d = table.clone(defaults:peek()) :: any
			local changed = false
			for _, k in FIELDS do
				local v = picked[k]
				if not equal(d[k], v) then
					d[k] = v
					changed = true
				end
			end
			-- (By name: a choice of none is a nil a table's loop skips.)
			for _, k in WHERE do
				if d[k] ~= where[k] then
					d[k] = where[k]
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
	local function run_device(): string?
		local d = device:get()
		if d == nil and session_id:get() == nil then
			local p = route.project:get()
			d = if p then p.device else nil
		end
		return d
	end
	-- Whether the catalog shown is `d`'s (one of this device's says none).
	local function shows_catalog_of(d: string?): boolean
		local shown = (store.catalog:get() :: any).device
		return shown == d or (shown == nil and (d == nil or d == sessions.now:get().here))
	end
	-- (Asked once per device: the host keeps a look current itself, a late
	-- answer to an older one changing nothing, src/chat.rs `reload_remote`.)
	ui.effect(function()
		local d = run_device()
		if d ~= catalog_device:peek() then
			catalog_device:set(d)
			store.catalog_on(d)
		end
	end)
	-- A new chat moved onto Cloud runs only the agents Cloud has accounts
	-- for: a model it doesn't run gives way to Cloud's first, as the
	-- shipping app drops one on the switch. So does one the last new chat
	-- used that its device doesn't offer (once all its agents have said: a
	-- model still on its way isn't dropped), so a chat isn't made on a
	-- device for an agent it doesn't have.
	ui.effect(function()
		local cat = store.catalog:get()
		if session_id:get() ~= nil then
			return
		end
		local d = run_device()
		if d == Where.CLOUD then
			if (cat :: any).device ~= Where.CLOUD then
				return
			end
		elseif (cat :: any).complete ~= true or not shows_catalog_of(d) then
			return
		end
		local picked = model:peek()
		for _, m in cat.models do
			if m.value == picked then
				return
			end
		end
		local first = default_of(cat.models)
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
	-- (A new chat's project, for what follows the route: its panel.)
	ui.effect(function()
		if new_chat() then
			local id = project:get()
			if route.new_chat_project:peek() ~= id then
				route.new_chat_project:set(id)
			end
		end
	end)

	local running = ui.computed(function(): boolean
		local s = open()
		return s ~= nil and s.running == true
	end)
	-- What the agent asks, and which of a question's options are chosen
	-- (fresh for each question); its queue, todos and tasks. (The same as
	-- a split pane's composer shows them: screens/chat/asks.)
	-- A line above it: one of its own (a send that failed), else the
	-- session's.
	local own_notice = ui.state("chat_own_notice", nil :: any)
	-- The lane of the transcript at its foot (screens/chat/shared's
	-- `at_foot`), while that's the chat `id`'s: in a split, the focused
	-- pane's (`split-<chat>`). Nil: the chat page's.
	local function foot_lane(id: string?): string?
		local lane = string.match(shared.at_foot:peek(), "^lane%.(.+)$")
		return if lane and id and lane == `split-{id}` then lane else nil
	end
	local asks = Asks.bind {
		id = function(): string?
			return session_id:peek()
		end,
		-- (A queued message steered in glides in the transcript at its foot.)
		name = function(): string?
			local lane = foot_lane(session_id:peek())
			return if lane then `lane.{lane}` else nil
		end,
		session = open,
		chosen = shared.chosen,
		-- (The request the options were picked for: kept, so a reload keeps
		-- them.)
		asked = ui.state("chat_asked_by", nil :: any),
		say = function(tone: string, key: string)
			own_notice:set { tone = tone, key = key, dismissable = true }
		end,
	}
	local ask, queue = asks.ask, asks.queue
	-- What runs behind it: this chat's subagents (the open chat's: its
	-- transcript is the session shown), the chats a Zeron chat started
	-- under it, and the processes it left running.
	local tasks = ui.computed(function(): { any }
		local out: { any } = {}
		local shown = store.session:get()
		local id = session_id:get()
		if shown and id and shown.id == id then
			out = lanes.tasks(id)
		end
		local first_ended = #out + 1
		for i, t in out do
			if t.ended then
				first_ended = i
				break
			end
		end
		-- (Processes after the subagents that run, before those that ended.)
		for _, t in asks.tasks:get() do
			table.insert(out, first_ended, t)
			first_ended += 1
		end
		return out
	end)
	-- Its own goes as the chat changes (kept: a reload isn't a change).
	local notice_for = ui.state("chat_notice_for", "?")
	ui.effect(function()
		local id = session_id:get() or ""
		if notice_for:peek() ~= id then
			notice_for:set(id)
			own_notice:set(nil)
		end
	end)
	-- Its Cloud machine, on a chat that has one: its state and its alert.
	local machine = CloudTray.bind {
		chat = function(): string?
			return session_id:get()
		end,
		device = function(): string?
			local s = open()
			return s and s.device
		end,
		running = function(): boolean
			return running:get()
		end,
		queue = function(): { any }
			return queue:get()
		end,
		waiting = function(): { any }?
			local s = open()
			return s and s.waiting
		end,
		retry = function(message: string?)
			local id = session_id:peek()
			if id and message then
				store.retry(id, message)
			end
		end,
		-- A send on its way: its machine coming up is said in the
		-- transcript, under it.
		sending = function(): boolean
			return CloudMachine.sending_last(store.messages)
		end,
	}
	-- This device's outbox for the chat (#1256): sends uploading or stopped,
	-- each removable where the engine can take one back. Whether it can is
	-- asked once a chat has one, by taking out a send that isn't there (an
	-- engine from before it refuses "unsupported"; then no Remove).
	local outbox = ui.computed(function(): { any }?
		local s = open()
		return s and s.waiting
	end)
	local removes_held = ui.state("chat_removes_held", nil :: { chat: string, can: boolean }?)
	ui.effect(function()
		local chat = session_id:get()
		if not chat or not CloudMachine.has_outbox(outbox:get()) then
			return
		end
		local known = removes_held:peek()
		if known and known.chat == chat then
			return
		end
		removes_held:set({ chat = chat, can = false })
		store.remove_held(chat, "zgx-asks-whether-it-can"):after(function()
			if session_id:peek() == chat then
				removes_held:set({ chat = chat, can = true })
			end
		end, function(err: any)
			local m = if type(err) == "table" and err.message then tostring(err.message) else tostring(err)
			-- (Unsure, not refused: asked again next time.)
			if m ~= "unsupported" and session_id:peek() == chat then
				removes_held:set(nil)
			end
		end)
	end)
	local function can_remove_held(): boolean
		local known = removes_held:get()
		return known ~= nil and known.chat == session_id:get() and known.can
	end
	-- The queue it shows: its own (one that waited long says how long), then
	-- what's held back for its machine (screens/chat/held), each removable,
	-- then this device's outbox.
	local shown_queue = ui.computed(function(): { any }
		local out: { any } = {}
		local now = machine.now()
		for _, q in queue:get() do
			if q.at and now - q.at > CloudMachine.LONG_WAIT then
				local row = table.clone(q)
				row.age = CloudMachine.age_words(now - q.at)
				table.insert(out, row)
			else
				table.insert(out, q)
			end
		end
		local reason = machine.hold_reason()
		for _, h in Held.of(session_id:get()) do
			table.insert(out, {
				id = Held.PREFIX .. h.id,
				text = h.text,
				-- (Refused, and its machine can take it: the person's to send.)
				waiting = if h.refused and not reason then nil else CloudMachine.held_label(reason),
			})
		end
		for _, row in CloudMachine.outbox_rows(outbox:get(), can_remove_held()) do
			table.insert(out, row)
		end
		return out
	end)
	-- A held message sent: out of the hold, to the chat. Refused, it's back in
	-- its place, marked so it doesn't only go again at once (and again): its
	-- row's Send sends it, Edit takes its words back, and the composer says
	-- it didn't send. Never lost: only Remove lets it go.
	local function release(chat: string, id: string)
		local e = Held.remove(chat, id)
		if not e then
			return
		end
		store.send(e.req :: any):after(function() end, function()
			local back = table.clone(e)
			back.refused = true
			Held.add(chat, back)
			own_notice:set { tone = "danger", key = "chat.composer.didnt_send", dismissable = true }
		end)
	end
	-- The machine can take them now (a size picked, a fresh machine): what's
	-- held goes, in order, however long it waited.
	ui.effect(function()
		local chat = session_id:get()
		if not chat or machine.hold_reason() ~= nil or machine.session() == nil then
			return
		end
		for _, e in Held.of(chat) do
			if not e.refused then
				release(chat, e.id)
			end
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
		local host_unreachable, host = false, nil
		if s and s.device and s.device ~= sessions.here:get() then
			for _, d in sessions.devices:get() or {} do
				if d.id == s.device then
					host_unreachable, host = d.online == false, d.name
				end
			end
		end
		return LinkNotice.of {
			connection = sessions.connection:get(),
			cloud = machine.cloud_link(string.find(shared.text:get(), "%S") ~= nil or #shared.cargo:get() > 0 or #queue:get() > 0),
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
	-- A new chat with no model yet: the first agent's default. (Never a session's:
	-- its model is its own.) Whenever that's so: the catalog come, or the
	-- new page come back to with none chosen there, so what shows is what a
	-- reload shows too.
	ui.effect(function()
		local c = store.catalog:get()
		if session_id:get() == nil and c.state == "ready" and model:get() == nil and #c.models > 0 then
			local first = default_of(c.models)
			model:set(first.value)
			effort:set(first.effort)
		end
	end)

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
	local completions = Completions.bind {
		key = "chat",
		text = shared.text,
		agent = agent,
		target = invocation_target,
		-- (A made chat's files are on its host: the device goes too, or this
		-- Mac's engine is asked about a chat it doesn't hold.)
		files = function(): any
			local id = session_id:peek()
			if id then
				local s = store.session:peek()
				return { chat = id, device = if s and s.id == id then s.device else nil }
			end
			local p = route.project:peek()
			return if p then { space = p.id, device = p.device } else nil
		end,
		in_chat = function(): boolean
			return session_id:get() ~= nil
		end,
		run = function(name: string)
			ZeronCommands.run(name, session_id:peek())
		end,
		-- (`@` lists what the draft carries, and the other chats.)
		cargo = function(): { any }
			return shared.cargo:get()
		end,
		chat = function(): string?
			return session_id:get()
		end,
	}

	-- A queued message being edited in the composer: which, and the draft
	-- it set aside (put back when the edit ends).
	local editing = ui.state("chat_editing", nil :: { item: string, aside: string, aside_cargo: { any }? }?)
	local edits = Asks.queue_edits {
		id = function(): string?
			return session_id:peek()
		end,
		text = shared.text,
		editing = editing,
		focus = function()
			shared.focus:add(1)
		end,
		say = function(tone: string, key: string)
			own_notice:set { tone = tone, key = key, dismissable = true }
		end,
		cargo = function(): { any }
			return shared.cargo:peek()
		end,
		set_cargo = function(list: { any })
			shared.cargo:set(list)
		end,
	}
	local end_edit = edits.finish
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

	-- Arriving on a chat (another session or a new chat shown: a sidebar or
	-- Activity row, Back or Forward, ⌘N, the palette; a split's pane taking
	-- the focus; back from Settings): the composer takes the keys. Once
	-- what brought it here is done (a palette closing
	-- puts focus back where it was first), so this comes last; a field
	-- that takes the keys after (a rename) has them. (Kept: a reload, or
	-- the window opening, isn't an arrival.)
	local arrived = ui.state("chat_composer_arrived", "?")
	ui.effect(function()
		local now = route.current:get()
		local pane = route.chat:get().session
		local chat = now.name == "session" or now.name == "home"
		local at = `{now.name}|{now.session or ""}|{now.project or ""}|{pane or ""}`
		local was = arrived:peek()
		if was == at then
			return
		end
		arrived:set(at)
		-- (A tab strip's arrows keep the keys on the tab they chose.)
		if route.take_keys_kept() or was == "?" or not chat then
			return
		end
		ui.after(0, function()
			shared.focus:add(1)
		end)
	end)

	local function send(text: string)
		-- Editing a queued message: Send saves it.
		if editing:peek() then
			end_edit("commit")
			return
		end
		-- A draft that is only one of Zeron's own commands runs it (its
		-- token goes; anything carried stays).
		local zeron = completions.command_of(shared.text:peek())
		if zeron then
			shared.text:set("")
			ZeronCommands.run(zeron, session_id:peek())
			return
		end
		local typed, carried = shared.take()
		local pack = Carried.pack(carried, typed, text)
		local attachments = pack.attachments
		-- (Each command or skill picked: its canonical reference.)
		-- (And each reference to what it carries, `@` picked: its name, as
		-- what follows names it.)
		local words = Carried.settle(Invocations.encode(if pack.typed ~= "" then pack.typed else text), carried)
		local text_out = Carried.text(words, pack)
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
					-- Its files too, before any added since.
					shared.restore_cargo_of(id, carried)
				end)
			end
			-- Its machine can't take it yet: kept, in the queue, till it can.
			if machine.hold_reason() then
				Held.add(id, { id = store.new_id(), at = os.time(), text = words, req = req })
				return
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
			-- (To the transcript at its foot: in a split, the focused pane's,
			-- which shows its lane's list, the prompt joining it there too.)
			local lane = foot_lane(id)
			local lists = { store.messages }
			if lane then
				table.insert(lists, lanes.view(lane).messages)
			end
			T.send(function()
				for _, list in lists do
					if list:index_of("id", mid) == nil then
						list:push {
							id = mid,
							role = "user",
							text = words,
							attachments = Carried.shown(pack),
							-- On its way from this moment: the line under it
							-- says so at once.
							waiting = "sending",
						}
					end
				end
				deliver(mid)
			end, mid, if lane then `lane.{lane}` else nil)
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
		-- (`why`: what to say instead, a Cloud chat's reason in words.)
		local failed = function(why: string?)
			own_notice:set { tone = "danger", text = why, key = if why then nil else "chat.composer.didnt_start", dismissable = true }
			if shared.text:peek() == "" then
				shared.text:set(text_out)
			end
		end
		-- On Cloud, as the shipping app starts one: the chat opens at once
		-- under the id it's made with, your message in it, sending, and its
		-- machine starting under that (a tool row, as the machine's own setup
		-- steps are); Cloud makes it meanwhile, and the message goes once it
		-- has. Refused, back to the new chat with the words and why.
		if plan.cloud then
			local chat_id = store.new_id()
			req.project = plan.project
			req.device = plan.device
			req.branch = plan.branch
			req.session = chat_id
			req.fresh = true
			req.message_id = store.new_id()
			req.worktree = plan.worktree
			T.sent_here(req.message_id :: string)
			own_notice:set(nil)
			store.creating(req)
			store.open(chat_id)
			-- (Down with its prompt: the dock's one slide.)
			slide_next = true
			route.open_session(chat_id)
			sessions
				.create({ id = chat_id, project = plan.project, branch = plan.branch, cloud = true })
				:after(function(made: any)
					local cwd = if type(made) == "table" then made.cwd else nil
					req.cwd = if type(cwd) == "string" and cwd ~= "" then cwd else nil
					store.created(chat_id)
					store.send(req):after(function() end, function(_e: any) end)
				end, function(e: any)
					local why = if type(e) == "table" and type(e.message) == "string" and string.find(e.message, " ", 1, true)
						then e.message
						else nil
					store.create_failed(chat_id)
					-- Still on it: back to the new chat, where the words and
					-- why are said. (Gone to another chat meanwhile, that
					-- chat's composer is left alone.)
					if route.current:peek().name == "session" and route.current:peek().session == chat_id then
						route.new_chat(plan.project)
						-- (Said once the new chat is back: a chat moving
						-- clears the composer's own line.)
						ui.after(0, function()
							failed(why)
						end)
					end
				end)
			return
		end
		own_notice:set { key = "chat.composer.starting", spinner = true }
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
	-- (The catalog's device, a new chat's project's host included.)
	local accounts = shared.model_accounts(run_device)
	local function branch_fixed(): ds.ComposerFixed?
		local s = open()
		if not s then
			return nil
		end
		local path: string?, project: string? = nil, nil
		for _, p in sessions.projects:get() do
			if p.id == s.project then
				path, project = p.path, p.name
			end
		end
		local lb = live_branch:get()
		local name = if lb.key:sub(1, #s.id + 1) == `{s.id}|` then lb.name else (s :: any).branch
		local on: string? = nil
		for _, d in sessions.devices:get() do
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

	local dictation = Dictation.bind(shared.text, function(): string
		return `{session_id:get() or ""}|{if editing:get() then editing:get().item else ""}|{route.current:get().name}`
	end, function(): boolean
		return asks.question:get() ~= nil or asks.approval:get() ~= nil or route.current:get().name == "settings" or route.current:get().name == "plugin"
	end, "main")

	local function composer(): Node
		return ds.Composer {
			dictation = dictation,
			-- Its measures, fold and open tray kept across a reload, so it
			-- is laid out as it was from the first frame.
			keep = "chat",
			toolbar_id = "composer-toolbar",
			open_model = shared.open_model,
			model_open = model_open,
			tray = TRAY,
			-- ("Jump to latest" of the transcript at its foot, while it
			-- shows: the trays' pill moves aside. In a split, the focused
			-- pane's: shared's `at_foot`.)
			aside = function(): number?
				return shared.pill_width_of("jump", shared.at_foot:get()):get()
			end,
			on_pill_width = shared.pill_teller("waiting", "window"),
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
			models_loading = function(): boolean
				return store.catalog:get().complete ~= true
			end,
			-- The session's model named by the host (its catalog's name, else
			-- its id), for the chip while the catalog shown doesn't list it.
			-- (Not a pick of another: that one's in the catalog. A session
			-- naming none: its agent's own default, by name.)
			model_label = function(): string?
				local s = open()
				if s and s.model == model:get() then
					return s.model_label
				end
				return nil
			end,
			-- A session naming no model runs its agent's own: the catalog's
			-- first of that agent (the host's `model_label` names it too),
			-- shown with its mark and reasoning, never sent as a pick.
			default_model = function(): string?
				local s = open()
				if not s or s.model ~= nil or s.harness == nil then
					return nil
				end
				local m = default_of(store.catalog:get().models, s.harness)
				return m and m.value
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
			queue = shown_queue,
			can_steer = function(): boolean
				return running:get()
			end,
			-- (A held row is the hold's: it goes on its own, or is let go.)
			-- (A held row goes on its own; one refused, by its Send.)
			on_queue_send = function(row: string)
				local held, chat = Held.id_of(row), session_id:peek()
				if CloudMachine.outbox_id(row) then
					return
				elseif not held then
					asks.on_queue_send(row)
				elseif chat and not machine.hold_reason() then
					release(chat, held)
				end
			end,
			-- (A refused held row's Edit: its words back in the composer,
			-- after what's there.)
			on_queue_edit = function(row: string)
				local held, chat = Held.id_of(row), session_id:peek()
				if CloudMachine.outbox_id(row) then
					return
				elseif not held then
					edits.on_queue_edit(row)
					return
				end
				local e = if chat then Held.remove(chat, held) else nil
				if e then
					local words, draft = e.req.text or e.text, shared.text:peek()
					shared.text:set(if draft == "" then words else `{draft}\n\n{words}`)
				end
			end,
			on_queue_save = edits.on_queue_save,
			on_queue_cancel = edits.on_queue_cancel,
			on_queue_remove = function(row: string)
				local held, chat = Held.id_of(row), session_id:peek()
				if held then
					if chat then
						Held.remove(chat, held)
					end
					return
				end
				-- An outbox send: out for good, and its prompt with it. (Gone
				-- already, it went out or was taken out elsewhere; the list
				-- says so as it changes.)
				local sent = CloudMachine.outbox_id(row)
				if sent then
					if chat and can_remove_held() then
						local message = nil
						for _, w in outbox:peek() or {} do
							if w.id == sent then
								message = w.message
							end
						end
						store.remove_held(chat, sent, message):after(function() end, function()
							own_notice:set { tone = "danger", key = "chat.composer.didnt_remove", dismissable = true }
						end)
					end
					return
				end
				asks.on_queue_remove(row)
			end,
			-- (Held rows stay after the queue's own: only those move.)
			on_queue_reorder = function(from: number, to: number)
				local own = #queue:peek()
				if from <= own and to <= own then
					asks.on_queue_reorder(from, to)
				end
			end,
			todos = asks.todos,
			tasks = tasks,
			-- A subagent's row opens it, a tab of the session's panel.
			on_open_task = function(ref: string)
				local id = session_id:peek()
				if id then
					lanes.open_task(id, ref)
				end
			end,
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
			question = asks.question,
			chosen = asks.chosen,
			on_pick = asks.on_pick,
			on_answer = asks.on_answer,
			on_close_question = asks.on_close_question,
			on_back = asks.on_back,
			approval = asks.approval,
			can_always = asks.can_always,
			on_approve = asks.on_approve,
			on_deny = asks.on_deny,
			notice = notice,
			on_dismiss_notice = function()
				own_notice:set(nil)
			end,
			-- (The only notice with an action: a sleeping machine's Wake.)
			on_notice_action = machine.wake,
			alerts = machine.alerts,
			on_alert_action = machine.on_alert,
			on_dismiss_alert = machine.dismiss,
			disabled = function(): boolean
				local s = store.session:get()
				local id = session_id:get()
				return id ~= nil and s ~= nil and s.id == id and (s.state == "missing" or s.state == "offline")
			end,
			focus = shared.focus,
			mentions = completions.lists,
			on_mention = completions.on_mention,
			mention_enabled = completions.enabled,
			mention_empty = completions.empty,
			mention_sections = completions.sections,
			draft_key = function(): any
				return session_id:get() or ""
			end,
		}
	end

	local page = ui.ref()
	-- The page's height as last laid out, kept (a reload keeps state): a
	-- composer made again reads it until its own page is measured, so its
	-- spring starts where it rests instead of rising into place again.
	local page_h = ui.state("chat_page_h", 0)
	local composer_zone = shared.drop_zone("composer")
	-- There to open the model picker in (the shell's ⌘/), while it is:
	-- counted, not a flag. A page that swaps its composer mounts the new one
	-- before the old one's cleanup runs, and a flag the old one cleared was
	-- left false under the new one: ⌘/ did nothing (seen on Windows). Each
	-- composer takes back only the count it added.
	local counted = false
	ui.on_mount(function()
		if not counted then
			counted = true
			shared.composer_mounted()
		end
	end)
	ui.on_cleanup(function()
		if counted then
			counted = false
			shared.composer_gone()
		end
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
		local hero = h / 2 + Dock.hero_offset()
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
	-- (Moving to another chat, as the focus moves between a split's
	-- panes, is shown for something else too: its draft, and its size for
	-- it, there at once rather than growing or shrinking into it.)
	local chat_was: string? = route.chat:peek().session
	ui.effect(function()
		local now = route.chat:get().session
		if now ~= chat_was then
			chat_was = now
			snap_until = ui.now() + SNAP_MS
			snap:set(snap:peek() + 1)
		end
	end)
	-- (The sidebar opening or closing moves the page's edges (a new
	-- chat's page goes to the window's edges as it closes): its place
	-- follows the page as it moves, at once, rather than easing after it on
	-- a clock of its own, which drew a small loop as the two went their own
	-- ways.)
	local shell_view = require("@app/screens/shell/view")
	local fit = require("@app/screens/shell/fit")
	local sidebar_was = not shell_view.collapsed:peek() and fit.current:peek().sidebar
	ui.effect(function()
		local now = not shell_view.collapsed:get() and fit.current:get().sidebar
		if now ~= sidebar_was then
			sidebar_was = now
			snap_until = ui.now() + SNAP_MS
		end
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
			-- (Hidden, Settings standing in its place, it has no height: not somewhere to
			-- move to, nor to glide back from.)
			if size.h >= 1 and math.abs(page_h:peek() - size.h) >= 1 then
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
				local at = Dock.hero_offset() - rest / 2
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
					if size.h >= 1 and math.abs(shared.dock_h:peek() - size.h) > 0.01 then
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
