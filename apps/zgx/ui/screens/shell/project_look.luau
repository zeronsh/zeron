-- A project's tile, as every screen draws it (ds/tile): its hue, and what
-- stands on it. A picture its people chose comes first (an emoji, or an
-- image they uploaded, fetched to a file here once), else its repository's
-- own artwork (as the shipping app finds it: crates/ui shell/project_icon.rs),
-- else its initial on its hue. That default (its hue and artwork) is its
-- repository's, the same on every device for every copy of it
-- (project_default); a `local:` repository or a folder keeps its own.
--
--   look.of(project)   { hue, emoji?, src? } (tracked: follows the files
--                      as they arrive)
--   look.by_id(id)     the same for a project by its id, nil if none
--   look.hue(id)       the hue it has by default, the same every time
--                      (tracked: a copy's repository may be learnt later)
--   look.artwork(p)    its repository's artwork (a file here), nil while
--                      unknown or none (tracked; asked for once per
--                      repository: from its oldest copy that has one)
--   look.file(url)     an uploaded image's file here (tracked; fetched once)
--
-- What's fetched is kept in named states, so a reload asks again for none
-- of it.

local sessions = require("@account/sessions")
local projects = require("@account/projects")
local default = require("@app/screens/shell/project_default")

local M = {}

export type Look = { hue: string, emoji: string?, src: string? }

-- The hues a project's tile may wear (the palette's), in the order the
-- picker shows them.
M.HUES = { "violet", "indigo", "blue", "sky", "cyan", "teal", "emerald", "lime", "amber", "orange", "rose", "pink" }

-- (Looked up by id, not searched: a list of projects each asks for its own.)
local by_id = ui.computed(function(): { [string]: sessions.Project }
	local out: { [string]: sessions.Project } = {}
	for _, p in sessions.projects:get() do
		out[p.id] = p
	end
	return out
end)

-- (An id that's no project's, its own hue, as ever.)
function M.hue(id: string): string
	local p = by_id:get()[id]
	return if p then default.hue(p) else default.hue_of(id)
end

-- Fetched: by URL, an uploaded image's file (false: none to be had); by
-- repository (a source's key), its artwork (false: it has none).
local files = ui.state("project_look_files", {} :: { [string]: string | false })
local artworks = ui.state("project_look_artwork", {} :: { [string]: string | false })
-- (On their way: asked once.)
local asking: { [string]: boolean } = {}

-- Answers, written together: hundreds of projects' answer in the same moment, and
-- what follows these maps needn't work everything out again for each. (Still
-- `asking` until written, so nothing asks again meanwhile.)
local answers: { { map: State<{ [string]: string | false }>, key: string, value: string | false, asked: string } } = {}
local function write_answers()
	if #answers == 0 then
		return
	end
	local got = answers
	answers = {}
	local maps: { [any]: { [string]: string | false } } = {}
	for _, a in got do
		local next = maps[a.map]
		if not next then
			next = table.clone(a.map:peek())
			maps[a.map] = next
		end
		next[a.key] = a.value
		asking[a.asked] = nil
	end
	for map, next in maps do
		(map :: any):set(next)
	end
end
-- (`soon`: at once, a picture someone just chose. Else a repository's artwork, which
-- may come in over seconds: at most every 100 ms, so hundreds spread out don't work
-- it all out again hundreds of times.)
local writing = false
local function with(map: State<{ [string]: string | false }>, key: string, value: string | false, asked: string, soon: boolean?)
	table.insert(answers, { map = map, key = key, value = value, asked = asked })
	if soon then
		ui.after(0, write_answers)
	elseif not writing then
		writing = true
		ui.after(100, function()
			writing = false
			write_answers()
		end)
	end
end

local function want_file(url: string)
	if files:peek()[url] ~= nil or asking[url] then
		return
	end
	asking[url] = true
	local ok = pcall(function()
		projects.icon_file(url):after(function(path: any)
			with(files, url, if type(path) == "string" and path ~= "" then path else false, url, true)
		end, function()
			with(files, url, false, url, true)
		end)
	end)
	if not ok then
		asking[url] = nil
	end
end

-- Where each project's artwork is looked for, by its id: its repository's
-- copies in one order (project_default), and the key it's kept under, the
-- same for them all. (Worked out once per change, not per tile.)
type Source = { key: string, copies: { sessions.Project } }
local sources = ui.computed(function(): { [string]: Source }
	local all = sessions.projects:get()
	local out: { [string]: Source } = {}
	for _, p in all do
		local copies = default.copies(p, all)
		-- (Its oldest copy in the key: a copy older still, learnt later,
		-- asks again from there.)
		out[p.id] = { key = `{default.key(p)}@{copies[1].id}`, copies = copies }
	end
	return out
end)
local function source_of(p: sessions.Project): Source
	return sources:get()[p.id] or { key = p.id, copies = { p } }
end

-- (Its copies asked in turn, until one has any: one unreachable, or with
-- none, the next.)
local function want_artwork(p: sessions.Project)
	local source = source_of(p)
	local key, asked = source.key, `art:{source.key}`
	if artworks:peek()[key] ~= nil or asking[asked] then
		return
	end
	asking[asked] = true
	local function try(i: number)
		local copy = source.copies[i]
		if not copy then
			with(artworks, key, false, asked)
			return
		end
		local ok = pcall(function()
			projects.artwork(copy, function(src: string?)
				if src then
					with(artworks, key, src, asked)
				else
					try(i + 1)
				end
			end)
		end)
		if not ok then
			try(i + 1)
		end
	end
	try(1)
end

-- Each project's picture asked for as it comes (or changes): its uploaded
-- image's file, or, wearing no picture of its own, its repository's.
ui.effect(function()
	if sessions.state:get() ~= "ready" then
		return
	end
	for _, p in sessions.projects:get() do
		local icon = p.icon
		if icon and icon.image then
			want_file(icon.image)
		elseif not (icon and icon.emoji) then
			want_artwork(p)
		end
	end
end)

function M.file(url: string?): string?
	if not url or url == "" then
		return nil
	end
	local f = files:get()[url]
	if f == nil then
		want_file(url)
	end
	return if f then f else nil
end

function M.artwork(p: sessions.Project): string?
	local a = artworks:get()[source_of(p).key]
	if a == nil then
		want_artwork(p)
	end
	return if a then a else nil
end

function M.of(p: sessions.Project): Look
	local icon = p.icon
	local hue = (icon and icon.hue) or default.hue(p)
	if icon and icon.image then
		-- (Its file not here yet: its initial meanwhile, on its hue.)
		return { hue = hue, src = M.file(icon.image) }
	elseif icon and icon.emoji then
		return { hue = hue, emoji = icon.emoji }
	end
	return { hue = hue, src = M.artwork(p) }
end

function M.by_id(id: string?): Look?
	if not id then
		return nil
	end
	local p = by_id:get()[id]
	return if p then M.of(p) else nil
end

return M
