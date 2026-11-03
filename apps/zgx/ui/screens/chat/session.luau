-- A session's page: its conversation, virtualized (ds/transcript), a reply
-- streaming into it in place, and room at its foot for the composer, which
-- docks there (screens/chat/composer: one composer for the window, its
-- height reported in `chat_dock_h`).
--
-- The session shown follows `id` (a string, a function or a state) or,
-- without one, the route's. Moving to another session watches that one
-- instead: its transcript comes, loading meanwhile, then shows from its
-- end. The page itself stays: only what shows a session's words changes.

local ds = require("@ds")
local T = require("@ds/transcript")
local store = require("@app/screens/chat/store")
local route = require("@app/screens/shell/route")
-- Its side chats and subagents, in a card at the top right (the title
-- bar's "Show session details").
local lanes = require("@app/screens/chat/lanes")
local shared = require("@app/screens/chat/shared")
local prefs = require("@app/screens/settings/prefs")
local ReplyImages = require("@app/screens/chat/reply_images")
local FileLinks = require("@app/screens/chat/file_links")
local LinkMenu = require("@app/screens/chat/link_menu")
local Find = require("@app/screens/chat/find")
local boundary = require("@app/screens/shell/boundary")

-- The pictures replies show, read from the open chat's folder on its device.
local pictures = ReplyImages.for_session(function(): any
	local s = store.session:get()
	return if s and s.state ~= "missing" then s else nil
end)
-- The pictures sent with its messages: on another device, read from there.
local sessions = require("@account/sessions")
local sent_pictures = ReplyImages.for_attachments(function(): any
	local s = store.session:get()
	if not s or s.state == "missing" then
		return nil
	end
	return { id = s.id, device = s.device, cwd = s.cwd, here = sessions.now:get().here }
end)

export type Id = string | () -> string? | State<string?> | nil

-- The docked composer's geometry and the room the transcript keeps under
-- it (screens/chat/dock: a side chat's tab docks its composer the same).
local Dock = require("@app/screens/chat/dock")

local function resolve(id: Id): () -> string?
	if type(id) == "string" then
		return function()
			return id
		end
	elseif type(id) == "function" then
		return id :: () -> string?
	elseif id ~= nil then
		local s = id :: any
		return function()
			return s:get()
		end
	end
	return function()
		local now = route.current:get()
		return if now.name == "session" then now.session else nil
	end
end

-- Placeholder rows while a transcript comes: a prompt, a reply's lines.
local function loading(): Node
	return ui.column {
		w_full = true,
		h_full = true,
		items = "center",
		pt = "$space.8",
		px = "$space.5",
		ui.column {
			w_full = true,
			max_w = function(): number
				return prefs.conversation_width:get()
			end,
			gap = "$space.3",
			ui.row { w_full = true, justify = "end", ds.Skeleton { w = 220, h = 36, rounded = "$zeron.prose.bubble.radius" } },
			ds.Skeleton { w = 120 },
			ds.Skeleton { w_full = true },
			ds.Skeleton { w = "80%" },
			ds.Skeleton { w = "60%" },
		},
	}
end

local function Session(id: Id?): Node
	local which = resolve(id :: any)
	-- A file a reply links or a prompt mentions: opened in the session's
	-- panel (a link outside its folder stays as it is); a chat mentioned,
	-- that chat.
	local function open_link(target: string)
		local chat = string.match(target, "^zeron%-chat:([%w%-_]+)$")
		if chat then
			route.open_session(chat)
			return
		end
		local s = store.session:peek()
		local r = FileLinks.resolve(target, if s then s.cwd else nil)
		local panel: any = boundary.load("@app/screens/panel")
		local id = which()
		if r and not r.outside and panel and panel.open_file and id then
			-- (Its line too, for the files pane to go to.)
			(panel.open_file :: any)(id, r.path, route.project:peek(), { line = r.line, column = r.column, end_line = r.end_line })
		end
	end
	-- Watch the session the page is on, and let it go as the page goes.
	ui.effect(function()
		store.open(which())
	end)
	-- Let go only once the route has left sessions: a page made again (a
	-- reload, a template saved) keeps watching, mid-reply included.
	ui.on_cleanup(function()
		ui.after(0, function()
			if route.current:peek().name ~= "session" then
				store.open(nil)
			end
		end)
	end)
	local offset = shared.offset
	local room = Dock.room(function(): number
		return shared.dock_h:get()
	end)
	-- Another session opened: where it was left (its place kept as the
	-- reader left it), else its latest; the one left keeps its place for
	-- coming back. (Only when the id moves: what the route reads changes as
	-- sessions do, and a page made again by a reload stays where it was.)
	-- The transcript is made anew for it (keyed by `shown`, below), which
	-- puts the view there.
	local entering: { id: string, at_end: boolean }? = nil
	ui.effect(function()
		local s = which()
		local left = shared.shown:peek()
		if s and s ~= left then
			ui.batch(function()
				local places = table.clone(shared.places:peek())
				if left then
					places[left] = shared.anchor:peek()
				end
				local back = places[s]
				shared.places:set(places)
				entering = { id = s, at_end = back == nil }
				offset:set(T.PAST_END)
				shared.anchor:set(back)
				shared.shown:set(s)
			end)
		end
	end)
	-- What shows: the transcript once its session is the one asked for and
	-- has come; a word when it can't be had.
	local kind = ui.computed(function(): string
		local s = store.session:get()
		local want = which()
		if not want then
			return "missing"
		end
		if not s or s.id ~= want or s.state == "loading" then
			return "loading"
		end
		return s.state
	end)
	-- The session it shows, for what lands inside the page (the transcript,
	-- the composer; a plugin's node): its id, which the host resolves.
	local session = ui.computed(function(): string?
		return which()
	end)
	-- Files carried over the page (the transcript, under the composer)
	-- attach to the composer, as a drop on it does.
	local zone = shared.drop_zone("session")
	return ui.column {
		id = "chat-session",
		provide = { session = session },
		on_file_hover = zone.on_file_hover,
		on_file_drop = zone.on_file_drop,
		drop_when = zone.drop_when,
		on_drag_over = zone.on_drag_over,
		on_drag_leave = zone.on_drag_leave,
		on_hover = zone.on_hover,
		on_drop = zone.on_drop,
		w_full = true,
		h_full = true,
		min_h = 0,
		-- A bare Escape that reached the page (nothing inside took it): the
		-- setting's "Escape stops the agent".
		on_key = function(e): boolean
			if e.key == "Escape" and not e.shift and not e.cmd and not e.alt then
				return store.escape()
			end
			return false
		end,
		ui.presence(kind, function(k: string): Node
			if k == "loading" then
				return loading()
			elseif k == "missing" then
				return ds.EmptyState {
					icon = "chat-round-line",
					title = ui.t("chat.session.missing_title") :: any,
					body = ui.t("chat.session.missing_body") :: any,
					action = {
						label = ui.t("chat.session.new_chat") :: any,
						on_click = function()
							route.new_chat(nil)
						end,
					},
					fill = true,
				}
			elseif k == "offline" then
				return ds.EmptyState {
					icon = "wifi-off",
					title = ui.t("chat.session.offline_title") :: any,
					body = ui.t("chat.session.offline_body") :: any,
					action = {
						label = ui.t("common.retry") :: any,
						on_click = function()
							store.open(nil)
							store.open(which())
						end,
					},
					fill = true,
				}
			end
			return ui.column {
				hook = "chat.transcript",
				w_full = true,
				h_full = true,
				min_h = 0,
				-- Under the composer, to its footer's line.
				pb = Dock.page_pad(),
				ui.column {
					w_full = true,
					grow = 1,
					min_h = 0,
					-- (The find bar floats over its top end.)
					relative = true,
					-- Made anew for each session shown (`shown`, which the switch
					-- sets with its place): a chat switched to opens where it was
					-- left, or at its latest.
					ui.presence(function(): string
						return shared.shown:get() or ""
					end, function(id: string): Node
						return T.Transcript {
							-- Settings' "Conversation width", followed as it moves.
							measure = function(): number
								return prefs.conversation_width:get()
							end,
							messages = store.messages,
							offset = offset,
							-- Made again (a reload, a template saved), it comes
							-- back where it was.
							keep = shared.anchor,
							-- (A chat switched back to: whether it was left at its end.)
							was_at_end = if entering and entering.id == id then entering.at_end else nil,
							-- Under the docked composer: the conversation runs
							-- behind it, its end held clear of it.
							foot = room.foot,
							draft = store.draft,
							-- Settings' compact transcript: each reply's work
							-- folded into one group.
							compact = function(): boolean
								return prefs.compact_transcript:get()
							end,
							on_mention = shared.mention,
							on_retry = function(message: string)
								local s = which()
								if s then
									store.retry(s, message)
								end
							end,
							on_attach = shared.attach_quote,
							pictures = pictures,
							sent_pictures = sent_pictures,
							-- A subagent's spawn opens its lane, a panel tab
							-- (the shipping app's `OpenSubagent`).
							on_open_subagent = function(sub: any)
								local s = which()
								if s then
									lanes.open_subagent(s, sub)
								end
							end,
							open_link = open_link,
							-- Their context menus (screens/chat/link_menu).
							link_menu = LinkMenu(open_link, function(): any
								return store.session:peek()
							end),
							-- ⌘F's find (screens/chat/find): its matches marked.
							find = Find.marks(),
						}
					end),
					Find.Bar(),
				},
			}
		end, { w_full = true, grow = 1, min_h = 0 }),
		lanes.Overlay(),
	}
end

return Session
