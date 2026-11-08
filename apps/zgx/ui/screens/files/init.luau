-- The session page's files: its folder's tree beside the file open, one band
-- across both (ds.FilesPane, the gallery's Files page), on the session's
-- device. Folders are read as they open, a file as it is picked; typing
-- into the file and ⌘S saves it; what changes on disk shows as it happens
-- (the tree in place, keeping its folds and scroll; the file followed, or,
-- under unsaved edits, a question which to keep).
--
--   Files.Pane { project = route.project, session = route.session }
--
-- Each is a value, a state or a function, followed as it changes: a
-- session { id, device?, cwd? } (or its id), a project { id, device?,
-- path?, name? } (or its id). Without either it reads the shell's route.
-- `radius`: the corner of the card it stands in, so its bars' buttons sit
-- concentric in it.
--
-- Plugins: the pane is `hook:files.pane`, the tree's tools
-- `hook:files.tree.tools`, the file's header actions
-- `hook:files.header.actions`, the notice over it `hook:files.notice`.
-- Deleting's question is out of their reach.

local ds = require("@ds")
local control = require("@ds/control")
local token = require("@ds/token")
local FilesPane = require("@ds/files_pane")
local Workspace = require("@app/screens/files/workspace")
local comments = require("@app/screens/files/comments")
local prefs = require("@app/screens/settings/prefs")
local api = require("@app/screens/files/api")
local paths = require("@app/screens/files/paths")

type Workspace = Workspace.Workspace
type View = Workspace.View

-- What plugins add to the files' menus (screens/plugins' registry): a menu that
-- can't load leaves the menus as they are.
local menus: any = nil
do
	local ok, found = pcall(function()
		return (require :: any)("@app/screens/plugins/menus")
	end)
	if ok then
		menus = found
	end
end

export type Props = {
	project: any?,
	session: any?,
	radius: string?,
	-- A tab's own view of its folder (the panel's): its id, and what opens
	-- a file in a new tab (a file picked while this one has unsaved edits).
	view: string?,
	on_open_in_new_tab: ((path: string) -> ())?,
	-- The file a new tab opens with.
	initial: string?,
	-- A file picked here that's open in another of the panel's tabs: that
	-- tab is shown instead (true), this one left as it was.
	open_elsewhere: ((path: string) -> boolean)?,
	-- Files carried with the chat's next message (the tree's row menu's Add
	-- to Chat), in place of the chat page's own.
	on_add_to_chat: ((paths: { string }, root: string?) -> ())?,
	-- How the keyboard leaves the code (ds/code_editor's `escape_hint`).
	escape_hint: string?,
	[any]: any,
}

local M = {}

-- The chat page's "carry these files with the next message" (screens/chat
-- `shared.add_to_chat`), if the chat page is there (a fixture mounts files
-- alone).
local function add_to_chat_fn(): ((paths: { string }, root: string?) -> ())?
	local ok, shared = pcall(function(): any
		return (require :: any)("@app/screens/chat/shared")
	end)
	if ok and shared and type(shared.add_to_chat) == "function" then
		return shared.add_to_chat
	end
	return nil
end
local add_to_chat = add_to_chat_fn()

