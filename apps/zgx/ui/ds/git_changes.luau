-- The changed files of a commit or a pull request as one list, drawn for
-- the worst case: a thousand files, a diff of thousands of lines. Zeron's
-- own Changes panel is a single virtualized list at line granularity
-- (changes.rs: every file header, hunk header and line a row of its own;
-- a folded file keeps no body rows), and this does the same with
-- `ui.list`, so a thousand files cost what the visible slice costs. The
-- list stands in a card of its own with what there is as the card's head
-- (the count, the lines, a commit's stage-all box) and the filter under
-- it; in the list, the folders as headers that fold, each file a row; at a
-- pane's width a file opens its diff in place under its row (one scroll
-- surface: the list never pushes anything else away), and at the window's
-- width a file is chosen for the diff beside the list. The card's
-- scrollbar is the app's rail, over the rows while they scroll and gone
-- otherwise.
--
-- `Changes.panel { files, filter, lead, extra, narrow, folded, open,
-- chosen, on_choose, box, lines, foot, after, on_comment, reveal, offset,
-- on_frame }` is the card; `Changes.list { … }` the list alone (`bare`: a
-- file's lines alone, no folders or file rows: a long diff beside the
-- list, drawn a slice at a time). A file's lines are `lines(f)`'s
-- (`git.parse` reads them from a unified diff), asked as it opens.
--
-- Files that move (a work tree's, asked again as it changes): `files` a
-- function of the list, read as the rows are, and each file's `rev` (its
-- content's id: a blob's) in its rows' keys, so a file whose content moved
-- is drawn again and the rest stay as they stood (open, folded, chosen and
-- the scroll are by path, and stay). `menu(f)`, a file row's context menu
-- (a secondary click, the Menu key); `trail(f)`, a control at its end that
-- shows under the pointer (Discard).

local Badge = require("@self/badge")
local FadeText = require("@self/fade_text")
local SearchField = require("@self/search_field")
local Rail = require("@self/rail")
local control = require("@self/control")
local ContextMenu = require("@self/context_menu")
local ToggleButton = require("@self/toggle_button")
local git = require("@self/git")
local locale = require("@self/locale")

local M = {}

-- Row heights, fixed where the list can know them: the card's head, a
-- folder, a file, a hunk's header, a line, a file's foot; a block (a
-- thread, a comment being written) measures itself and the list takes
-- its word.
M.HEAD, M.GROUP, M.FILE, M.HUNK, M.LINE, M.FOOT = 40, 36, 30, git.HUNK_H, git.LINE_H, 36
-- A file's row: 30 points, as the sidebar's; under the pointer the design
-- system's list row (ds/list_row), washed edge to edge in the card.
local BLOCK_GUESS = 140

export type File = { path: string, dir: string, name: string, status: string, add: number, del: number, rev: string?, title: string? }

-- A file of a change as the list keeps it: its folder and its own name
-- beside its path.
function M.file(path: string, status: string, add: number, del: number): File
	local dir, name = string.match(path, "^(.*)/([^/]+)$")
	return { path = path, dir = dir or "", name = name or path, status = status, add = add, del = del }
end

