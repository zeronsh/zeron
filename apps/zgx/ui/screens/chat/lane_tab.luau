--!strict
-- A lane in a chat's panel (screens/panel): a side chat, or a subagent, as
-- the shipping app's right-pane tabs show them (RightSurface::SideChat,
-- ::Subagent). The host watches its chat beside the open one, in a slot of
-- its own (the tab's id: `chat.open_lane`, let go as the tab goes). A side
-- chat is its transcript and a composer of its own (its draft its own, not
-- the chat page's); a subagent's is its transcript alone, to read.
--
--   LaneTab { tab = { id, kind = "side_chat" | "subagent" | "read", chat, title } }
--
-- `read`: a live chat's transcript alone, to read (the Control room's Chat
-- tab: the Brain dump's), its working row its own, `data.foot` the room
-- kept clear at its foot (what floats over it there), `data.live` whether
-- it shows (kept built while hidden: the reply being written held).

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
local Draft = require("@ds/draft_text")
local ComposerAtoms = require("@ds/composer_atoms")
-- (The chat page's places in its chats, which a split's panes share.)
local Shared = require("@app/screens/chat/shared")
local Asks = require("@app/screens/chat/asks")
local CardHold = require("@app/screens/chat/card_hold")
local Carried = require("@app/screens/chat/carried")
local Completions = require("@app/screens/chat/completions")
local Find = require("@app/screens/chat/find")
local Invocations = require("@app/screens/chat/invocations")
local ZeronCommands = require("@app/screens/chat/zeron_commands")
local look = require("@app/screens/shell/project_look")
local Appshots = require("@app/screens/chat/appshots")
local DropOverlay = require("@ds/drop_overlay")
-- Which of Zeron's built-in plugins run (Orchestrators' hooks here).
local plugins = require("@account/plugins")

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

-- A `Composer`'s model picker, asked for from elsewhere (the sidebar's
-- "Brain dump model…", on the way to the Control room): opened by that
-- composer, at once if it's there, else as it comes.
local model_wanted = ui.state("chat_lane_model_wanted", nil :: string?)
-- (How soon after opening its picker a composer let go hands it on, ms.)
local HAND_ON = 1000
-- (When it was asked: a request no composer took soon is let go, never
-- opening a picker later out of nowhere.)
local wanted_at = -math.huge
local WANT_FOR = 3000
function M.open_model_when(slot: string)
	wanted_at = ui.now()
	model_wanted:set(slot)
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
--
-- `take`: what a send does instead of messaging the chat (the Control
-- room's Capture: a dump): told the words as they'd be sent and what they
-- carry (Carried.pack's `attachments`, then its references as
-- `{ kind = "ref", ref }`), it returns a promise; the draft
-- clears as it goes and comes back, with its chips, if it fails. It's
-- told the pickers' picks too (`picks()`: model, its agent, effort…), and a
-- pick made there is the chat's at once (`store.configure`), as on the chat
-- page.
-- `placeholder`: the words in it while it's empty. `footer`: which of its
-- footer's pickers it shows (each by default): `access = false` leaves out
-- the access mode, `place = false` where it works (project, device,
-- checkout, branch); the model and agent always show.
export type Take = (text: string, attachments: { any }, picks: { [string]: any }) -> any
export type Footer = { access: boolean?, place: boolean? }
-- A second placement of a page's composer (the Control room's floating
-- one): the same draft, chips and picks as the first, shown another way.
-- `mirror`: the first one keeps the chat's picks and edits, this one only
-- shows them. `on_escape`: Escape's, first.
-- `on_focus`: told as its field takes the keys and lets them go (either
-- placement's, so the keys can follow it from one to the other).
-- `keeps`: a mirror with no first placement (the Control room's Capture,
-- a bar now): it keeps the chat's picks itself. `over_art`: its footer on
-- the pill's glass (ds.Composer's), where it floats over a page.
export type Form = { mirror: boolean?, keeps: boolean?, on_escape: (() -> boolean)?, on_focus: ((focused: boolean) -> ())?, over_art: (() -> boolean)? }
local function composer(chat: string, view: any, name: string, focus: State<number>, open_model: State<number>, split: boolean?, focused: (() -> boolean)?, own: boolean?, take: Take?, placeholder: string?, footer: Footer?, form: Form?, own_draft: State<string>?, own_atoms: State<{ ComposerAtoms.AtomEntry }>?, dock_target: State<number>?): Node
	local mirror = form ~= nil and form.mirror == true
	-- (Its draft kept by the chat, as its picks: a switch of tabs or a
	-- reload finds it as it was; a passage quoted from its transcript lands
	-- in it.)
	-- (A pane of the page's split that isn't focused shows the chat
	-- page's own draft for its chat, as that keeps it while another chat
	-- is shown (screens/chat/shared's drafts), so focused, the chat page's
	-- composer goes on with the same words.)
	local draft = if not split and own_draft then own_draft else ui.state(if split then `chat_split_draft_{chat}` else `chat_lane_draft_{chat}`, "")
	local atoms = if not split and own_atoms then own_atoms else ui.state(if split then `chat_split_atoms_{chat}` else `chat_lane_atoms_{chat}`, {} :: { ComposerAtoms.AtomEntry })
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
		if not split then
			local next_text, next_atoms, changed = ComposerAtoms.remove_by_cargo(draft:peek(), ComposerAtoms.live(atoms), cid)
			if changed then
				ComposerAtoms.apply(draft, atoms, next_text, next_atoms)
			end
		end
	end
	-- The options picked in a question its chat asks. (A split's pane: its
	-- chat's draft's, below. Else kept by the chat, as its draft is.)
	local chosen = if split then ui.use_state({} :: { [string]: boolean }) else ui.state(`chat_lane_chosen_{chat}`, {} :: { [string]: boolean })
	if split then
		-- (Its chat's draft, by its id: its words, chips and a question's
		-- picks, one draft per chat (screens/chat/shared), shown here and
		-- written back as they change, wherever it's kept. Never the focused
		-- chat's: the focus moving changes nothing this shows.)
		--
		-- Each side's write is told to the other by its length (`pulled`,
		-- `pushed`), so its echo isn't read back: both read the whole draft
		-- and compared it at every keystroke, both ways (3 ms a key at
		-- 30,000 lines). By length, so a write that changed nothing, and so
		-- came back as no run, never hides the next change. `known`: this
		-- pane's draft's length as last seen, so the mirror from the chat's
		-- draft reads this one's only where the lengths agree.
		local pulled, pushed, known = nil :: number?, nil :: number?, -1
		ui.effect(function()
			Shared.track_draft_of(chat)
			local echo = pushed
			pushed = nil
			local size = Shared.draft_size_of(chat)
			if echo ~= nil and size == echo then
				return
			end
			local d = Shared.draft_of(chat)
			if size ~= known or draft:peek() ~= d.text or #ComposerAtoms.live(atoms) ~= #d.atoms then
				pulled, known = size, size
				ComposerAtoms.apply(draft, atoms, d.text, d.atoms)
			end
			chosen:set(d.chosen)
		end)
		local first = true
		ui.effect(function()
			local size = Draft.size(draft)
			known = size
			local echo = pulled
			pulled = nil
			if echo ~= nil and size == echo then
				return
			end
			-- (Typed: it differs. Made: it may not.)
			local text = draft:peek()
			if first then
				first = false
				if Shared.draft_of(chat).text == text then
					return
				end
			end
			pushed = size
			Shared.set_draft_of(chat, "text", text)
		end)
		ui.effect(function()
			local picked = chosen:get()
			if Shared.chosen_of(chat) ~= picked then
				Shared.set_draft_of(chat, "chosen", picked)
			end
		end)
	end
	local notice = ui.use_state(nil :: any)
	-- A paused card's thread (screens/chat/card_hold): its queue held.
	local card_held, resume_card = CardHold.bind(function(): string?
		return chat
	end, function()
		notice:set({ tone = "danger", text = locale.s("chat.composer.didnt_resume"), dismissable = true })
	end)
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
	local editing = ui.state(`chat_lane_editing_{chat}`, nil :: { item: string, aside: string, aside_cargo: { any }?, aside_atoms: { any }? }?)
	local edits = Asks.queue_edits {
		id = function(): string?
			return chat
		end,
		text = draft,
		atoms = atoms,
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
	if not split and not mirror then
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
						ComposerAtoms.apply(draft, atoms, e.aside, e.aside_atoms or {})
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
		if s and s.harness and s.model then
			return s.harness, s.model, s.effort, s.access
		end
		-- (Its session not saying its model yet, as before its chat's row
		-- comes: the row's, the chat's own config, never a new chat's
		-- default.)
		local own: any = nil
		local now = sessions.now:get()
		for _, list in { now.chats or {}, now.archived or {} } do
			for _, c in list do
				if c.id == chat then
					own = c
				end
			end
		end
		for _, kids in now.children or {} do
			for _, c in kids do
				if c.id == chat then
					own = c
				end
			end
		end
		if own and own.model and own.harness then
			local m = own.model
			if not string.find(m, "/", 1, true) then
				m = `{own.harness}/{m}`
			end
			return own.harness, m, if s then s.effort else nil, if s then s.access else nil
		end
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
		if not out.harness and type(m) == "string" then
			out.harness = string.match(m, "^([^/]+)/")
		end
		return out
	end
	-- A page's own composer (`take`): a pick is the chat's at once, as on
	-- the chat page (screens/chat/composer), never only with the next send.
	-- (What the chat runs with already, or was seeded from it, isn't a pick.)
	if take and (not mirror or (form ~= nil and form.keeps == true)) then
		ui.effect(function()
			local m, e, f, o = model:get(), effort:get(), fast:get(), options:get()
			local s = session:get()
			if not s or s.state ~= "ready" then
				return
			end
			local was = seeded:peek()
			local patch: { [string]: any } = {}
			local changed = false
			if m and m ~= was.model and m ~= s.model then
				patch.model, changed = m, true
				for _, c in store.catalog:peek().models do
					if c.value == m then
						patch.harness = c.harness
					end
				end
			end
			if e and e ~= was.effort and e ~= s.effort then
				patch.effort, changed = e, true
			end
			-- (Off and none are one, as an empty set of options and none.)
			if (f == true) ~= (was.fast == true) and (f == true) ~= (s.fast == true) then
				patch.fast, changed = f == true, true
			end
			local function some(v: any): any
				return if type(v) == "table" and next(v) == nil then nil else v
			end
			if not same(some(o), some(was.options)) and not same(some(o), some(s.options)) then
				patch.options, changed = o, true
			end
			if changed then
				store.configure(chat, patch)
			end
		end)
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
				return Completions.agent_of(model:get(), if unsent() then nil else session:get())
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
	local p: { [string]: any } = {
		dictation = dictation,
		on_sizes = if dock_target
			then function(sizes: ds.ComposerSizes)
				if math.abs(dock_target:peek() - sizes.target) > 0.01 then
					dock_target:set(sizes.target)
				end
			end
			else nil,
		value = draft,
		atoms = atoms,
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
		cargo_chip = Carried.chip_spec,
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
						Shared.attach_pasted_image(path, chat)
						focus:add(1)
					end
				end, function() end)
				return true
			end,
		focus = if split then nil else focus,
		-- (ds.Composer's placeholder is a string: read as the tab is made.)
		-- (A split's pane is a chat of its own, as is a lane that shows one
		-- (`data.own`: an orchestrator's page): the chat page's words.)
		placeholder = placeholder or locale.s(if split or own then "chat.composer.placeholder" else "chat.lane_tab.placeholder"),
		running = running,
		access = access,
		access_auto = function(): boolean
			return store.offers_auto(model:get())
		end,
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
			if form and form.on_escape and form.on_escape() then
				return true
			end
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
			local link = machine.cloud_link(Draft.has_text(draft))
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
		account_group = accounts.account_group,
		on_account = accounts.on_account,
		on_accounts_open = accounts.on_accounts_open,
		-- The picker opened: the host looks again at agents short of
		-- models (none, a failed look), in the background; one's
		-- "Retry" asks for its models alone, its "Sign in" opens
		-- Settings → Providers.
		on_models_open = function()
			store.refresh_catalog()
		end,
		on_models_retry = function(harness: string)
			store.retry_catalog(harness)
		end,
		on_models_sign_in = accounts.on_sign_in,
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
				ui.batch(function()
					draft:set("")
					atoms:set({})
				end)
				run_command(command)
				return
			end
			-- (Encode inline chips first so atom offsets match the raw draft,
			-- then expand any quoted passages.)
			local said_quotes = quotes:peek()
			local said_atoms = ComposerAtoms.live(atoms)
			local encoded = ComposerAtoms.encode(text, said_atoms)
			local words = string.match(Quote.expand(encoded, Quote.kept(encoded, said_quotes)), "^%s*(.-)%s*$") or ""
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
				atoms:set({})
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
			-- Taken by its page (a dump): the words and what they carry, never
			-- a message of the chat's.
			if take then
				-- (Its references go too, each `{ kind = "ref", ref }` after
				-- the files: the page splits them off, ds/orch_ref.)
				local carry = table.clone(pack.attachments)
				for _, r in pack.refs do
					table.insert(carry, { kind = "ref", ref = r })
				end
				take(prompt, carry, picks()):after(function() end, function(err: any)
					ui.batch(function()
						ComposerAtoms.apply(draft, atoms, text, said_atoms)
						quotes:set(said_quotes)
					end)
					Shared.restore_cargo_of(chat, chips)
					local why = if type(err) == "table" and err.message then tostring(err.message) else nil
					notice:set({ tone = "danger", text = why or locale.s("chat.lane_tab.didnt_send"), dismissable = true })
				end)
				return
			end
			-- (While a turn runs it's queued instead: it shows in the queue,
			-- carrying what's picked so it runs on that agent/model.)
			local queued = not is_unsent and running:peek()
			local req: { [string]: any } = picks()
			req.attachments = pack.attachments
			req.appshots = Appshots.of_cargo(chips)
			local function failed(err: any)
				ui.batch(function()
					ComposerAtoms.apply(draft, atoms, text, said_atoms)
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
			local catalog = store.catalog:peek()
			local from_h = T.session_harness(catalog, session:peek())
			local to_h = req.harness or (if type(req.model) == "string" then string.match(req.model, "^([^/]+)/") else nil)
			local sw = if not is_unsent then T.switch_row(catalog, from_h, to_h, mid) else nil
			local sent: any = nil
			T.send(function()
				local here = view.messages:index_of("id", mid) ~= nil
				if not here then
					if sw and view.messages:len() > 0 then
						local tail = view.messages:at(view.messages:len())
						if (not tail or tail.role ~= "switch") and view.messages:index_of("id", sw.id) == nil then
							view.messages:push(sw)
						end
					end
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
		held = card_held,
		on_held_action = resume_card,
		-- (Steer only where its agent takes a word mid-turn: else the row's
		-- Send now stops the turn, as the host does.)
		can_steer = function(): boolean
			local s = session:get()
			return running:get() and s ~= nil and s.steers == true
		end,
		turn_running = function(): boolean
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
	-- (A footer without some of its pickers: a page's own composer, as the
	-- Control room's Capture keeps only the model.)
	if footer and footer.access == false then
		p.access = nil
	end
	if footer and footer.place == false then
		for _, k in { "project", "projects", "device", "devices", "checkout", "branch", "branches", "branches_empty", "branch_fixed", "on_new_project" } do
			p[k] = nil
		end
	end
	if form and form.on_focus then
		p.on_field_focus = form.on_focus
	end
	if form and form.over_art then
		p.over_art = form.over_art
	end
	return ds.Composer(p :: any)
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

-- Each page composer's focus, by its slot (`LaneTab.Composer`, its
-- `DropZone`): bumped, it takes the keys.
local focuses: { [string]: State<number> } = {}
local function focus_of(slot: string): State<number>
	local f = focuses[slot]
	if not f then
		f = ui.state(`lane_focus_{slot}`, 0)
		focuses[slot] = f
	end
	return f
end
-- A page composer's draft for `chat`, the words (a side chat's too), as
-- every placement of it shows them.
function M.draft(chat: string): State<string>
	return ui.state(`chat_lane_draft_{chat}`, "")
end

-- Gives `slot`'s page composer the keys (its first placement's).
function M.focus(slot: string)
	M.wake(slot)
	focus_of(slot):add(1)
end

-- A `lazy` page composer's slot whose chat is watched: only once it's
-- used (the pointer over it, the keys in it, its model asked for). Till
-- then its chat's transcript isn't taken in at all (the Control room's
-- Capture never shows it: the Brain dump's, hundreds of messages).
local woken = ui.state("chat_lane_woken", {} :: { [string]: boolean })
function M.wake(slot: string)
	if not woken:peek()[slot] then
		woken:set_key({ slot }, true)
	end
end
function M.woken(slot: string): boolean
	return woken:peek()[slot] == true
end

-- A chat's composer on a page of its own, outside the panel's tabs: the
-- side chat's (above), bound to chat `chat` in `slot` (its session watched
-- while it shows, so its model, agent and access are the chat's and change
-- it), its sends taken by `take` where given (the Control room's Capture:
-- each a dump). Laid in the page's flow, as wide as what holds it.
--
--   LaneTab.Composer { chat = function() return id end, slot = "capture",
--     take = function(text, attachments) return promise end,
--     placeholder = "Dump anything" }
--
-- A second placement of the same composer (`mirror = true`, the same
-- `slot`): one composer shown in two places, its draft, chips and picks
-- the first one's. Its focus is its own (`focus`); `on_escape` has Escape
-- first. While a mirror shows, the model picker asked for from
-- elsewhere opens in it rather than in the first (likely out of view).
-- The slot's session is watched while either is there.
local holders: { [string]: number } = {}
local mirrors: { [string]: number } = {}
-- Each slot's mirrors' `shown` (one kept built while out of sight).
local mirror_shown: { [string]: { () -> boolean } } = {}
local function any_mirror_shown(slot: string): boolean
	for _, shown in mirror_shown[slot] or {} do
		if shown() then
			return true
		end
	end
	return false
end
local live_pickers: { [string]: { State<number> } } = {}
M.Composer = ui.component(function(props: {
	chat: () -> string?,
	slot: string,
	take: Take?,
	placeholder: string?,
	footer: Footer?,
	drop: boolean?,
	id: string?,
	mirror: boolean?,
	keeps: boolean?,
	focus: State<number>?,
	on_escape: (() -> boolean)?,
	on_focus: ((focused: boolean) -> ())?,
	over_art: (() -> boolean)?,
	lazy: boolean?,
	-- Its chat's row and send path alone, no transcript watched (Capture's:
	-- it never shows one). Every placement of a slot passes the same.
	session_only: boolean?,
	-- A mirror kept built while out of sight (the Control room's floating
	-- composer, hidden on Overview with Capture in view): whether it shows.
	-- Only one that shows takes the model picker from the first.
	shown: (() -> boolean)?,
}): Node
	local slot = props.slot
	local mirror = props.mirror == true
	local lazy = props.lazy == true
	local mirror_showing = props.shown or function(): boolean
		return true
	end
	local view = lanes.view(slot)
	-- Lazy, till it's used: its session as the chats list has it (its
	-- agent and model, at rest), so its picker shows the chat's model and
	-- a pick is the chat's at once; the host's whole session (its effort,
	-- its queue) once it's woken.
	if lazy then
		local host_view = view
		local at_rest = ui.computed(function(): any
			local id = props.chat()
			if not id then
				return nil
			end
			local row: any = nil
			local now = sessions.now:get()
			for _, list in { now.chats or {}, now.archived or {} } do
				for _, c in list do
					if c.id == id then
						row = c
					end
				end
			end
			if not row then
				return nil
			end
			local model: string? = row.model
			local harness: string? = row.harness
			if model and not string.find(model, "/", 1, true) then
				if harness then
					model = `{harness}/{model}`
				else
					-- (A row naming its model alone: the catalog's entry for it.)
					for _, m in store.catalog:get().models or {} do
						if string.sub(m.value, -(#model + 1)) == `/{model}` then
							model, harness = m.value, m.harness
							break
						end
					end
				end
			end
			return { id = id, state = "ready", device = row.device, harness = harness, model = model, running = false }
		end)
		view = {
			session = ui.computed(function(): any
				if woken:get()[slot] then
					return host_view.session:get()
				end
				return at_rest:get()
			end),
			messages = host_view.messages,
			draft = host_view.draft,
		}
	end
	-- (Lazy: its chat watched once it's used, `M.wake`.)
	local on_focus = props.on_focus
	if lazy then
		local told = props.on_focus
		on_focus = function(focused: boolean)
			if focused then
				M.wake(slot)
			end
			if told then
				told(focused)
			end
		end
	end
	-- (Its focus, by its slot: a drop zone of its, around it or anywhere on
	-- its page, gives it the keys as something lands. A mirror's, its own.)
	local focus = props.focus or focus_of(slot)
	local open_model = ui.use_state(0)
	holders[slot] = (holders[slot] or 0) + 1
	if mirror then
		mirrors[slot] = (mirrors[slot] or 0) + 1
		local list = mirror_shown[slot] or {}
		table.insert(list, mirror_showing)
		mirror_shown[slot] = list
	end
	ui.effect(function()
		local id = props.chat()
		if lazy and not woken:get()[slot] then
			return
		end
		lanes.watch(slot, id, nil, nil, props.session_only)
	end)
	-- (Its picker, for `open_model`; one asked for before it came, opened
	-- once its chat is known. The first placement's: a mirror's picker is
	-- only its own.)
	if not mirror then
		local list = live_pickers[slot] or {}
		table.insert(list, open_model)
		live_pickers[slot] = list
		pickers[slot] = open_model
	end
	-- (Taken a frame on, once its composer and its picker are made, and
	-- only by one still there. A page coming may make it again as it lays
	-- itself out: one let go just after it opened its picker hands the
	-- request on to the next.)
	local alive = true
	local opened_at = -math.huge
	ui.effect(function()
		if model_wanted:get() == slot and props.chat() then
			M.wake(slot)
			ui.after(0, function()
				ui.after(0, function()
					-- (A mirror showing takes it: it's the one in view.)
					local mine = if mirror then mirror_showing() else not any_mirror_shown(slot)
					if alive and mine and model_wanted:peek() == slot then
						model_wanted:set(nil)
						if ui.now() - wanted_at < WANT_FOR then
							opened_at = ui.now()
							open_model:add(1)
						end
					end
				end)
			end)
		end
	end)
	ui.on_cleanup(function()
		alive = false
		if ui.now() - opened_at < HAND_ON then
			model_wanted:set(slot)
		end
		-- (The slot's session let go with the last placement of it, never
		-- under another still showing.)
		local left = (holders[slot] or 1) - 1
		holders[slot] = if left > 0 then left else nil
		if mirror then
			local m = (mirrors[slot] or 1) - 1
			mirrors[slot] = if m > 0 then m else nil
			local list = mirror_shown[slot] or {}
			local at = table.find(list, mirror_showing)
			if at then
				table.remove(list, at)
			end
			mirror_shown[slot] = if #list > 0 then list else nil
		end
		if left <= 0 then
			-- (A lazy one never used watched nothing.)
			if not lazy or woken:peek()[slot] then
				lanes.watch(slot, nil)
			end
			if woken:peek()[slot] then
				woken:set_key({ slot }, nil)
			end
		end
		if not mirror then
			local list = live_pickers[slot] or {}
			local at = table.find(list, open_model)
			if at then
				table.remove(list, at)
			end
			live_pickers[slot] = if #list > 0 then list else nil
			if pickers[slot] == open_model then
				pickers[slot] = list[#list]
			end
		end
	end)
	local function build(id: string, form: Form?): Node
		local c = composer(id, view, `lane.{slot}`, focus, open_model, false, nil, true, props.take, props.placeholder, props.footer, form)
		return if props.drop then M.DropZone { slot = slot, chat = props.chat, focus = props.focus, place = if mirror then "mirror" else nil, c } else c
	end
	-- (A real swap: the chat it's bound to, once known.)
	return ui.presence(function(): string
		return props.chat() or ""
	end, function(key: string): Node
		if key == "" then
			return ui.div {}
		end
		local id = key
		local form: Form? = nil
		if mirror or on_focus then
			local f: Form = { mirror = mirror, keeps = props.keeps, on_escape = props.on_escape, on_focus = on_focus, over_art = props.over_art }
			form = f
		end
		-- (Its `id` is the component's node's: zgx gives it there.)
		-- (Lazy: the pointer coming over it wakes its chat, so its picks are
		-- the chat's by the time one is pressed.)
		return ui.column {
			w_full = true,
			on_hover = if lazy then function(inside: boolean)
				if inside then
					M.wake(slot)
				end
			end else nil,
			build(id :: string, form),
		}
	end, { w_full = true })
end)

-- A drop zone for `LaneTab.Composer`'s draft (bound to `chat`, in
-- `slot`), round whatever it's given: a chat dragged from the sidebar
-- lands as an @-mention of it, files (and the files pane's) as chips to
-- send with it, the chat page's own overlays saying so while they're over
-- it; its composer takes the keys as one lands. A page puts it round as
-- much as it likes (the Control room: all of Capture); `LaneTab.Composer`'s
-- `drop = true`, round the composer alone. Round a second placement of
-- the composer (a mirror's): `focus`, that one's, and `place`, a name of
-- its own for the zone (its overlays its own, the draft the same). `refs`:
-- it takes the Control room's references too (ds/orch_ref, dragged from
-- its widgets: a chip each, sent with the dump), "Drop to reference" while
-- one is over it.
--
--   LaneTab.DropZone { slot = "control-capture", chat = function() return id end, refs = true, node }
M.DropZone = ui.component(function(props: { slot: string, chat: () -> string?, focus: State<number>?, place: string?, refs: boolean?, [number]: Node }): Node
	local slot = props.slot
	local focus = props.focus or focus_of(slot)
	local suffix = if props.place then `-{props.place}` else ""
	local body = ui.presence(function(): string
		return props.chat() or ""
	end, function(id: string): Node
		local zone_name = if props.place then `lane.{slot}.{props.place}` else `lane.{slot}`
		local node: { [any]: any } = { w_full = true, relative = true }
		for _, child in ipairs(props :: any) do
			table.insert(node, child)
		end
		if id == "" then
			return ui.column(node :: any)
		end
		local zone, carrying, carrying_session, carrying_ref = Shared.own_drop_zone(zone_name, id, function()
			focus:add(1)
		end, nil, props.refs)
		for k, v in zone do
			node[k] = v
		end
		table.insert(node, DropOverlay { shown = carrying, id = `lane-drop-{slot}{suffix}` })
		table.insert(node, DropOverlay { shown = carrying_session, id = `lane-drop-mention-{slot}{suffix}`, label = ui.t("chat.composer.drop_to_mention") :: any })
		if props.refs then
			table.insert(node, DropOverlay { shown = carrying_ref, id = `lane-drop-ref-{slot}{suffix}`, label = ui.t("chat.composer.drop_to_reference") :: any, tone = "accent" })
		end
		return ui.column(node :: any)
	end, { w_full = true })
	return body
end)

M.Tab = ui.component(function(props: { tab: any }): Node
	local tab = props.tab
	local slot, chat = tab.id, tab.chat
	local data = tab.data or {}
	-- (Its transcript alone, no composer of its own.)
	local reading = tab.kind == "subagent" or tab.kind == "read"
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
	-- (Or its caller's, `data.focus`: a page that hands it the keys, an
	-- orchestrator's just made.)
	local focus = data.focus or ui.use_state(0)
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
	local lane_draft = ui.state(`chat_lane_draft_{chat}`, "")
	local lane_atoms = ui.state(`chat_lane_atoms_{chat}`, {} :: { ComposerAtoms.AtomEntry })
	local zone, carrying, carrying_session = nil :: any, nil :: any, nil :: any
	if tab.kind == "side_chat" and data.split ~= true then
		zone, carrying, carrying_session = Shared.own_drop_zone(`lane.{slot}`, chat, function()
			focus:add(1)
		end, { text = lane_draft, atoms = lane_atoms })
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
	-- (A `read` tab kept built while hidden, `data.live`: the reply being
	-- written held meanwhile, ds/hold; its rows not written to.)
	-- (A `read` tab is the Control room's: Orchestrators', its `hold`.)
	local orch = if tab.kind == "read" and data.live then plugins.module("orchestrators") else nil
	if orch then
		reply = orch.hold.computed(data.live, function(): any
			return view.draft:get()
		end, "chat")
	end
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
	local dock_target = ui.state(`chat_lane_dock_target_{slot}`, Dock.REST)
	-- (A split's pane, focused: the window's composer stands at its foot.)
	local room = Dock.room(function(): number
		return if split and focused_now() then Shared.dock_h:get() else dock_h:get()
	end, function(): number
		return if split and focused_now() then Shared.dock_target:get() else dock_target:get()
	end)
	local function lane_session(): any
		local s = view.session:get()
		return if s and s.state ~= "missing" then s else nil
	end
	local pictures = ReplyImages.for_session(lane_session)
	local sent_pictures = ReplyImages.for_attachments(function(): any
		local s = lane_session()
		if s then
			return { id = s.id, device = s.device, cwd = s.cwd, here = sessions.here:get() }
		end
		-- A subagent's lane has no chat of its own (its doc is its parent's
		-- child): the pictures its tools kept are files on its parent's
		-- host, read as the parent's are.
		local parent = data.parent
		local open = store.session:get()
		if not parent or not open or open.id ~= parent or open.state == "missing" then
			return nil
		end
		return { id = open.id, device = open.device, cwd = open.cwd, here = sessions.here:get() }
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
		foot = if tab.kind == "read" then data.foot elseif tab.kind == "subagent" then nil else room.foot,
		foot_target = if reading then nil else room.foot_target,
		-- Its "Jump to latest" and the pill of the composer at its foot, a
		-- centred pair: its own composer's, or, a split's focused pane, the
		-- window's (screens/chat/shared's `pill_width_of`).
		beside = if reading
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
		-- (Orchestrators' blocks, while it's on: what the engine said into
		-- the chat, crew updates.)
		orchestrators = function(): any
			return plugins.module("orchestrators")
		end,
		-- (A crew ask said into an orchestrator's conversation: its ask as it
		-- stands, the decision overlay, and the thread at its question.)
		ask_of = function(id: string): any?
			local orch = plugins.module("orchestrators")
			for _, a in if orch then orch.orchestration.asks:get() else {} do
				if a.id == id then
					return a
				end
			end
			return nil
		end,
		-- (An answer an older engine sent names no ask: its question, as
		-- the engine cut it to one line, finds it.)
		ask_by_question = function(question: string): any?
			local want = (string.gsub((string.gsub(question, "\u{2026}$", "")), "%s+", " "))
			local orch = plugins.module("orchestrators")
			for _, a in if orch then orch.orchestration.asks:get() else {} do
				local q = (string.gsub(a.question or "", "%s+", " "))
				if want ~= "" and string.sub(q, 1, #want) == want then
					return a
				end
			end
			return nil
		end,
		open_ask = function(id: string)
			local orch = plugins.module("orchestrators")
			if orch then
				orch.decide.show(id)
			end
		end,
		open_at = function(chat: string, message: string?)
			route.open_session(chat, { message = message })
		end,
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
	if reading then
		return ui.column { id = `lane-{slot}`, class = if tab.kind == "read" then "lane-tab read-tab" else "lane-tab subagent-tab", provide = { session = chat }, grow = 1, w_full = true, min_h = 0, relative = true, transcript, find.Bar() }
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
					Dock.Dock { height = dock_h, composer = composer(chat, view, `lane.{slot}`, focus, open_model, data.split == true, if split then focused_now else nil, data.own == true, nil, nil, nil, nil, lane_draft, lane_atoms, dock_target), id = `lane-composer-{slot}` },
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
