-- What the chat page's parts share: the one composer's draft, what it
-- carries and the passages quoted into it, each chat's own (as Zeron's
-- composer keeps them, crates/ui composer.rs `drafts` and `attachments`:
-- per chat, a new chat's under its own key, kept for the run, not saved
-- across launches); how tall the docked composer is (the transcript
-- leaves that room); and a bump that hands the composer the keys.
--
-- The composer binds `text`, `cargo` and `quotes`: the chat shown's. Moving
-- to another chat (`show`) puts them away under the chat left and takes out
-- the next one's (empty if none), in place: the composer stays one node.

local Quote = require("@ds/quote")
local Annotation = require("@ds/annotation")
local sessions = require("@account/sessions")
local ChatLinks = require("@app/screens/chat/chat_links")

local M = {}

-- The draft, what it carries, the options picked in a question being
-- answered, and where the transcript is scrolled to. Shared state, not a
-- module's own: a template replacing any chat module (this one included)
-- and every hot reload find them where they were.
M.text = ui.shared("chat.composer.text", "")
M.cargo = ui.shared("chat.composer.cargo", {} :: { any })
M.quotes = ui.shared("chat.composer.quotes", {} :: { [string]: string })
M.chosen = ui.shared("chat.composer.chosen", {} :: { [string]: boolean })
-- Each session's agent, model, effort and Fast picked in its composer but
-- not yet sent with (`model`, `effort`, `fast`), by session id: part of its
-- draft, as the text is. The session takes them only when a message goes
-- with them.
M.picks = ui.shared("chat.composer.picks", {} :: { [string]: { [string]: any } })
-- (Past the end, as ds/transcript's PAST_END: never `math.huge`, which a
-- measured list takes as its top.)
M.offset = ui.shared("chat.transcript.offset", 1e9)
-- The session the transcript last showed: a reloaded page showing it
-- again keeps its place; another one opens at its latest.
M.shown = ui.shared("chat.transcript.session", nil :: string?)
-- Where in it the reader was (ds/transcript's `keep`), for a page made
-- again; nil at the end.
M.anchor = ui.shared("chat.transcript.anchor", nil :: any)
-- Each chat's place as it was left (its `anchor`; none, its end), by
-- session id: switched back to, a chat opens where it was read.
M.places = ui.shared("chat.transcript.places", {} :: { [string]: any })
-- Every other chat's draft, by key (a session's id; "" a new chat's), and
-- the key of the one shown.
local kept = ui.shared("chat.composer.drafts", {} :: { [string]: any })
local here = ui.shared("chat.composer.here", "")
-- Bumped, the composer takes the keys.
M.focus = ui.state("chat_focus", 0)
-- The docked composer's height, as laid out.
M.dock_h = ui.state("chat_dock_h", 52)
-- The chips made so far, for each its own id: kept with the cargo (which
-- a reload keeps), so a chip made after a reload never takes one's id.
local made = ui.shared("chat.composer.chips_made", 0)
local function chip_id(kind: string): string
	local n = made:peek() + 1
	made:set(n)
	return `{kind}{n}`
end

-- Whether a file's name is a picture the window can draw. (zgx decodes
-- no GIF or HEIC yet: those are file chips, so a thumbnail never fails.)
-- A project's file or folder as Zeron mentions it in a prompt, and back
-- (screens/chat/file_links).
local FileLinks = require("@app/screens/chat/file_links")
M.file_link = FileLinks.link
M.file_of = FileLinks.path_of

local function is_image(name: string): boolean
	local ext = string.match(string.lower(name), "%.(%a+)$")
	return ext == "png" or ext == "jpg" or ext == "jpeg" or ext == "webp"
end