-- What the files add up to: how many, in how many folders, and the lines.
function M.totals(files: { File }): { files: number, folders: number, add: number, del: number }
	local dirs: { [string]: boolean } = {}
	local folders, add, del = 0, 0, 0
	for _, f in files do
		if not dirs[f.dir] then
			dirs[f.dir] = true
			folders += 1
		end
		add += f.add
		del += f.del
	end
	return { files = #files, folders = folders, add = add, del = del }
end

-- A count in words: 1,000 reads; 1000 doesn't.
function M.count(n: number): string
	local s = tostring(n)
	return (string.gsub(string.reverse(s), "(%d%d%d)", "%1,"):reverse():gsub("^,", ""))
end

-- The rows the list shows, flattened from the folders, the filter, what is
-- folded and what is open: each with its key, its kind and its height.
-- (A row holds only what a state can: no builder; a block row names its
-- line and its place, and the builder is asked for again as it is built.)
-- (A line's, a hunk's, a block's or a foot's row says which file by its
-- `path` and which line by its `index`: what it shows is read from the
-- list's own Lua, never held in the rows, which are state.)
export type Row = { key: string, kind: string, h: number, file: File?, dir: string?, index: number?, lkey: string?, label: string?, path: string?, split: boolean? }

export type ListProps = {
	-- The files, or a function of them (read as the rows are: files that
	-- move, followed in place).
	files: { File } | () -> { File },
	-- True at a pane's width: a file opens its diff in place.
	narrow: boolean?,
	-- A file's lines alone (every file's, as open): no folders, no rows
	-- for the files themselves. The diff beside a list.
	bare: boolean?,
	-- Line numbers: one column (the new line's, or the old one's for a
	-- line taken out) or two. One when narrow, unless said.
	columns: number?,
	filter: State<string>?,
	-- Folders folded, by path; files open in place, by path.
	folded: State<{ [string]: boolean }>?,
	open: State<{ [string]: boolean }>?,
	-- At the window's width, the file chosen for the diff beside the list.
	chosen: State<string>?,
	on_choose: ((f: File) -> ())?,
	-- A staging box before each file (a commit), by its path.
	box: ((f: File) -> Node?)?,
	-- An open file's lines (read as the rows are: a state it reads moves
	-- them), and what stands under them, a row of its own (a link out, a
	-- long diff cut short).
	lines: ((f: File) -> { git.Line }?)?,
	foot: ((f: File) -> Node?)?,
	-- Whether a file has a foot now (read as the rows are: a state it reads
	-- brings it and takes it away); always, without it.
	foot_when: ((f: File) -> boolean)?,
	-- What stands under a line of an open diff (threads, a comment being
	-- written), and a comment asked for on a line.
	after: ((path: string, key: string) -> { git.Block }?)?,
	-- A comment asked for on a line: its key, the line; with `ranges`, a
	-- range dragged from a line's + over others gives its lines too (first
	-- to last; the key and line are the last's, where its box opens).
	on_comment: ((path: string, key: string, line: git.Line, range: { git.Line }?) -> ())?,
	ranges: boolean?,
	-- A line's mark (read as it's drawn: a state it reads moves it): nil,
	-- "on" (a comment rests on it) or "lit" (its chip pointed at).
	marks: ((path: string, key: string) -> string?)?,
	reveal: any?,
	-- Where the list's scroll stands, and its own ref.
	offset: State<number>?,
	ref: any?,
	-- Told the list's frame (the element the rows scroll in), for what
	-- brings itself into view inside it.
	on_frame: ((el: any) -> ())?,
	-- Its id, and its rail's ("changes-list", "changes-rail"): two lists on
	-- a page need their own.
	id: string?,
	-- A file row's class (for what finds them: "github-file").
	row_class: string?,
	-- A file row's context menu (asked as it opens), and a control at its
	-- end that shows under the pointer.
	menu: ((f: File) -> { ContextMenu.Entry }?)?,
	trail: ((f: File) -> Node?)?,
	-- Side by side, and long lines wrapped: the person's choice
	-- (`git.prefs`) unless said.
	split: State<boolean>?,
	wrap: State<boolean>?,
}

-- What `files` holds now.
local function files_of(files: { File } | () -> { File }): { File }
	return if type(files) == "function" then files() else files
end

-- A file's part of its rows' keys: its path, and its content's id when it
-- has one (a file whose content moved is drawn anew).
local function fkey(f: File): string
	return if f.rev then `{f.path}@{f.rev}` else f.path
end

-- A file row with a control at its end (`trail`) that shows while the
-- pointer is on the row: the list's quiet rows, an action at hand.
local Trailed = ui.component(function(props: { row: Node, trail: Node }): Node
	local over = ui.use_state(false)
	return ui.row {
		w_full = true,
		items = "center",
		on_hover = function(inside: boolean)
			over:set(inside)
		end,
		ui.div { grow = 1, min_w = 0, props.row },
		ui.div {
			shrink = 0,
			pr = "$space.2",
			opacity = function(): number
				return if over:get() then 1 else 0
			end,
			props.trail,
		},
	}
end)

