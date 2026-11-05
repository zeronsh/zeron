-- The files pane's host, made up for tests (no engine in a test host): a
-- disk in memory, the host's calls (screens/files/api) stubbed to read and
-- write it as the engine would, and `ui.command`s that change it as
-- someone else would (an agent, another editor), telling the pane as the
-- host's watch does. Required by ui/files_fixture.luau and
-- ui/panel_fixture.luau.
local api = require("@app/screens/files/api")
-- Every line the highlighter colours, counted (wrapped before any editor
-- takes it): what width changes must not cause.
local Syntax = require("@ds/syntax")
local highlighted = 0
do
	local made = Syntax.highlighter
	Syntax.highlighter = function(lang: string?): Syntax.Highlighter?
		local f = made(lang)
		if not f then
			return f
		end
		return function(line: string, state: any): ({ Syntax.Span }, any)
			highlighted += 1
			return f(line, state)
		end
	end
end
local highlights = ui.state("test_files_highlights", 0)
ui.command {
	id = "count_highlights",
	title = "Say how many lines were coloured",
	run = function()
		highlights:set(highlighted)
	end,
}
local Workspace = require("@app/screens/files/workspace")

-- The disk: each file's text (folders are what paths pass through, and
-- `dirs`, those made empty).
local disk: { [string]: string } = {}
local dirs: { [string]: boolean } = {}
local revs: { [string]: number } = {}
local function lines(prefix: string, n: number): string
	local out = table.create(n)
	for i = 1, n do
		table.insert(out, `{prefix} line {i}`)
	end
	return table.concat(out, "\n") .. "\n"
end
for i = 1, 40 do
	disk[string.format("src/a%02d.txt", i)] = `a{i}\n`
end
for i = 1, 30 do
	disk[string.format("src/deep/d%02d.txt", i)] = `d{i}\n`
end
disk["long.txt"] = lines("long", 400)
disk["notes.md"] = "# Notes\n"
-- Code, coloured as it shows.
do
	local out = table.create(400)
	for i = 1, 400 do
		table.insert(out, `local value{i} = function(x: number): number return x * {i} end -- line {i}`)
	end
	disk["code.luau"] = table.concat(out, "\n") .. "\n"
end
disk["zeta.txt"] = "z\n"
-- Lines far wider than a pane (last in the tree: the others keep their rows),
-- and enough of them that scrolling moves the editor's window of lines.
do
	local out = table.create(600)
	for i = 1, 600 do
		table.insert(out, `row {i} ` .. string.rep("w", 300) .. " end")
	end
	disk["zwide.txt"] = table.concat(out, "\n") .. "\n"
end
-- Another folder, for another session.
disk["other/readme.txt"] = "other\n"

local function hash(text: string): string
	local h = 5381
	for i = 1, #text do
		h = (h * 33 + string.byte(text, i)) % 4294967296
	end
	return `h{h}`
end

-- What was asked of the host, in order: `list:src`, `write:long.txt`, …
local said: { string } = {}
local calls = ui.state("test_files_calls", "")
local function note(what: string)
	table.insert(said, what)
	calls:set(table.concat(said, ","))
end