-- The chat shown is now `key` (a session's id, or "" for a new chat): the
-- draft shown is put away under the chat left (nothing kept for an empty
-- one), and `key`'s own is taken out.
-- The chat whose draft is out ("" for a new chat's).
function M.showing(): string
	return here:peek()
end

function M.show(key: string)
	local was = here:peek()
	if was == key then
		return
	end
	ui.batch(function()
		local all = table.clone(kept:peek())
		local text, cargo, quotes, chosen = M.text:peek(), M.cargo:peek(), M.quotes:peek(), M.chosen:peek()
		if text ~= "" or #cargo > 0 or next(chosen) ~= nil then
			all[was] = { text = text, cargo = cargo, quotes = quotes, chosen = chosen }
		else
			all[was] = nil
		end
		local now = all[key]
		all[key] = nil
		kept:set(all)
		here:set(key)
		M.text:set(if now then now.text else "")
		M.cargo:set(if now then now.cargo else {})
		M.quotes:set(if now and now.quotes then now.quotes else {})
		M.chosen:set(if now and now.chosen then now.chosen else {})
	end)
end

-- What was typed and carried, taken for sending (the composer cleared).
function M.take(): (string, { any })
	local text = Quote.expand(M.text:peek(), Quote.kept(M.text:peek(), M.quotes:peek()))
	local cargo = M.cargo:peek()
	ui.batch(function()
		M.text:set("")
		M.cargo:set({})
		M.quotes:set({})
	end)
	return text, cargo
end

-- A passage of the transcript, quoted into the draft (its line marks it)
-- or carried as a chip.
function M.mention(q: Quote.Quote)
	local line = Quote.line(q)
	local quotes = table.clone(M.quotes:peek())
	quotes[line] = q.text
	ui.batch(function()
		M.quotes:set(quotes)
		M.text:set(Quote.add(M.text:peek(), line))
	end)
	M.focus:add(1)
end

function M.attach_quote(q: Quote.Quote)
	-- The same passage twice (attached again, or its menu and its bar both
	-- taken) is one chip.
	for _, item in M.cargo:peek() do
		if item.kind == "annotation" and item.quote and Quote.same({ text = item.quote, who = item.name, from = item.from }, q) then
			M.focus:add(1)
			return
		end
	end
	M.cargo:push(Quote.chip(q, chip_id("quote")))
	M.focus:add(1)
end

-- An element (or a region) picked on a page in the panel's browser
-- (screens/panel/browser): a chip, numbered after the draft's other picks
-- (its "Annotation N", the number the page's mark wears). `extra`: the tab
-- it was picked in and the page's marker. The chip's id and its number.
-- The number the next pick gets: one past the draft's highest.
function M.next_annotation(): number
	local index = 0
	for _, item in M.cargo:peek() do
		if item.kind == "annotation" and item.index then
			index = math.max(index, item.index)
		end
	end
	return index + 1
end

function M.attach_annotation(pick: { [string]: any }, extra: { tab: string?, marker: number? }?): (string, number)
	local index = M.next_annotation()
	local with_index = table.clone(pick)
	with_index.index = index
	local id = chip_id("pick")
	M.cargo:push(Annotation.chip(with_index, id, extra))
	return id, index
end

-- The words typed for a pick (its note field): kept with its chip, sent
-- after its token.
function M.note_cargo(id: string, note: string)
	local list = table.clone(M.cargo:peek())
	for i, item in list do
		if item.id == id then
			local next_item = table.clone(item)
			next_item.note = if note ~= "" then note else nil
			list[i] = next_item
		end
	end
	M.cargo:set(list)
end

-- Files picked or dropped: chips, carried with the message.
function M.attach_files(files: { any })
	for _, f in files do
		local name = f.name or f.path or "file"
		local picture = is_image(name)
		M.cargo:push {
			id = chip_id("file"),
			kind = if picture then "image" else "file",
			name = name,
			src = if picture then f.path else nil,
			path = f.path,
		}
	end
	M.focus:add(1)
end

-- Files of the project (the files pane's, dragged here or its "Add to
-- chat"): chips, carried with the message as files from outside are. Each
-- is already on the session's computer (its project is), so it goes by its
-- path there, not uploaded again; `paths` are the project's own (relative
-- to `root`, the project's folder there), and a chip says where in it the
-- file is. One already carried isn't added twice.
function M.add_to_chat(paths: { string }, root: string?)
	local have: { [string]: boolean } = {}
	for _, item in M.cargo:peek() do
		if item.path then
			have[item.path] = true
		end
	end
	-- On this computer (the project whose folder it is lives here): a
	-- picture shows as one, its thumbnail read from where it is. On
	-- another, a file chip.
	local here = false
	local now = sessions.now:peek()
	for _, p in now.projects do
		if p.path == root and p.device == now.here then
			here = true
		end
	end
	for _, rel in paths do
		local at = if root and root ~= "" then `{root}/{rel}` else rel
		if not have[at] then
			have[at] = true
			local name = string.match(rel, "([^/]+)/?$") or rel
			local picture = here and is_image(name)
			M.cargo:push {
				id = chip_id("file"),
				kind = if picture then "image" else "file",
				name = name,
				src = if picture then at else nil,
				-- (Where in the project, under its name.)
				meta = string.match(rel, "^(.*)/[^/]+$"),
				path = at,
				rel = rel,
				on_host = true,
			}
		end
	end
	M.focus:add(1)
end

-- The composer's model picker, opened from elsewhere (the shell's ⌘/):
-- bumped, it opens. `open_model_picker` says whether there was a composer
-- to open it in.
M.open_model = ui.shared("chat.composer.open_model", 0)
M.composer_shown = ui.shared("chat.composer.shown", false)
function M.open_model_picker(): boolean
	if not M.composer_shown:peek() then
		return false
	end
	M.open_model:set(M.open_model:peek() + 1)
	return true
end

-- Files carried over the chat (from outside, or the files pane), by the
-- zone they're over: the session page, a new chat's page, the composer's
-- pill. While any is, the whole chat shows "Drop to attach"; a drop on any
-- attaches to the composer.
M.dropping = ui.shared("chat.dropping", {} :: { [string]: boolean })

-- (`what`: "session" while a sidebar session is carried, its drop a
-- mention; else files.)
function M.over(zone: string, inside: boolean, what: string?)
	local now = M.dropping:peek()
	local want = if inside then (what or true) else nil
	if now[zone] == want then
		return
	end
	local next = table.clone(now)
	next[zone] = want
	M.dropping:set(next)
end

-- Whether files are being carried over the chat ("Drop to attach").
function M.carrying(): boolean
	for _, what in M.dropping:get() do
		if what ~= "session" then
			return true
		end
	end
	return false
end

-- Whether a sidebar session is being carried over it ("Drop to mention").
function M.carrying_session(): boolean
	for _, what in M.dropping:get() do
		if what == "session" then
			return true
		end
	end
	return false
end

-- Files carried from outside end with a drop or with their leave. One lost
-- on the way (the platform's drag ended elsewhere, or what the files were
-- over went first) left the chat dimmed under "Drop to attach" for good,
-- Escape and all. Any sign the drag is over lets go of them: Escape, the
-- window losing focus, the pointer over the chat (none reaches a window
-- while another app's drag is over it), or no word of them for a while. A
-- sidebar session carried is the window's own drag, whose end is always
-- heard: kept. True if files were let go of.
function M.let_go(): boolean
	local now = M.dropping:peek()
	local next, any = {}, false
	for zone, what in now do
		if what == "session" then
			next[zone] = what
		else
			any = true
		end
	end
	if any then
		M.dropping:set(next)
	end
	return any
end

-- No word of them for this long (each move from one part of the chat to
-- another is one): let go. The pointer's word counts only once they've
-- been over the chat a moment: a move it made just before the drag came
-- can be heard after it.
local STALE_MS = 8000
local SETTLE_MS = 300
local settled = false
ui.effect(function()
	settled = false
	if M.carrying() then
		ui.after(SETTLE_MS, function()
			settled = true
		end)
		ui.after(STALE_MS, function()
			M.let_go()
		end)
	end
end)
ui.effect(function()
	if ui.window.active:get() == false then
		M.let_go()
	end
end)

-- The pointer over the chat again: no drag from outside is under way.
function M.pointer_back()
	if settled then
		M.let_go()
	end
end

-- Another chat mentioned (a sidebar session dropped on the chat): a chip,
-- sent as Zeron's chat mention (screens/chat/chat_links). Once each.
function M.mention_chat(id: string)
	if not ChatLinks.valid(id) then
		return
	end
	for _, item in M.cargo:peek() do
		if item.kind == "chat" and item.chat == id then
			return
		end
	end
	local title, mark = nil :: string?, nil :: string?
	for _, c in sessions.now:peek().chats do
		if c.id == id then
			title, mark = c.title, c.agent
		end
	end
	M.cargo:push { id = chip_id("chat"), kind = "chat", name = ChatLinks.title(title), chat = id, mark = mark }
end

-- A chat chip's text as it goes.
function M.chat_link(title: string, id: string): string
	return ChatLinks.link(title, id) or ""
end

-- A drag from inside the window that drops here: the files pane's files
-- (their paths). Other drags (a session, a tab, a section) aren't ours.
local function workspace_paths(payload: any): { string }?
	if type(payload) == "table" and type(payload.paths) == "table" and #payload.paths > 0 then
		return payload.paths
	end
	return nil
end

-- The props that make a node a drop zone of the chat, named `zone`: files
-- from outside, and the files pane's, attach as chips.
function M.drop_zone(zone: string): { [string]: any }
	return {
		on_file_hover = function(inside: boolean)
			M.over(zone, inside)
		end,
		on_file_drop = function(files: { any })
			M.dropping:set({})
			M.attach_files(files)
		end,
		drop_when = function(payload: any): boolean
			-- (Files, and a sidebar session: a mention.)
			return workspace_paths(payload) ~= nil or (type(payload) == "table" and type(payload.session) == "string")
		end,
		on_drag_over = function(payload: any)
			M.over(zone, true, if type(payload) == "table" and type(payload.session) == "string" then "session" else nil)
		end,
		on_drag_leave = function()
			M.over(zone, false)
		end,
		on_hover = function(inside: boolean)
			if inside then
				M.pointer_back()
			end
		end,
		on_drop = function(payload: any)
			M.dropping:set({})
			local paths = workspace_paths(payload)
			if paths then
				M.add_to_chat(paths, payload.root)
			elseif type(payload) == "table" and type(payload.session) == "string" then
				M.mention_chat(payload.session)
			end
		end,
	}
end

-- A review comment left on a project's file (the files pane's editor or
-- its preview): a chip on the composer, sent as the review comments block
-- after the words (screens/chat/comments). `file` is the project's path
-- (relative to its folder), `line` the line it's on. Its id comes back, to
-- change it (`update_comment`) or take it off (`remove_cargo`). On the
-- cargo it's an item of kind "comment" with `file`, `line`, `body` (and,
-- from the files pane, its `path` and the project's `root` too: given as
-- `{ path, line, body, root }`, `file` is the path within the root).
-- On a diff (a plugin's, `zeron.compose_comment`): `side` ("L" or "R"), a
-- range's last line `to`, the code it's on `snippet`; `plugin` and `key`
-- say whose it is (published back to it: screens/plugins), and `session`
-- the chat whose draft takes it (the one shown, else its draft kept).
function M.attach_comment(c: {
	file: string?,
	path: string?,
	line: number,
	body: string,
	root: string?,
	to: number?,
	side: string?,
	snippet: string?,
	plugin: string?,
	key: string?,
	session: string?,
}): string
	local id = chip_id("comment")
	local file = c.file or c.path or ""
	local root = c.root
	if not c.file and root and root ~= "" and string.sub(file, 1, #root + 1) == root .. "/" then
		file = string.sub(file, #root + 2)
	end
	local name = string.match(file, "([^/]+)$") or file
	local to = if c.to and c.to > c.line then c.to else nil
	local item = {
		id = id,
		kind = "comment",
		name = if to then `{name}:{c.line}–{to}` else `{name}:{c.line}`,
		meta = (string.gsub(c.body, "%s+", " ")),
		file = file,
		path = c.path or file,
		root = root,
		line = c.line,
		to = to,
		side = if c.side == "L" or c.side == "R" then c.side else nil,
		snippet = c.snippet,
		body = c.body,
		plugin = c.plugin,
		key = c.key,
	}
	local session = c.session
	if session and session ~= here:peek() then
		-- Another chat's draft, kept: the chip waits in it.
		local all = table.clone(kept:peek())
		local draft = table.clone(all[session] or { text = "", cargo = {}, quotes = {} })
		local cargo = table.clone(draft.cargo or {})
		table.insert(cargo, item)
		draft.cargo = cargo
		all[session] = draft
		kept:set(all)
		return id
	end
	M.cargo:push(item)
	return id
end

-- Every comment a plugin added still carried, in the draft shown and the
-- drafts kept: by plugin, its key → the chip's id and its session.
function M.plugin_comments(): { [string]: { [string]: { id: string, session: string } } }
	local out: { [string]: { [string]: { id: string, session: string } } } = {}
	local function take(cargo: { any }?, session: string)
		for _, item in cargo or {} do
			if item.kind == "comment" and type(item.plugin) == "string" and type(item.key) == "string" then
				out[item.plugin] = out[item.plugin] or {}
				out[item.plugin][item.key] = { id = item.id, session = session }
			end
		end
	end
	take(M.cargo:get(), here:get())
	for session, draft in kept:get() do
		take(draft.cargo, session)
	end
	return out
end

-- A comment carried to another line (the edits above it moved it): its
-- chip says the new line.
function M.move_comment(id: string, line: number)
	local list = M.cargo:peek()
	for i, item in list do
		if item.id == id and item.kind == "comment" then
			local next = table.clone(item)
			next.line = line
			local name = string.match(item.file or "", "([^/]+)$") or item.file or ""
			next.name = `{name}:{line}`
			M.cargo:set_key({ i }, next)
			return
		end
	end
end

-- A comment's words changed (its chip follows).
function M.update_comment(id: string, body: string)
	local list = M.cargo:peek()
	for i, item in list do
		if item.id == id and item.kind == "comment" then
			local next = table.clone(item)
			next.body = body
			next.meta = (string.gsub(body, "%s+", " "))
			M.cargo:set_key({ i }, next)
			return
		end
	end
end

function M.remove_cargo(id: string)
	local left = {}
	for _, item in M.cargo:peek() do
		if item.id ~= id then
			table.insert(left, item)
		end
	end
	M.cargo:set(M.renumber(left))
end

-- Chat `key`'s draft, wherever it is: out in the composer while it shows
-- that chat, else kept for it: its words, its chips and the options picked
-- in a question it's asked. What shows a chat's draft by its id (a split
-- pane's composer) reads it here, never the chat shown's; `show` moves a
-- draft between the two in one batch, so it's never read half moved.
function M.draft_of(key: string): { text: string, cargo: { any }, chosen: { [string]: boolean } }
	if here:get() == key then
		return { text = M.text:get(), cargo = M.cargo:get(), chosen = M.chosen:get() }
	end
	local d = kept:get()[key]
	return {
		text = if d and type(d.text) == "string" then d.text else "",
		cargo = if d and type(d.cargo) == "table" then d.cargo else {},
		chosen = if d and type(d.chosen) == "table" then d.chosen else {},
	}
end

-- Chat `key`'s draft changed where it's shown by its id (`field`: "text"
-- or "chosen"): written wherever it is, as `draft_of` reads it.
function M.set_draft_of(key: string, field: string, value: any)
	if here:peek() == key then
		if field == "text" then
			M.text:set(value)
		else
			M.chosen:set(value)
		end
		return
	end
	local all = table.clone(kept:peek())
	local d = table.clone(all[key] or { text = "", cargo = {}, quotes = {} })
	d[field] = value
	all[key] = d
	kept:set(all)
end

-- A chip taken off chat `key`'s draft, wherever the draft is.
function M.remove_cargo_of(key: string, id: string)
	if here:peek() == key then
		M.remove_cargo(id)
		return
	end
	local all = table.clone(kept:peek())
	local d = all[key]
	if not d then
		return
	end
	d = table.clone(d)
	local left = {}
	for _, item in d.cargo or {} do
		if item.id ~= id then
			table.insert(left, item)
		end
	end
	d.cargo = M.renumber(left)
	all[key] = d
	kept:set(all)
end

-- The page's picks numbered 1, 2, 3… in the order made, no gap where one
-- left: the chips, the tokens they send, and (the browser tab following)
-- the marks on the page.
function M.renumber(list: { any }): { any }
	local n = 0
	local out = {}
	for _, item in list do
		if item.kind == "annotation" and item.element ~= nil and item.index then
			n += 1
			if item.index ~= n then
				local next_item = table.clone(item)
				next_item.index = n
				if type(item.annotation) == "table" then
					local a = table.clone(item.annotation)
					a.index = n
					next_item.annotation = a
				end
				item = next_item
			end
		end
		table.insert(out, item)
	end
	return out
end

return M