-- The folder's name as a row: its path (the repository's root, "/"), how
-- many files it holds, a chevron that turns as it folds. A quiet header,
-- as the sidebar's sections are: small words a step down, no fill of its
-- own (a filled band read heavier than anything else in the panel);
-- under the pointer its words firm up (the file rows' wash would read as
-- one of them).
local GroupRow = ui.component(function(props: { dir: string, n: number, folded: State<{ [string]: boolean }> }): Node
	local dir, n, folded = props.dir, props.n, props.folded
	local over = ui.use_state(false)
	local shown = if dir == "" then "/" else dir
	return ui.row {
		w_full = true,
		h = M.GROUP,
		px = "$space.3",
		gap = "$space.2",
		items = "center",
		cursor = "pointer",
		role = "button",
		label = `{shown}, {n} files`,
		expanded = function(): boolean
			return not folded:get()[dir]
		end,
		on_hover = function(inside: boolean)
			over:set(inside)
		end,
		on_click = function()
			local now = table.clone(folded:peek())
			now[dir] = if now[dir] then nil else true
			folded:set(now)
		end,
		git.chevron(function(): number
			return if folded:get()[dir] then -90 else 0
		end),
		FadeText {
			shown,
			grow = 1,
			text_size = "$font.size.sm",
			weight = "$font.weight.medium",
			color = function(): string
				return if over:get() then "$text.secondary" else "$text.tertiary"
			end,
		},
		ui.text { tostring(n), shrink = 0, text_size = "$font.size.xs", color = "$text.tertiary", tabular = true },
	}
end)

local function group_row(dir: string, n: number, folded: State<{ [string]: boolean }>): Node
	return GroupRow { dir = dir, n = n, folded = folded }
end

