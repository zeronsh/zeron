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
local T = require("@ds/transcript")
local lanes = require("@app/screens/chat/lanes")
local store = require("@app/screens/chat/store")
local prefs = require("@app/screens/settings/prefs")
local locale = require("@ds/locale")
local sessions = require("@account/sessions")
local route = require("@app/screens/shell/route")
local Where = require("@app/screens/chat/where")
local Dock = require("@app/screens/chat/dock")
local ReplyImages = require("@app/screens/chat/reply_images")
local FileLinks = require("@app/screens/chat/file_links")
local LinkMenu = require("@app/screens/chat/link_menu")
local boundary = require("@app/screens/shell/boundary")
local Quote = require("@ds/quote")
-- (The chat page's places in its chats, which a split's panes share.)
local Shared = require("@app/screens/chat/shared")

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

function M.title(tab: any): string
	if tab.title and tab.title ~= "" then
		return tab.title
	end
	return locale.s(if tab.kind == "subagent" then "chat.lane_tab.subagent" else "chat.lane_tab.side_chat")
end

function M.icon(tab: any): string
	return if tab.kind == "subagent" then "bot" else "side-chat"
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
local function composer(chat: string, view: any, name: string, split: boolean?, focused: (() -> boolean)?): Node
	-- (Its draft kept by the chat, as its picks: a switch of tabs or a
	-- reload finds it as it was; a passage quoted from its transcript lands
	-- in it.)
	-- (A pane of the page's split that isn't focused shows the chat
	-- page's own draft for its chat, as that keeps it while another chat
	-- is shown (screens/chat/shared's drafts), so focused, the chat page's
	-- composer goes on with the same words.)
	local draft = ui.state(if split then `chat_split_draft_{chat}` else `chat_lane_draft_{chat}`, "")
	-- (A split's pane: its draft's chips, as the chat page keeps them.)
	local carried = ui.use_state({} :: { any })
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
	-- (The chat page's drafts: the one it shows (`here`'s), and the others
	-- kept, by chat.)
	local kept = ui.shared("chat.composer.drafts", {} :: { [string]: any })
	local here = ui.shared("chat.composer.here", "")
	-- A chip taken off a split pane's draft, wherever the draft is now.
	local function remove_carried(cid: string)
		if here:peek() == chat then
			Shared.remove_cargo(cid)
			return
		end
		local all = table.clone(kept:peek())
		local d = all[chat]
		if not d then
			return
		end
		d = table.clone(d)
		local left = {}
		for _, item in d.cargo or {} do
			if item.id ~= cid then
				table.insert(left, item)
			end
		end
		d.cargo = left
		all[chat] = d
		kept:set(all)
	end
	if split then
		-- (Wherever its words are now: the chat page's composer's own while
		-- it shows this chat, else kept for it. Read from both, never empty
		-- as they move from one to the other, so the composer never shrinks
		-- and grows again as the focus moves.)
		ui.effect(function()
			local text: string
			if here:get() == chat then
				text = Shared.text:get()
			else
				local d = kept:get()[chat]
				text = if d and type(d.text) == "string" then d.text else ""
			end
			if draft:peek() ~= text then
				draft:set(text)
			end
		end)
		-- (What the draft carries, its chips (files, pictures, comments,
		-- chats mentioned), from the same place as its words.)
		ui.effect(function()
			local items: { any }
			if here:get() == chat then
				items = Shared.cargo:get()
			else
				local d = kept:get()[chat]
				items = if d and type(d.cargo) == "table" then d.cargo else {}
			end
			carried:set(items)
		end)
	end
	local session = view.session
	local notice = ui.use_state(nil :: any)
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
		if now.model ~= was.model or now.effort ~= was.effort or now.access ~= was.access then
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
		project:set(place.project)
		device:set(place.device)
		checkout:set(place.checkout)
		branch:set(place.branch)
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
			for _, p in sessions.now:get().projects do
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
		return {
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
		local out: { [string]: any } = { model = m, effort = effort:peek(), access = access:peek() }
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
	return ds.Composer {
		value = draft,
		snap = if split then snap else nil,
		cargo = if split then carried else nil,
		-- (Its chips' × under the pointer, as the chat page's.)
		on_remove_cargo = if split then remove_carried else nil,
		-- (ds.Composer's placeholder is a string: read as the tab is made.)
		-- (A split's pane is a chat of its own: the chat page's words.)
		placeholder = locale.s(if split then "chat.composer.placeholder" else "chat.lane_tab.placeholder"),
		running = running,
		access = access,
		model = model,
		effort = effort,
		notice = notice,
		on_dismiss_notice = function()
			notice:set(nil)
		end,
		harnesses = function(): { any }
			return store.catalog:get().harnesses
		end,
		models = function(): { any }
			return store.catalog:get().models
		end,
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
		on_send = function(text: string)
			local words = string.match(text, "^%s*(.-)%s*$") or ""
			if words == "" then
				return
			end
			-- (Nowhere it can start, a project gone: the line says why.)
			local is_unsent = lanes.unsent:peek()[chat] ~= nil
			if is_unsent and where.blocked:peek() then
				notice:set({ tone = "danger", text = where.blocked:peek(), dismissable = true })
				return
			end
			draft:set("")
			notice:set(nil)
			-- As the chat page sends: the prompt joins its transcript as the
			-- transcript's send steps clear of the end (it glides to the top,
			-- its reply's room opening under it), then goes to the host with
			-- that id, so the host's copy is the same row.
			local mid = store.new_id()
			local req = picks()
			req.message_id = mid
			local sent: any = nil
			T.send(function()
				local here = false
				for _, m in view.messages:peek() do
					if m.id == mid then
						here = true
					end
				end
				if not here then
					-- (On its way from this moment: the line under it says so.)
					view.messages:push { id = mid, role = "user", text = words, waiting = "sending" }
				end
			end, mid, name)
			-- Its first message makes it (New side chat), then sends.
			if is_unsent then
				sent = lanes.send_first(chat, words, req, place())
			else
				req.session, req.text = chat, words
				sent = store.send(req :: any)
			end
			sent:after(function() end, function(err: any)
				draft:set(words)
				local why = if type(err) == "table" and err.message then tostring(err.message) else nil
				notice:set({
					tone = "danger",
					text = if is_unsent and why then why else locale.s("chat.lane_tab.didnt_send"),
					dismissable = true,
				})
			end)
		end,
		on_stop = function()
			store.interrupt(chat)
		end,
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
	-- Watched once there's a chat to watch, while its tab is open: the
	-- panel lets it go as the tab closes. (A reload asks for the same chat
	-- again, which the host already has: nothing more happens.)
	-- (A split's pane is watched by the page, only while it isn't
	-- the focused one.)
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
	-- (A split's pane. Its transcript is its own for as long as it's
	-- in the split, focused or not: never made again, so the focus moving
	-- never moves it. Focused, it reads the chat as the window watches it
	-- (the chat page's store); not, as its lane does. Between the two (the
	-- focus just moved, the other not come yet), it keeps the latest it
	-- had, so nothing it shows goes and comes back. It's made once there's
	-- something to show, opening where the chat page left the chat
	-- (`shared.places`).)
	local function focused_now(): boolean
		local f = data.focused
		return f ~= nil and f() == true
	end
	local loaded = ui.computed(function(): boolean
		local s = view.session:get()
		return s ~= nil and s.id == chat and s.state ~= "loading"
	end)
	local function main_ready(): boolean
		local s = store.session:get()
		return s ~= nil and s.id == chat and s.state == "ready"
	end
	local latest: { any } = {}
	local messages: any = view.messages
	local reply: any = view.draft
	if split then
		-- (The same conversation: its first message the one it had. As the
		-- window's chat moves to this one, its list holds the last chat's for
		-- a moment, ready by its session's word; taken then, the transcript
		-- would show another conversation, lose its place and land at its
		-- end.)
		local function same_chat(list: { any }): boolean
			if #latest == 0 or #list == 0 then
				return #list > 0
			end
			return list[1].id == latest[1].id
		end
		messages = ui.computed(function(): { any }
			if focused_now() and main_ready() then
				local list = store.messages:get()
				if same_chat(list) then
					latest = list
					return list
				end
				return latest
			end
			local list = view.messages:get()
			if loaded:get() and #list >= #latest then
				latest = list
				return list
			end
			return latest
		end)
		reply = ui.computed(function(): any
			if focused_now() and main_ready() then
				return store.draft:get()
			end
			return view.draft:get()
		end)
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
		ever = #messages:get() > 0 or loaded:get() or (focused_now() and main_ready())
		return ever
	end)
	local dock_h = ui.state(`chat_lane_dock_h_{slot}`, Dock.REST)
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
		return { id = s.id, device = s.device, cwd = s.cwd, here = sessions.now:get().here }
	end)
	-- The session whose panel it's in (the page's): where a file it links
	-- opens, and a subagent it spawned.
	local function home_session(): string?
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
		if r and not r.outside and panel and panel.open_file and home then
			(panel.open_file :: any)(home, r.path, route.project:peek(), { line = r.line, column = r.column, end_line = r.end_line })
		end
	end
	-- A passage quoted (its menu's Mention, or Attach): into its own draft.
	local draft = ui.state(`chat_lane_draft_{chat}`, "")
	local function quote(q: any)
		draft:set(Quote.add(draft:peek(), Quote.line(q)))
	end
	local function make_transcript(): Node
		return T.Transcript {
		-- (Its own glide, runway and place: never the chat page's.)
		name = `lane.{slot}`,
		measure = function(): number
			return prefs.conversation_width:get()
		end,
		messages = messages,
		draft = reply,
		offset = offset,
		keep = anchor,
		foot = if tab.kind == "subagent" then nil else room.foot,
		compact = function(): boolean
			return prefs.compact_transcript:get()
		end,
		on_mention = quote,
		on_attach = quote,
		on_retry = function(message: string)
			store.retry(chat, message)
		end,
		pictures = pictures,
		sent_pictures = sent_pictures,
		on_open_subagent = function(sub: any)
			local home = home_session()
			if home then
				lanes.open_subagent(home, sub)
			end
		end,
		open_link = open_link,
		link_menu = LinkMenu(open_link, function(): any
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
		return ui.column { id = `lane-{slot}`, class = "lane-tab subagent-tab", provide = { session = chat }, grow = 1, w_full = true, min_h = 0, transcript }
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
			return ui.column {
				grow = 1,
				w_full = true,
				min_h = 0,
				relative = true,
				ui.column {
					w_full = true,
					h_full = true,
					min_h = 0,
					pb = Dock.page_pad(),
					ui.column { w_full = true, grow = 1, min_h = 0, relative = true, transcript },
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
					Dock.Dock { height = dock_h, composer = composer(chat, view, `lane.{slot}`, data.split == true, if split then focused_now else nil), id = `lane-composer-{slot}` },
				},
			}
		end, { grow = 1, w_full = true, min_h = 0 }),
	}
end)

return M
