-- Its states are here, not in the pane (screens/files/init.luau, which a
-- template may replace and which reloads as it is saved): the file open,
-- unsaved edits, where each file's editor stood, the open folders, the
-- scroll and the filter all outlive the pane, so a pane made again comes
-- back as it was.
--
-- One folder's files as the pane shows them: the tree's model, read a
-- folder at a time as folders open (and found by name as the filter is
-- typed); the file open; and both kept true to the disk as the host says
-- what changed there. One per folder (a session's, or its project's), kept
-- while the app runs, so going back to a session finds its tree, its scroll
-- and its file where they were.
--
-- Everything that changes is a state the pane reads (`revision` for the
-- tree's model, `doc` for the file), so a change on disk reaches only what
-- shows it: the tree lays out again keeping its folds and scroll, and the
-- editor keeps its caret and scroll across a save.

local FileTree = require("@ds/file_tree")
local FileViewer = require("@ds/file_viewer")
local api = require("@app/screens/files/api")
local prefs = require("@app/screens/settings/prefs")

-- What the tree lists of a folder's entries: all of them with Settings →
-- Files' "Show hidden and ignored files" on; else none whose name starts
-- with a dot, and none the project's .gitignore leaves out.
local function shown(e: api.Entry): boolean
	return prefs.show_all_files:peek() or (string.sub(e.name, 1, 1) ~= "." and not e.ignored)
end

type Doc = FileViewer.Doc
type Target = api.Target

local M = {}

-- What a file was read as, for saving it back.
type Meta = { hash: string?, checkout: string?, encoding: string?, line_ending: string?, readonly: string? }
-- A line for the pane's notice: what went wrong, and a way on. Kept as
-- keys and words (state holds no messages), said where it's drawn: `what`
-- a message key said with the file's `name`; why, the engine's own words
-- (`why`) or ours (`why_key`).
export type Problem = { what: string, name: string?, why: string?, why_key: string?, retry: boolean? }

export type Workspace = {
	key: string,
	target: Target,
	name: string,
	model: FileTree.Model,
	-- Moves after the model changes (the tree lays out again).
	revision: State<number>,
	-- The top folder's first reading: "loading", "ready", "offline", "failed".
	status: State<string>,
	offset: State<number>,
	-- The tree's open folders, kept here so a tree made again opens as it was.
	folds: State<{ string }>,
	-- The pane's last width and whether its tree's drawer is out (narrow).
	pane_room: State<number>,
	drawer: State<boolean>,
	-- Unsaved edits by path, kept here so a viewer made again finds them.
	drafts: State<{ [string]: { text: string, base: string } }>,
	-- Where each file's editor stood (scroll and selection).
	places: State<{ [string]: { offset: number, anchor: number, focus: number } }>,
	filter: State<string>,
	problem: State<Problem?>,
	-- Paths waiting on the question before they're deleted.
	confirm: State<{ string }?>,
	-- Whether files can be renamed and deleted here (the host says).
	can_move: boolean,
	can_delete: boolean,
	-- Whether its files are on this Mac (Reveal and Open apply).
	here: State<boolean>,
	checkout: string?,
	info: { [string]: { rev: string?, kind: string } },
	meta: { [string]: Meta },
	dirty: { [string]: boolean },
	saving: { [string]: boolean },
	creating: { [string]: boolean },
	inflight: { [string]: boolean },
	again: { [string]: boolean },
	queued: { [string]: boolean },
	-- The latest re-read asked for: only its timer reads (a timer made in an
	-- effect run is dropped when the effect runs again, so nothing waits on a
	-- flag one would clear).
	flush_gen: number,
	-- Its views (file tabs) showing now, by id.
	views: { [string]: View },
	-- The last change frame read, and the host's sequence.
	seen: number,
	sequence: number?,
	-- A save to try again.
	retry: (() -> ())?,
	-- Whether its top folder has been read (in this run, or kept from one).
	listed: boolean,
	kept: State<Kept>,
}

-- A view of a folder: one file tab's file open (the tree, what's read and
-- the watch are the folder's, shared by its tabs). Its states come from a
-- pool made as the module loads, a tab given one the first time and keeping
-- it (`views` below), so a tab made again (a reload, a template) is where
-- it was.
export type View = {
	id: string,
	w: Workspace,
	active: State<string?>,
	reveal: State<string?>,
	doc: State<Doc?>,
	-- Whether its file has unsaved edits (the tab's dot).
	dirty: State<boolean>,
	opening: number,
}

local IMAGE = { png = true, jpg = true, jpeg = true, gif = true, webp = true }
local function ext(path: string): string
	return string.lower(string.match(path, "%.([^%./]+)$") or "")
end
local function parent_of(path: string): string
	return string.match(path, "^(.*)/[^/]*$") or ""
end
local base_name = require("@app/screens/files/paths").base_name
local function under(path: string, dir: string): boolean
	return path == dir or string.sub(path, 1, #dir + 1) == dir .. "/"
end
local function message(err: any): string
	return if type(err) == "table" and err.message then tostring(err.message) else tostring(err)
end
-- What a failure says on screen (a file's error line: a string, in the
-- language of the moment it failed).
local function said(err: any): string
	local m = message(err)
	if m == "offline" then
		return ui.t("files.workspace.offline"):get()
	end
	return m
end

-- Why, for a problem: words of ours (`{ ours = key }`), the engine's as
-- given (a string), or an error, read as `said` reads it.
local function why_of(p: Problem, err: any): Problem
	if type(err) == "table" and err.ours then
		p.why_key = err.ours
	elseif message(err) == "offline" then
		p.why_key = "files.workspace.offline"
	else
		p.why = message(err)
	end
	return p
end

-- A problem with `path`: what (a key), and why.
local function trouble(what: string, path: string, err: any, retry: boolean?): Problem
	return why_of({ what = what, name = base_name(path), retry = retry }, err)
end

local cache: { [string]: Workspace } = {}

type ViewSlot = { active: State<string?>, reveal: State<string?>, doc: State<Doc?>, dirty: State<boolean> }
local VIEW_SLOTS = 48
local view_slots: { ViewSlot } = {}
for i = 1, VIEW_SLOTS do
	view_slots[i] = {
		active = ui.state(`view{i}.active`, nil :: string?),
		reveal = ui.state(`view{i}.reveal`, nil :: string?),
		doc = ui.state(`view{i}.doc`, nil :: Doc?),
		dirty = ui.state(`view{i}.dirty`, false),
	}
end
-- Which view (tab) has which slot.
local view_assigned = ui.state("views", {} :: { [string]: number })
local view_cache: { [string]: View } = {}
-- When each view was last asked for (`uses` counts both kinds of slot).
local view_used: { [string]: number } = {}

-- The states, made as the module loads, never as a pane is built (a state
-- made while a pane is built again, as a reload or a template does, left
-- that pane's timers dead): a fixed set of slots, a folder given one the
-- first time it shows and keeping it (`assigned`, which a reload keeps
-- too). A folder long unseen gives its slot up, unless it holds unsaved
-- edits.
type Slot = {
	revision: State<number>,
	status: State<string>,
	offset: State<number>,
	folds: State<{ string }>,
	pane_room: State<number>,
	drawer: State<boolean>,
	filter: State<string>,
	problem: State<Problem?>,
	confirm: State<{ string }?>,
	here: State<boolean>,
	drafts: State<{ [string]: { text: string, base: string } }>,
	places: State<{ [string]: { offset: number, anchor: number, focus: number } }>,
	-- What was read, kept so a workspace made again (a reload, a template)
	-- shows at once from it: each folder's entries ("." the top), each
	-- file's read facts, the top's, and where the watch was read to.
	kept: State<Kept>,
}
export type Kept = {
	listings: { [string]: { api.Entry } },
	meta: { [string]: Meta },
	top: { checkout: string?, can_move: boolean?, can_delete: boolean?, listed: boolean? },
	seen: number,
	sequence: number?,
}
local function no_kept(): Kept
	return { listings = {}, meta = {}, top = {}, seen = 0 }
end
local SLOTS = 24
local slots: { Slot } = {}
for i = 1, SLOTS do
	slots[i] = {
		revision = ui.state(`slot{i}.revision`, 0),
		status = ui.state(`slot{i}.status`, "loading"),
		offset = ui.state(`slot{i}.offset`, 0),
		folds = ui.state(`slot{i}.folds`, {} :: { string }),
		pane_room = ui.state(`slot{i}.pane_room`, 0),
		drawer = ui.state(`slot{i}.drawer`, false),
		filter = ui.state(`slot{i}.filter`, ""),
		problem = ui.state(`slot{i}.problem`, nil :: Problem?),
		confirm = ui.state(`slot{i}.confirm`, nil :: { string }?),
		here = ui.state(`slot{i}.here`, false),
		drafts = ui.state(`slot{i}.drafts`, {} :: { [string]: { text: string, base: string } }),
		places = ui.state(`slot{i}.places`, {} :: { [string]: { offset: number, anchor: number, focus: number } }),
		kept = ui.state(`slot{i}.kept`, no_kept()),
	}
end
-- Which folder has which slot, and when each was last asked for.
local assigned = ui.state("slots", {} :: { [string]: number })
local last_used: { [number]: number } = {}
local uses = 0

local function clear(slot: Slot)
	ui.batch(function()
		slot.status:set("loading")
		slot.offset:set(0)
		slot.folds:set({})
		slot.drawer:set(false)
		slot.filter:set("")
		slot.problem:set(nil)
		slot.confirm:set(nil)
		slot.here:set(false)
		slot.drafts:set({})
		slot.places:set({})
		slot.kept:set(no_kept())
	end)
end

local function slot_for(key: string): Slot
	uses += 1
	local map = assigned:peek()
	local i = map[key]
	if not i then
		local taken: { [number]: string } = {}
		for k, n in map do
			taken[n] = k
		end
		-- A free one; else the longest unseen without unsaved edits.
		for n = 1, SLOTS do
			if not taken[n] then
				i = n
				break
			end
		end
		if not i then
			-- The longest unseen: first of those without unsaved edits that
			-- no view shows, then of those without unsaved edits; only if
			-- every one has edits, the longest unseen of all.
			local function oldest(ok: (n: number) -> boolean): number?
				local best: number? = nil
				for n = 1, SLOTS do
					if ok(n) and (not best or (last_used[n] or 0) < (last_used[best] or 0)) then
						best = n
					end
				end
				return best
			end
			local function clean(n: number): boolean
				return next(slots[n].drafts:peek()) == nil
			end
			local function unseen(n: number): boolean
				local old = cache[taken[n]]
				return old == nil or next(old.views) == nil
			end
			i = oldest(function(n: number): boolean
				return clean(n) and unseen(n)
			end) or oldest(clean) or oldest(function(): boolean
				return true
			end)
			-- Its folder let go: made again from nothing if it's asked for
			-- again, its views with it.
			local old_key = taken[i :: number]
			local old = cache[old_key]
			if old then
				for id in old.views do
					view_cache[id] = nil
				end
			end
			assigned:set_key(old_key, nil)
			cache[old_key] = nil
		end
		clear(slots[i :: number])
		assigned:set_key(key, i)
	end
	last_used[i :: number] = uses
	return slots[i :: number]
end

-- The workspace for `key`, made the first time (`target` says where it is;
-- a later session in the same folder brings its own, used from then on).
function M.get(key: string, target: Target, name: string): Workspace
	local w = cache[key]
	if w then
		w.target = target
		return w
	end
	local slot = slot_for(key)
	local made: Workspace = {
		key = key,
		target = target,
		name = name,
		model = FileTree.model(name, target.root),
		revision = slot.revision,
		status = slot.status,
		offset = slot.offset,
		folds = slot.folds,
		pane_room = slot.pane_room,
		drawer = slot.drawer,
		filter = slot.filter,
		problem = slot.problem,
		confirm = slot.confirm,
		drafts = slot.drafts,
		places = slot.places,
		can_move = false,
		can_delete = false,
		here = slot.here,
		checkout = nil,
		info = {},
		meta = {},
		dirty = {},
		saving = {},
		creating = {},
		inflight = {},
		again = {},
		queued = {},
		flush_gen = 0,
		views = {},
		seen = 0,
		sequence = nil,
		retry = nil,
		listed = false,
		kept = slot.kept,
	}
	-- (A reload of the scripts keeps the states but not the model: it is
	-- made again here from what was kept, top folder first, so the tree
	-- shows at once and nothing is read again unless it changed.)
	local kept = slot.kept:peek()
	local dirs: { string } = {}
	for d in kept.listings do
		table.insert(dirs, d)
	end
	local function depth(d: string): number
		if d == "." then
			return -1
		end
		local _, n = string.gsub(d, "/", "")
		return n
	end
	table.sort(dirs, function(a: string, b: string): boolean
		local da, db = depth(a), depth(b)
		if da ~= db then
			return da < db
		end
		return a < b
	end)
	for _, d in dirs do
		local listed: { FileTree.Listed } = {}
		for _, e in kept.listings[d] do
			if shown(e) then
				table.insert(listed, { name = e.name, dir = e.dir, size = e.size, ignored = e.ignored })
			end
			made.info[e.path] = { rev = e.rev, kind = e.kind }
		end
		FileTree.sync(made.model, if d == "." then "" else d, listed)
	end
	for path, m in kept.meta do
		made.meta[path] = table.clone(m)
	end
	made.checkout = kept.top.checkout
	made.can_move = kept.top.can_move == true
	made.can_delete = kept.top.can_delete == true
	made.listed = kept.top.listed == true
	made.seen = kept.seen or 0
	made.sequence = kept.sequence
	cache[key] = made
	return made
end

-- The view `id` of `w` (a file tab's), made the first time; given its slot
-- for as long as the tab is open (`release`).
function M.view(w: Workspace, id: string): View
	uses += 1
	view_used[id] = uses
	local v = view_cache[id]
	if v and v.w == w then
		w.views[id] = v
		return v
	end
	local map = view_assigned:peek()
	local i = map[id]
	if not i then
		local taken: { [number]: boolean } = {}
		for _, n in map do
			taken[n] = true
		end
		for n = 1, VIEW_SLOTS do
			if not taken[n] then
				i = n
				break
			end
		end
		-- All taken: the longest unused view without unsaved edits lets its
		-- slot go (else the longest unused of all), rather than two views
		-- sharing one.
		if not i then
			local victim: string? = nil
			local function consider(clean_only: boolean)
				for vid, n in map do
					local ok = not clean_only or not view_slots[n].dirty:peek()
					if ok and vid ~= id and (not victim or (view_used[vid] or 0) < (view_used[victim] or 0)) then
						victim = vid
					end
				end
			end
			consider(true)
			if not victim then
				consider(false)
			end
			i = if victim then map[victim] else 1
			if victim then
				M.release(victim)
			end
		end
		local slot = view_slots[i :: number]
		ui.batch(function()
			slot.active:set(nil)
			slot.reveal:set(nil)
			slot.doc:set(nil)
			slot.dirty:set(false)
		end)
		view_assigned:set_key(id, i)
	end
	local slot = view_slots[i :: number]
	local made: View = {
		id = id,
		w = w,
		active = slot.active,
		reveal = slot.reveal,
		doc = slot.doc,
		dirty = slot.dirty,
		opening = 0,
	}
	if v then
		v.w.views[id] = nil
	end
	view_cache[id] = made
	w.views[id] = made
	return made
end

-- A view's tab closed: its slot goes back to the pool.
function M.release(id: string)
	local v = view_cache[id]
	if v then
		v.w.views[id] = nil
		view_cache[id] = nil
	end
	view_assigned:set_key(id, nil)
	view_used[id] = nil
end

-- The view `id`'s file now, if it has one (a tab's title).
function M.view_file(id: string): (string?, boolean)
	local i = view_assigned:get()[id]
	if not i then
		return nil, false
	end
	local slot = view_slots[i]
	return slot.active:get(), slot.dirty:get()
end

-- Shows `path` in tab view `id` as a pick in its tree would (its rules
-- apply: a clean file is replaced; one with unsaved edits stays, the next
-- going elsewhere). Whether that view is there.
function M.show_in_view(id: string, path: string): boolean
	local i = view_assigned:peek()[id]
	if not i then
		return false
	end
	view_slots[i].active:set(path)
	return true
end

-- A link or picture in the document at `document`, as a file of its
-- folder: its path from the top and its `#anchor`, or nil for one that
-- isn't (the web's, an absolute path, past the top). The shipping app's
-- `relative_target` (files/markdown_preview.rs).
function M.relative(document: string, target: string): (string?, string?)
	if string.sub(target, 1, 11) == "zeron-file:" then
		return M.relative("", string.sub(target, 12))
	end
	if string.sub(target, 1, 1) == "/" or string.find(target, "[:\\]") then
		return nil, nil
	end
	local function decode(t: string): string?
		local bad = false
		local out = string.gsub(t, "%%(..)", function(hex: string): string
			local n = tonumber(hex, 16)
			if not n then
				bad = true
				return ""
			end
			return string.char(n)
		end)
		return if bad then nil else out
	end
	local raw, anchor_raw = string.match(target, "^([^#]*)#(.*)$")
	local path = decode(raw or target)
	local anchor = if anchor_raw then decode(anchor_raw) else nil
	if not path or string.find(path, "[\\:%z]") or string.sub(path, 1, 1) == "/" then
		return nil, nil
	end
	if path == "" then
		return document, anchor
	end
	local parts = string.split(document, "/")
	table.remove(parts)
	for _, part in string.split(path, "/") do
		if part == ".." then
			if #parts == 0 then
				return nil, nil
			end
			table.remove(parts)
		elseif part ~= "" and part ~= "." then
			table.insert(parts, part)
		end
	end
	if #parts == 0 then
		return nil, nil
	end
	return table.concat(parts, "/"), anchor
end

-- Pictures of documents on other devices, read from there: by folder and
-- path, the file they're kept in here, or false for one that can't come.
local pictures = ui.state("files_pictures", {} :: { [string]: string | false })
local PICTURES = 32
local reading: { [string]: boolean } = {}
local function count_of(t: { [string]: any }): number
	local n = 0
	for _ in t do
		n += 1
	end
	return n
end

-- What a picture in the document at `document` shows from (`src` as
-- written): the file, on this Mac; otherwise read through the engine first
-- (nil until it's here, and for one that isn't a picture of the folder's,
-- or can't come). Reactive.
function M.picture(w: Workspace, document: string, src: string): string?
	local path = M.relative(document, src)
	if not path or not IMAGE[ext(path)] then
		return nil
	end
	-- On this Mac, a file the tree knows shows from where it is. (One it
	-- doesn't, perhaps not there at all, is asked of the engine: a missing
	-- file is its answer, not the image's error.) `absolute`'s path, which
	-- comes later.
	w.revision:get()
	local id = w.model.by_path[path]
	if w.here:get() and w.target.root and id and not w.model.nodes[id].dir then
		return `{w.target.root}/{path}`
	end
	local key = `{w.key}\0{path}`
	local have = pictures:get()[key]
	if have ~= nil then
		return have or nil
	end
	if not reading[key] and count_of(pictures:peek()) + count_of(reading) < PICTURES then
		-- (Read once however often it's asked for meanwhile; the answer
		-- kept outside this reading of it.)
		reading[key] = true
		api.image(w.target, path, w.checkout or ""):after(function(r: any)
			reading[key] = nil
			pictures:set_key(key, r.src)
		end, function()
			reading[key] = nil
			pictures:set_key(key, false)
		end)
	end
	return nil
end

-- Lines to go to, by view: once `path` shows in view `id`, its editor goes
-- to `at` (a line, or `{ line, column?, end_line? }`).
M.lines = ui.state("files_lines", {} :: { [string]: { path: string, at: any } })

function M.show_line(id: string, path: string, at: any)
	M.lines:set_key(id, { path = path, at = at })
end

-- Folder `w`'s git statuses as the host has them now, `{ [path] = status }`.
function M.git_of(w: Workspace): { [string]: string }
	return (api.git:peek() :: any)[w.key] or {}
end

local function bump_first(w: Workspace)
	w.revision:set(w.revision:peek() + 1)
end

-- Git's statuses, newly heard: the tree's marks, and each open file's in
-- its header.
function M.apply_git(w: Workspace)
	local map = M.git_of(w)
	if FileTree.statuses(w.model, map) then
		bump_first(w)
	end
	for _, v in w.views do
		local d = v.doc:peek()
		if d and d.path and (d :: any).status ~= map[d.path] then
			v.doc:set_key("status", map[d.path] :: any)
		end
	end
end

local function bump(w: Workspace)
	w.revision:set(w.revision:peek() + 1)
end

local function absolute(w: Workspace, path: string): string?
	return if w.target.root then `{w.target.root}/{path}` else nil
end

-- A folder's entries, read again ("" the top). Readings of one folder never
-- overlap: one asked for while another runs follows it.
function M.load(w: Workspace, dir: string)
	if w.inflight[dir] then
		w.again[dir] = true
		return
	end
	w.inflight[dir] = true
	api.list(w.target, dir):after(function(listing: api.Listing)
		w.inflight[dir] = nil
		if dir == "" then
			w.listed = true
			w.checkout = listing.checkout
			w.can_move = listing.can_move
			w.can_delete = listing.can_delete
			if w.here:peek() ~= (listing.here == true) then
				w.here:set(listing.here == true)
			end
		end
		local entries: { FileTree.Listed } = {}
		for _, e in listing.entries do
			if shown(e) then
				table.insert(entries, { name = e.name, dir = e.dir, size = e.size, ignored = e.ignored })
			end
			w.info[e.path] = { rev = e.rev, kind = e.kind }
		end
		local synced = FileTree.sync(w.model, dir, entries)
		-- (What it read marked as git has it.)
		local marked = FileTree.statuses(w.model, M.git_of(w))
		if synced or marked or dir == "" then
			bump(w)
		end
		w.kept:set_key({ "listings", if dir == "" then "." else dir }, listing.entries)
		if dir == "" then
			w.kept:set_key("top", { checkout = listing.checkout, can_move = listing.can_move, can_delete = listing.can_delete, listed = true })
		end
		if dir == "" and w.status:peek() ~= "ready" then
			w.status:set("ready")
		end
		if w.again[dir] then
			w.again[dir] = nil
			M.load(w, dir)
		end
	end, function(err: any)
		w.inflight[dir] = nil
		if dir == "" and w.status:peek() ~= "ready" then
			w.status:set(if message(err) == "offline" then "offline" else "failed")
		end
	end)
end

-- Reads what's queued a beat from now: the latest one asked for does, the
-- others (superseded) nothing.
local function arm_flush(w: Workspace)
	w.flush_gen += 1
	local gen = w.flush_gen
	ui.after(40, function()
		if w.flush_gen ~= gen then
			return
		end
		local dirs = w.queued
		w.queued = {}
		for d in dirs do
			M.load(w, d)
		end
	end)
end

-- Folders to read again, a beat later and once each however many changes
-- named them.
local function queue(w: Workspace, dir: string)
	if dir ~= "" then
		local id = w.model.by_path[dir]
		-- (A folder not read yet, or not here, is read as it opens.)
		if not id or w.model.nodes[id].loaded == false then
			return
		end
	end
	w.queued[dir] = true
	arm_flush(w)
end

-- A file's read facts (for saving it back), kept for a workspace made again.
local function set_meta(w: Workspace, path: string, m: Meta)
	w.meta[path] = m
	w.kept:set_key({ "meta", path }, table.clone(m))
end

-- The open file as read: what the viewer shows, and how to save it back.
-- (An image on another device shows loading: `show` reads its bytes.)
local function doc_from(w: Workspace, path: string, f: api.File): Doc
	set_meta(w, path, { hash = f.hash, checkout = f.checkout, encoding = f.encoding, line_ending = f.line_ending, readonly = f.readonly })
	local d: Doc = { path = path, state = "ready", size = f.size, absolute = absolute(w, path), status = M.git_of(w)[path] :: any }
	if IMAGE[ext(path)] then
		if f.here and w.target.root then
			d.image = absolute(w, path)
		else
			d.state = "loading"
		end
	elseif f.encoding == "binary" then
		d.state = "binary"
	elseif f.readonly == "tooLarge" or f.truncated then
		d.state = "large"
	elseif f.text == nil then
		d.state = "error"
		d.error = ui.t("files.workspace.unreadable_text"):get()
	else
		d.text = f.text
	end
	return d
end

-- What view `v` shows of `path` as read (`f`), if it's still the one asked
-- for (`mine`); an image on another device, once its bytes are here.
local function show(v: View, path: string, f: api.File, mine: number, conflict: boolean?)
	local w = v.w
	local d = doc_from(w, path, f)
	if conflict then
		d.conflict = true
	end
	v.doc:set(d)
	if not (IMAGE[ext(path)] and d.state == "loading") then
		return
	end
	local function still(): Doc?
		local now = v.doc:peek()
		if mine ~= v.opening or not now or now.path ~= path then
			return nil
		end
		return now
	end
	api.image(w.target, path, f.checkout or ""):after(function(r: any)
		local now = still()
		if now then
			local ready = table.clone(now)
			ready.state = "ready"
			ready.image = r.src
			v.doc:set(ready)
		end
	end, function(err: any)
		local now = still()
		if now then
			local failed = table.clone(now)
			failed.state = "error"
			failed.error = said(err)
			v.doc:set(failed)
		end
	end)
end

-- Opens `path` in view `v` (nil: nothing open).
function M.open(v: View, path: string?)
	local w = v.w
	v.opening += 1
	local mine = v.opening
	if not path then
		v.doc:set(nil)
		return
	end
	local id = w.model.by_path[path]
	local size = if id then w.model.nodes[id].size else nil
	v.doc:set({ path = path, state = "loading", size = size, absolute = absolute(w, path) })
	-- (One being made opens once it is.)
	if w.creating[path] then
		return
	end
	api.read(w.target, path):after(function(f: api.File)
		if mine == v.opening then
			show(v, path, f, mine)
		end
	end, function(err: any)
		if mine ~= v.opening then
			return
		end
		local m = string.lower(message(err))
		local gone = string.find(m, "not found", 1, true) ~= nil or string.find(m, "no such", 1, true) ~= nil
		v.doc:set({
			path = path,
			state = if gone then "gone" else "error",
			error = if gone then nil else said(err),
			absolute = absolute(w, path),
		})
	end)
end

-- A view's open file read again after it changed on disk. Unchanged (the
-- save just made, heard back), nothing happens; changed, the viewer follows
-- it, or asks which to keep if it has edits. `conflict`: a save just found
-- it changed, so the edits are kept and it asks.
local function reread(v: View, path: string, conflict: boolean?)
	local w = v.w
	local mine = v.opening
	api.read(w.target, path):after(function(f: api.File)
		local d = v.doc:peek()
		if mine ~= v.opening or not d or d.path ~= path then
			return
		end
		local meta = w.meta[path]
		if not conflict and meta and meta.hash and meta.hash == f.hash and d.state == "ready" then
			return
		end
		show(v, path, f, mine, conflict)
	end, function() end)
end

-- Saves a view's open file. One deleted on disk is made again first.
function M.save(v: View, path: string, text: string)
	local w = v.w
	local d0 = v.doc:peek()
	if d0 and d0.path == path and d0.deleted then
		api.create(w.target, path, false, w.checkout or ""):after(function(r: any)
			if r.status ~= "applied" and r.reason ~= "destinationExists" then
				w.problem:set(trouble("files.workspace.not_remade", path, r.message or ""))
				return
			end
			-- (Empty now: read for the hash a save is checked against.)
			api.read(w.target, path):after(function(f: api.File)
				set_meta(w, path, { hash = f.hash, checkout = f.checkout, encoding = f.encoding, line_ending = f.line_ending })
				local d = v.doc:peek()
				if d and d.path == path then
					-- (Its text the one being saved: the editor holds it.)
					local next_doc = table.clone(d)
					next_doc.deleted = nil
					next_doc.state = "ready"
					next_doc.text = text
					v.doc:set(next_doc)
				end
				M.save(v, path, text)
			end, function(err: any)
				w.problem:set(trouble("files.workspace.not_saved", path, err))
			end)
		end, function(err: any)
			w.problem:set(trouble("files.workspace.not_remade", path, err))
		end)
		return
	end
	local meta = w.meta[path]
	if not meta or not meta.hash then
		w.problem:set(trouble("files.workspace.not_saved", path, { ours = "files.workspace.not_read_yet" }))
		return
	end
	if meta.readonly then
		w.problem:set(trouble("files.workspace.cant_save_here", path, { ours = "files.workspace.read_only" }))
		return
	end
	w.saving[path] = true
	w.retry = nil
	api.write(w.target, {
		path = path,
		text = text,
		hash = meta.hash :: string,
		checkout = meta.checkout or w.checkout or "",
		encoding = meta.encoding,
		line_ending = meta.line_ending,
	}):after(function(r: any)
		w.saving[path] = nil
		if r.status == "written" then
			meta.hash = r.hash
			set_meta(w, path, meta)
			-- In place, in every view showing it: the same file, its text as
			-- saved. The editor already holds it, so nothing in it moves.
			for _, o in w.views do
				local d = o.doc:peek()
				if d and d.path == path and d.state == "ready" then
					local next_doc = table.clone(d)
					next_doc.text = text
					next_doc.size = r.size or #text
					next_doc.conflict = nil
					o.doc:set(next_doc)
				end
			end
			local p = w.problem:peek()
			if p and p.retry then
				w.problem:set(nil)
			end
		elseif r.reason == "deleted" then
			M.mark_deleted(v, path)
		else
			reread(v, path, true)
		end
	end, function(err: any)
		w.saving[path] = nil
		w.retry = function()
			M.save(v, path, text)
		end
		w.problem:set(trouble("files.workspace.not_saved", path, err, true))
	end)
end

-- Every folder read, and each view's file, read again: after a gap in what
-- the host said, or a reconnection.
function M.resync(w: Workspace)
	queue(w, "")
	for _, n in w.model.nodes do
		if n.dir and n.parent >= 0 and n.loaded ~= false then
			queue(w, n.path)
		end
	end
	for _, v in w.views do
		local d = v.doc:peek()
		if d and d.state == "ready" then
			reread(v, d.path)
		end
	end
end

-- What the host said changed on disk.
function M.apply(w: Workspace, frame: api.Frame)
	if frame.resync or (w.sequence and frame.sequence and frame.sequence ~= w.sequence + 1) then
		w.sequence = frame.sequence
		M.resync(w)
		return
	end
	w.sequence = frame.sequence or w.sequence
	for _, c in frame.changes do
		queue(w, parent_of(c.path))
		if c.old_path then
			queue(w, parent_of(c.old_path))
		end
		for _, v in w.views do
			local d = v.doc:peek()
			if d then
				if c.kind == "renamed" and c.old_path and under(d.path, c.old_path) then
					-- The open file moved: it follows (the tree shows it there).
					v.active:set(c.path .. string.sub(d.path, #c.old_path + 1))
				elseif c.kind == "removed" and under(d.path, c.path) then
					if w.dirty[d.path] and d.state == "ready" then
						M.mark_deleted(v, d.path)
					else
						v.doc:set({ path = d.path, state = "gone", absolute = d.absolute })
					end
				elseif (c.kind == "modified" or c.kind == "created" or c.kind == "renamed") and c.path == d.path and not w.saving[d.path] then
					reread(v, d.path)
				end
			end
		end
	end
end

-- Reads the frames past the last one read for this workspace.
function M.drain(w: Workspace, events: api.Events)
	local next_n = events[w.key]
	if not next_n or next_n - 1 == w.seen then
		return
	end
	if next_n - 1 < w.seen then
		-- (The host counts again from the start: another run. Start over.)
		w.seen = next_n - 1
		M.resync(w)
		return
	end
	local frames = api.frames(w.key, w.seen)
	if #frames > 0 and w.seen > 0 and frames[1].n > w.seen + 1 then
		-- (Missed some: start over.)
		w.seen = frames[#frames].n
		M.resync(w)
		return
	end
	local was_seen, was_sequence = w.seen, w.sequence
	for _, f in frames do
		if f.n > w.seen then
			w.seen = f.n
			if f.key == w.key then
				M.apply(w, f)
			end
		end
	end
	-- (Where it was read to, kept: a workspace made again goes on from there.)
	if w.seen ~= was_seen or w.sequence ~= was_sequence then
		w.kept:patch({ seen = w.seen, sequence = w.sequence })
	end
	-- (Still to read: armed again from this run, as an earlier run's timer
	-- went with it.)
	if next(w.queued) ~= nil then
		arm_flush(w)
	end
end

-- Follows the disk from now on (until the returned function stops it).
-- The host keeps a watch let go of for a moment (a reload asks again
-- within it), and says whether it was kept: then every change is in its
-- frames, read on from where this was. Otherwise what was kept may be
-- stale, and is read again.
function M.watch(w: Workspace): () -> ()
	if w.seen == 0 then
		w.seen = ((api.events:peek() :: any)[w.key] or 1) - 1
	end
	api.watch(w.key, w.target):after(function(r: any)
		if not (r and r.kept) then
			w.sequence = nil
			if w.listed then
				M.resync(w)
			end
		end
	end, function() end)
	return function()
		api.unwatch(w.key)
	end
end

-- Names the host found for the filter, in folders not read yet: placed
-- in the tree (made unread, so they're read as they open).
function M.place_found(w: Workspace, found: { { path: string, dir: boolean } })
	local added = false
	for _, f in found do
		if not w.model.by_path[f.path] then
			FileTree.place(w.model, f.path, f.dir)
			added = true
		end
	end
	if added then
		bump(w)
	end
end

-- The tree's changes, sent on as they're made (the tree shows them at
-- once; a refusal reads the folders again, putting them back).

-- (`what` a message key, said with `path`'s name.)
local function refused(w: Workspace, what: string, path: string, err: any, dirs: { string })
	w.problem:set(trouble(what, path, err))
	for _, d in dirs do
		queue(w, d)
	end
end

local function move_one(w: Workspace, from: string, to: string)
	local info = w.info[from]
	if not info or not info.rev then
		refused(w, "files.workspace.not_moved", from, { ours = "files.workspace.changed" }, { parent_of(from), parent_of(to) })
		return
	end
	api.move(w.target, { from = from, to = to, rev = info.rev :: string, kind = info.kind, checkout = w.checkout or "" }):after(function(r: any)
		if r.status ~= "applied" then
			refused(w, "files.workspace.not_moved", from, r.message or "", { parent_of(from), parent_of(to) })
		end
	end, function(err: any)
		refused(w, "files.workspace.not_moved", from, err, { parent_of(from), parent_of(to) })
	end)
end

function M.rename(w: Workspace, from: string, to: string): string?
	if not w.can_move then
		return ui.t("files.workspace.cant_rename"):get()
	end
	move_one(w, from, to)
	return nil
end

function M.move(w: Workspace, paths: { string }, into: string): string?
	if not w.can_move then
		return ui.t("files.workspace.cant_move"):get()
	end
	for _, p in paths do
		move_one(w, p, if into == "" then base_name(p) else `{into}/{base_name(p)}`)
	end
	return nil
end

function M.create(w: Workspace, path: string, dir: boolean): string?
	w.creating[path] = true
	api.create(w.target, path, dir, w.checkout or ""):after(function(r: any)
		w.creating[path] = nil
		if r and r.status == "rejected" then
			refused(w, "files.workspace.not_made", path, r.message or "", { parent_of(path) })
			for _, v in w.views do
				if v.active:peek() == path then
					v.active:set(nil)
				end
			end
			return
		end
		queue(w, parent_of(path))
		for _, v in w.views do
			if v.active:peek() == path then
				M.open(v, path)
			end
		end
	end, function(err: any)
		w.creating[path] = nil
		refused(w, "files.workspace.not_made", path, err, { parent_of(path) })
		for _, v in w.views do
			if v.active:peek() == path then
				v.active:set(nil)
			end
		end
	end)
	return nil
end

-- Deleting asks first (`confirmed` when the row menu has asked already):
-- the files go for good, not to the Trash.
function M.delete(w: Workspace, paths: { string }, confirmed: boolean): string?
	if not w.can_delete then
		return ui.t("files.workspace.cant_delete"):get()
	end
	if not confirmed then
		w.confirm:set(table.clone(paths))
		return ""
	end
	for _, p in paths do
		local info = w.info[p]
		if info and info.rev then
			api.delete(w.target, {
				path = p,
				rev = info.rev :: string,
				kind = info.kind,
				checkout = w.checkout or "",
				recursive = info.kind == "directory",
			}):after(function(r: any)
				if r.status ~= "applied" then
					refused(w, "files.workspace.not_deleted", p, r.message or "", { parent_of(p) })
				end
			end, function(err: any)
				refused(w, "files.workspace.not_deleted", p, err, { parent_of(p) })
			end)
		else
			refused(w, "files.workspace.not_deleted", p, { ours = "files.workspace.changed" }, { parent_of(p) })
		end
	end
	return nil
end

-- The question answered: they go (from the tree at once).
function M.confirm_delete(w: Workspace)
	local paths = w.confirm:peek()
	w.confirm:set(nil)
	if not paths then
		return
	end
	M.delete(w, paths, true)
	local changed = false
	for _, p in paths do
		changed = FileTree.drop(w.model, p) or changed
	end
	if changed then
		bump(w)
	end
end

-- A view's file, deleted on disk with unsaved edits: they stay, and the
-- viewer asks whether to save them (making it again) or let them go.
function M.mark_deleted(v: View, path: string)
	local d = v.doc:peek()
	if not d or d.path ~= path then
		return
	end
	local next_doc = table.clone(d)
	next_doc.deleted = true
	next_doc.conflict = nil
	v.doc:set(next_doc)
end

-- The edits of a file deleted on disk let go: it shows gone.
function M.discard(v: View, path: string)
	v.w.dirty[path] = nil
	v.dirty:set(false)
	local d = v.doc:peek()
	if d and d.path == path then
		v.doc:set({ path = path, state = "gone", absolute = d.absolute })
	end
end

-- A path the viewer or tree gives (absolute under the root, or relative),
-- relative to the root.
local function relative(w: Workspace, path: string): string
	local root = w.target.root
	if root and string.sub(path, 1, #root + 1) == root .. "/" then
		return string.sub(path, #root + 2)
	end
	return path
end

function M.reveal(w: Workspace, path: string)
	api.reveal(w.target, relative(w, path)):after(function() end, function(err: any)
		w.problem:set(trouble("files.workspace.not_revealed", path, err))
	end)
end

function M.open_external(w: Workspace, path: string)
	api.open_external(w.target, relative(w, path)):after(function() end, function(err: any)
		w.problem:set(trouble("files.workspace.not_opened", path, err))
	end)
end

M.base_name = base_name
M.parent_of = parent_of

-- For tests: forgets every workspace.
function M.forget()
	cache = {}
	view_cache = {}
end

-- The setting turned: every open workspace's tree lists again from what
-- it has read (nothing read again), in place.
local showing_all = prefs.show_all_files:peek()
ui.effect(function()
	local all = prefs.show_all_files:get()
	if all == showing_all then
		return
	end
	showing_all = all
	for _, w in cache do
		for d, entries in (w.kept:peek().listings or {}) :: { [string]: { any } } do
			local listed: { FileTree.Listed } = {}
			for _, e in entries do
				if shown(e) then
					table.insert(listed, { name = e.name, dir = e.dir, size = e.size, ignored = e.ignored })
				end
			end
			FileTree.sync(w.model, if d == "." then "" else d, listed)
		end
		bump(w)
	end
end)

return M
