-- A project's files as a tree: Zeron's file panel (crates/ui/src/files),
-- and a step past it.
--
-- One virtualized list, however many files: only the rows in view are
-- built, and a folder opening or closing re-lays it out from heights it
-- knows (as the sidebar's list does). A folder folds smoothly: its rows
-- grow in under it, their ink after them; closing, the ink goes first. A
-- folder of ten thousand moves only the rows that can show; the rest stand
-- in as one.
--
-- Each row: a chevron (folders), the file's icon (Zeron's Symbols set), its
-- name fading out where it doesn't fit, and at its end its git status: a
-- letter in the status's hue (M, A, U, R, D, C), its name in the same ink;
-- a folder, a dot for the most pressing thing inside it. What git ignores
-- recedes. Under each folder, a hairline guide runs down from its chevron
-- and turns into its last row with a rounded elbow (the sidebar's tree);
-- the guide of the folder you're in is firmer. Scrolled deep, the folders
-- you're inside stay at the top, each pushed up by the next as its rows
-- end; a click on one goes back to it.
--
-- Filtering keeps each match's folders around it, open, the matched
-- letters marked. The keys: ↑ ↓ (⇧ extends), ← closes or goes to the
-- folder, → opens or goes in (⌥ for everything inside), Home, End, Page
-- Up/Down, ↵ opens, F2 renames, ⌘⌫ moves to the Trash, ⌘A, Esc, and typing
-- a name's first letters jumps to it. Click, ⌘-click and ⇧-click select;
-- rows drag (onto a folder, or a file's folder; held over a closed one, it
-- opens; near an edge, the list scrolls), and out of the window as files.
-- A secondary click, the Menu key or ⇧F10 opens a row's menu.
--
-- `FileTree { model = FileTree.lines(name, text), active = state }`. The
-- model is built once (`FileTree.lines`, or `FileTree.model` with `add`
-- and `finish` for one made in code) and kept by the tree, which changes
-- it as files are renamed, moved, made or deleted (each asks its handler
-- first: one that returns a message refuses, and the message shows; an
-- empty one refuses quietly, for a handler that asks first).
--
-- A project too big to read whole loads a folder as it opens: its folders
-- are made `loaded = false` (`FileTree.sync` puts a folder's entries in,
-- `FileTree.place` a path found elsewhere, `FileTree.drop` one gone), the
-- tree asks for one as it opens (`on_expand`), and a `revision` state
-- moved after any change outside tells it to lay out again, keeping its
-- folds, selection and scroll.

local control = require("@self/control")
local token = require("@self/token")
-- (Reduce motion: what travels lands at once.)
local motion = require("@self/motion")
-- A filter match's weight in a span (which takes a number, not a token).
local MATCH_WEIGHT: number = (ui.theme.get("font.weight.medium") :: number?) or 500
local Icon = require("@self/icon")
local FileIcon = require("@self/file_icons")
local ContextMenu = require("@self/context_menu")
local Kbd = require("@self/kbd")
local FadeText = require("@self/fade_text")
local glass = require("@self/glass")
local IconButtonModule = require("@self/icon_button")
local IconButton = IconButtonModule.IconButton
local Rail = require("@self/rail")
local locale = require("@self/locale")
local EditorMenu = require("@self/editor_menu")

export type Status = "modified" | "added" | "untracked" | "renamed" | "deleted" | "conflict" | "ignored"

-- A file or folder. `kids`: a folder's, sorted (folders first, then by
-- name); `roll`: the most pressing status in it and under it.
export type Entry = {
	name: string,
	lower: string,
	path: string,
	dir: boolean,
	depth: number,
	parent: number,
	kids: { number }?,
	status: Status?,
	roll: Status?,
	ignored: boolean?,
	size: number?,
	gone: boolean?,
	-- A folder whose entries aren't read yet (false): opening it asks.
	loaded: boolean?,
}
export type Model = {
	name: string,
	-- Where it is on disk (a copied path is absolute under it).
	root: string?,
	nodes: { Entry },
	top: { number },
	by_path: { [string]: number },
	files: number,
}

-- Geometry: a row (its wash a point in from each edge, so washes never
-- touch unless they're one selection), its inset from the panel's edge,
-- each level's indent (Zeron's), the chevron's slot and the icon.
local ROW = 26
local PAD_PANE = 8
-- A 28 control's room in the 40 head band.
local BAND_ROOM_PANE = 6
-- Floating (`float`, a panel of its own over something): one inset all
-- round, so the rows' washes, the filter and the tools are concentric with
-- the panel's corner (FLOAT_CORNER less its edge and a row's corner).
local FLOAT_CORNER = "$radius.xl"
-- The soft edge where a head slides under a control laid over it.
local HEAD_FADE = 12
local INSET = 4
local INDENT = 14
local CHEV = 16
local ICON = 16
-- The elbow's radius (the sidebar's).
local BEND = 6
-- At most this many folders stay at the top as you scroll inside them.
local MAX_STICKY = 4
-- Folding: going, the ink fades first, then the rows close; coming, they
-- open, then the ink fades in (the sidebar's rhythm, a beat quicker: a tree
-- folds often).
local GO = 70
local CLOSE, CLOSE_AFTER = 180, 40
local OPEN = 200
local COME, COME_AFTER = 140, 70
-- Typing: letters within this long make one name (the context menu's).
local TYPEAHEAD = 750
-- A drag near an edge scrolls: the band, and the fastest step a frame.
local SCROLL_BAND, SCROLL_MAX = 28, 14

-- Which status matters most: a folder shows its most pressing.
local RANK: { [string]: number } = { untracked = 1, added = 2, modified = 3, renamed = 4, deleted = 5, conflict = 6 }
local LETTER: { [string]: string } = { modified = "M", added = "A", untracked = "U", renamed = "R", deleted = "D", conflict = "C" }
-- Each status's name, and what a folder holding it says: their keys, read
-- where they're drawn (a tooltip as a string, as it's built).
local SAID: { [string]: string } = {
	modified = "ds.file_tree.status.modified",
	added = "ds.file_tree.status.added",
	untracked = "ds.file_tree.status.untracked",
	renamed = "ds.file_tree.status.renamed",
	deleted = "ds.file_tree.status.deleted",
	conflict = "ds.file_tree.status.conflict",
}
local HOLDS: { [string]: string } = {
	modified = "ds.file_tree.holds.modified",
	added = "ds.file_tree.holds.added",
	untracked = "ds.file_tree.holds.untracked",
	renamed = "ds.file_tree.holds.renamed",
	deleted = "ds.file_tree.holds.deleted",
	conflict = "ds.file_tree.holds.conflict",
}

-- The model.

local M = {}

function M.model(name: string, root: string?): Model
	return { name = name, root = root, nodes = {}, top = {}, by_path = {}, files = 0 }
end

-- A file or folder under `parent` (0: the top), its index.
function M.add(m: Model, parent: number, name: string, dir: boolean, status: Status?, size: number?): number
	local p = if parent > 0 then m.nodes[parent] else nil
	local path = if p then p.path .. "/" .. name else name
	local n: Entry = {
		name = name,
		lower = string.lower(name),
		path = path,
		dir = dir,
		depth = if p then p.depth + 1 else 0,
		parent = parent,
		kids = if dir then {} else nil,
		status = if status == "ignored" then nil else status,
		ignored = status == "ignored" or (p ~= nil and p.ignored) or nil,
		size = size,
		gone = status == "deleted" or nil,
	}
	table.insert(m.nodes, n)
	local id = #m.nodes
	if p then
		table.insert(p.kids :: { number }, id)
	else
		table.insert(m.top, id)
	end
	m.by_path[path] = id
	if not dir then
		m.files += 1
	end
	return id
end

local function before(m: Model): (number, number) -> boolean
	local nodes = m.nodes
	return function(a: number, b: number): boolean
		local x, y = nodes[a], nodes[b]
		if x.dir ~= y.dir then
			return x.dir
		end
		if x.lower ~= y.lower then
			return x.lower < y.lower
		end
		return x.name < y.name
	end
end

