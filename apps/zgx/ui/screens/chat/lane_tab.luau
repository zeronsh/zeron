--!strict
-- A lane in a chat's panel (screens/panel): a side chat, or a subagent, as
-- the shipping app's right-pane tabs show them (RightSurface::SideChat,
-- ::Subagent). The host watches its chat beside the open one, in a slot of
-- its own (the tab's id: `chat.open_lane`, let go as the tab goes). A side
-- chat is its transcript and a composer of its own (its draft its own, not
-- the chat page's); a subagent's is its transcript alone, to read.
--
--   LaneTab { tab = { id, kind = "side_chat" | "subagent", chat, title } }

local ds = require("@ds")
local Dictation = require("@app/screens/chat/dictation")
local T = require("@ds/transcript")
local lanes = require("@app/screens/chat/lanes")
local TurnGlue = require("@app/screens/chat/turn_glue")
local store = require("@app/screens/chat/store")
local prefs = require("@app/screens/settings/prefs")
local locale = require("@ds/locale")
local sessions = require("@account/sessions")
local route = require("@app/screens/shell/route")
local Where = require("@app/screens/chat/where")
local Dock = require("@app/screens/chat/dock")
local CloudTray = require("@app/screens/chat/cloud_tray")
local LinkNotice = require("@app/screens/chat/link_notice")
local ReplyImages = require("@app/screens/chat/reply_images")
local FileLinks = require("@app/screens/chat/file_links")
local LinkChips = require("@app/screens/chat/link_chips")
local LinkMenu = require("@app/screens/chat/link_menu")
local boundary = require("@app/screens/shell/boundary")
local Quote = require("@ds/quote")
-- (The chat page's places in its chats, which a split's panes share.)
local Shared = require("@app/screens/chat/shared")
local Asks = require("@app/screens/chat/asks")
local Carried = require("@app/screens/chat/carried")
local Completions = require("@app/screens/chat/completions")
local Find = require("@app/screens/chat/find")
local Invocations = require("@app/screens/chat/invocations")
local ZeronCommands = require("@app/screens/chat/zeron_commands")
local look = require("@app/screens/shell/project_look")
local Appshots = require("@app/screens/chat/appshots")
local DropOverlay = require("@ds/drop_overlay")

-- Two places in a transcript (ds/transcript's `keep`), the same.
local function same(a: any, b: any): boolean
	if a == b then
		return true
	end
	if type(a) ~= "table" or type(b) ~= "table" then
		return false
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

-- What its tab says, and its icon, as the shipping app's.
local M = {}

-- A side chat's row in the session list, as it stands now.
local function row_of(chat: string?): any
	if not chat then
		return nil
	end
	for _, list in sessions.now:get().children or {} do
		for _, c in list do
			if c.id == chat then
				return c
			end
		end
	end
	return nil
end

function M.title(tab: any): string
	-- (Its chat's name as it's named or renamed; before that, as it was opened.)
	local row = if tab.kind == "side_chat" then row_of(tab.chat) else nil
	if row and row.title and row.title ~= "" then
		return row.title
	end
	if tab.title and tab.title ~= "" then
		return tab.title
	end
	return locale.s(if tab.kind == "subagent" then "chat.lane_tab.subagent" else "chat.lane_tab.side_chat")
end

function M.icon(_tab: any): string
	return "side-chat"
end

-- A side chat's project: where its composer has it before its first message,
-- then its chat's; its parent's while neither is known.
local function project_of(tab: any, picked: State<string?>): any
	local chat = tab.chat
	local parent = lanes.unsent:get()[chat]
	local id: string? = nil
	if parent then
		id = picked:get()
	else
		local s = lanes.view(tab.id).session:get()
		if s and s.id == chat and s.harness then
			id = s.project
			parent = nil
		else
			local row = row_of(chat)
			parent = if row then row.parent else nil
		end
	end
	local now = sessions.now:get()
	if not id and parent then
		for _, c in now.chats do
			if c.id == parent then
				id = c.project
			end
		end
	end
	for _, p in now.projects do
		if p.id == id then
			return p
		end
	end
	return nil
end

-- A side chat's tile in its tab: its project's, as a chat's on the top bar (one
-- with no project, the side chat's glyph, there too).
function M.tile(tab: any): Node
	local picked = ui.state(`chat_lane_project_{tab.chat}`, nil :: string?)
	return ui.presence(function(): string
		local p = project_of(tab, picked)
		return if p then p.id else ""
	end, function(which: string): Node
		if which == "" then
			return ds.Tile { size = 20, inside = ds.Icon { name = "side-chat", size = 14 } }
		end
		return ds.Tile {
			size = 20,
			label = function(): string?
				local p = project_of(tab, picked)
				return if p then p.name else nil
			end,
			hue = function(): string?
				local p = project_of(tab, picked)
				return if p then look.of(p).hue else nil
			end,
			src = function(): string?
				local p = project_of(tab, picked)
				return if p then look.of(p).src else nil
			end,
			emoji = function(): string?
				local p = project_of(tab, picked)
				return if p then look.of(p).emoji else nil
			end,
			project = function(): string?
				local p = project_of(tab, picked)
				return if p then p.id else nil
			end,
		}
	end, { w = 20, h = 20, shrink = 0 })
end

-- What a side chat's tab says at its end: whether it awaits a person or is at
-- work (what a chat's tab on the top bar says).
local TAB_STATES: { [string]: boolean } = { approval = true, input = true, working = true, connecting = true }
function M.state(tab: any): string?
	local row = row_of(tab.chat)
	local kind = if row then row.status else nil
	return if kind and TAB_STATES[kind] then kind else nil
end

-- Each tab's find bar and model picker, by its slot, while it's made.
local finders: { [string]: any } = {}
local pickers: { [string]: State<number> } = {}

function M.open_find(tab: any): boolean
	local find = finders[tab.id]
	return find ~= nil and find.open()
end

function M.open_model(tab: any): boolean
	local picker = pickers[tab.id]
	if not picker then
		return false
	end
	picker:add(1)
	return true
end

-- A side chat's composer: the chat page's (ds.Composer), its footer the
-- same row: the access mode, where it works, the model. Its draft is its
-- own. Its picks start as its chat has them (the engine's `harness/model`,
-- its access, its place), or, before its first message makes it, as its
-- parent's are (the shipping app's `create_child_chat` copies the parent's
-- config and place): the parent's agent and model (an agent with no model
-- named starts on that agent's first), its access, its project, device and
-- checkout. Until then where it works is a picker, as a new chat's (Cloud
-- left out: a side chat of a Cloud chat shares its machine); after, it's
-- said, fixed, as a session's is. A pick the person made stays theirs, kept
-- by the chat across tab switches and a reload, and every message goes with
-- what's picked (a changed agent, model or access becomes the chat's, as the
-- chat page's does). A message that didn't go says so on the composer's own
-- notice line, above it, never over it.
local function composer(chat: string, view: any, name: string, focus: State<number>, open_model: State<number>, split: boolean?, focused: (() -> boolean)?): Node
	-- (Its draft kept by the chat, as its picks: a switch of tabs or a
	-- reload finds it as it was; a passage quoted from its transcript lands
	-- in it.)
	-- (A pane of the page's split that isn't focused shows the chat
	-- page's own draft for its chat, as that keeps it while another chat
	-- is shown (screens/chat/shared's drafts), so focused, the chat page's
	-- composer goes on with the same words.)
	local draft = ui.state(if split then `chat_split_draft_{chat}` else `chat_lane_draft_{chat}`, "")
	-- Its draft's chips: one draft's per chat (screens/chat/shared), wherever
	-- it's kept. (A split's pane shows the chat page's.)
	local carried = ui.use_state({} :: { any })
	ui.effect(function()
		carried:set(Shared.cargo_of(chat))
	end)
	-- The passages mentioned in its draft, by their lines (the chat page's
	-- `shared.quotes`): each goes with the message in full.
	local quotes = ui.state(`chat_lane_quotes_{chat}`, {} :: { [string]: string })
	-- (A split's pane: as the focus comes or goes, its size there at once,
	-- never growing into it (ds.Composer's `snap`).)
	local snap = ui.use_state(0)
	if split and focused then
		local was = focused()
		ui.effect(function()
			local now = focused()
			if now ~= was then
				was = now
				snap:set(snap:peek() + 1)
			end
		end)
	end
	-- A chip taken off its draft, wherever the draft is now.
	local function remove_carried(cid: string)
		Shared.remove_cargo_of(chat, cid)
	end
	-- The options picked in a question its chat asks. (A split's pane: its
	-- chat's draft's, below. Else kept by the chat, as its draft is.)
	local chosen = if split then ui.use_state({} :: { [string]: boolean }) else ui.state(`chat_lane_chosen_{chat}`, {} :: { [string]: boolean })
	if split then
		-- (Its chat's draft, by its id: its words, chips and a question's
		-- picks, one draft per chat (screens/chat/shared), shown here and
		-- written back as they change, wherever it's kept. Never the focused
		-- chat's: the focus moving changes nothing this shows.)
		ui.effect(function()
			local d = Shared.draft_of(chat)
			if draft:peek() ~= d.text then
				draft:set(d.text)
			end
			chosen:set(d.chosen)
		end)
		ui.effect(function()
			local text = draft:get()
			if Shared.draft_of(chat).text ~= text then
				Shared.set_draft_of(chat, "text", text)
			end
		end)
		ui.effect(function()
			local picked = chosen:get()
			if Shared.draft_of(chat).chosen ~= picked then
				Shared.set_draft_of(chat, "chosen", picked)
			end
		end)
	end
	local notice = ui.use_state(nil :: any)
	-- What its chat asks, and its queue, todos and tasks, from its own
	-- session, as the window's composer shows them.
	local asks = Asks.bind {
		id = function(): string?
			return chat
		end,
		session = function(): any
			return view.session:get()
		end,
		chosen = chosen,
		asked = ui.state(`chat_split_asked_{chat}`, nil :: any),
		-- (A steer glides in its own transcript, as its sends do.)
		name = name,
		say = function(tone: string, key: string)
			notice:set({ tone = tone, text = locale.s(key), dismissable = true })
		end,
	}
	local tray = ui.state(`chat_split_tray_{chat}`, nil :: any)
	local session = view.session
	-- Its Cloud machine, on a chat that has one (a side chat shares its
	-- parent's).
	local machine = CloudTray.bind {
		chat = function(): string?
			return chat
		end,
		device = function(): string?
			local s = session:get()
			return s and s.device
		end,
		running = function(): boolean
			local s = session:get()
			return s ~= nil and s.running == true
		end,
	}
	-- A queued message edited in its composer. (A split's pane: a press
	-- focuses it, the window's composer doing this.)
	local editing = ui.state(`chat_lane_editing_{chat}`, nil :: { item: string, aside: string, aside_cargo: { any }? }?)
	local edits = Asks.queue_edits {
		id = function(): string?
			return chat
		end,
		text = draft,
		editing = editing,
		focus = function()
			focus:add(1)
		end,
		say = function(tone: string, key: string)
			notice:set({ tone = tone, text = locale.s(key), dismissable = true })
		end,
		cargo = function(): { any }
			return Shared.peek_cargo(chat)
		end,
		set_cargo = function(list: { any })
			Shared.set_cargo_of(chat, list)
		end,
	}
	-- A tab closed mid-edit lets go of the edit, and made again finds its row
	-- no longer held: the draft it set aside back. (A row gone is another
	-- matter: it keeps the words, and saving says so.)
	if not split then
		ui.effect(function()
			local e = editing:get()
			local s = session:get()
			if not e or not s or s.state == "loading" then
				return
			end
			for _, row in s.queue or {} do
				if row.id == e.item and not row.editing then
					ui.batch(function()
						editing:set(nil)
						draft:set(e.aside)
						if e.aside_cargo then
							Shared.set_cargo_of(chat, e.aside_cargo)
						end
					end)
				end
			end
		end)
	end
	local running = ui.computed(function(): boolean
		local s = session:get()
		return s ~= nil and s.running == true
	end)
	-- (Kept by the chat, outside the view: a pick stays across a switch of
	-- tabs and a reload.)
	local model = ui.state(`chat_lane_model_{chat}`, nil :: string?)
	local effort = ui.state(`chat_lane_effort_{chat}`, nil :: string?)
	local access = ui.state(`chat_lane_access_{chat}`, nil :: string?)
	local project = ui.state(`chat_lane_project_{chat}`, nil :: string?)
	local device = ui.state(`chat_lane_device_{chat}`, nil :: string?)
	local checkout = ui.state(`chat_lane_checkout_{chat}`, nil :: string?)
	local fast = ui.state(`chat_lane_fast_{chat}`, false)
	local options = ui.state(`chat_lane_options_{chat}`, nil :: { [string]: any }?)
	local plan = ui.state(`chat_lane_plan_{chat}`, false)
	local branch = ui.state(`chat_lane_branch_{chat}`, nil :: string?)
	-- Where it was seeded from (its parent's place), so a send can tell
	-- whether the person moved it.
	local seed = ui.state(`chat_lane_place_{chat}`, nil :: { [string]: any }?)
	local function unsent(): boolean
		return lanes.unsent:get()[chat] ~= nil
	end
	-- Its parent: the open chat's record when that's it (its access is
	-- there), else its row in the sidebar's list.
	local function parent_of(): (any?, any?)
		local parent = lanes.unsent:get()[chat]
		if not parent then
			return nil, nil
		end
		local row: any = nil
		for _, c in sessions.now:get().chats do
			if c.id == parent then
				row = c
			end
		end
		local open = store.session:get()
		return row, if open and open.id == parent then open else nil
	end
	-- What it runs as its chat (or its parent) has it: the agent, the
	-- model's picker value, the effort, the access.
	local function as_made(): (string?, string?, string?, string?)
		local s = session:get()
		if s and s.harness then
			return s.harness, s.model, s.effort, s.access
		end
		local row, open = parent_of()
		if open and open.harness then
			return open.harness, open.model, open.effort, open.access
		end
		if row then
			local m = row.model
			if m and row.harness and not string.find(m, "/", 1, true) then
				m = `{row.harness}/{m}`
			end
			return row.harness, m, nil, nil
		end
		return nil, nil, nil, nil
	end
	-- (What was put in the pickers last from the chat: a pick that differs
	-- is the person's, and stays. Kept by the chat too, so a tab made again,
	-- or a reload, tells the person's pick from the chat's.)
	local seeded = ui.state(`chat_lane_seeded_{chat}`, {} :: { [string]: any })
	ui.effect(function()
		local harness, want, eff, acc = as_made()
		if not want and harness then
			for _, m in store.catalog:get().models do
				if m.harness == harness then
					want = m.value
					break
				end
			end
		end
		acc = acc or "full"
		local was = seeded:peek()
		local now = table.clone(was)
		-- (Never seeded: whatever stands is the chat's to replace.)
		local first = next(was) == nil
		if want then
			if (first or model:peek() == was.model) and model:peek() ~= want then
				model:set(want)
			end
			now.model = want
		end
		if (first or effort:peek() == was.effort) and effort:peek() ~= eff then
			effort:set(eff)
		end
		now.effort = eff
		if (first or access:peek() == was.access) and access:peek() ~= acc then
			access:set(acc)
		end
		now.access = acc
		-- (Its Fast, the model's options and plan mode the same.)
		local made = session:get()
		if not made or not made.harness then
			local _, open = parent_of()
			made = open
		end
		for key, st in { fast = fast, options = options, plan = plan } :: { [string]: any } do
			local v = if made then made[key] else nil
			-- (Off is what one with none set runs with.)
			local before = if was[key] ~= nil then was[key] elseif key == "options" then nil else false
			if v ~= nil and (first or same(st:peek(), before)) and not same(st:peek(), v) then
				st:set(v)
			end
			now[key] = v
		end
		if not same(now, was) then
			seeded:set(now)
		end
	end)
	-- Before its first message: where it works starts where its parent
	-- does (once, as the parent is known).
	ui.effect(function()
		if not unsent() or seed:peek() ~= nil then
			return
		end
		local row = parent_of()
		if not row then
			return
		end
		local place = {
			project = row.project or Where.NO_PROJECT,
			device = row.device,
			checkout = "local",
			branch = row.branch,
		}
		-- (Started elsewhere, the panel home's group "+": that project or
		-- device instead, its branch the project's own. The seed stays the
		-- parent's, so the send carries the place.)
		local at = lanes.starts:peek()[chat]
		local start_project = if at and at.project then at.project elseif at and at.device and at.device ~= place.device then nil else place.project
		local start_device = if at and at.device then at.device else place.device
		local moved = start_project ~= place.project or start_device ~= place.device
		project:set(start_project)
		device:set(start_device)
		checkout:set(place.checkout)
		branch:set(if moved then nil else place.branch)
		seed:set(place)
	end)
	local where = Where.bind {
		device = device,
		project = project,
		checkout = checkout,
		branch = branch,
		active = unsent,
		slot = `lane.{chat}`,
		remember = false,
	}
	-- (No Cloud machine of its own for a side chat.)
	local devices = ui.computed(function(): { any }
		local out = {}
		for _, d in where.devices:get() do
			if not d.cloud then
				table.insert(out, d)
			end
		end
		return out
	end)
	-- Once made: where it works, said.
	local function branch_fixed(): any
		if unsent() then
			return nil
		end
		local s = session:get()
		local function project_row(id: string?): any?
			for _, p in sessions.projects:get() do
				if p.id == id then
					return p
				end
			end
			return nil
		end
		if not s or s.harness == nil then
			-- (Just made, its chat not heard yet: where it was made.)
			local p = project_row(project:get())
			return {
				name = branch:get(),
				worktree = checkout:get() == "worktree",
				project = if p then p.name else nil,
				hue = if p then (route.look(p.id) or { hue = route.hue(p.id) }).hue else nil,
				src = if p then (route.look(p.id) or {}).src else nil,
				emoji = if p then (route.look(p.id) or {}).emoji else nil,
			}
		end
		local p = project_row(s.project)
		local on: string? = nil
		for _, d in sessions.devices:get() do
			if d.id == s.device then
				on = d.name
			end
		end
		return {
			device = on,
			name = s.branch,
			worktree = p ~= nil and s.cwd ~= nil and s.cwd ~= p.path,
			project = if p then p.name else nil,
			hue = if p then (route.look(p.id) or { hue = route.hue(p.id) }).hue else nil,
			src = if p then (route.look(p.id) or {}).src else nil,
			emoji = if p then (route.look(p.id) or {}).emoji else nil,
		}
	end
	-- What a message goes with: the pick, its agent, the effort, the access.
	local function picks(): { [string]: any }
		local m = model:peek()
		local out: { [string]: any } = { model = m, effort = effort:peek(), access = access:peek(), fast = fast:peek(), options = options:peek(), plan = plan:peek() }
		for _, c in store.catalog:peek().models do
			if c.value == m then
				out.harness = c.harness
			end
		end
		return out
	end
	-- Where its first message makes it: nil, its parent's place as it is;
	-- else where the person moved it (`where.plan()`).
	local function place(): { [string]: any }?
		local was = seed:peek()
		if
			was
			and project:peek() == was.project
			and device:peek() == was.device
			and checkout:peek() == was.checkout
			and branch:peek() == was.branch
		then
			return nil
		end
		return where.plan() :: any
	end
	-- A new side chat's composer takes the keys as it's made.
	if lanes.take_keys:peek()[chat] then
		ui.on_mount(function()
			local left = table.clone(lanes.take_keys:peek())
			left[chat] = nil
			lanes.take_keys:set(left)
			focus:add(1)
		end)
	end
	-- (A chat sent already has its host in the session: `device` is only
	-- seeded while it's unsent, so reading it alone asked this computer's
	-- engine for a chat running on another's.)
	local accounts = Shared.model_accounts(function(): string?
		if unsent() then
			return device:get()
		end
		local s = session:get()
		return if s then s.device else nil
	end)
	-- What `@`, `/` and `$` offer, as the chat page's. (A split's pane: the
	-- window's composer types, a press focusing it.)
	local function run_command(command: string)
		if command == "model" then
			open_model:add(1)
		else
			ZeronCommands.run(command, if unsent() then nil else chat)
		end
	end
	local completions = if split
		then nil
		else Completions.bind {
			key = `chat_lane_{chat}`,
			text = draft,
			agent = function(): string?
				local chosen = model:get()
				for _, m in store.catalog:get().models do
					if m.value == chosen then
						return m.harness
					end
				end
				return nil
			end,
			target = function(): any
				if unsent() then
					local p = project:get()
					return if p and p ~= Where.NO_PROJECT then { space = p, device = device:get() } else nil
				end
				local s = session:get()
				return { chat = chat, device = if s then s.device else nil }
			end,
			files = function(): any
				if unsent() then
					local p = project:peek()
					return if p and p ~= Where.NO_PROJECT then { space = p, device = device:peek() } else nil
				end
				local s = session:peek()
				return { chat = chat, device = if s then s.device else nil }
			end,
			in_chat = function(): boolean
				return not unsent()
			end,
			run = run_command,
			-- (`@` lists what the draft carries, and the other chats.)
			cargo = function(): { any }
				return carried:get()
			end,
			chat = function(): string?
				return chat
			end,
		}
	local dictation = Dictation.bind(draft, function(): string
		local edit = editing:get()
		return `{chat}|{name}|{if edit then edit.item else ""}`
	end, function(): boolean
		local current = session:get()
		return asks.question:get() ~= nil or asks.approval:get() ~= nil
			or (current ~= nil and (current.state == "missing" or current.state == "offline"))
			or (focused ~= nil and not focused())
	end, name)
	return ds.Composer {
		dictation = dictation,
		value = draft,
		snap = if split then snap else nil,
		-- (Its transcript's "Jump to latest", while it shows: its trays'
		-- pill moves aside, the two a centred pair. By the lane's name, its
		-- transcript's: screens/chat/shared's `pill_width_of`.)
		aside = function(): number?
			return Shared.pill_width_of("jump", name):get()
		end,
		on_pill_width = Shared.pill_teller("waiting", name),
		cargo = carried,
		-- (Its chips' × under the pointer, as the chat page's.)
		on_remove_cargo = remove_carried,
		-- (What's dropped on it is its tab's to take, or a split pane's card,
		-- under their own overlays.)
		drop_overlay = false,
		-- Its + and a picture pasted: chips on its own draft, as the chat page's.
		-- (A split's pane: a press focuses it, the window's composer taking
		-- over.)
		on_attach = if split
			then nil
			else function()
				ui.open_files({ title = ui.t("chat.composer.attach_files"):get(), multiple = true }):after(function(files: any)
					if files and #files > 0 then
						Shared.attach_files(files, chat)
						focus:add(1)
					end
				end, function() end)
			end,
		on_paste_image = if split
			then nil
			else function(): boolean
				store.paste_image():after(function(path: string?)
					if type(path) == "string" and path ~= "" then
						Shared.attach_files({ { name = "image.png", path = path } }, chat)
						focus:add(1)
					end
				end, function() end)
				return true
			end,
		focus = if split then nil else focus,
		-- (ds.Composer's placeholder is a string: read as the tab is made.)
		-- (A split's pane is a chat of its own: the chat page's words.)
		placeholder = locale.s(if split then "chat.composer.placeholder" else "chat.lane_tab.placeholder"),
		running = running,
		access = access,
		plan = plan,
		model = model,
		effort = effort,
		fast = fast,
		options = options,
		open_model = open_model,
		-- The app's settings: what sends, and whether Escape stops the agent.
		send_with = function(): string
			return prefs.send_with:get()
		end,
		on_escape = function(): boolean
			return store.escape_chat(chat, running:peek())
		end,
		disabled = function(): boolean
			local s = session:get()
			return s ~= nil and (s.state == "missing" or s.state == "offline")
		end,
		mentions = if completions then completions.lists else nil,
		on_mention = if completions then completions.on_mention else nil,
		mention_empty = if completions then completions.empty else nil,
		mention_sections = if completions then completions.sections else nil,
		-- (Its own line first, else its Cloud machine's, on a chat with one.)
		notice = ui.computed(function(): any
			local mine = notice:get()
			if mine then
				return mine
			end
			local link = machine.cloud_link(string.find(draft:get(), "%S") ~= nil)
			return if link then LinkNotice.of { cloud = link } else nil
		end),
		on_dismiss_notice = function()
			notice:set(nil)
		end,
		on_notice_action = machine.wake,
		alerts = machine.alerts,
		on_alert_action = machine.on_alert,
		on_dismiss_alert = machine.dismiss,
		harnesses = function(): { any }
			return store.catalog:get().harnesses
		end,
		models_loading = function(): boolean
			return store.catalog:get().complete ~= true
		end,
		models = function(): { any }
			return store.catalog:get().models
		end,
		accounts = accounts.accounts,
		on_account = accounts.on_account,
		on_accounts_open = accounts.on_accounts_open,
		-- What its context holds: the chat page's ring and card, from the
		-- side chat's own numbers (once it has run and they're reported).
		usage = function(): any
			local s = session:get()
			if not s or (s.context_tokens == nil and s.context_window == nil) then
				return nil
			end
			return { tokens = s.context_tokens, window = s.context_window }
		end,
		project = project,
		projects = where.projects,
		device = device,
		devices = devices,
		checkout = checkout,
		branch = branch,
		branches = where.branches,
		branches_empty = where.branches_empty,
		branch_fixed = branch_fixed,
		-- (Work in's "New project": the shell's New project palette.)
		on_new_project = function()
			local add: any = boundary.load("@app/screens/shell/add_project")
			if add then
				add.open()
			end
		end,
		on_send = function(text: string)
			-- Editing a queued message: Send saves it.
			if editing:peek() then
				edits.finish("commit")
				return
			end
			-- A draft that is only one of Zeron's own commands runs it.
			local command = if completions then completions.command_of(draft:peek()) else nil
			if command then
				draft:set("")
				run_command(command)
				return
			end
			-- (The passages it mentions, each in full.)
			local said_quotes = quotes:peek()
			local words = string.match(Quote.expand(text, Quote.kept(text, said_quotes)), "^%s*(.-)%s*$") or ""
			if words == "" and #carried:peek() == 0 then
				return
			end
			-- (Nowhere it can start, a project gone: the line says why.)
			local is_unsent = lanes.unsent:peek()[chat] ~= nil
			if is_unsent and where.blocked:peek() then
				notice:set({ tone = "danger", text = where.blocked:peek(), dismissable = true })
				return
			end
			ui.batch(function()
				draft:set("")
				quotes:set({})
			end)
			notice:set(nil)
			-- What it carries goes with it, as the chat page's does.
			local chips = Shared.take_cargo_of(chat)
			local pack = Carried.pack(chips, words, words)
			-- (Each command or skill picked: its canonical reference.)
			-- (And each reference to what it carries, `@` picked: its name.)
			local typed = Carried.settle(Invocations.encode(pack.typed), chips)
			local prompt = Carried.text(typed, pack)
			-- As the chat page sends: the prompt joins its transcript as the
			-- transcript's send steps clear of the end (it glides to the top,
			-- its reply's room opening under it), then goes to the host with
			-- that id, so the host's copy is the same row.
			-- (While a turn runs it's queued instead: it shows in the queue, and
			-- what's picked waits for the turn it will run in.)
			local queued = not is_unsent and running:peek()
			local req: { [string]: any } = if queued then {} else picks()
			req.attachments = pack.attachments
			req.appshots = Appshots.of_cargo(chips)
			local function failed(err: any)
				ui.batch(function()
					draft:set(text)
					quotes:set(said_quotes)
				end)
				Shared.restore_cargo_of(chat, chips)
				local why = if type(err) == "table" and err.message then tostring(err.message) else nil
				notice:set({
					tone = "danger",
					text = if is_unsent and why then why else locale.s("chat.lane_tab.didnt_send"),
					dismissable = true,
				})
			end
			if queued then
				req.session, req.text = chat, prompt
				store.send(req :: any):after(function() end, failed)
				return
			end
			local mid = store.new_id()
			req.message_id = mid
			local sent: any = nil
			T.send(function()
				local here = view.messages:index_of("id", mid) ~= nil
				if not here then
					-- (On its way from this moment: the line under it says so.)
					view.messages:push { id = mid, role = "user", text = typed, attachments = Carried.shown(pack), waiting = "sending" }
				end
			end, mid, name)
			-- Its first message makes it (New side chat), then sends.
			if is_unsent then
				sent = lanes.send_first(chat, prompt, req, place())
			else
				req.session, req.text = chat, prompt
				sent = store.send(req :: any)
			end
			sent:after(function() end, failed)
		end,
		on_stop = function()
			store.interrupt(chat)
		end,
		-- (Hidden while focused, its draft shown by the window's composer: only that
		-- one's menus open, or a trigger typed there opens a second here.)
		mention_enabled = if split and focused
			then function(_trigger: string): boolean
				return not focused()
			end
			elseif completions then completions.enabled
			else nil,
		-- (Its chat's own.)
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
		queue = asks.queue,
		can_steer = function(): boolean
			return running:get()
		end,
		on_queue_send = asks.on_queue_send,
		on_queue_edit = if split then nil else edits.on_queue_edit,
		on_queue_save = if split then nil else edits.on_queue_save,
		on_queue_cancel = if split then nil else edits.on_queue_cancel,
		on_queue_remove = asks.on_queue_remove,
		on_queue_reorder = asks.on_queue_reorder,
		todos = asks.todos,
		tasks = asks.tasks,
		on_stop_task = function(task: string)
			store.stop_task(chat, task):after(function() end, function()
				notice:set({ tone = "danger", text = locale.s("chat.composer.couldnt_stop_task"), dismissable = true })
			end)
		end,
		tray = tray,
	}
end

-- A fork's tab while the engine copies it, or once it couldn't: the
-- shipping app's words, and Retry (the same fork again).
local function forking(chat: string): Node
	return ui.presence(function(): string?
		local f = lanes.forks:get()[chat]
		return if f then f.state else nil
	end, function(state: string): Node
		if state == "failed" then
			return ds.EmptyState {
				icon = "git-branch",
				size = "sm",
				fill = true,
				title = ui.t("chat.lane_tab.couldnt_fork") :: any,
				body = function(): string
					local f = lanes.forks:get()[chat]
					return if f and f.error then f.error else ""
				end,
				action = {
					label = ui.t("chat.lane_tab.retry") :: any,
					on_click = function()
						lanes.retry_fork(chat)
					end,
				},
			}
		end
		return ds.EmptyState { icon = "git-branch", size = "sm", fill = true, title = ui.t("chat.lane_tab.forking") :: any }
	end, { grow = 1, w_full = true })
end

-- A lane's composer's height as measured, by its slot.
function M.dock_height(slot: string): State<number>
	return ui.state(`chat_lane_dock_h_{slot}`, Dock.REST)
end

M.Tab = ui.component(function(props: { tab: any }): Node
	local tab = props.tab
	local slot, chat = tab.id, tab.chat
	local data = tab.data or {}
	local view = lanes.view(slot)
	-- What the tab is now: a fork under way or failed, or its chat (a new
	-- side chat's not yet made, until its first message).
	local kind = ui.computed(function(): string
		local f = lanes.forks:get()[chat]
		if f then
			return "fork"
		end
		return if lanes.unsent:get()[chat] then "unsent" else "chat"
	end)
	-- Files, and the files pane's, carried over it, dropped anywhere on it, as
	-- on the chat page: chips on its own draft (a sidebar chat, mentioned), its
	-- composer taking the keys. (A split's pane is the page's to take: its card.)
	local focus = ui.use_state(0)
	-- Bumped, its model picker opens (⌘/, /model), as the chat page's.
	local open_model = ui.use_state(0)
	-- ⌘F's own, over its own transcript.
	local find = Find.bind(`chat.lane.{slot}.find`, view.messages, function(): boolean
		return true
	end, `lane-find-{slot}`)
	finders[slot], pickers[slot] = find, open_model
	ui.on_cleanup(function()
		if finders[slot] == find then
			finders[slot], pickers[slot] = nil, nil
		end
	end)
	local zone, carrying, carrying_session = nil :: any, nil :: any, nil :: any
	if tab.kind == "side_chat" and data.split ~= true then
		zone, carrying, carrying_session = Shared.own_drop_zone(`lane.{slot}`, chat, function()
			focus:add(1)
		end)
	end
	-- Watched once there's a chat to watch, while its tab is open: the
	-- panel lets it go as the tab closes. (A reload asks for the same chat
	-- again, which the host already has: nothing more happens.)
	-- (A split's pane is watched by the page, for as long as it's in the
	-- split, focused or not.)
	local split = data.split == true
	ui.effect(function()
		if kind:get() == "chat" and not split then
			lanes.watch(slot, chat, data.frozen == true, data.parent)
		end
	end)
	-- The transcript as the chat page has it (screens/chat/session), its
	-- composer docked over its foot the same way (screens/chat/dock): the
	-- send's runway and glide, the working row and the jump-to-latest pill
	-- held clear of the composer, its place kept across a switch of tabs
	-- and a reload (its anchor and offset kept by the tab), pictures read
	-- from its own folder, links, quotes into its own draft.
	local offset = ui.state(if split then `chat_split_offset_{chat}` else `chat_lane_offset_{slot}`, T.PAST_END)
	local anchor = ui.state(if split then `chat_split_anchor_{chat}` else `chat_lane_anchor_{slot}`, nil :: any)
	-- (A split's pane. Its transcript is its own chat's, from its own
	-- lane, focused or not, and made once for as long as it's in the
	-- split: the focus moving changes nothing it shows. Only what stands
	-- at its foot follows the focus: the window's composer over the
	-- focused one, its own composer over another. It's made once there's
	-- something to show, opening where the chat was last read
	-- (`shared.places`).)
	local function focused_now(): boolean
		local f = data.focused
		return f ~= nil and f() == true
	end
	local loaded = ui.computed(function(): boolean
		local s = view.session:get()
		return s ~= nil and s.id == chat and s.state ~= "loading"
	end)
	local messages: any = view.messages
	local reply: any = view.draft
	-- Where the chat was left: as the chat page last showed it (its place
	-- now, if that's this chat), else its place kept for it.
	local function place_of(): any
		if Shared.shown:peek() == chat then
			return Shared.anchor:peek()
		end
		return Shared.places:peek()[chat]
	end
	-- Where it's read here, kept the same way, once it has settled: the
	-- chat page, a pane made again (the split shown again, the chat opened
	-- alone) open there.
	local settled = false
	if split then
		ui.effect(function()
			local a = anchor:get()
			if not settled then
				return
			end
			local now = Shared.places:peek()
			if not same(now[chat], a) then
				local c = table.clone(now)
				c[chat] = a
				Shared.places:set(c)
			end
			if Shared.shown:peek() == chat and not same(Shared.anchor:peek(), a) then
				Shared.anchor:set(a)
			end
		end)
	end
	local ever = false
	local showable = ui.computed(function(): boolean
		if ever then
			return true
		end
		ever = messages:len() > 0 or loaded:get()
		return ever
	end)
	local dock_h = M.dock_height(slot)
	-- (A split's pane, focused: the window's composer stands at its foot.)
	local room = Dock.room(function(): number
		return if split and focused_now() then Shared.dock_h:get() else dock_h:get()
	end)
	local function lane_session(): any
		local s = view.session:get()
		return if s and s.state ~= "missing" then s else nil
	end
	local pictures = ReplyImages.for_session(lane_session)
	local sent_pictures = ReplyImages.for_attachments(function(): any
		local s = lane_session()
		if not s then
			return nil
		end
		return { id = s.id, device = s.device, cwd = s.cwd, here = sessions.here:get() }
	end)
	-- The session whose panel it's in (the page's): where a file it links
	-- opens, and a subagent it spawned.
	local function home_session(): string?
		-- (A split's pane: its own chat's panel, which shows as it's
		-- focused.)
		if split then
			return chat
		end
		local s = route.session:peek()
		return if s then s.id else nil
	end
	local function open_link(target: string)
		local other = string.match(target, "^zeron%-chat:([%w%-_]+)$")
		if other then
			route.open_session(other)
			return
		end
		local s = view.session:peek()
		local r = FileLinks.resolve(target, if s then s.cwd else nil)
		local panel: any = boundary.load("@app/screens/panel")
		local home = home_session()
		-- (A split's pane: focused, so its panel, where the file opens, shows.)
		if split and r and not r.outside then
			route.open_session(chat)
		end
		if r and not r.outside and panel and panel.open_file and home then
			(panel.open_file :: any)(home, r.path, route.project:peek(), { line = r.line, column = r.column, end_line = r.end_line })
		end
	end
	-- A passage quoted (its menu's Mention, or Attach): into its own draft.
	local draft = ui.state(`chat_lane_draft_{chat}`, "")
	local quotes = ui.state(`chat_lane_quotes_{chat}`, {} :: { [string]: string })
	local function quote(q: any)
		-- (A split's pane: into its chat's own draft, which the window's
		-- composer types into once it's focused.)
		if split then
			route.open_session(chat)
			ui.after(0, function()
				Shared.mention(q)
			end)
			return
		end
		local line = Quote.line(q)
		local now = table.clone(quotes:peek())
		now[line] = q.text
		ui.batch(function()
			quotes:set(now)
			draft:set(Quote.add(draft:peek(), line))
		end)
		focus:add(1)
	end
	-- Or attached (its bar's Attach): a chip, as the chat page's.
	local function attach_quote(q: any)
		if split then
			route.open_session(chat)
			ui.after(0, function()
				Shared.attach_quote(q)
			end)
			return
		end
		Shared.attach_quote(q, chat)
		focus:add(1)
	end
	local function make_transcript(): Node
		return T.Transcript {
		-- (Its own glide, runway and place: never the chat page's.)
		name = `lane.{slot}`,
		-- (A split's pane reads as the chat page: a little wider than its
		-- composer, running under its head and past its foot, faded.)
		measure = function(): number
			return prefs.conversation_width:get() + (if split then 24 else 0)
		end,
		side = if split then 20 else nil,
		fade_lead = if split then 12 else nil,
		messages = messages,
		draft = reply,
		offset = offset,
		keep = anchor,
		foot = if tab.kind == "subagent" then nil else room.foot,
		-- Its "Jump to latest" and the pill of the composer at its foot, a
		-- centred pair: its own composer's, or, a split's focused pane, the
		-- window's (screens/chat/shared's `pill_width_of`).
		beside = if tab.kind == "subagent"
			then nil
			else function(): number?
				return Shared.pill_width_of("waiting", if split and focused_now() then "window" else `lane.{slot}`):get()
			end,
		on_jump_width = Shared.pill_teller("jump", `lane.{slot}`),
		compact = function(): boolean
			return prefs.compact_transcript:get()
		end,
		on_mention = quote,
		on_attach = attach_quote,
		find = if split then nil else find.marks(),
		-- (A subagent's liveness is its own doc's: no session of its own.)
		turn_state = if tab.kind == "subagent" then nil else TurnGlue.of(function(): any
			return view.session:get()
		end),
		on_retry = function(message: string)
			store.retry(chat, message)
		end,
		-- (A split's pane: the keys stay with this chat, in the window's composer.)
		on_jump = if split
			then function()
				ui.after(0, function()
					Shared.focus:add(1)
				end)
			end
			else nil,
		pictures = pictures,
		sent_pictures = sent_pictures,
		on_open_subagent = function(sub: any)
			local home = home_session()
			if home then
				lanes.open_subagent(home, sub)
			end
		end,
		open_link = open_link,
		chat_of = Shared.chat_of,
		-- (The chat this lane is beside is already on show: to it, as ever.)
		open_sender = function(chat: string, title: string)
			local home = home_session()
			if chat == home then
				open_link(`zeron-chat:{chat}`)
			elseif home then
				lanes.open(home, { kind = "side_chat", id = chat, title = title })
			end
		end,
		link_menu = LinkMenu(open_link, function(): any
			return view.session:peek()
		end),
		-- (A reply's link to one of its files or its agent's skills: a chip.)
		link_chip = LinkChips(function(): any
			return view.session:peek()
		end),
		-- (A split's pane, made again at the place taken up: whether
		-- that was its end.)
		was_at_end = if split then anchor:peek() == nil else nil,
	}
	end
	local transcript = if split
		then ui.presence(function(): string
			return if showable:get() then "shown" else "waiting"
		end, function(k: string): Node
			if k == "waiting" then
				return ui.div {}
			end
			ui.batch(function()
				offset:set(T.PAST_END)
				anchor:set(place_of())
			end)
			settled = false
			-- (Its place put back over its first frames: not said again
			-- until it has settled there.)
			ui.after(400, function()
				settled = true
			end)
			return make_transcript()
		end, { w_full = true, grow = 1, min_h = 0 })
		else make_transcript()
	if tab.kind == "subagent" then
		return ui.column { id = `lane-{slot}`, class = "lane-tab subagent-tab", provide = { session = chat }, grow = 1, w_full = true, min_h = 0, relative = true, transcript, find.Bar() }
	end
	return ui.column {
		id = `lane-{slot}`,
		class = "lane-tab side-chat-tab",
		provide = { session = chat },
		grow = 1,
		w_full = true,
		min_h = 0,
		-- (A real swap: the fork's word, then its chat.)
		ui.presence(function(): string
			return if kind:get() == "fork" then "fork" else "chat"
		end, function(k: string): Node
			if k == "fork" then
				return forking(chat)
			end
			-- As the chat page: the transcript to the composer's footer line,
			-- running on behind the composer, which floats at the foot.
			local body: { [any]: any } = {
				grow = 1,
				w_full = true,
				min_h = 0,
				relative = true,
				ui.column {
					w_full = true,
					h_full = true,
					min_h = 0,
					pb = Dock.page_pad(),
					ui.column { w_full = true, grow = 1, min_h = 0, relative = true, transcript, if split then nil else find.Bar() },
				},
				-- (A split's pane, focused: the window's composer stands over
				-- this one, which is kept, unseen and out of Tab's way, rather
				-- than made again as the focus leaves, which would grow it
				-- from nothing each time.)
				ui.div {
					absolute = true,
					left = 0,
					right = 0,
					top = 0,
					bottom = 0,
					pointer_events = "none",
					opacity = function(): number
						return if split and focused_now() then 0 else 1
					end,
					hidden = function(): boolean
						return split and focused_now()
					end,
					Dock.Dock { height = dock_h, composer = composer(chat, view, `lane.{slot}`, focus, open_model, data.split == true, if split then focused_now else nil), id = `lane-composer-{slot}` },
				},
			}
			if zone then
				for k, v in zone do
					body[k] = v
				end
				table.insert(body, DropOverlay { shown = carrying, id = `lane-drop-{slot}` })
				table.insert(body, DropOverlay { shown = carrying_session, id = `lane-drop-mention-{slot}`, label = ui.t("chat.composer.drop_to_mention") :: any })
				-- A bare Escape that reached it (nothing inside took it): the
				-- setting's "Escape stops the agent", as the chat page's.
				body.on_key = function(e: any): boolean
					if e.key == "Escape" and not e.shift and not e.cmd and not e.alt then
						local s = view.session:peek()
						return store.escape_chat(chat, s ~= nil and s.running == true)
					end
					return false
				end
			end
			return ui.column(body :: any)
		end, { grow = 1, w_full = true, min_h = 0 }),
	}
end)

return M