-- A promise as the host's settle: a beat later, from one clock the script
-- keeps (a timer made by whoever asked would go with them, where the
-- host's answer comes anyway).
local due: { () -> () } = {}
ui.every(16, function()
	if #due > 0 then
		local now = due
		due = {}
		for _, f in now do
			f()
		end
	end
	return true
end)
local function settle(value: any, err: string?): any
	return {
		-- (In a task, as a resource's load: answered at once.)
		await = function(_): any
			if err then
				error(err, 0)
			end
			return value
		end,
		after = function(_, ok: (any) -> (), fail: ((any) -> ())?)
			table.insert(due, function()
				if err then
					if fail then
						fail({ message = err })
					end
				else
					ok(value)
				end
			end)
		end,
	}
end

-- Where the files are: the made-up project's folder, or "other".
local function root_of(target: api.Target): string
	return if target.root == "/other" then "other/" else ""
end

local stub = api :: any
-- The folder as on another device (`test_files_away`): its pictures read
-- from there.
local away = ui.state("test_files_away", false)
stub.list = function(target: api.Target, dir: string): any
	note(`list:{dir}`)
	local base = root_of(target)
	local prefix = base .. (if dir == "" then "" else dir .. "/")
	local seen: { [string]: boolean } = {}
	local entries: { api.Entry } = {}
	local function add(rest: string, is_dir: boolean, size: number?)
		if seen[rest] then
			return
		end
		seen[rest] = true
		local path = if dir == "" then rest else `{dir}/{rest}`
		revs[base .. path] = revs[base .. path] or 1
		table.insert(entries, {
			name = rest,
			path = path,
			dir = is_dir,
			kind = if is_dir then "directory" else "file",
			size = size,
			-- (What's under build/ is what a .gitignore leaves out.)
			ignored = string.find(path, "^build") ~= nil,
			readonly = false,
			rev = `r{revs[base .. path]}`,
		})
	end
	local function walk(path: string, text: string?)
		if string.sub(path, 1, #prefix) ~= prefix then
			return
		end
		local rest = string.sub(path, #prefix + 1)
		if base == "" and string.sub(rest, 1, 6) == "other/" and dir == "" then
			return
		end
		local cut = string.find(rest, "/", 1, true)
		if cut then
			add(string.sub(rest, 1, cut - 1), true)
		elseif rest ~= "" then
			add(rest, text == nil, if text then #text else nil)
		end
	end
	for path, text in disk do
		walk(path, text)
	end
	for path in dirs do
		walk(path, nil)
	end
	return settle({ checkout = "c1", can_move = true, can_delete = true, here = not away:peek(), truncated = false, entries = entries })
end
stub.read = function(target: api.Target, path: string): any
	note(`read:{path}`)
	local text = disk[root_of(target) .. path]
	if not text then
		return settle(nil, "file not found")
	end
	-- (A picture "on another device": its bytes only from there, through
	-- `image`; `away.png` fails to come.)
	if string.find(path, "%.png$") then
		return settle({ checkout = "c1", path = path, hash = hash(text), size = #text, encoding = "binary", truncated = false, here = false })
	end
	return settle({ checkout = "c1", path = path, text = text, hash = hash(text), size = #text, encoding = "utf8", line_ending = "lf", truncated = false, here = true })
end
stub.image = function(_target: api.Target, path: string, checkout: string): any
	note(`image:{path}@{checkout}`)
	if path == "away.png" then
		return settle(nil, "The device is offline.")
	end
	-- (A small picture of the app's own, as a file kept here would be.)
	return settle({ src = "assets/favicons/zgx.png", size = 1 })
end
stub.write = function(target: api.Target, request: api.Write): any
	note(`write:{request.path}`)
	local full = root_of(target) .. request.path
	local now = disk[full]
	if not now then
		return settle({ status = "conflict", reason = "deleted" })
	end
	if hash(now) ~= request.hash then
		return settle({ status = "conflict", reason = "changed", hash = hash(now) })
	end
	disk[full] = request.text
	return settle({ status = "written", hash = hash(request.text), size = #request.text })
end
stub.move = function(_target: api.Target, request: api.Move): any
	note(`move:{request.from}:{request.to}`)
	return settle({ status = "applied" })
end
stub.delete = function(_target: api.Target, request: api.Delete): any
	note(`delete:{request.path}`)
	return settle({ status = "applied" })
end
stub.create = function(target: api.Target, path: string, dir: boolean, _checkout: string): any
	note(`create:{path}`)
	local full = root_of(target) .. path
	if disk[full] or dirs[full] then
		return settle({ status = "rejected", reason = "destinationExists", message = "Something already has that name" })
	end
	if dir then
		dirs[full] = true
	else
		disk[full] = ""
	end
	return settle({ status = "applied" })
end
stub.reveal = function(_target: api.Target, path: string): any
	note(`reveal:{path}`)
	return settle(nil)
end
stub.open_external = function(_target: api.Target, path: string): any
	note(`open_external:{path}`)
	return settle(nil)
end
stub.search = function(_target: api.Target, query: string): any
	note(`search:{query}`)
	local found: { { path: string, dir: boolean } } = {}
	for path in disk do
		local name = string.match(path, "[^/]+$") or path
		if string.find(name, query, 1, true) then
			table.insert(found, { path = path, dir = false })
		end
	end
	return settle(found)
end
-- The host's watches: let go of, one lives on for a moment (a reload asks
-- again within it, and finds it kept); here, for the test's length.
local watching: { [string]: boolean } = {}
stub.watch = function(key: string, _target: api.Target): any
	note(`watch:{key}`)
	local kept = watching[key] == true
	watching[key] = true
	return settle({ kept = kept })
end
stub.unwatch = function(key: string)
	note(`unwatch:{key}`)
end

-- The host's watch: a frame for every watch, numbered per folder, kept
-- here and read through `frames` as the host's are.
local sequence = 0
local kept_frames: { [string]: { api.Frame } } = {}
local function tell(changes: { api.Change })
	sequence += 1
	local next_of = table.clone(api.events:peek())
	for key in watching do
		local n = next_of[key] or 1
		kept_frames[key] = kept_frames[key] or {}
		table.insert(kept_frames[key], { n = n, key = key, sequence = sequence, resync = false, changes = changes })
		next_of[key] = n + 1
	end
	api.events:set(next_of)
end
stub.frames = function(key: string, since: number): { api.Frame }
	local out = {}
	for _, f in kept_frames[key] or {} do
		if f.n > since then
			table.insert(out, f)
		end
	end
	return out
end

ui.command {
	id = "disk_add",
	title = "A file made in src",
	run = function()
		disk["src/a05b.txt"] = "new\n"
		tell({ { kind = "created", path = "src/a05b.txt" } })
	end,
}
-- Four files made in src, a change frame each, 10 ms apart: each lands in
-- the beat the last one's re-read waits.
ui.command {
	id = "disk_burst",
	title = "Files made in src, quickly",
	run = function()
		for i = 1, 4 do
			ui.after(i * 10, function()
				local path = `src/0{i}-new.txt`
				disk[path] = "b\n"
				tell({ { kind = "created", path = path } })
			end)
		end
	end,
}
-- A document with a picture beside it and a link to another file.
ui.command {
	id = "disk_md_links",
	title = "A guide with a picture and a link",
	run = function()
		disk["docs/guide.md"] = "# Guide\n\n- [ ] Write it\n- [x] Plan it\n\n```\n- [ ] not a task\n```\n\n![shot](../shot.png)\n\nSee [the notes](../notes.md).\n"
		disk["shot.png"] = "png"
		tell({ { kind = "created", path = "docs/guide.md" }, { kind = "created", path = "shot.png" } })
	end,
}
-- Pictures made on another device: one that comes, one that can't.
ui.command {
	id = "disk_remote_images",
	title = "Pictures on another device",
	run = function()
		disk["shot.png"] = "png"
		disk["away.png"] = "png"
		tell({ { kind = "created", path = "shot.png" }, { kind = "created", path = "away.png" } })
	end,
}
ui.command {
	id = "disk_remove",
	title = "A file deleted in src",
	run = function()
		disk["src/a03.txt"] = nil
		tell({ { kind = "removed", path = "src/a03.txt" } })
	end,
}
ui.command {
	id = "disk_edit",
	title = "long.txt changed on disk",
	run = function()
		disk["long.txt"] = "changed elsewhere\n" .. disk["long.txt"]
		revs["long.txt"] = (revs["long.txt"] or 1) + 1
		tell({ { kind = "modified", path = "long.txt" } })
	end,
}
ui.command {
	id = "disk_delete",
	title = "long.txt deleted on disk",
	run = function()
		disk["long.txt"] = nil
		tell({ { kind = "removed", path = "long.txt" } })
	end,
}
ui.command {
	id = "make_file",
	title = "Make src/0-made.txt",
	run = function()
		Workspace.create(Workspace.get("here:/proj", { chat = "chat-1", root = "/proj" }, "proj"), "src/0-made.txt", false)
	end,
}
-- Many folders opened after this one, which has unsaved edits and isn't
-- shown in a view: whether its edits outlast the slots running out
-- (`test_files_kept`), and whether each folder opened has slots of its own.
ui.command {
	id = "many_folders",
	title = "Open many folders",
	run = function()
		local pane = Workspace.get("here:/proj", { chat = "chat-1", root = "/proj" }, "proj")
		local first = Workspace.get("away:/one", { chat = "chat-x", root = "/one" }, "one")
		first.drafts:set_key("a.txt", { text = "edited", base = "orig" })
		local shared = 0
		for n = 1, 30 do
			local w = Workspace.get(`away:/f{n}`, { chat = `chat-{n}`, root = `/f{n}` }, `f{n}`)
			if w.drafts == first.drafts then
				shared += 1
			end
		end
		local again = Workspace.get("away:/one", { chat = "chat-x", root = "/one" }, "one")
		local kept = again.drafts:peek()["a.txt"]
		-- (The pane's folder, shown in its view all along: never let go.)
		local same = Workspace.get("here:/proj", { chat = "chat-1", root = "/proj" }, "proj") == pane
		ui.state("test_files_kept", ""):set(`{if kept then kept.text else "lost"},shared:{shared},pane:{same}`)
	end,
}
-- A plugin's item on file menus, as the host publishes them.
ui.shared("plugins.menus", {} :: { any })
ui.command {
	id = "plugin_item",
	title = "A plugin adds a file menu item",
	run = function()
		ui.shared("plugins.menus", {} :: { any }):set({
			{ plugin = "counter", plugin_name = "Line Counter", kind = "file", id = "count", label = "Count Lines" },
		})
	end,
}
ui.command {
	id = "disk_edit_unheard",
	title = "long.txt changed on disk, its change not heard yet",
	run = function()
		disk["long.txt"] = "changed elsewhere\n" .. disk["long.txt"]
	end,
}

ui.command {
	id = "disk_hidden_files",
	title = "A dotfile and an ignored folder appear (Settings → Files' filter)",
	run = function()
		disk[".env"] = "SECRET=1\n"
		disk["build/out.txt"] = "out\n"
		tell({ { kind = "created", path = ".env" }, { kind = "created", path = "build/out.txt" } })
	end,
}

ui.command {
	id = "open_file",
	title = "Open long.txt",
	run = function()
		local w = Workspace.get("here:/proj", { chat = "chat-1", root = "/proj" }, "proj")
		Workspace.view(w, "pane:here:/proj").active:set("long.txt")
	end,
}
ui.command {
	id = "ask_delete",
	title = "Ask to delete zeta.txt",
	run = function()
		local w = Workspace.get("here:/proj", { chat = "chat-1", root = "/proj" }, "proj")
		Workspace.delete(w, { "zeta.txt" }, false)
	end,
}

-- Review comments, as the chat's composer would stage them: kept here,
-- each call recorded with the host's.
do
	local comments = require("@app/screens/files/comments") :: any
	local staged = ui.state("test_files_comments", {} :: { any })
	local made = 0
	comments.available = function(): boolean
		return true
	end
	comments.add = function(c: any): string
		made += 1
		local id = `comment-{made}`
		note(`comment:{c.path}:{c.line}:{c.body}`)
		local list = table.clone(staged:peek())
		table.insert(list, { id = id, path = c.path, line = c.line, body = c.body })
		staged:set(list)
		return id
	end
	comments.of = function(_root: string?, path: string): { any }
		local out = {}
		for _, c in staged:get() do
			if c.path == path then
				table.insert(out, { id = c.id, line = c.line, body = c.body })
			end
		end
		return out
	end
	local function set(id: string, field: string, value: any)
		local list = table.clone(staged:peek())
		for i, c in list do
			if c.id == id then
				local next_c = table.clone(c)
				next_c[field] = value
				list[i] = next_c
			end
		end
		staged:set(list)
	end
	comments.edit = function(id: string, body: string)
		note(`comment-edit:{id}:{body}`)
		set(id, "body", body)
	end
	comments.move = function(id: string, line: number)
		set(id, "line", line)
	end
	comments.remove = function(id: string)
		note(`comment-remove:{id}`)
		local list = {}
		for _, c in staged:peek() do
			if c.id ~= id then
				table.insert(list, c)
			end
		end
		staged:set(list)
	end
end

-- (`note`: a call of the app's own recorded with the host's, as the
-- fixture's Add to Chat.)
return { settle = settle, note = note }
