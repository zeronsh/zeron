-- The gallery's Files page: a project's tree beside the file it shows.
-- This checkout's own files, as git lists them (a snapshot taken by
-- tools/files_snapshot.py: a script reads only files the user picks), with
-- their status and, for a sample of them, their text; or fifty
-- thousand made-up files, to feel the tree at size. Opening takes a moment,
-- as reading would. A page is { id, title, about, demo }.

local ds = require("@ds")
local control = require("@ds/control")
local FileTree = require("@ds/file_tree")
local FileViewer = require("@ds/file_viewer")
local FilesPane = require("@ds/files_pane")
local Inline = require("@ds/inline_block")
local snapshot = require("@demos/files_snapshot")

type Doc = FileViewer.Doc
-- The snapshot's pictures' sizes (width and height) and files' text, by
-- path. (Generated data, its tables keyed by literal paths: read here by
-- any path, so typed as the maps they are, through this one cast.)
local by_path = snapshot :: any
local IMAGE_SIZES: { [string]: { number } } = by_path.images
local TEXTS: { [string]: string } = by_path.texts

-- Which project: this checkout, or the large one.
local source = ui.state("files_source", "checkout")
local active = ui.state("files_active", nil :: string?)
local reveal = ui.state("files_reveal", nil :: string?)
local doc = ui.state("files_doc", nil :: Doc?)
-- How wide the pane is, tree and file together (0: all there is): Zeron's
-- right pane at its narrowest and its default, where the tree can't sit
-- beside the file and comes over it instead.
local pane = ui.state("files_pane", 0)
-- Whether the tree shows beside the file: remembered, as the app would.
local tree_shown = ui.persist("files_tree", true)
-- The file header's shape, to compare: one line, two, or the path.
local head = ui.state("files_head", "line" :: FileViewer.Head)
-- The pane's own commands (`toggle`), for the key bound below.
local pane_commands: { [string]: () -> () } = {}
local pane_ref = ui.ref()
-- ⇧⌘E hides and shows the tree from anywhere while the page shows.
ui.command {
	id = "files.tree",
	title = "Toggle File Tree",
	menu = "View",
	key = FilesPane.keys,
	run = function()
		local toggle = pane_commands.toggle
		if pane_ref.mounted and toggle then
			toggle()
		end
	end,
}
-- What stands between the file's lines: a comment being written, and any
-- note or thread put there, and the words of each.
type Note = { key: string, line: number, kind: string? }
local notes = ui.state("files_notes", {} :: { Note })
local note_draft = ui.state("files_note_draft", "")
local function drop_note(key: string)
	local keep: { Note } = {}
	for _, n in notes:peek() do
		if n.key ~= key then
			table.insert(keep, n)
		end
	end
	notes:set(keep)
end
-- The tree's scroll position (named, so `zgx inspect set` can drive it).
local offset = ui.state("files_offset", 0)
-- What a picked file holds (`Open…`), outside either project.
local picked: { path: string, text: string?, image: string? }? = nil