-- The list: its rows, virtualized, and the app's rail over them. (A
-- component, for the state of its own it keeps: how tall it is, and the
-- heights its blocks report.)
local List = ui.component(function(props: ListProps): Node
	local filter = props.filter or ui.use_state("")
	local folded = props.folded or ui.use_state({} :: { [string]: boolean })
	local open = props.open
	local narrow = props.narrow == true
	local bare = props.bare == true
	local one_column = if props.columns then props.columns == 1 else narrow
	local offset = props.offset or ui.use_state(0)
	-- A range being dragged out from a line's + (`ranges`): its file and
	-- the indexes (into the file's lines) it runs between, lit as it grows.
	local span = ui.use_state(nil :: { path: string, from: number, to: number }?)
	local drag_tag = `{props.id or "changes-list"}:range`
	local split = props.split or git.prefs.split
	local wrap = props.wrap or git.prefs.wrap
	local id = props.id or "changes-list"
	-- The room between a row's wash and the card's edge over the first
	-- row and under the last: the list's gap, as between the washes, so
	-- with the wash's own inset the room is the sides' (README rule 6).
	local gap = (ui.theme.get("zeron.list.gap") :: number?) or 2
	-- Blocks measure themselves; the list takes the height it last saw.
	local measured: { [string]: number } = {}
	local remeasure = ui.use_state(0)
	-- How tall the list is, for its rail.
	local seen = ui.use_state(0)
	-- What the rows show, kept here in Lua as the rows are worked out: each
	-- open file's lines (and side by side, their pairs), each file by its
	-- path, each row by its key. (The rows are state: a value in them is
	-- converted and weighed on every change, so a long diff's lines in each
	-- of its rows went past a plugin's memory and cost a frame each time.)
	local file_lines: { [string]: { git.Line } } = {}
	local file_pairs: { [string]: { git.Pair } } = {}
	local files_by: { [string]: File } = {}
	local row_of: { [string]: Row } = {}
	local rows = ui.computed(function(): { Row }
		local q = string.lower(filter:get())
		local fold = folded:get()
		local opened = if open then open:get() else {}
		local out: { Row } = {}
		file_lines, file_pairs, files_by, row_of = {}, {}, {}, {}
		-- The files that pass the filter, in their folders' order.
		local shown: { File } = {}
		for _, f in files_of(props.files) do
			if bare or q == "" or string.find(string.lower(f.path), q, 1, true) then
				table.insert(shown, f)
			end
		end
		-- How many each folder holds, for its header.
		local counts: { [string]: number } = {}
		for _, f in shown do
			counts[f.dir] = (counts[f.dir] or 0) + 1
		end
		-- Folders in the order they first appear; files under their own.
		local order: { string } = {}
		local by_dir: { [string]: { File } } = {}
		for _, f in shown do
			if not by_dir[f.dir] then
				by_dir[f.dir] = {}
				table.insert(order, f.dir)
			end
			table.insert(by_dir[f.dir], f)
		end
		local grouped = #order > 1 and not bare
		for _, dir in order do
			if grouped then
				table.insert(out, { key = `g:{dir}`, kind = "group", h = M.GROUP, dir = dir, index = counts[dir] })
			end
			if grouped and fold[dir] then
				continue
			end
			for _, f in by_dir[dir] do
				files_by[f.path] = f
				-- Under its folder's header, a file goes by its own name.
				if not bare then
					table.insert(out, { key = `f:{fkey(f)}`, kind = "file", h = M.FILE, file = f, label = if grouped then f.name else (f.title or f.path) })
				end
				if bare or (narrow and opened[f.path]) then
					local lines = if props.lines then props.lines(f) or {} else {}
					file_lines[f.path] = lines
					if bare then
						-- (Room over the first hunk, as the diff card keeps.)
						table.insert(out, { key = `t:{fkey(f)}`, kind = "pad", h = ui.theme.get("space.1") :: number })
					end
					-- (Wrapped, a line is as tall as its text: measured, as a
					-- block is, `heights` taking what it says; its key says so,
					-- so the lines are drawn anew.)
					local wrapped = wrap:get()
					local w = if wrapped then ":w" else ""
					-- What stands under a line (its threads), after it.
					local function blocks_after(l: git.Line)
						local lkey = git.line_key(l)
						local after = props.after
						local blocks = if after then after(f.path, lkey) else nil
						for n, b in blocks or {} do
							-- (Keyed by kind too: a thread that takes a closed
							-- composer's place is a new row, not the old one kept.)
							local key = `b:{fkey(f)}:{lkey}:{n}:{b.kind}`
							table.insert(out, { key = key, kind = "block", h = BLOCK_GUESS, path = f.path, index = n, lkey = lkey })
						end
					end
					if split:get() then
						local pairs = git.pairs(lines)
						file_pairs[f.path] = pairs
						for i, p in pairs do
							if p.hunk then
								table.insert(out, { key = `h:{fkey(f)}:s{i}`, kind = "hunk", h = M.HUNK, path = f.path, index = i, split = true })
							else
								local key = `p:{fkey(f)}:{i}{w}`
								table.insert(out, { key = key, kind = "pair", h = M.LINE, path = f.path, index = i })
								if p.left and p.left ~= p.right then
									blocks_after(p.left)
								end
								if p.right then
									blocks_after(p.right)
								end
							end
						end
					else
						for i, l in lines do
							if l.kind == "hunk" then
								table.insert(out, { key = `h:{fkey(f)}:{i}`, kind = "hunk", h = M.HUNK, path = f.path, index = i })
							else
								local key = `l:{fkey(f)}:{i}{w}`
								table.insert(out, { key = key, kind = "line", h = M.LINE, path = f.path, index = i })
								blocks_after(l)
							end
						end
					end
					if props.foot and (not props.foot_when or props.foot_when(f)) then
						table.insert(out, { key = `e:{fkey(f)}`, kind = "foot", h = M.FOOT, path = f.path })
					end
				end
			end
		end
		-- A file row first or last gets the gap's room to the card's edge;
		-- a folder's strip or a diff's lines run to it.
		if #out > 0 and out[1].kind == "file" then
			table.insert(out, 1, { key = "pad:top", kind = "pad", h = gap })
		end
		if #out > 0 and out[#out].kind == "file" then
			table.insert(out, { key = "pad:bottom", kind = "pad", h = gap })
		end
		for _, r in out do
			row_of[r.key] = r
		end
		return out
	end)
	-- Each row's height: what it measured (a wrapped line, a block), else
	-- its own. (A measure moves these alone, not the rows.)
	local heights = ui.computed(function(): { number }
		remeasure:get()
		local hs: { number } = {}
		for i, r in rows:get() do
			hs[i] = measured[r.key] or r.h
		end
		return hs
	end)
	local total = ui.computed(function(): number
		local sum = 0
		for _, h in heights:get() do
			sum += h
		end
		return sum
	end)
	-- A file open in place: its row sticks to the top while its lines
	-- scroll under it, the next file's row pushing it up (zgx's sticky
	-- rows; production's sticky file headers).
	local sticky = ui.computed(function(): { number }
		local out: { number } = {}
		if bare or not narrow or not open then
			return out
		end
		local opened = open:get()
		for i, r in rows:get() do
			if r.kind == "file" and opened[(r.file :: File).path] then
				table.insert(out, i)
			end
		end
		return out
	end)
	local inset = git.gutter(one_column)
	local function build(key: string): Node
		rows:peek()
		local r = row_of[key]
		if not r then
			return ui.div { h = M.LINE }
		end
		if r.kind == "pad" then
			return ui.div { w_full = true, h = r.h }
		elseif r.kind == "group" then
			return group_row(r.dir :: string, r.index or 0, folded)
		elseif r.kind == "file" then
			local f = r.file :: File
			local function slot(node: Node): Node
				return node
			end
			local row = git.file_row(f, if props.box then props.box(f) else nil, r.label, { h = M.FILE })
			local trail = if props.trail then props.trail(f) else nil
			if trail then
				row = Trailed { row = row, trail = trail }
			end
			local function with_menu(node: Node): Node
				local menu = props.menu
				if not menu then
					return node
				end
				return ContextMenu {
					label = f.path,
					ring = "$radius.sm",
					w_full = true,
					items = function()
						return menu(f)
					end,
					node,
				}
			end
			if narrow then
				-- The row opens its diff under it; a chevron says so, turning
				-- in a slot of its own at the row's end. The same row as the
				-- window's, hover wash and all, never chosen.
				local is_open = function(): boolean
					return (open and open:get()[f.path]) == true
				end
				return slot(with_menu(git.selectable(ui.row {
					w_full = true,
					h = M.FILE,
					items = "center",
					ui.div { grow = 1, min_w = 0, row },
					ui.div {
						shrink = 0,
						pr = "$space.3",
						git.chevron(function(): number
							return if is_open() then 180 else 0
						end),
					},
				}, function(): boolean
					return false
				end, f.path, function()
					if open then
						local now = table.clone(open:peek())
						now[f.path] = if now[f.path] then nil else true
						open:set(now)
					end
				end, props.row_class)))
			end
			local chosen = props.chosen
			return slot(with_menu(git.selectable(row, function(): boolean
				return chosen ~= nil and chosen:get() == f.path
			end, f.path, function()
				if props.on_choose then
					props.on_choose(f)
				end
			end, props.row_class)))
		elseif r.kind == "hunk" then
			-- (Cut at the card's edge where it runs long, as the lines are,
			-- not faded: the diff card keeps no edge fades.)
			local line = if r.split then (file_pairs[r.path :: string] or {})[r.index or 0].hunk else (file_lines[r.path :: string] or {})[r.index or 0]
			if not line then
				return ui.div { h = M.HUNK }
			end
			return git.hunk_row(line :: git.Line)
		elseif r.kind == "line" or r.kind == "pair" then
			local f = files_by[r.path :: string]
			local lines: { git.Line } = file_lines[r.path :: string] or {}
			local pair: git.Pair? = if r.kind == "pair" then (file_pairs[r.path :: string] or {})[r.index or 0] else nil
			local own: git.Line? = if r.kind == "line" then lines[r.index or 0] else nil
			if not f or (r.kind == "pair" and not pair) or (r.kind == "line" and not own) then
				return ui.div { h = M.LINE }
			end
			-- (Its colours, worked out for the whole file as its first line
			-- is drawn: the lines are the list's own Lua, one table a file,
			-- so the highlighter's cache keeps them.)
			local hl = git.highlight(lines, f.path)
			local wrapped = string.sub(key, -2) == ":w"
			-- The line a comment goes on (a pair's: its right, else its
			-- left), and where it stands in the file's lines.
			local target: git.Line? = nil
			local at: number? = nil
			if r.kind == "pair" then
				local p = pair :: git.Pair
				target, at = p.right or p.left, if p.right then p.ri else p.li
			else
				target, at = own, r.index
			end
			local function mark(): string?
				local now = span:get()
				if now and at and now.path == f.path and at >= math.min(now.from, now.to) and at <= math.max(now.from, now.to) then
					return "lit"
				end
				return if props.marks and target then props.marks(f.path, git.line_key(target)) else nil
			end
			-- A range: dragged from this line's +, over this file's lines;
			-- dropped, the comment is asked for over all of them.
			local slot_extra: { [string]: any }? = nil
			local row_extra: { [string]: any }? = nil
			if props.ranges and props.on_comment and target and at then
				local from = at
				slot_extra = {
					on_drag = function(): any
						span:set({ path = f.path, from = from, to = from })
						return { tag = drag_tag, path = f.path, at = from }
					end,
					-- (Nothing follows the pointer: the lines light instead.)
					drag_preview = function(): Node
						return ui.div { w = 1, h = 1 }
					end,
					on_drag_end = function(_payload: any, accepted: boolean)
						if not accepted then
							span:set(nil)
						end
					end,
				}
				row_extra = {
					drop_when = function(payload: any): boolean
						return type(payload) == "table" and payload.tag == drag_tag and payload.path == f.path
					end,
					on_drag_over = function(payload: any)
						local now = span:peek()
						if not now or now.to ~= from then
							span:set({ path = f.path, from = payload.at, to = from })
						end
					end,
					on_drop = function(payload: any)
						span:set(nil)
						local lo, hi = math.min(payload.at, from), math.max(payload.at, from)
						local range: { git.Line } = {}
						for i = lo, hi do
							local l = lines[i]
							if l and l.kind ~= "hunk" then
								table.insert(range, l)
							end
						end
						local last = range[#range]
						if last then
							(props.on_comment :: any)(f.path, git.line_key(last), last, if #range > 1 then range else nil)
						end
					end,
				}
			end
			local marked = if props.marks or props.ranges then mark else nil
			local node: Node
			if r.kind == "pair" then
				local p = pair :: git.Pair
				node = git.SplitLine {
					pair = p,
					left_runs = if hl and p.li then hl[p.li] else nil,
					right_runs = if hl and p.ri then hl[p.ri] else nil,
					wrap = wrapped,
					on_comment = if props.on_comment and p.right
						then function()
							(props.on_comment :: any)(f.path, git.line_key(p.right :: git.Line), p.right)
						end
						else nil,
					mark = marked,
					slot = slot_extra,
					row = row_extra,
				}
			else
				local l = own :: git.Line
				node = git.DiffLine {
					line = l,
					narrow = one_column,
					runs = if hl then hl[r.index or 0] else nil,
					wrap = wrapped,
					on_comment = if props.on_comment
						then function()
							(props.on_comment :: any)(f.path, git.line_key(l), l)
						end
						else nil,
					mark = marked,
					slot = slot_extra,
					row = row_extra,
				}
			end
			if not wrapped then
				return node
			end
			-- (Wrapped: it measures itself, and the list takes its word.)
			return ui.div {
				w_full = true,
				on_resize = function(size)
					if math.abs((measured[key] or M.LINE) - size.h) > 0.5 then
						measured[key] = size.h
						remeasure:set(remeasure:peek() + 1)
					end
				end,
				node,
			}
		elseif r.kind == "foot" then
			local f = files_by[r.path :: string]
			if not f then
				return ui.div { h = M.FOOT }
			end
			return ui.row {
				w_full = true,
				h = M.FOOT,
				items = "center",
				pl = ui.raw(inset, "where the code's text starts, past its gutter"),
				pr = "$space.3",
				if props.foot then props.foot(f) else nil,
			}
		end
		-- A block: its builder asked for again, built with the diff's gutter
		-- as its inset, measuring itself for the list.
		local path = r.path :: string
		local blocks = if props.after then props.after(path, r.lkey :: string) else nil
		local b = if blocks then blocks[r.index or 1] else nil
		if not b then
			return ui.div { h = M.LINE }
		end
		return ui.div {
			w_full = true,
			on_resize = function(size)
				if math.abs((measured[key] or 0) - size.h) > 0.5 then
					measured[key] = size.h
					remeasure:set(remeasure:peek() + 1)
				end
			end,
			b.build({ inset = inset, joined = (r.index or 1) > 1, reveal = props.reveal }),
		}
	end
	return ui.div {
		id = id,
		w_full = true,
		grow = 1,
		min_h = 0,
		relative = true,
		on_resize = function(size, el)
			seen:set(size.h)
			if props.on_frame then
				props.on_frame(el)
			end
		end,
		ui.list {
			ref = props.ref,
			w_full = true,
			h_full = true,
			offset = offset,
			overscan = 6,
			heights = function(): { number }
				return heights:get()
			end,
			sticky = function(): { number }
				return sticky:get()
			end,
			-- (Read as the rows are: rows whose keys change and whose
			-- heights don't, a diff wrapped or split, are drawn anew.)
			key = function(i: number): string
				local r = rows:get()[i]
				return if r then r.key else `none:{i}`
			end,
			row = function(key: string): Node
				return build(key)
			end,
			row_text = function(i: number): string
				local r = rows:peek()[i]
				if r and r.kind == "line" then
					local l = (file_lines[r.path :: string] or {})[r.index or 0]
					return if l then l.text else ""
				end
				return if r and r.file then r.file.path elseif r and r.path then r.path elseif r and r.dir then r.dir else ""
			end,
		},
		Rail { id = if props.id then `{props.id}-rail` else "changes-rail", offset = offset, extent = total, view = seen },
	}
end)

function M.list(props: ListProps): Node
	return List(props)
end

-- The card: its head says what there is (with `lead` before it: a
-- commit's stage-all box; `extra` after it), the filter under the head,
-- the list under that. The head is as tall as a diff card's, so beside
-- one the two cards' first rows share a line.
export type Panel = ListProps & {
	filter: State<string>,
	lead: Node?,
	extra: (string | () -> string)?,
}

function M.panel(props: Panel): Node
	-- (What the files add up to, followed as they move.)
	local totals = ui.computed(function(): { files: number, folders: number, add: number, del: number }
		return M.totals(files_of(props.files))
	end)
	local function all(): string
		local t = totals:get()
		local args = { count = M.count(t.files), n = t.files, folders = tostring(t.folders) }
		return if t.folders > 1 then locale.s("ds.git_changes.files_in_folders", args) else locale.s("ds.git_changes.files", args)
	end
	-- Filtering, the head counts what matches instead.
	local function what(): string
		local q = string.lower(props.filter:get())
		if q == "" then
			return all()
		end
		local n = 0
		for _, f in files_of(props.files) do
			if string.find(string.lower(f.path), q, 1, true) then
				n += 1
			end
		end
		return locale.s("ds.git_changes.files_match", { n = M.count(n), total = M.count(totals:get().files) })
	end
	local head = ui.row {
		w_full = true,
		h = M.HEAD,
		shrink = 0,
		pl = "$space.3",
		-- (Its trailing controls end where every card head's do.)
		pr = git.HEAD_END,
		gap = "$space.2",
		items = "center",
		props.lead,
		FadeText {
			function(): string
				local e = props.extra
				local extra = if type(e) == "function" then e() else e
				local w = what()
				return if extra and extra ~= "" then `{w} · {extra}` else w
			end,
			grow = 1,
			text_size = "$font.size.sm",
			weight = "$font.weight.medium",
			color = "$text.secondary",
		},
		-- (Drawn again only as the lines move.)
		ui.presence(function(): string
			local t = totals:get()
			return `{t.add}/{t.del}`
		end, function(): Node
			local t = totals:peek()
			return Badge.DiffStat { added = t.add, removed = t.del }
		end, { shrink = 0 }),
		-- How the diffs read: side by side, long lines wrapped.
		ui.row {
			id = "diff-view",
			shrink = 0,
			gap = git.HEAD_GAP,
			items = "center",
			ToggleButton { on = props.split or git.prefs.split, icon = "split-columns", label = locale.s("ds.git.split"), size = "md" },
			ToggleButton { on = props.wrap or git.prefs.wrap, icon = "wrap-text", label = locale.s("ds.git.wrap"), size = "md" },
		},
	}
	local function rule(): Node
		return ui.div { h = 1, w_full = true, shrink = 0, bg = "$zeron.settings.rule", aria_hidden = true }
	end
	-- The filter: a quiet field, a row of the card, its words on the head's
	-- column; inset evenly, with the corner that inset gives it inside the
	-- card (for its focus ring).
	local filter = ui.div {
		w_full = true,
		shrink = 0,
		p = "$space.1",
		theme = { radius = { md = control.nest(git.RADIUS, git.EDGE, "$space.1") } },
		SearchField { value = props.filter, placeholder = locale.t("ds.git_changes.filter"), size = "sm", quiet = true, w_full = true },
	}
	return ui.column {
		w_full = true,
		grow = 1,
		min_h = 0,
		rounded = git.RADIUS,
		bg = "$zeron.settings.card",
		border = git.EDGE,
		border_color = "$zeron.settings.edge",
		overflow = "hidden",
		head,
		rule(),
		filter,
		rule(),
		M.list(props),
	}
end

return M