-- How big a file opens (the host's editable limit, as the shipping app).
local LIMIT = 8 * 1024 * 1024

-- Whether the tree shows beside the file: remembered, as the app would.
local tree_shown = ui.persist("files_tree", true)

type Where = { key: string, target: api.Target, name: string }

-- Which folder the pane shows, and where it is.
local function where_of(project: any, session: any): Where?
	local target = paths.target_of(project, session)
	if not target then
		return nil
	end
	local pv: any = control.value(project)
	local root = target.root
	local name: string = (if type(pv) == "table" then pv.name :: string? else nil)
		or (if root then string.match(root, "[^/]+$") else nil)
		or ui.t("files.name_fallback"):get()
	-- (One folder, one tree: sessions in the same folder share it.)
	local key = `{target.device or "here"}:{root or target.chat or target.space}`
	return { key = key, target = target, name = name }
end

-- Deleting, asked first: what goes, and that it goes for good.
local DeleteQuestion = ui.component(function(props: { w: Workspace }): Node
	local w = props.w
	local asking = ui.use_state(false)
	ui.effect(function()
		asking:set(w.confirm:get() ~= nil)
	end)
	ui.effect(function()
		if not asking:get() and w.confirm:peek() ~= nil then
			w.confirm:set(nil)
		end
	end)
	return ui.div {
		-- (Deletes for good: no plugin reaches it.)
		hook = false,
		ds.Dialog {
			open = asking,
			title = ui.t("files.delete.title") :: any,
			tone = "danger",
			confirm = {
				label = ui.t("common.delete") :: any,
				on_click = function()
					Workspace.confirm_delete(w)
				end,
			},
			ui.text {
				function(): string
					local paths = w.confirm:get() or {}
					if #paths == 1 then
						return ui.t("files.delete.one", { name = Workspace.base_name(paths[1]) }):get()
					end
					return ui.t("files.delete.many", { n = #paths }):get()
				end,
				literal = true,
				wrap = true,
				text_size = "$font.size.md",
				color = "$text.secondary",
			},
		},
	}
end)

-- What went wrong (a save, a rename), over the pane: dismissed, or tried
-- again where that's a way on.
local function notice(w: Workspace): Node
	-- (Said here, in the language chosen: the problem keeps its words' keys
	-- and the file's name.)
	local function title(): string
		local p = w.problem:get()
		return if p then ui.t(p.what, { name = p.name or "" }):get() else ""
	end
	local function body(): string
		local p = w.problem:get()
		if not p then
			return ""
		end
		return if p.why_key then ui.t(p.why_key):get() else p.why or ""
	end
	local function dismiss()
		w.problem:set(nil)
	end
	return ui.column {
		hook = "files.notice",
		w_full = true,
		shrink = 0,
		ui.show(function(): boolean
			local p = w.problem:get()
			return p ~= nil and p.retry == true
		end, ds.Banner {
			edge = true,
			tone = "warning",
			title = title,
			body = body,
			on_dismiss = dismiss,
			actions = {
				{
					label = ui.t("common.try_again"),
					primary = true,
					on_click = function()
						local retry = w.retry
						w.problem:set(nil)
						if retry then
							retry()
						end
					end,
				},
			},
		}),
		ui.show(function(): boolean
			local p = w.problem:get()
			return p ~= nil and p.retry ~= true
		end, ds.Banner { edge = true, tone = "warning", title = title, body = body, on_dismiss = dismiss }),
	}
end

-- The pane for one folder, through view `v` (the file open).
local function folder(w: Workspace, v: View, props: Props, here: () -> Where?): Node
	-- A review comment being written in this view: where, and (editing
	-- one staged) which; its words. Kept by the view, across a reload.
	local draft = ui.state(`files_comment:{v.id}`, nil :: { path: string, line: number, editing: string? }?)
	local draft_text = ui.state(`files_comment_text:{v.id}`, "")
	-- A line asked for (a link to `path:line`), handed to the editor once
	-- that file shows here.
	local reveal_line = ui.state(`files_reveal:{v.id}`, nil :: any)
	ui.effect(function()
		local want = Workspace.lines:get()[v.id]
		local d = v.doc:get()
		if want and d and d.path == want.path and d.state == "ready" then
			ui.batch(function()
				Workspace.lines:set_key(v.id, nil)
				reveal_line:set(want.at)
			end)
		end
	end)
	-- A later session in the same folder: its own chat to address.
	ui.effect(function()
		local now = here()
		if now and now.key == w.key then
			w.target = now.target
		end
	end)
	-- Following the disk while it shows; read, or read again (a folder
	-- shown before may have changed while it wasn't followed).
	-- (Stopped by the build's own cleanup: one made inside an effect isn't
	-- run when a presence swaps its content.)
	ui.on_cleanup(Workspace.watch(w))
	-- (A beat after it's built, so a build begun again starts them again,
	-- rather than leaving them half-done.)
	-- (Read kept from before: nothing read again. The watch says if
	-- something may have changed meanwhile.)
	ui.after(0, function()
		if not w.listed then
			Workspace.load(w, "")
		end
	end)
	ui.effect(function()
		Workspace.drain(w, api.events:get())
	end)
	-- Git's statuses for this folder, as they change: the tree's marks and
	-- the file's.
	ui.effect(function()
		api.git:get()
		Workspace.apply_git(w)
	end)
	-- The file follows the tree's pick (one kept from before is read again
	-- if this run hasn't read it). In a tab, a file picked while this one
	-- has unsaved edits opens in a tab of its own, this one staying put.
	if props.initial and v.active:peek() == nil and v.doc:peek() == nil then
		v.active:set(props.initial)
	end
	local d = v.doc:peek()
	local last: string? = if d and w.meta[d.path] then d.path else nil
	ui.effect(function()
		local a = v.active:get()
		if a == last then
			return
		end
		-- Open in another tab already: that one, this one as it was.
		if a and props.open_elsewhere and props.open_elsewhere(a) then
			v.active:set(last)
			return
		end
		if last and a and props.on_open_in_new_tab and v.dirty:peek() then
			-- (This one back to its file first: the panel asks which tab
			-- shows `a`, and this one doesn't.)
			v.active:set(last)
			props.on_open_in_new_tab(a)
			return
		end
		last = a
		Workspace.open(v, a)
	end)
	-- The filter finds names in folders not read yet: asked once typing
	-- pauses, only the last answer kept, dropped if the pane goes first.
	local query = ui.debounced(w.filter, 160)
	local found = ui.resource(function(): any
		local q = query:get()
		if #q < 2 or string.find(q, "/", 1, true) then
			return nil
		end
		return api.search(w.target, q):await()
	end)
	ui.effect(function()
		local list = found.data:get()
		if list then
			Workspace.place_found(w, list)
		end
	end)

	local radius = props.radius
	local function here(): boolean
		return w.here:get()
	end
	local function session_id(): string?
		return w.target.chat
	end
	local function more_menu(path: string, kind: string): { any }
		if not menus then
			return {}
		end
		return menus.entries("file", { path = path, kind = kind, session = session_id() })
	end
	local function reveal(path: string)
		Workspace.reveal(w, path)
	end
	local function open_external(path: string)
		Workspace.open_external(w, path)
	end
	local pane = FilesPane {
		id = "files-pane",
		hook = "files.pane",
		shown = tree_shown,
		room = w.pane_room,
		head = "line",
		w_full = true,
		grow = 1,
		min_h = 0,
		rounded = radius,
		-- The bars' buttons sit in the card's corners: concentric.
		theme = if radius then { zeron = { icon_button = { radius = control.nest(radius, 1, 6) } } } else nil,
		tree = {
			model = w.model,
			active = v.active,
			reveal = v.reveal,
			offset = w.offset,
			folds = w.folds,
			filter = w.filter,
			revision = w.revision,
			trash = false,
			hooks = { tools = "files.tree.tools" },
			loading = function(): boolean
				w.revision:get()
				return w.status:get() == "loading" or not w.listed
			end,
			on_expand = function(path: string)
				Workspace.load(w, path)
			end,
			on_rename = function(from: string, to: string): string?
				return Workspace.rename(w, from, to)
			end,
			on_move = function(paths: { string }, into: string): string?
				return Workspace.move(w, paths, into)
			end,
			on_create = function(path: string, dir: boolean): string?
				return Workspace.create(w, path, dir)
			end,
			on_delete = function(paths: { string }, confirmed: boolean): string?
				return Workspace.delete(w, paths, confirmed)
			end,
			on_reveal = reveal,
			openable = here,
			more_menu = more_menu :: any,
			-- (In a panel's tab: a file of its own tab beside this one.)
			on_open_new = props.on_open_in_new_tab,
			-- Carried with the chat's next message (the chat page's, if it's
			-- there).
			on_add_to_chat = props.on_add_to_chat or add_to_chat,
		},
		viewer = {
			id = "file-viewer",
			-- (What's under it, for plugins: the open file, as a token.)
			provide = {
				file = ui.computed(function(): string?
					local d = v.doc:get()
					return if d then token.file(d.path) else nil
				end),
			},
			doc = v.doc,
			drafts = w.drafts,
			places = w.places,
			root = w.name,
			limit = LIMIT,
			hooks = { actions = "files.header.actions" },
			on_save = function(path: string, text: string)
				Workspace.save(v, path, text)
			end,
			-- Settings → Files: wrapping to begin with, and saving on its own.
			wrap_default = function(): boolean
				return prefs.word_wrap:get()
			end,
			autosave = function(): number?
				return if prefs.autosave:get() then prefs.autosave_delay:get() else nil
			end,
			-- Review comments on the code's lines, staged on the chat's
			-- composer (screens/files/comments): a + in the gutter starts one.
			blocks = function(): { any }
				local d = v.doc:get()
				if not d then
					return {}
				end
				local out: { any } = {}
				local writing = draft:get()
				for _, c in comments.of(w.target.root, d.path) do
					if not (writing and writing.editing == c.id) then
						table.insert(out, { key = c.id, line = c.line, kind = "comment", body = c.body })
					end
				end
				if writing and writing.path == d.path then
					table.insert(out, { key = "draft", line = writing.line, kind = "draft" })
				end
				return out
			end,
			block_view = function(b: any, c: any): Node
				local d = v.doc:peek()
				local path = if d then d.path else ""
				if b.kind == "draft" then
					return ds.InlineComposer {
						inset = c.inset,
						joined = c.joined,
						reveal = c.reveal,
						location = `{path}:{b.line}`,
						value = draft_text,
						placeholder = ui.t("files.comment.placeholder") :: any,
						submit = ui.t("files.comment.submit") :: any,
						on_cancel = function()
							draft:set(nil)
							draft_text:set("")
						end,
						on_submit = function(text: string)
							local body = string.match(text, "^%s*(.-)%s*$") or ""
							local writing = draft:peek()
							if body ~= "" and writing then
								if writing.editing then
									comments.edit(writing.editing, body)
								else
									comments.add({ path = writing.path, line = writing.line, body = body, root = w.target.root })
								end
							end
							draft:set(nil)
							draft_text:set("")
						end,
					}
				end
				return ds.InlineBlock {
					inset = c.inset,
					joined = c.joined,
					ds.CommentCard {
						-- (A string: the card says it in its name.)
						-- zgx-lint: allow(untracked-get)
						author = { id = "me", name = ui.t("files.comment.you"):get() },
						-- (A draft that goes with your next message: no time,
						-- and nothing to call it pending, every one is.)
						body = b.body,
						mine = true,
						compact = true,
						on_edit = function()
							draft_text:set(b.body)
							draft:set({ path = path, line = b.line, editing = b.key })
						end,
						on_delete = function()
							comments.remove(b.key)
						end,
					},
				}
			end,
			on_add_block = if comments.available()
				then function(line: number)
					local d = v.doc:peek()
					if d then
						draft_text:set("")
						draft:set({ path = d.path, line = line })
					end
				end
				else nil,
			on_block_moved = function(key: string, line: number)
				local writing = draft:peek()
				if key == "draft" and writing then
					local moved = table.clone(writing)
					moved.line = line
					draft:set(moved)
				else
					comments.move(key, line)
				end
			end,
			-- A Markdown preview's pictures (the folder's files) and links to
			-- its files: one opens here, a folder shows in the tree.
			picture = function(path: string, src: string): string?
				return Workspace.picture(w, path, src)
			end,
			on_link = function(path: string, target: string)
				local to = Workspace.relative(path, target)
				if not to or to == path then
					return
				end
				local id = w.model.by_path[to]
				if id and w.model.nodes[id].dir then
					v.reveal:set(to)
				else
					v.active:set(to)
				end
			end,
			reveal_line = reveal_line,
			escape_hint = props.escape_hint,
			-- Settings' code font size (nil until Settings has one: the
			-- design system's).
			code_size = function(): number?
				local size = (prefs :: any).code_font_size
				return if size then size:get() else nil
			end,
			on_dirty = function(path: string, on: boolean)
				w.dirty[path] = if on then true else nil
				local open = v.doc:peek()
				if open and open.path == path and v.dirty:peek() ~= on then
					v.dirty:set(on)
				end
			end,
			on_crumb = function(dir: string)
				v.reveal:set(if dir == "" then nil else dir)
			end,
			on_retry = function(path: string)
				Workspace.open(v, path)
			end,
			on_discard = function(path: string)
				Workspace.discard(v, path)
			end,
			on_reveal = reveal,
			on_open_default = open_external,
			on_open_editor = open_external,
			openable = here,
			more_menu = more_menu :: any,
		},
	}
	return ui.column {
		w_full = true,
		h_full = true,
		min_h = 0,
		notice(w),
		-- Reading the folder failed: why, and a way on (a real swap, by
		-- which shows; reading again brings the pane).
		ui.presence(function(): string
			local s = w.status:get()
			return if s == "offline" or s == "failed" then s else "pane"
		end, function(kind: string): Node
			if kind == "pane" then
				return pane
			end
			return ds.EmptyState {
				icon = "folder",
				size = "sm",
				fill = true,
				title = (if kind == "offline" then ui.t("files.unreachable.title") else ui.t("files.unreadable.title")) :: any,
				body = (if kind == "offline"
					then ui.t("files.unreachable.body")
					else ui.t("files.unreadable.body")) :: any,
				action = {
					label = ui.t("common.try_again") :: any,
					on_click = function()
						w.status:set("loading")
						Workspace.load(w, "")
					end,
				},
			}
		end, { w_full = true, grow = 1, min_h = 0 }),
		DeleteQuestion { w = w },
	}
end

-- A session's files through a view (`props.view`, a tab's; else the
-- folder's own pane view).
local Folder = ui.component(function(props: Props): Node
	local project, session = props.project, props.session
	if project == nil and session == nil then
		-- Not told: where the shell is.
		local route = require("@app/screens/shell/route") :: any
		project, session = route.project, route.session
	end
	local here = ui.computed(function(): Where?
		return where_of(project, session)
	end)
	local function now(): Where?
		return here:get()
	end
	local box: { [any]: any } = {}
	for k, v in props do
		if k ~= "project" and k ~= "session" and k ~= "radius" and k ~= "children" and k ~= "view" and k ~= "on_open_in_new_tab" and k ~= "initial" and k ~= "open_elsewhere" and k ~= "on_add_to_chat" and k ~= "escape_hint" then
			box[k] = v
		end
	end
	-- (It fills the room it's given, as a pane does.)
	if box.w == nil and box.w_full == nil then
		box.w_full = true
	end
	if box.h == nil and box.h_full == nil and box.grow == nil then
		box.h_full = true
	end
	box.min_h = box.min_h or 0
	-- Another folder is another tree (ds/file_tree takes its model as it is
	-- made): keyed by the folder alone, never by what's in it.
	box[#box + 1] = ui.presence(function(): string
		local w = here:get()
		return if w then w.key else ""
	end, function(key: string): Node
		local w = here:peek()
		if key == "" or not w then
			return ds.EmptyState {
				icon = "folder",
				size = "sm",
				fill = true,
				title = ui.t("files.none.title") :: any,
				body = ui.t("files.none.body") :: any,
			}
		end
		-- (A pane that fails to build, a template's or a plugin's mistake,
		-- says so here: the window, and the edits kept, carry on.)
		local ws = Workspace.get(w.key, w.target, w.name)
		local v = Workspace.view(ws, props.view or `pane:{w.key}`)
		local ok, built = pcall(folder, ws, v, props, now)
		if ok then
			return built
		end
		print(`zeron-zgx: the files pane didn't build: {built}`)
		return ds.EmptyState {
			icon = "danger-triangle",
			size = "sm",
			fill = true,
			title = ui.t("files.broken.title") :: any,
			body = ui.t("files.broken.body") :: any,
		}
	end, { w_full = true, grow = 1, min_h = 0 })
	return ui.column(box)
end)

-- The files pane on its own (the session page's, before the panel).
M.Pane = Folder

-- One file tab of the panel: `view` its tab's id.
M.Tab = Folder

-- A file tab's file and whether it has unsaved edits (its title).
M.tab_file = Workspace.view_file
M.show_in_tab = Workspace.show_in_view
-- Once `path` shows in tab view `id`, its editor goes to `at`.
M.show_line = Workspace.show_line

-- A file tab closed: its view's slot goes back.
function M.close_tab(id: string)
	Workspace.release(id)
end

return M