local IMAGE = { png = true, jpg = true, jpeg = true }
local BINARY = {
	ttf = true, otf = true, woff = true, woff2 = true, ico = true, icns = true, zip = true, gz = true,
	tgz = true, a = true, so = true, dylib = true, wasm = true, bin = true, car = true, mp4 = true,
	mov = true, jar = true, rlib = true, pdf = true, keystore = true, gif = true, webp = true, db = true,
	sqlite = true, xcuserstate = true, otc = true, ttc = true, riv = true,
}
local LIMIT = 8 * 1024 * 1024
-- Where the gallery's own files sit in the checkout: image src paths are
-- relative to ui/.
local UI_DIR = "apps/zgx/ui/"
-- What was renamed or moved (each `from` to `to`, in order) and made, so a
-- file keeps its text wherever it goes and a new one opens empty.
local moves: { { from: string, to: string } } = {}
local made: { [string]: boolean } = {}
local function original(path: string): string
	for i = #moves, 1, -1 do
		local mv = moves[i]
		if path == mv.to then
			path = mv.from
		elseif string.sub(path, 1, #mv.to + 1) == mv.to .. "/" then
			path = mv.from .. string.sub(path, #mv.to + 1)
		end
	end
	return path
end

local function ext(path: string): string
	return string.lower(string.match(path, "%.([^%./]+)$") or "")
end

-- This checkout.
local checkout = FileTree.lines(snapshot.name, snapshot.files, snapshot.root)

-- Fifty thousand files: eighty packages of modules, a generated folder of
-- five thousand, and a vendored one git ignores. Made in order, so no sort.
local large_model: FileTree.Model? = nil
local function hash(n: number): number
	n = bit32.bxor(n, bit32.rshift(n, 16))
	n = (n * 0x45d9f3b) % 0x100000000
	n = bit32.bxor(n, bit32.rshift(n, 16))
	return n
end
local function status_for(i: number): FileTree.Status?
	local r = hash(i) % 1000
	if r < 30 then
		return "modified"
	elseif r < 40 then
		return "added"
	elseif r < 52 then
		return "untracked"
	elseif r < 55 then
		return "deleted"
	elseif r < 57 then
		return "renamed"
	elseif r < 58 then
		return "conflict"
	end
	return nil
end
local WORDS = { "auth", "billing", "cache", "config", "editor", "events", "files", "graph", "index", "layout", "mesh", "net", "parse", "query", "render", "search", "store", "sync", "theme", "worker" }
local function large(): FileTree.Model
	if large_model then
		return large_model
	end
	local m = FileTree.model("monorepo", nil)
	local add = FileTree.add
	local n = 0
	local pkgs = add(m, 0, "packages", true)
	local gen = add(m, 0, "generated", true)
	local vendor = add(m, 0, "vendor", true, "ignored")
	for p = 1, 80 do
		local pkg = add(m, pkgs, string.format("pkg-%02d-%s", p, WORDS[(p - 1) % #WORDS + 1]), true)
		local src = add(m, pkg, "src", true)
		local tests = add(m, pkg, "tests", true)
		for mod = 1, 20 do
			local dir = add(m, src, string.format("%s-%02d", WORDS[(mod - 1) % #WORDS + 1], mod), true)
			for f = 1, 24 do
				n += 1
				add(m, dir, string.format("%s_%02d.ts", WORDS[(f + mod) % #WORDS + 1], f), false, status_for(n), 800 + hash(n) % 24000)
			end
		end
		for f = 1, 40 do
			n += 1
			add(m, tests, string.format("case_%03d.test.ts", f), false, status_for(n), 400 + hash(n) % 6000)
		end
		n += 1
		add(m, pkg, "package.json", false, status_for(n), 640)
		add(m, pkg, "README.md", false, nil, 1800)
		add(m, pkg, "tsconfig.json", false, nil, 320)
	end
	for f = 1, 5000 do
		n += 1
		add(m, gen, string.format("schema_%04d.json", f), false, if f % 97 == 0 then "modified" else nil, 2000 + hash(n) % 40000)
	end
	for f = 1, 400 do
		add(m, vendor, string.format("dep_%03d.js", f), false, "ignored", 9000)
	end
	add(m, 0, "dump.sql", false, "untracked", 48 * 1024 * 1024)
	add(m, 0, "logo.bin", false, nil, 92 * 1024)
	add(m, 0, "README.md", false, "modified", 2400)
	local built = FileTree.finish(m, true)
	large_model = built
	return built
end

local function model(): FileTree.Model
	return if source:peek() == "large" then large() else checkout
end

-- A made-up file's text: a module's shape, the same each time it opens.
local function made_up(path: string, size: number): string
	local lines: { string } = {}
	local seed = #path
	for i = 1, #path do
		seed = (seed * 31 + string.byte(path, i)) % 1000003
	end
	local name = string.match(path, "([^/]+)%.%w+$") or "module"
	if ext(path) == "md" then
		return `# {name}\n\nA package of the monorepo: fifty thousand files, made up to show the tree at size.\n\n- Its modules are under \`src/\`.\n- Its tests under \`tests/\`.\n`
	elseif ext(path) == "json" then
		return `\{\n  "name": "{name}",\n  "version": "1.{seed % 40}.0",\n  "private": true\n}\n`
	end
	table.insert(lines, `import \{ {WORDS[seed % #WORDS + 1]} } from "../{WORDS[(seed + 3) % #WORDS + 1]}";`)
	table.insert(lines, "")
	local count = math.clamp(math.floor(size / 40), 20, 4000)
	local i = 0
	while #lines < count do
		i += 1
		local w = WORDS[(seed + i) % #WORDS + 1]
		table.insert(lines, `// {w}: what it does, in a line.`)
		table.insert(lines, `export function {w}{i}(input: string, depth = {i % 7}): number \{`)
		table.insert(lines, `  const parts = input.split("/").filter(Boolean);`)
		table.insert(lines, `  return parts.length * depth + {(seed * i) % 97};`)
		table.insert(lines, "}")
		table.insert(lines, "")
	end
	return table.concat(lines, "\n")
end

-- Opening a file: loading for a moment (longer for a bigger one), then
-- what it is.
local opening = 0
local function open(path: string?)
	opening += 1
	local mine = opening
	if not path then
		doc:set(nil)
		return
	end
	if picked and picked.path == path then
		local p = picked
		doc:set({ path = path, state = "ready", text = p.text, image = p.image, absolute = path })
		return
	end
	local m = model()
	local id = m.by_path[path]
	local node = if id then m.nodes[id] else nil
	local size = if node then node.size else nil
	local status: any = if node then (if node.ignored then "ignored" else node.status) else nil
	local absolute = if m.root then `{m.root}/{path}` else nil
	doc:set({ path = path, state = "loading", size = size, status = status, absolute = absolute })
	local wait = 60 + math.min(320, (size or 0) / 600)
	ui.after(wait, function()
		if mine ~= opening then
			return
		end
		local e = ext(path)
		local d: Doc = { path = path, state = "ready", size = size, status = status, absolute = absolute }
		if not node then
			d.state = "error"
			d.error = "It isn't in this project any more."
		elseif node.gone then
			d.state = "gone"
		elseif IMAGE[e] and m == checkout then
			-- The snapshot's root is a made-up place, so only the gallery's own
			-- pictures (beside this script, under ui/) can be shown.
			local from = original(path)
			if string.sub(from, 1, #UI_DIR) == UI_DIR then
				d.image = string.sub(from, #UI_DIR + 1)
				local dims = IMAGE_SIZES[from]
				if dims then
					d.width, d.height = dims[1], dims[2]
				end
			else
				d.state = "error"
				d.error = "The gallery can show only its own pictures from this checkout. Open this one from disk with Open… to see it."
			end
		elseif IMAGE[e] and m.root then
			d.image = `{m.root}/{original(path)}`
			local dims = IMAGE_SIZES[original(path)]
			if dims then
				d.width, d.height = dims[1], dims[2]
			end
		elseif BINARY[e] then
			d.state = "binary"
		elseif (size or 0) > LIMIT then
			d.state = "large"
		elseif made[path] then
			d.text = ""
		elseif m == checkout then
			local text = TEXTS[original(path)]
			if text then
				d.text = text
			else
				d.state = "error"
				d.error = "This file is not in this demo: the gallery carries the text of only a sample of this checkout. Open it from disk with Open… to see it."
			end
		else
			d.text = made_up(path, size or 2000)
		end
		doc:set(d)
	end)
end
-- Another project: nothing open, and its tree from the top.
local last_source = source:peek()
ui.effect(function()
	local now = source:get()
	if now ~= last_source then
		last_source = now
		active:set(nil)
		offset:set(0)
	end
end)
-- The doc follows the file the tree opens.
local last_active: string? = nil
ui.effect(function()
	local a = active:get()
	if a ~= last_active then
		last_active = a
		open(a)
	end
end)

-- A file from disk, picked: shown as it is (outside either project).
local function open_from_disk()
	ui.open_files({ title = "Open a File" }):after(function(files)
		local f = files and files[1]
		if not f then
			return
		end
		local e = ext(f.name)
		if IMAGE[e] then
			picked = { path = f.path, image = f.path }
			active:set(f.path)
			return
		end
		ui.read_file(f):after(function(text: string)
			picked = { path = f.path, text = text }
			active:set(f.path)
		end)
	end)
end

local did = ui.state("files_did", "")

return {
	id = "files",
	title = "Files",
	fill = true,
	about = "A project's files beside the one they show, one band across both: the tree hides and shows (its toggle, ⇧⌘E), and in a narrow pane comes over the file. This checkout as git lists it, its status and much of its text; or fifty thousand files, to feel the tree at size.",
	demo = function(): Node
		return ui.column {
			w_full = true,
			h_full = true,
			gap = "$space.3",
			ui.row {
				gap = "$space.3",
				items = "center",
				ds.Segmented {
					value = source,
					label = "Project",
					options = { { value = "checkout", label = "This checkout" }, { value = "large", label = "50,000 files" } },
				},
				ds.Segmented {
					value = pane,
					label = "Pane",
					options = { { value = 0, label = "Full" }, { value = 520, label = "520" }, { value = 360, label = "360" } },
				},
				ds.Segmented {
					value = head,
					label = "Header",
					options = { { value = "line", label = "One line" }, { value = "stack", label = "Two lines" }, { value = "path", label = "Path" } },
				},
				ds.Button {
					"Open…",
					tone = "secondary",
					size = "sm",
					on_click = open_from_disk,
				},
				ui.text {
					function(): string
						return did:get()
					end,
					literal = true,
					text_size = "$font.size.sm",
					color = "$text.tertiary",
				},
			},
			ui.presence(function(): string
				return `{source:get()}|{head:get()}`
			end, function(key: string): Node
				local which = string.match(key, "^[^|]*") or "checkout"
				local m = if which == "large" then large() else checkout
				-- The pane, in a frame: Zeron's right pane at its narrowest
				-- and its default, or all there is.
				return FilesPane {
					id = "files-pane",
					ref = pane_ref,
					shown = tree_shown,
					head = head:peek(),
					commands = pane_commands,
					w = function(): Length
						local w = pane:get()
						return if w == 0 then "auto" else w
					end,
					grow = function(): number
						return if pane:get() == 0 then 1 else 0
					end,
					self = "start",
					h_full = true,
					rounded = "$radius.xl",
					-- The bars' buttons sit in the card's corners: concentric.
					theme = { zeron = { icon_button = { radius = control.nest("$radius.xl", 1, 6) } } },
					border = 1,
					border_color = "$border.subtle",
					bg = "$surface.base",
					tree = {
						model = m,
						active = active,
						reveal = reveal,
						offset = offset,
						on_reveal = function(path: string)
							did:set(`Would reveal {path} in Finder`)
						end,
						on_rename = function(from: string, to: string): string?
							table.insert(moves, { from = from, to = to })
							did:set(`Renamed {from} → {to}`)
							return nil
						end,
						on_move = function(paths: { string }, into: string): string?
							for _, p in paths do
								local name = string.match(p, "[^/]+$") or p
								table.insert(moves, { from = p, to = if into == "" then name else `{into}/{name}` })
							end
							did:set(`Moved {#paths} into {if into == "" then m.name else into}`)
							return nil
						end,
						on_create = function(path: string, dir: boolean): string?
							made[path] = true
							did:set(`Made {if dir then "folder" else "file"} {path}`)
							return nil
						end,
						on_delete = function(paths: { string }): string?
							did:set(`Moved {#paths} to the Trash`)
							return nil
						end,
					},
					viewer = {
						id = "file-pane",
						doc = doc,
						root = m.name,
						limit = LIMIT,
						blocks = notes,
						block_view = function(b: any, c: any): Node
							if b.kind == "note" then
								return Inline.Note {
									inset = c.inset,
									joined = c.joined,
									from = "Claude",
									body = "This file has no tests yet. `Badge` is the place to start.",
									actions = {
										{
											label = "Dismiss",
											on_click = function()
												drop_note(b.key)
											end,
										},
									},
								}
							elseif b.kind == "thread" then
								return Inline.Thread {
									inset = c.inset,
									joined = c.joined,
									draft = note_draft,
									me = { id = "mira", name = "Mira" },
									comments = {
										{ author = { id = "ada", name = "Ada" }, at = os.time() - 7200, body = "Is this the wash every badge should use?" },
										{ author = { id = "mira", name = "Mira" }, at = os.time() - 1800, body = "Each theme has its own.", mine = true },
									},
								}
							end
							local d = doc:peek()
							return Inline.Composer {
								inset = c.inset,
								joined = c.joined,
								value = note_draft,
								location = `{if d then d.path else ""}:{b.line}`,
								on_cancel = function()
									drop_note(b.key)
								end,
								on_submit = function()
									drop_note(b.key)
								end,
							}
						end,
						on_add_block = function(line: number)
							notes:set({ { key = `note-{line}`, line = line } })
						end,
						on_save = function(path: string, text: string)
							local d = doc:peek()
							if d and d.path == path then
								local next_doc = table.clone(d)
								next_doc.text = text
								next_doc.size = #text
								doc:set(next_doc)
							end
							did:set(`Saved {path}`)
						end,
						on_crumb = function(folder: string)
							reveal:set(if folder == "" then nil else folder)
						end,
						on_reveal = function(path: string)
							did:set(`Would reveal {path} in Finder`)
						end,
						on_open_editor = function(path: string)
							did:set(`Would open {path} in your editor`)
						end,
						on_open_default = function(path: string)
							did:set(`Would open {path} in its app`)
						end,
						on_retry = function(path: string)
							open(path)
						end,
					},
				}
			end, { w_full = true, grow = 1, min_h = 0, items = "start" }),
		}
	end,
}