-- Each folder's most pressing status, from its files up.
local function roll_up(m: Model)
	local nodes = m.nodes
	local rank: { number } = table.create(#nodes, 0)
	for i = #nodes, 1, -1 do
		local n = nodes[i]
		if n.parent >= 0 then
			local own = if n.status then RANK[n.status] or 0 else 0
			local r = math.max(own, rank[i])
			rank[i] = r
			if n.parent > 0 and r > rank[n.parent] then
				rank[n.parent] = r
			end
		end
	end
	local by_rank: { [number]: Status } = {}
	for s, r in RANK do
		if s ~= "untracked" or not by_rank[r] then
			by_rank[r] = s :: Status
		end
	end
	for i, n in nodes do
		n.roll = if rank[i] > 0 then by_rank[rank[i]] else nil
	end
end

-- Sorts each folder (unless made in order: `sorted`) and rolls statuses up.
function M.finish(m: Model, sorted: boolean?): Model
	if not sorted then
		local cmp = before(m)
		table.sort(m.top, cmp)
		for _, n in m.nodes do
			if n.kids and #n.kids > 1 then
				table.sort(n.kids, cmp)
			end
		end
	end
	roll_up(m)
	return m
end

-- A model from lines of `path<TAB>size<TAB>status` (a path ending in `/`
-- is a folder; folders on the way are made).
function M.lines(name: string, text: string, root: string?): Model
	local m = M.model(name, root)
	local function folder(path: string): number
		if path == "" then
			return 0
		end
		local id = m.by_path[path]
		if id then
			return id
		end
		local cut = string.find(path, "/[^/]*$")
		local parent = if cut then folder(string.sub(path, 1, cut - 1)) else 0
		return M.add(m, parent, if cut then string.sub(path, cut + 1) else path, true)
	end
	for line in string.gmatch(text, "[^\n]+") do
		local path, size, status = string.match(line, "^([^\t]*)\t?([^\t]*)\t?([^\t]*)$")
		if path and path ~= "" then
			local dir = string.sub(path, -1) == "/"
			if dir then
				path = string.sub(path, 1, -2)
			end
			local st: Status? = if status and status ~= "" then status :: Status else nil
			local cut = string.find(path, "/[^/]*$")
			local parent = if cut then folder(string.sub(path, 1, cut - 1)) else 0
			local leaf = if cut then string.sub(path, cut + 1) else path
			if dir then
				local id = folder(path)
				if st == "ignored" then
					m.nodes[id].ignored = true
				end
			elseif not m.by_path[path] then
				M.add(m, parent, leaf, false, st, tonumber(size))
			end
		end
	end
	-- (A folder marked ignored after its files were added: theirs too; and
	-- a folder holding nothing but what's ignored is ignored itself.)
	for _, n in m.nodes do
		if n.parent > 0 and m.nodes[n.parent].ignored then
			n.ignored = true
		end
	end
	for i = #m.nodes, 1, -1 do
		local n = m.nodes[i]
		if n.kids and #n.kids > 0 and not n.ignored then
			local all = true
			for _, k in n.kids do
				if not m.nodes[k].ignored then
					all = false
					break
				end
			end
			if all then
				n.ignored = true
			end
		end
	end
	return M.finish(m)
end

-- Its changes: each keeps `path`, `depth`, `by_path` and the rolled
-- statuses true.
local function repath(m: Model, id: number)
	local n = m.nodes[id]
	local p = if n.parent > 0 then m.nodes[n.parent] else nil
	m.by_path[n.path] = nil
	n.path = if p then p.path .. "/" .. n.name else n.name
	n.depth = if p then p.depth + 1 else 0
	m.by_path[n.path] = id
	if n.kids then
		for _, k in n.kids do
			repath(m, k)
		end
	end
end
local function siblings(m: Model, parent: number): { number }
	return if parent > 0 then m.nodes[parent].kids :: { number } else m.top
end
local function detach(m: Model, id: number)
	local list = siblings(m, m.nodes[id].parent)
	local i = table.find(list, id)
	if i then
		table.remove(list, i)
	end
end
local function attach(m: Model, id: number, parent: number)
	local list = siblings(m, parent)
	local cmp = before(m)
	local at = #list + 1
	for i, k in list do
		if cmp(id, k) then
			at = i
			break
		end
	end
	table.insert(list, at, id)
	m.nodes[id].parent = parent
end
local function forget(m: Model, id: number)
	local n = m.nodes[id]
	m.by_path[n.path] = nil
	n.parent = -1
	if not n.dir then
		m.files -= 1
	end
	if n.kids then
		for _, k in n.kids do
			forget(m, k)
		end
	end
end

-- Whether `id` is `under` or inside it.
local function within(m: Model, id: number, under: number): boolean
	local i = id
	while i > 0 do
		if i == under then
			return true
		end
		i = m.nodes[i].parent
	end
	return false
end

-- Changes from outside (a lazily read project): each keeps the model as
-- `finish` leaves it. Tell the tree with its `revision`.

export type Listed = { name: string, dir: boolean, size: number?, ignored: boolean? }

-- A folder's entries as read (`path` "" for the top): what's new added in
-- order, what's gone forgotten, sizes brought up to date, the folder marked
-- loaded. Answers whether anything changed.
function M.sync(m: Model, path: string, entries: { Listed }): boolean
	local parent = if path == "" then 0 else m.by_path[path]
	if not parent then
		return false
	end
	local changed = false
	if parent > 0 and m.nodes[parent].loaded == false then
		m.nodes[parent].loaded = true
		changed = true
	end
	local seen: { [string]: boolean } = {}
	local added = false
	local under_ignored = parent > 0 and m.nodes[parent].ignored == true
	for _, e in entries do
		seen[e.name] = true
		local full = if path == "" then e.name else path .. "/" .. e.name
		local id = m.by_path[full]
		if id and m.nodes[id].dir ~= e.dir then
			detach(m, id)
			forget(m, id)
			id = nil
		end
		if id then
			local n = m.nodes[id]
			local ign = (e.ignored or under_ignored) or nil
			if n.size ~= e.size or n.ignored ~= ign then
				n.size = e.size
				n.ignored = ign
				changed = true
			end
		else
			-- (Added at the end, and the folder sorted once: a first reading
			-- of thousands stays linear-ish.)
			local new = M.add(m, parent, e.name, e.dir, if e.ignored then "ignored" else nil, e.size)
			if e.dir then
				m.nodes[new].loaded = false
			end
			added = true
			changed = true
		end
	end
	for _, k in table.clone(siblings(m, parent)) do
		if not seen[m.nodes[k].name] then
			detach(m, k)
			forget(m, k)
			changed = true
		end
	end
	if added then
		table.sort(siblings(m, parent), before(m))
	end
	return changed
end

-- Git's statuses for the whole folder, `{ [path] = status }` (paths from
-- the top; absent, clean): each file read so far marked, and each folder
-- marked with the most pressing status under it, read or not (a folder not
-- opened yet still shows what changed inside it), as the shipping app
-- rolls them up (crates/ui/src/files/git_status.rs). What git ignores keeps
-- its own mark. Answers whether anything changed.
function M.statuses(m: Model, map: { [string]: string }): boolean
	local nodes = m.nodes
	local rank: { [number]: number } = {}
	for path, st in map do
		local r = RANK[st] or 0
		if r > 0 then
			local id = m.by_path[path]
			if id then
				rank[id] = math.max(rank[id] or 0, r)
			end
			-- (Each folder on its way, as far as it's known.)
			local dir = path
			while true do
				local cut = string.find(dir, "/[^/]*$")
				if not cut then
					break
				end
				dir = string.sub(dir, 1, cut - 1)
				local up = m.by_path[dir]
				if up then
					rank[up] = math.max(rank[up] or 0, r)
				end
			end
		end
	end
	local by_rank: { [number]: Status } = {}
	for st, r in RANK do
		if st ~= "untracked" or not by_rank[r] then
			by_rank[r] = st :: Status
		end
	end
	local changed = false
	for i, n in nodes do
		if n.parent >= 0 then
			local own: Status? = if n.dir or n.ignored then nil else (map[n.path] :: Status?)
			local roll: Status? = if rank[i] then by_rank[rank[i]] else nil
			if n.status ~= own or n.roll ~= roll then
				n.status = own
				n.roll = roll
				changed = true
			end
		end
	end
	return changed
end

-- A path found elsewhere (a search), with the folders on its way: each made
-- unread if it's new. Answers its index.
function M.place(m: Model, path: string, dir: boolean, size: number?): number
	local id = m.by_path[path]
	if id then
		return id
	end
	local cut = string.find(path, "/[^/]*$")
	local parent = if cut then M.place(m, string.sub(path, 1, cut - 1), true) else 0
	local new = M.add(m, parent, if cut then string.sub(path, cut + 1) else path, dir, nil, size)
	detach(m, new)
	attach(m, new, parent)
	if dir then
		m.nodes[new].loaded = false
	end
	return new
end

-- A path gone, and everything in it. Answers whether it was there.
function M.drop(m: Model, path: string): boolean
	local id = m.by_path[path]
	if not id then
		return false
	end
	detach(m, id)
	forget(m, id)
	return true
end

-- What's wrong with `name` for a file in `parent` (nil: nothing).
local function name_problem(m: Model, parent: number, name: string, self_id: number?): string?
	if name == "" or string.match(name, "^%s*$") then
		return locale.s("ds.file_tree.name_needed")
	elseif string.find(name, "/", 1, true) or string.find(name, "\\", 1, true) then
		return locale.s("ds.file_tree.name_slash")
	elseif name == "." or name == ".." or name == ".git" then
		return locale.s("ds.file_tree.name_reserved", { name = name })
	end
	local lower = string.lower(name)
	for _, k in siblings(m, parent) do
		if k ~= self_id and m.nodes[k].lower == lower then
			return locale.s(
				if m.nodes[k].dir then "ds.file_tree.name_taken_folder" else "ds.file_tree.name_taken_file",
				{ name = m.nodes[k].name }
			)
		end
	end
	return nil
end

-- A query against a name: where it matched (a run of letters, or the
-- letters in order), or nil.
local function matched(lower: string, q: string): { number }?
	local s = string.find(lower, q, 1, true)
	if s then
		local out: { number } = table.create(#q, 0)
		for i = 0, #q - 1 do
			out[i + 1] = s + i
		end
		return out
	end
	local out: { number } = {}
	local at = 1
	for i = 1, #q do
		local c = string.sub(q, i, i)
		local f = string.find(lower, c, at, true)
		if not f then
			return nil
		end
		table.insert(out, f)
		at = f + 1
	end
	return out
end

-- The component.

export type Props = {
	model: Model,
	-- The file shown: a click or ↵ sets it; set elsewhere, the tree shows
	-- it (its folders opened, scrolled to, selected).
	active: State<string?>?,
	-- The filter, and whether its field shows at the top (true).
	filter: State<string>?,
	search: boolean?,
	-- Whether what git ignores shows (dimmed); true by default.
	show_ignored: State<boolean>?,
	-- Set a path: shown, as `active` changing would, then set back to nil.
	reveal: State<string?>?,
	-- Its scroll position (its own by default), to keep or drive it.
	offset: State<number>?,
	on_open: ((path: string) -> ())?,
	-- A file opened beside the one showing (a tab of its own): its row
	-- menu's Open in New Tab, a middle click, ⌘↵. Absent, none of them.
	on_open_new: ((path: string) -> ())?,
	-- A file or folder (each of the selection) carried with the next
	-- message: its row menu's Add to Chat. `paths` from the top, `root` the
	-- folder's own. Absent, no such item.
	on_add_to_chat: ((paths: { string }, root: string?) -> ())?,
	on_rename: ((from: string, to: string) -> string?)?,
	on_move: ((paths: { string }, into: string) -> string?)?,
	on_create: ((path: string, dir: boolean) -> string?)?,
	-- (`confirmed`: asked already, as the row menu's Delete does; ⌘⌫ doesn't.)
	on_delete: ((paths: { string }, confirmed: boolean) -> string?)?,
	-- Deleting puts files in the Trash (true, by default) or ends them for
	-- good (false): what its menu and question say.
	trash: boolean?,
	-- Whether Reveal in Finder applies now (the files are on this machine).
	openable: (() -> boolean)?,
	-- A floating panel's tree (FLOAT_CORNER, a hairline edge): its rows,
	-- filter and tools one inset in all round, concentric with that
	-- corner; its head as tall as the filter and that inset either side.
	float: boolean?,
	-- How much of its head's start is under a control laid over it (a
	-- pane's toggle) as it slides: cut there, fading out as it passes
	-- under, so the control never has the head's words behind it.
	head_cut: (() -> number)?,
	-- Entries others add to a row's menu (plugins'), asked for as it opens.
	more_menu: ((path: string, kind: "file" | "folder") -> { ContextMenu.Entry })?,
	-- Whether its top is still being read (a lazily read model): its empty
	-- words wait.
	loading: (() -> boolean)?,
	-- Declared hooks for plugins (zgx's `hook`): its tools in the head.
	hooks: { tools: string? }?,
	on_reveal: ((path: string) -> ())?,
	-- Its head (the filter and its tools) as a pane's band (ds/files_pane):
	-- its height, with a hairline under it that the file's header beside
	-- it carries on; and room kept at its start for a control laid over the
	-- pane (the tree's toggle), in points or a function of them. Without
	-- `head`, the head keeps its own shape: a row with a little air under.
	head: number?,
	lead: (number | () -> number)?,
	-- A lazily read model: a folder not read yet (`loaded = false`) opening,
	-- by hand or as kept from before; and a state moved after the model is
	-- changed outside (`FileTree.sync`, `place`, `drop`).
	on_expand: ((path: string) -> ())?,
	revision: State<number>?,
	-- The open folders' paths, if the caller keeps them (its own by
	-- default): a tree made again for the same files opens as it was.
	folds: State<{ string }>?,
	[any]: any,
}

local BOX_KEYS = {
	model = true,
	active = true,
	filter = true,
	search = true,
	show_ignored = true,
	reveal = true,
	offset = true,
	on_open = true,
	on_open_new = true,
	on_add_to_chat = true,
	on_rename = true,
	on_move = true,
	on_create = true,
	on_delete = true,
	on_reveal = true,
	head = true,
	lead = true,
	on_expand = true,
	revision = true,
	trash = true,
	loading = true,
	openable = true,
	head_cut = true,
	float = true,
	more_menu = true,
	folds = true,
	hooks = true,
	children = true,
}

-- A floating tree's one inset: its panel's corner less its edge and a
-- row's corner.
function M.float_inset(): number
	return control.nest(FLOAT_CORNER, 1, "$radius.md")
end
-- Its head's height, floating: the filter's and that inset either side.
function M.float_head(): number
	return (ui.theme.get("zeron.icon_button.lg") :: number? or 28) + 2 * M.float_inset()
end
M.FLOAT_CORNER = FLOAT_CORNER

local function component(props: Props): Node
	local floating = props.float == true
	local PAD: number = if floating then M.float_inset() else PAD_PANE
	local BAND_ROOM: number = if floating then M.float_inset() else BAND_ROOM_PANE
	local m = props.model
	local nodes = m.nodes
	local active = props.active or ui.use_state(nil :: string?)
	local filter = props.filter or ui.use_state("")
	local show_ignored = props.show_ignored or ui.use_state(true)

	-- What changes the rows: the model (renamed, moved, made, deleted), the
	-- folds, a fold starting or settling.
	local version = ui.use_state(0)
	local narrow_reset: () -> () = function() end
	local function changed()
		narrow_reset()
		version:set(version:peek() + 1)
	end
	-- Open folders, by index (and, while filtering, those closed by hand:
	-- a filter opens every folder holding a match).
	local expanded: { [number]: boolean } = {}
	local closed_in_filter: { [number]: boolean } = {}
	-- (Kept by path too, so the folds outlive the script being saved.)
	local kept_open = props.folds or ui.use_state({} :: { string })
	-- A folder opening that isn't read yet asks for its entries.
	-- (Asked at once, never from a timer: one made while the rows are laid
	-- out belongs to that layout, and goes with the next.)
	local function wants_entries(id: number)
		local n = nodes[id]
		if n and n.dir and n.loaded == false and props.on_expand then
			props.on_expand(n.path)
		end
	end
	-- (And in a lazily read model, those found as their folders are read.)
	local function restore_kept(): boolean
		local any = false
		for _, path in kept_open:peek() do
			local id = m.by_path[path]
			if id and not expanded[id] then
				expanded[id] = true
				any = true
				wants_entries(id)
			end
		end
		return any
	end
	restore_kept()
	local keep_pending = false
	local function keep_folds()
		if keep_pending then
			return
		end
		keep_pending = true
		ui.after(0, function()
			keep_pending = false
			local out: { string } = {}
			for id in expanded do
				if nodes[id] and nodes[id].parent >= 0 then
					table.insert(out, nodes[id].path)
				end
			end
			-- (Kept ones not read yet stay kept, for when they are.)
			for _, path in kept_open:peek() do
				if not m.by_path[path] and props.on_expand then
					table.insert(out, path)
				end
			end
			kept_open:set(out)
		end)
	end
	-- Selection: by index, the row the keys move from (`cursor`) and where a
	-- ⇧ range runs from (`anchor`).
	local selected: { [number]: boolean } = {}
	local sel_v = ui.use_state(0)
	local cursor = ui.use_state(0)
	local anchor = 0
	local focused = ui.use_state(false)
	local hover_id = ui.use_state(0)
	local renaming = ui.use_state(0)
	local rename_error = ui.use_state("")
	-- A file being made: its index, removed again if it's let go unnamed.
	local creating = 0
	-- Where a drag would land (a folder's index; -1, the top; 0, nowhere).
	local drop_at = ui.use_state(0)
	local dragging = ui.use_state(false)
	local drag_y: number? = nil
	-- A note at the foot (what was typed, a path copied), fading.
	local note = ui.use_state("")
	local note_v = 0

	local list_offset = props.offset or ui.use_state(0)
	local list_view = ui.ref()
	local root_ref = ui.ref()
	local row_refs: { [number]: any } = {}

	-- The filter the rows follow: at once, or in a big tree a beat after
	-- typing pauses (each letter there lays out thousands of rows), so the
	-- field never waits on them.
	local applied = ui.use_state(filter:peek())
	local BIG = 20000
	local typed_v = 0
	ui.effect(function()
		local q = filter:get()
		if #nodes < BIG or q == "" then
			applied:set(q)
			return
		end
		typed_v += 1
		local v = typed_v
		ui.after(90, function()
			if v == typed_v then
				applied:set(q)
			end
		end)
	end)
	local function filtering(): boolean
		return applied:peek() ~= ""
	end
	local function say(text: string)
		note_v += 1
		local v = note_v
		note:set(text)
		ui.after(1200, function()
			if note_v == v then
				note:set("")
			end
		end)
	end

	-- Folds animate on one clock (milliseconds), running only while one
	-- moves, as the sidebar's do.
	local clock = ui.use_state(0)
	type Anim = { from: number, to: number, at: number, dur: number, delay: number }
	local function value(a: Anim, t: number): number
		if a.dur <= 0 then
			return a.to
		end
		local p = math.clamp((t - a.at - a.delay) / a.dur, 0, 1)
		p = 1 - (1 - p) ^ 3
		return a.from + (a.to - a.from) * p
	end
	local function still(v: number): Anim
		return { from = v, to = v, at = 0, dur = 0, delay = 0 }
	end
	type Fold = { id: number, h: Anim, o: Anim, done: number }
	local folds: { [number]: Fold } = {}
	local ticking: any = nil
	local function settle()
		local t = clock:peek()
		local any_left, settled = false, false
		for id, f in folds do
			if t >= f.done then
				folds[id] = nil
				settled = true
			else
				any_left = true
			end
		end
		if settled then
			changed()
		end
		if not any_left and ticking then
			ticking:cancel()
			ticking = nil
		end
	end
	local function anim(a: Anim?, from: number, to: number, dur: number, delay: number?): Anim
		local t = clock:peek()
		return { from = if a then value(a, t) else from, to = to, at = t, dur = dur, delay = delay or 0 }
	end
	local function start_clock()
		if not ticking then
			ticking = anime.animate(clock, {
				value = clock:peek() + 1e7,
				duration = 1e7,
				ease = "linear",
				onUpdate = settle,
			})
		end
	end

	-- A folder's state, as the rows see it (still there while it closes).
	local function is_open(id: number): boolean
		if folds[id] then
			return true
		end
		if filtering() then
			return not closed_in_filter[id]
		end
		return expanded[id] == true
	end
	-- Opened or closed by hand (what a filter or a click says).
	local function wants_open(id: number): boolean
		if filtering() then
			return not closed_in_filter[id]
		end
		return expanded[id] == true
	end
	local function write_open(id: number, on: boolean)
		if filtering() then
			closed_in_filter[id] = if on then nil else true
		else
			expanded[id] = if on then true else nil
			keep_folds()
		end
		if on then
			wants_entries(id)
		end
	end
	local function set_open(id: number, on: boolean, still_fold: boolean?)
		local n = nodes[id]
		if not n or not n.dir or wants_open(id) == on then
			return
		end
		write_open(id, on)
		if not still_fold then
			local f = folds[id]
			local t = clock:peek()
			if on then
				folds[id] = {
					id = id,
					h = anim(f and f.h, 0, 1, OPEN),
					o = anim(f and f.o, 0, 1, COME, COME_AFTER),
					done = t + math.max(OPEN, COME + COME_AFTER),
				}
			else
				folds[id] = {
					id = id,
					o = anim(f and f.o, 1, 0, GO),
					h = anim(f and f.h, 1, 0, CLOSE, CLOSE_AFTER),
					done = t + CLOSE + CLOSE_AFTER,
				}
			end
			start_clock()
		end
		changed()
	end
	-- Everything inside it opened or closed (⌥), it with them.
	local function set_open_deep(id: number, on: boolean)
		local function walk(i: number)
			local n = nodes[i]
			if n.kids then
				for _, k in n.kids do
					if nodes[k].dir then
						write_open(k, on)
						walk(k)
					end
				end
			end
		end
		walk(id)
		if wants_open(id) == on then
			changed()
		else
			set_open(id, on)
		end
	end
	local function collapse_all()
		for id in expanded do
			expanded[id] = nil
		end
		keep_folds()
		if filtering() then
			for i, n in nodes do
				if n.dir then
					closed_in_filter[i] = true
				end
			end
		end
		changed()
	end

	-- The filter: each match and the folders holding it.
	local keep: { [number]: boolean } = {}
	local hits: { [number]: boolean } = {}
	local match_count = ui.use_state(0)
	-- (A query typed on from the last only tests the last one's matches.)
	type Narrow = { q: string, mode: string, ids: { number } }
	local narrow: Narrow? = nil
	-- What the rows mark: the query and how it matched.
	local marking: { q: string, mode: string } = { q = "", mode = "literal" }
	narrow_reset = function()
		narrow = nil
	end
	local function refilter(q: string)
		keep, hits = {}, {}
		local count = 0
		if q == "" then
			narrow = nil
			match_count:set(0)
			return
		end
		local lq = string.lower(q)
		local by_path = string.find(lq, "/", 1, true) ~= nil
		local from: { number }? = nil
		local was = narrow
		if was and string.sub(lq, 1, #was.q) == was.q and (was.mode == "path") == by_path then
			from = was.ids
		end
		-- (Where it matched is worked out as a row shows, not here.)
		local function test(i: number, mode: string): boolean
			local n = nodes[i]
			if n.parent < 0 then
				return false
			end
			if mode == "path" then
				return string.find(string.lower(n.path), lq, 1, true) ~= nil
			elseif mode == "fuzzy" then
				return matched(n.lower, lq) ~= nil
			end
			return string.find(n.lower, lq, 1, true) ~= nil
		end
		local found: { number } = {}
		local function scan(mode: string)
			found = {}
			hits = {}
			if from then
				for _, i in from do
					if test(i, mode) then
						hits[i] = true
						table.insert(found, i)
					end
				end
			else
				for i in nodes do
					if test(i, mode) then
						hits[i] = true
						table.insert(found, i)
					end
				end
			end
		end
		-- Letters in a row first; only with none anywhere, letters in order.
		local mode = if by_path then "path" else "literal"
		if was and from and was.mode == "fuzzy" then
			mode = "fuzzy"
		end
		scan(mode)
		if #found == 0 and mode == "literal" then
			-- (Letters in order aren't among the last's runs: all again.)
			mode = "fuzzy"
			from = nil
			scan(mode)
		end
		narrow = { q = lq, mode = mode, ids = found }
		marking = { q = lq, mode = mode }
		for _, i in found do
			if not nodes[i].dir then
				count += 1
			end
			local p = i
			while p > 0 and not keep[p] do
				keep[p] = true
				p = nodes[p].parent
			end
		end
		match_count:set(count)
	end

	-- The rows: which entries show, in order. While a fold moves, its rows
	-- past what can show stand in as one (`rest`), so the list never finds a
	-- thousand tiny rows in view and builds them all.
	type Shape = {
		ids: { number },
		index: { [number]: number },
		-- The row after each folder's last (its rows run to it).
		ends: { [number]: number },
		rest: { [number]: number },
		chain: { [number]: { Fold } },
		moving: { number },
	}
	local current: Shape = { ids = {}, index = {}, ends = {}, rest = {}, chain = {}, moving = {} }
	local last_query = nil :: string?
	local last_revision = if props.revision then props.revision:peek() else 0
	local function build_shape(): Shape
		local q = applied:get()
		show_ignored:get()
		version:get()
		-- Changed outside: kept folds found again, the filter run again.
		local rev = if props.revision then props.revision:get() else 0
		if rev ~= last_revision then
			last_revision = rev
			narrow = nil
			if q ~= "" then
				last_query = nil
			end
		end
		if q ~= last_query then
			last_query = q
			refilter(q)
			closed_in_filter = {}
		end
		local f_on = q ~= ""
		local ign = show_ignored:peek()
		local s: Shape = { ids = {}, index = {}, ends = {}, rest = {}, chain = {}, moving = {} }
		local ids, index, ends = s.ids, s.index, s.ends
		local view = math.max(600, (list_view.height or 0)) + 2 * ROW
		local budget_rows = math.ceil(view / ROW) + 2
		local function shows(id: number): boolean
			local n = nodes[id]
			if f_on and not keep[id] then
				return false
			end
			if n.ignored and not ign then
				return false
			end
			return true
		end
		-- How many rows a folder's open insides would make.
		local function count(kids: { number }): number
			local c = 0
			for _, k in kids do
				if shows(k) then
					c += 1
					local n = nodes[k]
					if n.kids and is_open(k) then
						c += count(n.kids)
					end
				end
			end
			return c
		end
		type Budget = { left: number, rest: number }
		local function walk(kids: { number }, chain: { Fold }?, budget: Budget?)
			for _, id in kids do
				if shows(id) then
					local n = nodes[id]
					if budget and budget.left <= 0 then
						budget.rest += 1
						if n.kids and is_open(id) then
							budget.rest += count(n.kids)
						end
					else
						if budget then
							budget.left -= 1
						end
						table.insert(ids, id)
						local row = #ids
						index[id] = row
						if chain then
							s.chain[row] = chain
							table.insert(s.moving, row)
						end
						if n.kids and is_open(id) then
							local f = folds[id]
							if f then
								local inner = table.clone(chain or {})
								table.insert(inner, f)
								local b = { left = budget_rows, rest = 0 }
								walk(n.kids, inner, b)
								if b.rest > 0 then
									table.insert(ids, 0)
									local r = #ids
									s.rest[r] = b.rest
									s.chain[r] = inner
									table.insert(s.moving, r)
								end
							else
								walk(n.kids, chain, budget)
							end
						end
						ends[id] = #ids + 1
					end
				end
			end
		end
		walk(m.top, nil, nil)
		return s
	end
	-- Changed outside: kept folds found as their folders come in, opened
	-- (an effect: opening asks for entries, which mustn't happen while the
	-- rows are laid out).
	if props.revision then
		local rev_state = props.revision
		ui.effect(function()
			rev_state:get()
			if restore_kept() then
				changed()
			end
		end)
	end
	local shape_v = 0
	local shaped = ui.computed(function(): number
		current = build_shape()
		shape_v += 1
		return shape_v
	end)
	local function shape(): Shape
		shaped:get()
		return current
	end
	-- Made again with a scroll kept from before (`offset` the caller's) over
	-- a model still being read: the list can't go there until the rows
	-- are in, and would settle at the top. So the place is held and gone
	-- back to as rows come, until it's reached or the list is scrolled by
	-- hand (a long while passing too: then the rows there are, are what
	-- there is). Never by the clock alone in the ordinary case: a slow
	-- machine reads folders slowly.
	local wanted: number? = if props.offset and list_offset:peek() > 0 then list_offset:peek() else nil
	if wanted then
		ui.after(10000, function()
			wanted = nil
		end)
		ui.effect(function()
			local s = shape()
			local want = wanted
			if not want then
				return
			end
			local most = math.max(0, #s.ids * ROW - (list_view.height or 0))
			local to = math.min(want, most)
			if to >= want - 0.5 then
				wanted = nil
				list_offset:set(want)
			elseif to > list_offset:peek() then
				list_offset:set(to)
			end
		end)
	end
	local function shape_peek(): Shape
		shaped:peek()
		return current
	end
	local heights = ui.computed(function(): { number }
		local s = shape()
		local n = #s.ids
		local hs = table.create(n, ROW)
		if #s.moving > 0 then
			local t = clock:get()
			for _, i in s.moving do
				local h = if s.rest[i] then s.rest[i] * ROW else ROW
				for _, f in s.chain[i] do
					h *= value(f.h, t)
				end
				hs[i] = h
			end
		end
		return hs
	end)
	-- Where row `i` starts, and the row at `y` (rows are ROW tall but for
	-- those moving).
	local function top_of(i: number): number
		local s, hs = shape_peek(), heights:peek()
		local y = (i - 1) * ROW
		for _, j in s.moving do
			if j >= i then
				break
			end
			y += hs[j] - ROW
		end
		return y
	end
	local function row_at(y: number): number
		local s, hs = shape_peek(), heights:peek()
		local adj = 0
		for _, j in s.moving do
			local top = (j - 1) * ROW + adj
			if y < top then
				break
			end
			if y < top + hs[j] then
				return j
			end
			adj += hs[j] - ROW
		end
		return math.clamp(math.floor((y - adj) / ROW) + 1, 1, math.max(1, #s.ids))
	end
	local function total(): number
		local s = shape_peek()
		return top_of(#s.ids + 1)
	end

	-- Selection.
	local function select_only(id: number)
		selected = if id > 0 then { [id] = true } else {}
		anchor = id
		cursor:set(id)
		sel_v:set(sel_v:peek() + 1)
	end
	local function select_range(to: number)
		local s = shape_peek()
		local a, b = s.index[anchor] or s.index[to], s.index[to]
		if not a or not b then
			return select_only(to)
		end
		selected = {}
		for i = math.min(a, b), math.max(a, b) do
			local id = s.ids[i]
			if id > 0 then
				selected[id] = true
			end
		end
		cursor:set(to)
		sel_v:set(sel_v:peek() + 1)
	end
	local function toggle_selected(id: number)
		selected[id] = if selected[id] then nil else true
		anchor = id
		cursor:set(id)
		sel_v:set(sel_v:peek() + 1)
	end
	local function is_selected(id: number): boolean
		sel_v:get()
		return selected[id] == true
	end
	-- What an action takes: the selection, if `id` is in it; else `id`.
	local function targets(id: number): { number }
		if selected[id] then
			local out: { number } = {}
			for _, i in shape_peek().ids do
				if i > 0 and selected[i] then
					table.insert(out, i)
				end
			end
			return out
		end
		return { id }
	end
	local function paths_of(ids: { number }): { string }
		local out: { string } = {}
		for _, i in ids do
			table.insert(out, nodes[i].path)
		end
		return out
	end
	local function absolute(path: string): string
		return if m.root then m.root .. "/" .. path else path
	end

	-- How many folders stand at the top over row `i` (as many of its own
	-- as can), so a row brought into view isn't under them.
	local function stack_over(i: number): number
		local id = shape_peek().ids[i] or 0
		local d = if id > 0 then nodes[id].depth else 0
		return math.min(d, MAX_STICKY)
	end
	local function scroll_into_view(i: number, center: boolean?)
		local view = list_view.height or 0
		if view <= 0 then
			return
		end
		local top = top_of(i)
		local o = list_offset:peek()
		local cover = stack_over(i) * ROW
		local most = math.max(0, total() - view)
		if center then
			list_offset:set(math.clamp(top - (view - ROW) / 2, 0, most))
		elseif top < o + cover then
			list_offset:set(math.clamp(top - cover, 0, most))
		elseif top + ROW > o + view then
			list_offset:set(math.clamp(top + ROW - view, 0, most))
		end
	end
	-- (A row scrolled to is built a frame later: it's waited for.)
	local function focus_row(id: number, tries: number?)
		ui.after(0, function()
			local r = row_refs[id]
			local el = if r then r:peek() else nil
			if el then
				el:focus()
			elseif (tries or 0) < 4 then
				focus_row(id, (tries or 0) + 1)
			end
		end)
	end
	local function go_to(id: number, how: ("only" | "extend")?, center: boolean?)
		if id <= 0 then
			return
		end
		if how == "extend" then
			select_range(id)
		else
			select_only(id)
		end
		local i = shape_peek().index[id]
		if i then
			scroll_into_view(i, center)
		end
		focus_row(id)
	end

	-- Shows a path: its folders opened (still: no fold to wait for), scrolled
	-- to the middle, selected.
	local function reveal(path: string)
		local id = m.by_path[path]
		if not id then
			return
		end
		local p = nodes[id].parent
		while p > 0 do
			if not wants_open(p) then
				write_open(p, true)
			end
			p = nodes[p].parent
		end
		if nodes[id].ignored and not show_ignored:peek() then
			show_ignored:set(true)
		end
		changed()
		select_only(id)
		ui.after(0, function()
			local i = shape_peek().index[id]
			if i then
				local view = list_view.height or 0
				local top = top_of(i)
				local o = list_offset:peek()
				-- (Only if it doesn't show already.)
				if top < o + stack_over(i) * ROW or top + ROW > o + view then
					scroll_into_view(i, true)
				end
			end
		end)
	end
	-- (Made again with its folds and scroll kept by the caller, the file
	-- open is where it was left: not revealed again, which would scroll
	-- away from there.)
	local opened_here: string? = if props.folds and props.offset then active:peek() else nil
	ui.effect(function()
		local a = active:get()
		if a and a ~= opened_here then
			reveal(a)
		end
		opened_here = nil
	end)
	if props.reveal then
		local r = props.reveal
		ui.effect(function()
			local p = r:get()
			if p then
				reveal(p)
				ui.after(0, function()
					r:set(nil)
				end)
			end
		end)
	end

	local function open_file(id: number)
		local n = nodes[id]
		if n.dir then
			set_open(id, not wants_open(id))
			return
		end
		opened_here = n.path
		active:set(n.path)
		if props.on_open then
			props.on_open(n.path)
		end
	end

	-- A file in a tab of its own (the one showing stays).
	local function open_new(id: number)
		local n = nodes[id]
		local open = props.on_open_new
		if n.dir or not open then
			return
		end
		open(n.path)
	end

	-- Changes, each asking its handler first.
	local function apply_rename(id: number, name: string): string?
		local n = nodes[id]
		local problem = name_problem(m, n.parent, name, id)
		if problem then
			return problem
		end
		if name == n.name then
			return nil
		end
		local parent_path = if n.parent > 0 then nodes[n.parent].path .. "/" else ""
		if props.on_rename then
			local refused = props.on_rename(n.path, parent_path .. name)
			if refused then
				return refused
			end
		end
		local was = n.path
		detach(m, id)
		n.name, n.lower = name, string.lower(name)
		attach(m, id, n.parent)
		repath(m, id)
		local a = active:peek()
		if a and (a == was or string.sub(a, 1, #was + 1) == was .. "/") then
			opened_here = n.path .. string.sub(a, #was + 1)
			active:set(opened_here)
		end
		changed()
		return nil
	end
	local function finish_create(id: number, name: string): string?
		local n = nodes[id]
		local problem = name_problem(m, n.parent, name, id)
		if problem then
			return problem
		end
		local parent_path = if n.parent > 0 then nodes[n.parent].path .. "/" else ""
		if props.on_create then
			local refused = props.on_create(parent_path .. name, n.dir)
			if refused then
				return refused
			end
		end
		detach(m, id)
		n.name, n.lower = name, string.lower(name)
		attach(m, id, n.parent)
		repath(m, id)
		n.status = "untracked"
		roll_up(m)
		creating = 0
		changed()
		if not n.dir then
			open_file(id)
		end
		return nil
	end
	local function drop_created()
		if creating > 0 then
			local id = creating
			creating = 0
			detach(m, id)
			forget(m, id)
			changed()
		end
	end
	local function start_rename(id: number)
		rename_error:set("")
		renaming:set(id)
		select_only(id)
		local i = shape_peek().index[id]
		if i then
			scroll_into_view(i)
		end
	end
	local function end_rename(commit: boolean, text: string?)
		local id = renaming:peek()
		if id == 0 then
			return
		end
		if commit and text then
			local problem = if id == creating then finish_create(id, text) else apply_rename(id, text)
			if problem then
				rename_error:set(problem)
				return
			end
		elseif id == creating then
			drop_created()
		end
		renaming:set(0)
		rename_error:set("")
		focus_row(id)
	end
	local function create(dir: boolean, near: number)
		-- In the folder clicked (or the file's), opened.
		local parent = 0
		if near > 0 then
			parent = if nodes[near].dir then near else nodes[near].parent
		end
		if parent > 0 then
			set_open(parent, true, true)
		end
		drop_created()
		local id = M.add(m, parent, "", dir)
		-- (First among its kind while it's named.)
		detach(m, id)
		local list = siblings(m, parent)
		local at = 1
		if not dir then
			for i, k in list do
				at = i + 1
				if not nodes[k].dir then
					at = i
					break
				end
			end
		end
		table.insert(list, math.min(at, #list + 1), id)
		creating = id
		changed()
		start_rename(id)
	end
	local function move(ids: { number }, into: number): string?
		-- (`into` 0: the top.)
		for _, id in ids do
			if into > 0 and within(m, into, id) then
				return locale.s("ds.file_tree.into_itself")
			end
			local problem = name_problem(m, into, nodes[id].name, id)
			if problem and nodes[id].parent ~= into then
				return problem
			end
		end
		local moving: { number } = {}
		for _, id in ids do
			if nodes[id].parent ~= into then
				table.insert(moving, id)
			end
		end
		if #moving == 0 then
			return nil
		end
		if props.on_move then
			local refused = props.on_move(paths_of(moving), if into > 0 then nodes[into].path else "")
			if refused then
				return refused
			end
		end
		local a = active:peek()
		for _, id in moving do
			local was = nodes[id].path
			detach(m, id)
			attach(m, id, into)
			repath(m, id)
			if a and (a == was or string.sub(a, 1, #was + 1) == was .. "/") then
				opened_here = nodes[id].path .. string.sub(a, #was + 1)
				active:set(opened_here)
			end
		end
		if into > 0 then
			set_open(into, true)
		end
		roll_up(m)
		changed()
		selected = {}
		for _, id in moving do
			selected[id] = true
		end
		sel_v:set(sel_v:peek() + 1)
		return nil
	end
	local function delete(ids: { number }, confirmed: boolean?)
		if props.on_delete then
			local refused = props.on_delete(paths_of(ids), confirmed == true)
			if refused then
				if refused ~= "" then
					say(refused)
				end
				return
			end
		end
		-- The row after them takes the cursor.
		local s = shape_peek()
		local last = 0
		for _, id in ids do
			last = math.max(last, s.ends[id] or ((s.index[id] or 0) + 1))
		end
		local next_id = s.ids[last] or 0
		for _, id in ids do
			if nodes[id].parent >= 0 then
				detach(m, id)
				forget(m, id)
			end
		end
		roll_up(m)
		changed()
		select_only(if next_id > 0 and nodes[next_id].parent >= 0 then next_id else 0)
	end
	local function copy(text: string, what: string)
		ui.clipboard.write(text)
		say(what)
	end

	-- A row's menu (the selection's, if it's in it).
	local function menu_for(id: number): { ContextMenu.Entry }
		if not selected[id] then
			select_only(id)
		end
		local ids = targets(id)
		local n = nodes[id]
		local many = #ids > 1
		local paths = paths_of(ids)
		local abs: { string } = {}
		for _, p in paths do
			table.insert(abs, absolute(p))
		end
		local entries: { ContextMenu.Entry } = {}
		local function add(e: ContextMenu.Entry)
			table.insert(entries, e)
		end
		if not many then
			if n.dir then
				add({
					label = if wants_open(id) then locale.s("common.close") else locale.s("common.open"),
					icon = if wants_open(id) then "alt-arrow-down" else "alt-arrow-right",
					shortcut = "enter",
					on_select = function()
						set_open(id, not wants_open(id))
					end,
				})
			else
				add({ label = locale.s("common.open"), icon = "document", shortcut = "enter", on_select = function()
					open_file(id)
				end })
				if props.on_open_new then
					add({ label = locale.s("ds.file_tree.open_new_tab"), icon = "split-columns", shortcut = "cmd+enter", on_select = function()
						open_new(id)
					end })
				end
			end
			if props.on_reveal and (props.openable == nil or props.openable()) then
				add({ label = locale.s("ds.file_tree.reveal"), icon = "folder", shortcut = "alt+cmd+r", on_select = function()
					(props.on_reveal :: (string) -> ())(abs[1])
				end })
			end
			add({ separator = true })
			add({ label = locale.s("ds.file_tree.new_file"), icon = "document-add", on_select = function()
				create(false, id)
			end })
			add({ label = locale.s("ds.file_tree.new_folder"), icon = "folder", on_select = function()
				create(true, id)
			end })
			add({ separator = true })
		end
		-- (The shipping app's first: the file, or each of the selection,
		-- carried with the next message.)
		if props.on_add_to_chat then
			add({ label = locale.s("ds.file_tree.add_to_chat"), icon = "chat-round-line", on_select = function()
				(props.on_add_to_chat :: (paths: { string }, root: string?) -> ())(paths, m.root)
			end })
		end
		add({
			label = if many then locale.s("ds.file_tree.copy_paths") else locale.s("ds.file_tree.copy_path"),
			icon = "copy",
			shortcut = "alt+cmd+c",
			on_select = function()
				copy(table.concat(abs, "\n"), locale.s("ds.file_tree.copied", { n = #abs }))
			end,
		})
		add({
			label = if many then locale.s("ds.file_tree.copy_relative_paths") else locale.s("ds.file_tree.copy_relative_path"),
			shortcut = "alt+shift+cmd+c",
			on_select = function()
				copy(table.concat(paths, "\n"), locale.s("ds.file_tree.copied_relative", { n = #paths }))
			end,
		})
		if n.dir and not many then
			add({ separator = true })
			add({ label = locale.s("ds.file_tree.expand_all_inside"), icon = "expand-arrows", shortcut = "alt+right", on_select = function()
				set_open_deep(id, true)
			end })
			add({ label = locale.s("ds.file_tree.collapse_all_inside"), icon = "collapse-arrows", shortcut = "alt+left", on_select = function()
				set_open_deep(id, false)
			end })
		end
		add({ separator = true })
		if not many then
			add({ label = locale.s("ds.file_tree.rename"), icon = "pen", shortcut = "F2", on_select = function()
				start_rename(id)
			end })
		end
		-- What others add (plugins), asked for as it opens: after a rule,
		-- before Delete.
		if not many and props.more_menu then
			local extra = props.more_menu(n.path, if n.dir then "folder" else "file")
			if #extra > 0 then
				add({ separator = true })
				for _, e in extra do
					add(e)
				end
				add({ separator = true })
			end
		end
		local trash = props.trash ~= false
		add({
			label = locale.s(if trash then "ds.file_tree.trash" else "ds.file_tree.delete", { n = #ids }),
			icon = "trash-bin-minimalistic",
			shortcut = "cmd+backspace",
			danger = true,
			confirm = if trash
				then (if many
					then locale.s("ds.file_tree.confirm_trash_many", { n = #ids })
					else locale.s("ds.file_tree.confirm_trash_one", { name = n.name }))
				else (if many
					then locale.s("ds.file_tree.confirm_delete_many", { n = #ids })
					else locale.s("ds.file_tree.confirm_delete_one", { name = n.name })),
			on_select = function()
				delete(ids, true)
			end,
		})
		return entries
	end
	-- The empty list's menu.
	local function area_menu(): { ContextMenu.Entry }
		return {
			{ label = locale.s("ds.file_tree.new_file"), icon = "document-add", on_select = function()
				create(false, 0)
			end },
			{ label = locale.s("ds.file_tree.new_folder"), icon = "folder", on_select = function()
				create(true, 0)
			end },
			{ separator = true },
			{ label = locale.s("ds.file_tree.collapse_all_folders"), icon = "collapse-arrows", on_select = collapse_all },
			{
				label = locale.s("ds.file_tree.show_ignored_files"),
				icon = "eye",
				checked = function(): boolean
					return show_ignored:get()
				end,
				on_select = function()
					show_ignored:set(not show_ignored:peek())
				end,
			},
			{ separator = true },
			{ label = locale.s("ds.file_tree.copy_path"), icon = "copy", on_select = function()
				copy(m.root or m.name, locale.s("ds.file_tree.copied", { n = 1 }))
			end },
		}
	end

	-- The keys.
	local typed, typed_at = "", 0
	local function visible_ids(): { number }
		return shape_peek().ids
	end
	local function step(by: number, extend: boolean)
		local s = shape_peek()
		local ids = s.ids
		if #ids == 0 then
			return
		end
		local i = s.index[cursor:peek()] or (if by > 0 then 0 else #ids + 1)
		local j = math.clamp(i + by, 1, #ids)
		while ids[j] == 0 and j > 1 and j < #ids do
			j += if by > 0 then 1 else -1
		end
		if ids[j] > 0 then
			go_to(ids[j], if extend then "extend" else "only")
		end
	end
	local function type_ahead(ch: string)
		local now = ui.now()
		local again = now - typed_at < TYPEAHEAD
		typed = if again then typed .. string.lower(ch) else string.lower(ch)
		typed_at = now
		say(typed)
		local ids = visible_ids()
		local s = shape_peek()
		local from = s.index[cursor:peek()] or 0
		-- The same letter again steps through those it starts; more letters
		-- stay on the row while it still matches.
		local same = string.rep(string.sub(typed, 1, 1), #typed) == typed
		local start = if same or not again then from + 1 else math.max(from, 1)
		local q = if same then string.sub(typed, 1, 1) else typed
		for k = 0, #ids - 1 do
			local i = (start - 1 + k) % #ids + 1
			local id = ids[i]
			if id > 0 and string.sub(nodes[id].lower, 1, #q) == q then
				go_to(id)
				return
			end
		end
	end
	local function on_key(e: any): boolean?
		if renaming:peek() > 0 then
			return nil
		end
		if e.editing then
			return nil
		end
		local id = cursor:peek()
		local n = if id > 0 then nodes[id] else nil
		local k = e.key
		if k == "ArrowDown" then
			step(1, e.shift)
		elseif k == "ArrowUp" then
			step(-1, e.shift)
		elseif k == "PageDown" or k == "PageUp" then
			step((if k == "PageDown" then 1 else -1) * math.max(1, math.floor((list_view.height or 0) / ROW) - 1), e.shift)
		elseif k == "Home" then
			local ids = visible_ids()
			if ids[1] then
				go_to(ids[1], if e.shift then "extend" else "only")
			end
		elseif k == "End" then
			local ids = visible_ids()
			local j = #ids
			while j > 0 and ids[j] == 0 do
				j -= 1
			end
			if j > 0 then
				go_to(ids[j], if e.shift then "extend" else "only")
			end
		elseif k == "ArrowRight" and n then
			if n.dir and e.alt then
				set_open_deep(id, true)
			elseif n.dir and not wants_open(id) then
				set_open(id, true)
			elseif n.dir and n.kids then
				local s = shape_peek()
				for _, kid in n.kids do
					if s.index[kid] then
						go_to(kid)
						break
					end
				end
			end
		elseif k == "ArrowLeft" and n then
			if n.dir and e.alt then
				set_open_deep(id, false)
			elseif n.dir and wants_open(id) then
				set_open(id, false)
			elseif n.parent > 0 then
				go_to(n.parent)
			end
		elseif k == "Enter" and e.cmd and n and props.on_open_new then
			open_new(id)
		elseif (k == "Enter" or k == " ") and n then
			open_file(id)
		elseif k == "F2" and n then
			start_rename(id)
		elseif k == "Backspace" and e.cmd and n then
			delete(targets(id))
		elseif e.cmd and k == "a" then
			selected = {}
			for _, i in visible_ids() do
				if i > 0 then
					selected[i] = true
				end
			end
			sel_v:set(sel_v:peek() + 1)
		elseif e.cmd and e.alt and k == "c" and n then
			local ps = paths_of(targets(id))
			for i, p in ps do
				ps[i] = absolute(p)
			end
			copy(table.concat(ps, "\n"), locale.s("ds.file_tree.copied", { n = #ps }))
		elseif k == "Escape" then
			if filter:peek() ~= "" then
				filter:set("")
			elseif id > 0 and #targets(id) > 1 then
				select_only(id)
			else
				-- (Nothing of its own to undo: Esc goes on, to what holds the
				-- tree — a floating one closes.)
				return nil
			end
		elseif #k == 1 and not e.cmd and not e.ctrl and not e.alt and k ~= " " then
			type_ahead(k)
		else
			return nil
		end
		return true
	end

	-- A row.
	local function guides(id: number): Node
		local n = nodes[id]
		local d = n.depth
		if d == 0 then
			return ui.div {}
		end
		-- Whether each of its folders' guides runs on past it: so while a
		-- row on the way down isn't its folder's last.
		local function last_kid(i: number): boolean
			local p = nodes[i].parent
			local list = siblings(m, p)
			local s = shape_peek()
			for j = #list, 1, -1 do
				if s.index[list[j]] then
					return list[j] == i
				end
			end
			return true
		end
		return ui.canvas {
			absolute = true,
			left = 0,
			top = 0,
			w = PAD + INSET + d * INDENT + 1,
			h = ROW,
			pointer_events = "none",
			draw = function(c, _, h)
				shape()
				local cur = cursor:get()
				local here = if cur > 0 and nodes[cur] then (if nodes[cur].dir and wants_open(cur) and false then cur else nodes[cur].parent) else 0
				local i = id
				local mid = h / 2
				for level = d - 1, 0, -1 do
					local p = nodes[i].parent
					local x = PAD + INSET + level * INDENT + CHEV / 2
					local ink = if p == here then "$zeron.files.guide" else "$zeron.tree"
					local lastk = last_kid(i)
					if i == id then
						if lastk then
							c:stroke(
								ui.path():move(x, 0):line(x, mid - BEND):quad(x, mid, x + BEND, mid),
								ink,
								1
							)
						else
							c:line(x, 0, x, h, ink, 1)
						end
					elseif not lastk then
						c:line(x, 0, x, h, ink, 1)
					end
					i = p
				end
			end,
		}
	end
	local function name_node(id: number, strong: () -> boolean): Node
		local n = nodes[id]
		local text_ref, clip_ref = ui.ref(), ui.ref()
		local function ink(): string
			-- (Its status can change with nothing else: git's, newly heard.)
			if props.revision then
				props.revision:get()
			end
			local st = if n.dir then nil else n.status
			if st then
				return `$zeron.git.{st}`
			end
			return if strong() then "$text.primary" else "$text.secondary"
		end
		local inner: Node
		local where = if hits[id] and marking.mode ~= "path" then matched(n.lower, marking.q) else nil
		if where and #where > 0 and applied:peek() ~= "" then
			local spans: RichProps = {}
			local mark: { [number]: boolean } = {}
			for _, w in where do
				mark[w] = true
			end
			local run, run_on = "", false
			local function flush()
				if run ~= "" then
					-- (A span's weight takes a number, not a token: the theme's
					-- medium, read as a number.)
					spans[#spans + 1] = if run_on
						then ui.span { run, literal = true, color = "$zeron.match.ink", bg = "$zeron.match.wash", weight = MATCH_WEIGHT }
						else ui.span { run, literal = true }
				end
				run = ""
			end
			for i = 1, #n.name do
				local on = mark[i] == true
				if on ~= run_on then
					flush()
					run_on = on
				end
				run ..= string.sub(n.name, i, i)
			end
			flush()
			spans.ref = text_ref
			spans.shrink = 0
			spans.text_size = "$font.size.md"
			spans.color = ink
			inner = ui.rich(spans)
		else
			inner = ui.text {
				n.name,
				literal = true,
				ref = text_ref,
				shrink = 0,
				text_size = "$font.size.md",
				color = ink,
			}
		end
		-- Fading where it doesn't fit (FadeText's way, for runs too).
		return ui.row {
			ref = clip_ref,
			grow = 1,
			min_w = 0,
			overflow = "hidden",
			items = "center",
			fade_x = function()
				local over = (text_ref.width or 0) > (clip_ref.width or 0) + 0.5
				return { 0, if over then 24 else 0 }
			end,
			inner,
		}
	end
	-- Its git status at its end: a file's letter, a folder's dot.
	local function status_node(id: number): Node?
		local n = nodes[id]
		if n.dir then
			local r = n.roll
			if not r then
				return nil
			end
			return ui.div {
				-- (Said, as it's shown: what's inside it.)
				role = "img",
				label = locale.t(HOLDS[r]),
				w = 16,
				h = 16,
				shrink = 0,
				items = "center",
				justify = "center",
				tooltip = locale.s(HOLDS[r]),
				ui.div { w = 6, h = 6, rounded = "$radius.full", bg = `$zeron.git.{r}`, opacity = 0.9 },
			}
		end
		local st = n.status
		if not st then
			return nil
		end
		return ui.text {
			LETTER[st],
			tooltip = locale.s(SAID[st]),
			w = 16,
			shrink = 0,
			align = "center",
			text_size = "$font.size.xs",
			weight = "$font.weight.semibold",
			color = `$zeron.git.{st}`,
			tabular = true,
		}
	end
	local function chevron(id: number): Node
		local turned = motion.tween(
			ui.computed(function(): number
				version:get()
				return if wants_open(id) then 90 else 0
			end),
			{ duration = OPEN, ease = "out(3)" }
		)
		return ui.div {
			w = CHEV,
			h = CHEV,
			shrink = 0,
			items = "center",
			justify = "center",
			color = "$text.tertiary",
			rotate = turned,
			Icon { name = "alt-arrow-right", size = 12 },
		}
	end
	-- The inline field a name is typed into (renaming, or a new file).
	local function rename_field(id: number): Node
		local n = nodes[id]
		local text = ui.use_state(n.name)
		local field = ui.ref()
		return ui.column {
			grow = 1,
			min_w = 0,
			relative = true,
			ui.input(EditorMenu.on({
				ref = field,
				value = text,
				label = if id == creating
					then locale.t(if n.dir then "ds.file_tree.new_folder_name" else "ds.file_tree.new_file_name")
					else locale.t("ds.file_tree.rename_named", { name = n.name }),
				placeholder = if id == creating then locale.t(if n.dir then "ds.file_tree.folder_name" else "ds.file_tree.file_name") else nil,
				w_full = true,
				h = ROW - 4,
				px = "$space.1",
				py = 0,
				text_size = "$font.size.md",
				rounded = "$radius.sm",
				bg = "$surface.sunken",
				border = 1,
				border_color = function(): string
					return if rename_error:get() ~= "" then "$danger" else "$focus.ring"
				end,
				on_mount = function(el)
					ui.after(0, function()
						el:focus()
						-- The name without its extension, as Finder selects it.
						local dot = string.find(n.name, "%.[^%.]*$")
						local stop = if dot and dot > 1 and not n.dir then dot - 1 else #n.name
						el:set_selection(0, utf8.len(string.sub(n.name, 1, stop)) or stop)
					end)
				end,
				on_key = function(e): boolean?
					if e.key == "Enter" then
						end_rename(true, text:peek())
						return true
					elseif e.key == "Escape" then
						end_rename(false)
						return true
					end
					rename_error:set("")
					return nil
				end,
				on_blur = function()
					if renaming:peek() ~= id then
						return
					end
					-- Left: kept if it's fine, else let go.
					local t = text:peek()
					if t ~= n.name and name_problem(m, n.parent, t, id) == nil then
						end_rename(true, t)
					else
						end_rename(false)
					end
				end,
			})),
		}
	end

	-- The rows' drag: what it carries, and where it can land.
	local function drop_target(id: number): number
		-- A file's folder; the top is -1.
		if id <= 0 then
			return -1
		end
		local n = nodes[id]
		if n.dir then
			return id
		end
		return if n.parent > 0 then n.parent else -1
	end
	local function can_drop(payload: any, into: number): boolean
		if type(payload) ~= "table" or not payload.tree_files then
			return false
		end
		local folder = if into == -1 then 0 else into
		for _, id in payload.tree_files do
			if nodes[id].parent == folder then
				return false
			end
			if folder > 0 and within(m, folder, id) then
				return false
			end
		end
		return true
	end
	-- Whether row `id` is inside where a drag would land (its rows washed).
	local function in_drop(id: number): boolean
		local d = drop_at:get()
		if d <= 0 then
			return false
		end
		return within(m, id, d)
	end

	local function row(id: number, index: State<number>): Node
		if id == 0 then
			return ui.div { w_full = true }
		end
		local n = nodes[id]
		local hovered = ui.use_state(false)
		local ref = ui.ref()
		row_refs[id] = ref
		local function sel(): boolean
			return is_selected(id)
		end
		local function strong(): boolean
			return sel() or active:get() == n.path
		end
		-- Its neighbours' selection, so one selection's washes run together.
		local function sel_at(i: number): boolean
			local s = shape()
			local other = s.ids[i]
			return other ~= nil and other > 0 and is_selected(other)
		end
		local function joined_up(): boolean
			return sel() and sel_at(index:get() - 1)
		end
		local function joined_down(): boolean
			return sel() and sel_at(index:get() + 1)
		end
		-- A row's wash: square-cornered, the look the user chose (Nov 1) over
		-- the sidebar's rounded rows, so no corners to work out; a
		-- selection's rows run together, their gaps closed.
		local wash = ui.div {
			absolute = true,
			left = PAD,
			right = PAD,
			top = function(): number
				return if joined_up() then 0 else 1
			end,
			bottom = function(): number
				return if joined_down() then 0 else 1
			end,
			bg = function(): string
				if drop_at:get() == id then
					return "$zeron.files.drop"
				end
				if sel() then
					return if focused:get() then "$zeron.selected.flat.bg" else "$hover"
				end
				if active:get() == n.path then
					return "$hover"
				end
				if in_drop(id) then
					return "$zeron.files.drop"
				end
				return if hovered:get() then "$hover" else "#00000000"
			end,
			pointer_events = "none",
		}
		local x0 = PAD + INSET + n.depth * INDENT
		local content = ui.row {
			absolute = true,
			left = x0,
			right = PAD + INSET,
			top = 0,
			h = ROW,
			items = "center",
			gap = "$zeron.space.tight",
			opacity = if n.ignored then 0.5 else 1,
			ui.row {
				shrink = 0,
				items = "center",
				if n.dir then chevron(id) else ui.div { w = CHEV, shrink = 0 },
				FileIcon { name = n.name, dir = n.dir, size = ICON },
			},
			-- (The field is built only while it's renamed.)
			ui.presence(function(): string
				return if renaming:get() == id then "rename" else "name"
			end, function(mode: string): Node
				return if mode == "rename" then rename_field(id) else name_node(id, strong)
			end, { grow = 1, min_w = 0, h_full = true, items = "center" }),
			-- Its git mark, as it stands (a status newly heard swaps it; nothing
			-- else does).
			ui.show(function(): boolean
				return renaming:get() ~= id
			end, ui.presence(function(): string
				if props.revision then
					props.revision:get()
				end
				local n2 = nodes[id]
				if not n2 then
					return ""
				end
				return if n2.dir then `d:{n2.roll or ""}` else `f:{n2.status or ""}`
			end, function(_key: string): Node
				return status_node(id) or ui.div {}
			end, { shrink = 0, items = "center" })),
		}
		local body = ui.div {
			ref = ref,
			w_full = true,
			h = ROW,
			relative = true,
			focusable = true,
			role = "treeitem",
			label = n.name,
			-- (What plugins place by, and what's under them: the row's file,
			-- as a token: its path is "files.read"'s, ds/token.)
			class = "file-row",
			provide = { file = token.file(n.path) },
			focus = { border = 0 },
			cursor = "default",
			on_hover = function(inside: boolean)
				hovered:set(inside)
			end,
			on_focus = function()
				focused:set(true)
				if cursor:peek() ~= id and not selected[id] then
					select_only(id)
				end
			end,
			on_blur = function()
				focused:set(false)
			end,
			on_press = function(e)
				if renaming:peek() == id then
					return
				end
				if e.clicks and e.clicks >= 2 then
					return
				end
				if e.cmd then
					toggle_selected(id)
				elseif e.shift then
					select_range(id)
				elseif not selected[id] then
					select_only(id)
				end
			end,
			-- (A middle click: a file in a tab of its own.)
			on_aux_click = function(e)
				if e.button == "middle" and renaming:peek() ~= id then
					open_new(id)
				end
			end,
			on_click = function(e)
				if renaming:peek() == id or e.cmd or e.shift then
					return
				end
				select_only(id)
				if n.dir and e.alt then
					set_open_deep(id, not wants_open(id))
				else
					open_file(id)
				end
			end,
			-- Dragged: the selection, if it's in it.
			on_drag = function()
				if renaming:peek() == id then
					return nil
				end
				dragging:set(true)
				-- (Its paths too, for a drop elsewhere: the chat mentions them.)
				local ids = targets(id)
				return { tree_files = ids, paths = paths_of(ids), root = m.root }
			end,
			drag_preview = function(payload: any)
				local count = #payload.tree_files
				local first = nodes[payload.tree_files[1]]
				-- A card of the menus' glass, a row's corner (the sidebar's
				-- dragged rows are the same thing).
				local card: ContainerProps = {
					h = ROW,
					px = "$space.2",
					gap = "$zeron.space.tight",
					items = "center",
					rounded = "$radius.md",
					FileIcon { name = first.name, dir = first.dir, size = ICON },
					FadeText {
						if count > 1 then `{count} items` else first.name,
						literal = true,
						text_size = "$font.size.md",
						color = "$text.primary",
						max_w = 220,
					},
				}
				glass.style(card, "menu")
				return ui.row(card)
			end,
			drag_preview_anchor = { 0, 0.5 },
			drag_preview_offset = { 12, 0 },
			drag_export = function(payload: any)
				local files: { string } = {}
				for _, i in payload.tree_files do
					table.insert(files, absolute(nodes[i].path))
				end
				return { text = table.concat(files, "\n"), files = if m.root then files else nil }
			end,
			on_drag_end = function()
				dragging:set(false)
				drop_at:set(0)
			end,
			drop_when = function(payload: any): boolean
				return can_drop(payload, drop_target(id))
			end,
			on_drag_over = function(payload: any, e)
				drag_y = e.y
				drop_at:set(drop_target(id))
			end,
			on_drag_leave = function()
				if drop_at:peek() == drop_target(id) then
					drop_at:set(0)
				end
			end,
			on_drop = function(payload: any)
				local into = drop_target(id)
				drop_at:set(0)
				local problem = move(payload.tree_files, if into == -1 then 0 else into)
				if problem then
					say(problem)
				end
			end,
			-- Held over a closed folder, it opens.
			on_drag_spring = if n.dir
				then function(): boolean
					if not wants_open(id) then
						set_open(id, true)
						return true
					end
					return false
				end
				else nil,
			wash,
			guides(id),
			content,
		}
		return ContextMenu {
			label = `{n.name} actions`,
			ring = "$radius.md",
			w_full = true,
			h = function(): number
				local s = shape()
				local i = s.index[id]
				return if i then heights:get()[i] or ROW else ROW
			end,
			overflow = function(): string
				return if renaming:get() == id then "visible" else "hidden"
			end,
			opacity = function(): number
				local s = shape()
				local i = s.index[id]
				local chain = if i then s.chain[i] else nil
				if not chain then
					return 1
				end
				local t = clock:get()
				local o = 1
				for _, f in chain do
					o *= value(f.o, t)
				end
				return o
			end,
			items = function()
				return menu_for(id)
			end,
			body,
		}
	end

	-- The folders kept at the top: those whose rows have scrolled under
	-- the stack, outermost first; each pushed up as its last row passes.
	type Stuck = { id: number, row: number, y: number }
	local stuck = ui.computed(function(): { Stuck }
		local s = shape()
		local o = list_offset:get()
		heights:get()
		local out: { Stuck } = {}
		if #s.ids == 0 or o <= 0 then
			return out
		end
		local chain: { number } = {}
		while #chain < MAX_STICKY do
			local r = row_at(o + #chain * ROW)
			local id = s.ids[r]
			-- (A rest row: the row before it says where it is.)
			while id == 0 and r > 1 do
				r -= 1
				id = s.ids[r]
			end
			if id <= 0 then
				break
			end
			-- Its folders, outermost first.
			local up: { number } = {}
			local p = nodes[id].parent
			while p > 0 do
				table.insert(up, 1, p)
				p = nodes[p].parent
			end
			local nxt = up[#chain + 1]
			if not nxt or not s.index[nxt] then
				break
			end
			if top_of(s.index[nxt]) >= o + #chain * ROW then
				break
			end
			table.insert(chain, nxt)
		end
		for k, id in chain do
			local i = s.index[id]
			local bottom = top_of(s.ends[id] or (#s.ids + 1)) - o
			table.insert(out, { id = id, row = i, y = math.min((k - 1) * ROW, bottom - ROW) })
		end
		return out
	end)
	-- (A folder's row in the stack is its own for as long as it stays
	-- there: `k`, its place in the stack, read as the stack stands.)
	local function stuck_row(id: number): Node
		local function k_of(): number
			for k, st in stuck:get() do
				if st.id == id then
					return k
				end
			end
			return 0
		end
		local n = nodes[id]
		-- (Inset as a row's wash is; its words where the row's are.)
		local x0 = INSET + n.depth * INDENT
		return ui.div {
			absolute = true,
			left = PAD,
			right = PAD,
			h = ROW,
			top = function(): number
				local st = stuck:get()[k_of()]
				return if st then st.y else -ROW
			end,
			-- No fill: the rows are cut away under the stack (the list's clip),
			-- so the panel's own surface shows, whatever it is. One pushed up
			-- under the one above it is cut where that one ends.
			clip_inset = function(): { number }
				local st = stuck:get()
				local k = k_of()
				local me, above = st[k], st[k - 1]
				local cut = if me and above then math.max(0, above.y + ROW - me.y) else 0
				return { cut, 0, 0, 0 }
			end,
			cursor = "pointer",
			label = locale.t("ds.file_tree.go_to", { name = n.name }),
			on_click = function()
				local i = shape_peek().index[id]
				if i then
					list_offset:set(math.max(0, top_of(i) - (k_of() - 1) * ROW))
				end
				go_to(id)
			end,
			rounded = "$radius.md",
			hover = { bg = "$hover" },
			ui.row {
				absolute = true,
				left = x0,
				right = INSET,
				top = 0,
				h = ROW,
				items = "center",
				gap = "$zeron.space.tight",
				ui.row {
					shrink = 0,
					items = "center",
					ui.div {
						w = CHEV,
						h = CHEV,
						items = "center",
						justify = "center",
						color = "$text.tertiary",
						rotate = 90,
						Icon { name = "alt-arrow-right", size = 12 },
					},
					FileIcon { name = n.name, dir = true, size = ICON },
				},
				name_node(id, function(): boolean
					return false
				end),
			},
		}
	end
	-- The stack: each folder's row keyed by the folder, deepest first (one
	-- pushed up slides under those above it), so a folder joining or leaving
	-- as the list scrolls leaves the rest where they are.
	local stack_ids = ui.computed(function(): { number }
		local st = stuck:get()
		local out: { number } = {}
		for j = #st, 1, -1 do
			table.insert(out, st[j].id)
		end
		return out
	end)
	local sticky = ui.div {
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		pointer_events = "none",
		ui.each(stack_ids, stuck_row, {
			key = function(id: number): number
				return id
			end,
		}),
		-- Under the stack, a hairline where the rows pass under it.
		ui.show(
			function(): boolean
				return #stuck:get() > 0
			end,
			ui.div {
				absolute = true,
				left = 0,
				right = 0,
				h = 1,
				bg = "$border",
				top = function(): number
					local st = stuck:get()
					local last = st[#st]
					return if last then last.y + ROW else 0
				end,
			}
		),
	}

	-- A drag near the list's top or bottom scrolls it.
	local function step_scroll(): boolean
		local view = list_view:peek()
		local y = drag_y
		if not view or not y then
			return true
		end
		local top = view.y or 0
		local bottom = top + (view.height or 0)
		local d = 0
		if y < top + SCROLL_BAND then
			d = -SCROLL_MAX * math.clamp((top + SCROLL_BAND - y) / SCROLL_BAND, 0, 1)
		elseif y > bottom - SCROLL_BAND then
			d = SCROLL_MAX * math.clamp((y - (bottom - SCROLL_BAND)) / SCROLL_BAND, 0, 1)
		end
		if d ~= 0 then
			local most = math.max(0, total() - (view.height or 0))
			list_offset:set(math.clamp(list_offset:peek() + d, 0, most))
		end
		return true
	end
	ui.effect(function()
		if dragging:get() then
			ui.every(16, step_scroll)
		end
	end)

	-- Where the stack at the top ends: the rows are cut away above it.
	local function stack_bottom(): number
		local st = stuck:get()
		local last = st[#st]
		return if last then math.max(0, last.y + ROW) else 0
	end
	local list = ui.list {
		ref = list_view,
		clip_inset = function(): { number }
			-- (Past the hairline under the stack too: a guide running on under
			-- it would cross it, and the two faint inks would add up.)
			local bottom = stack_bottom()
			return { if bottom > 0 then bottom + 1 else 0, 0, 0, 0 }
		end,
		w_full = true,
		grow = 1,
		h = 0,
		offset = list_offset,
		-- (Scrolled by hand: a place being held from before is let go.)
		on_wheel = function(): boolean?
			wanted = nil
			return nil
		end,
		-- (Its scrollbar is the rail over it, below: the engine's bar is the
		-- list's own child, so its fades and its clip under the stack would
		-- take the bar with the rows.)
		overscan = 4,
		heights = function(): { number }
			return heights:get()
		end,
		key = function(i: number): string
			local s = shape_peek()
			local id = s.ids[i]
			if id == 0 then
				return `rest:{i}`
			end
			return tostring(id)
		end,
		row = function(key: string, index: State<number>): Node
			local id = tonumber(key) or 0
			return row(id, index)
		end,
		row_text = function(i: number): string
			local id = shape_peek().ids[i]
			return if id and id > 0 then nodes[id].name else ""
		end,
		fade = function(): { number }
			local o = list_offset:get()
			heights:get()
			local over = math.max(0, total() - (list_view.height or 0))
			local top_fade = if #stuck:get() > 0 then 0 else math.min(o, 12)
			return { top_fade, math.min(math.max(0, over - o), 12) }
		end,
		-- Dropped past the rows: the top.
		drop_when = function(payload: any): boolean
			return can_drop(payload, -1)
		end,
		on_drag_over = function(payload: any, e)
			drag_y = e.y
			local view = list_view:peek()
			if view and e.y - (view.y or 0) + list_offset:peek() >= total() then
				drop_at:set(-1)
			end
		end,
		on_drop = function(payload: any)
			-- (Only past the rows: over one that refused, nothing.)
			local here = drop_at:peek()
			drop_at:set(0)
			if here ~= -1 then
				return
			end
			local problem = move(payload.tree_files, 0)
			if problem then
				say(problem)
			end
		end,
	}

	-- The filter at the top.
	local filter_ref = ui.ref()
	-- Beside it, the tree's own actions: quiet until touched, a step
	-- firmer while what they toggle is on.
	-- (`key`: its name's message; the tooltip a string, read as it's built.)
	local function tool(key: string, icon: Icon.Name | () -> Icon.Name, on_click: () -> (), on: (() -> boolean)?): Node
		local button: IconButtonModule.Props = {
			label = locale.t(key),
			tooltip = locale.s(key),
			icon = icon,
			size = "lg",
			tone = (function(): string
				return if on and on() then "normal" else "quiet"
			end) :: any,
			-- (Its tooltip under it: above is the pane's edge.)
			tooltip_side = "bottom",
			on_click = on_click,
		}
		if floating then
			-- (By the panel's corner: concentric with it.)
			button.rounded = ui.raw(control.nest(FLOAT_CORNER, 1, BAND_ROOM), "concentric with the floating panel's corner")
		end
		return IconButton(button)
	end
	-- Room kept at the head's start for a control laid over the pane.
	local function lead(): number
		local l = props.lead
		return if type(l) == "function" then l() else (l or 0)
	end
	local as_band = props.head ~= nil
	local field = ui.row {
		-- (The files' part of the pane's band: what plugins place by.)
		id = "files-header",
		w_full = true,
		h = props.head or 32,
		-- The list's own inset, so the field and the rows' washes share
		-- their edges, on both sides; at its start, the room kept.
		pl = function(): any
			return ui.raw(PAD + lead(), "the list's inset, past the room kept for the pane's control")
		end,
		-- (In the 40 band, as far from the end as from its top and foot.)
		pr = ui.raw(if as_band then BAND_ROOM else PAD, "as far from the end as the field is from the band's top and foot"),
		-- As a band, the hairline the file's header carries on runs under
		-- it; on its own, a little air under instead.
		pb = if as_band then nil else "$space.1",
		-- (Cut under a control as it slides, the hairline stays whole: the
		-- box round it draws it then.)
		border_sides = if as_band and not props.head_cut then { 0, 0, 1, 0 } else nil,
		border_color = "$border.subtle",
		gap = "$space.1",
		items = "center",
		shrink = 0,
		ui.row {
			grow = 1,
			min_w = 0,
			-- (As tall as the tools beside it; on its own, the row less its
			-- air.)
			h = if as_band then "$zeron.icon_button.lg" else nil,
			h_full = if as_band then nil else true,
			px = "$space.2",
			gap = "$space.2",
			items = "center",
			rounded = if as_band then ui.raw(control.nest(if floating then FLOAT_CORNER else "$radius.xl", 1, BAND_ROOM), "concentric with the pane: its corner less its edge and the band's room") else "$radius.md",
			bg = "$zeron.secondary.bg",
			color = "$text.tertiary",
			Icon { name = "search", size = 14 },
			ui.input(EditorMenu.on({
				ref = filter_ref,
				value = filter,
				label = locale.t("ds.file_tree.filter"),
				placeholder = locale.t("ds.file_tree.filter"),
				grow = 1,
				h = 28,
				px = 0,
				py = 0,
				bg = "#00000000",
				border = 0,
				text_size = "$font.size.md",
				focus = { border = 0 },
				on_key = function(e): boolean?
					if e.key == "Escape" then
						if filter:peek() ~= "" then
							filter:set("")
						else
							local c = cursor:peek()
							local first = shape_peek().ids[1] or 0
							go_to(if c > 0 and shape_peek().index[c] then c else first)
						end
						return true
					elseif e.key == "ArrowDown" or e.key == "Enter" then
						-- Into the rows: the first match.
						local s = shape_peek()
						for _, id in s.ids do
							if id > 0 and (applied:peek() == "" or (hits[id] and not nodes[id].dir)) then
								if e.key == "Enter" then
									go_to(id)
									open_file(id)
								else
									go_to(id)
								end
								break
							end
						end
						return true
					end
					return nil
				end,
			})),
			ui.show(function(): boolean
				return filter:get() ~= ""
			end, ui.row {
				gap = "$space.1",
				items = "center",
				shrink = 0,
				ui.text {
					function(): string
						local c = match_count:get()
						return if c == 1 then "1 file" else `{c} files`
					end,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
					tabular = true,
				},
				-- (Smaller than an icon button's least: it sits inside the
				-- field, beside its count.)
				IconButton {
					label = locale.t("ds.file_tree.clear_filter"),
					tooltip = locale.s("common.clear"),
					tone = "quiet",
					size = "sm",
					w = 18,
					h = 18,
					rounded = ui.raw(control.nest("$radius.md", 5), "concentric with the field: its corner less the room round the ×"),
					on_click = function()
						filter:set("")
					end,
					Icon { name = "close", size = 12 },
				},
			}, ui.div {
				shrink = 0,
				Kbd { text = "/", tone = "quiet" },
			}),
		},
		-- Its tools, a pair.
		ui.row {
			hook = if props.hooks then props.hooks.tools else nil,
			gap = "$zeron.space.hair",
			items = "center",
			shrink = 0,
			tool("ds.file_tree.show_ignored_files", function(): Icon.Name
				return if show_ignored:get() then "eye" else "eye-closed"
			end, function()
				show_ignored:set(not show_ignored:peek())
			end, function(): boolean
				return show_ignored:get()
			end),
			tool("ds.file_tree.collapse_all_folders", "collapse-arrows", collapse_all),
		},
	}

	-- With nothing to show: why, and the way out.
	local empty = ui.show(function(): boolean
		-- (Nothing said while the top is still being read.)
		return #shape().ids == 0 and not (props.loading and props.loading())
	end, ui.column {
		absolute = true,
		left = 0,
		right = 0,
		top = 24,
		items = "center",
		gap = "$space.2",
		px = "$space.4",
		ui.text {
			function(): string
				local q = applied:get()
				if q ~= "" then
					return locale.s("ds.file_tree.no_match", { q = q })
				end
				return locale.s("ds.file_tree.empty")
			end,
			literal = true,
			text_size = "$font.size.md",
			color = "$text.secondary",
			align = "center",
			wrap = true,
		},
		ui.show(function(): boolean
			return filter:get() ~= ""
		end, ui.button {
			label = locale.t("ds.file_tree.clear_filter"),
			h = 24,
			px = "$space.2",
			rounded = "$radius.lg",
			bg = "$zeron.secondary.bg",
			hover = { bg = "$zeron.secondary.hover" },
			active = { bg = "$zeron.secondary.active" },
			on_click = function()
				filter:set("")
			end,
			ui.text { locale.t("ds.file_tree.clear_filter"), text_size = "$font.size.sm", color = "$text.secondary" },
		}),
	})

	-- A name refused, under the field (over the rows below it).
	local rename_err = ui.show(function(): boolean
		return renaming:get() > 0 and rename_error:get() ~= ""
	end, ui.div {
		absolute = true,
		left = function(): number
			local id = renaming:get()
			local n = nodes[id]
			return if n then PAD + INSET + n.depth * INDENT + CHEV + ICON else 0
		end,
		right = PAD,
		top = function(): number
			heights:get()
			local i = shape().index[renaming:get()]
			return if i then top_of(i) + ROW - list_offset:get() else 0
		end,
		px = "$space.2",
		py = "$space.1",
		rounded = "$radius.sm",
		bg = "$zeron.menu.solid",
		border = 1,
		border_color = "$danger",
		shadow = "$shadow.md",
		pointer_events = "none",
		ui.text { function(): string
			return rename_error:get()
		end, text_size = "$font.size.sm", color = "$danger", literal = true, wrap = true },
	})

	-- The note at the foot: what's being typed, or what was just done.
	local foot_note = ui.div {
		absolute = true,
		left = 0,
		right = 0,
		-- (Insets take points, not spacing tokens.)
		bottom = 8,
		items = "center",
		pointer_events = "none",
		-- One note while there is something to say, its words following what
		-- is typed (keeping the last as it fades).
		ui.presence(function(): string?
			return if note:get() == "" then nil else "note"
		end, function(): Node
			local said = note:peek()
			-- The menus' glass, with a closer shadow: a note, not a panel.
			local note_props = glass.style({
				h = 22,
				px = "$space.2",
				items = "center",
				rounded = "$radius.md",
				ui.text {
					function(): string
						local t = note:get()
						if t ~= "" then
							said = t
						end
						return said
					end,
					text_size = "$font.size.sm",
					color = "$text.secondary",
				},
			}, "menu")
			note_props.shadow = "$shadow.md"
			return ui.row(note_props)
		end, {
			enter = motion.transition({ opacity = { 0, 1 }, y = { 4, 0 }, duration = 140 }),
			exit = motion.transition({ opacity = 0, duration = 160 }),
		}),
	}

	-- The box: other props are its own.
	local box: { [any]: any } = {}
	for k, v in props do
		if not BOX_KEYS[k] then
			box[k] = v
		end
	end
	box.ref = root_ref
	-- Under a control laid over the pane (its toggle) as the tree slides:
	-- the head goes on sliding but is cut where that control's room ends,
	-- fading out as it passes under it, so the control reads alone. (A
	-- window over the head whose start stays at the cut, the head inside
	-- it put back where it would be.)
	local head_node: Node? = if props.search ~= false then field else nil
	if head_node and props.head_cut then
		local head_cut = props.head_cut
		local band_w = ui.use_state(if type(props.w) == "number" then props.w else 0)
		local function cut(): number
			return math.clamp(head_cut(), 0, band_w:get())
		end
		head_node = ui.div {
			relative = true,
			w_full = true,
			h = props.head or 32,
			shrink = 0,
			border_sides = if as_band then { 0, 0, 1, 0 } else nil,
			border_color = "$border.subtle",
			on_resize = function(size: { w: number, h: number })
				band_w:set(size.w)
			end,
			ui.div {
				absolute = true,
				top = 0,
				bottom = 0,
				left = cut,
				w = function(): number
					return math.max(0, band_w:get() - cut())
				end,
				overflow = "hidden",
				fade_x = { HEAD_FADE, 0 },
				ui.div {
					absolute = true,
					top = 0,
					bottom = 0,
					left = function(): number
						return -cut()
					end,
					w = function(): number
						return band_w:get()
					end,
					field,
				},
			},
		}
	end
	box[#box + 1] = head_node
	box[#box + 1] = ContextMenu {
		label = locale.t("ds.file_tree.actions") :: any,
		w_full = true,
		grow = 1,
		h = 0,
		items = function()
			return area_menu()
		end,
		ui.column {
			w_full = true,
			h_full = true,
			relative = true,
			id = "file-tree",
			role = "tree",
			label = m.name,
			-- One Tab stop: Tab lands on the row last moved to.
			focus_group = "vertical",
			on_key = function(e): boolean?
				if (e.key == "/" or (e.cmd and e.key == "f")) and not e.editing then
					local f = filter_ref:peek()
					if f then
						f:focus()
						return true
					end
				end
				return on_key(e)
			end,
			list,
			sticky,
			empty,
			rename_err,
			foot_note,
			-- The scrollbar, over all of it, never faded.
			Rail {
				id = "file-tree-rail",
				offset = list_offset,
				extent = function(): number
					heights:get()
					return total()
				end,
				view = function(): number
					return list_view.height or 0
				end,
			},
		},
	}
	return ui.column(box)
end

local made = ui.component(component)
return setmetatable(M, {
	__call = function(_, props: Props): Node
		return made(props)
	end,
})
