-- A project's tile, as every screen draws it (ds/tile): its hue, and what
-- stands on it. A picture its people chose comes first (an emoji, or an
-- image they uploaded, fetched to a file here once), else its repository's
-- own artwork (as the shipping app finds it: crates/ui shell/project_icon.rs),
-- else its initial on its hue.
--
--   look.of(project)   { hue, emoji?, src? } (tracked: follows the files
--                      as they arrive)
--   look.by_id(id)     the same for a project by its id, nil if none
--   look.hue(id)       the hue it has by default, the same every time
--   look.artwork(p)    its repository's artwork (a file here), nil while
--                      unknown or none (tracked; asked for once)
--   look.file(url)     an uploaded image's file here (tracked; fetched once)
--
-- What's fetched is kept in named states, so a reload asks again for none
-- of it.

local sessions = require("@account/sessions")
local projects = require("@account/projects")

local M = {}

export type Look = { hue: string, emoji: string?, src: string? }

-- The hues a project's tile may wear (the palette's), in the order the
-- picker shows them.
M.HUES = { "violet", "indigo", "blue", "sky", "cyan", "teal", "emerald", "lime", "amber", "orange", "rose", "pink" }
-- (The default's order, kept as it was so no project's colour changes.)
local DEFAULT_HUES = { "violet", "blue", "emerald", "sky", "amber", "rose", "lime", "teal", "orange", "indigo", "pink", "cyan" }

function M.hue(id: string): string
	local n = 0
	for i = 1, #id do
		n = (n * 31 + string.byte(id, i)) % 9973
	end
	return DEFAULT_HUES[n % #DEFAULT_HUES + 1]
end

-- Fetched: by URL, an uploaded image's file (false: none to be had); by
-- project, its repository's artwork (false: it has none).
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

local function want_artwork(p: sessions.Project)
	local key = `art:{p.id}`
	if artworks:peek()[p.id] ~= nil or asking[key] then
		return
	end
	asking[key] = true
	local ok = pcall(function()
		projects.artwork(p, function(src: string?)
			with(artworks, p.id, src or false, key)
		end)
	end)
	if not ok then
		asking[key] = nil
	end
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
	local a = artworks:get()[p.id]
	if a == nil then
		want_artwork(p)
	end
	return if a then a else nil
end

function M.of(p: sessions.Project): Look
	local icon = p.icon
	local hue = (icon and icon.hue) or M.hue(p.id)
	if icon and icon.image then
		-- (Its file not here yet: its initial meanwhile, on its hue.)
		return { hue = hue, src = M.file(icon.image) }
	elseif icon and icon.emoji then
		return { hue = hue, emoji = icon.emoji }
	end
	return { hue = hue, src = M.artwork(p) }
end

-- (Looked up by id, not searched: a list of projects each asks for its own.)
local by_id = ui.computed(function(): { [string]: sessions.Project }
	local out: { [string]: sessions.Project } = {}
	for _, p in sessions.projects:get() do
		out[p.id] = p
	end
	return out
end)
function M.by_id(id: string?): Look?
	if not id then
		return nil
	end
	local p = by_id:get()[id]
	return if p then M.of(p) else nil
end

return M
