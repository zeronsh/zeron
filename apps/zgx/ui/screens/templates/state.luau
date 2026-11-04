--!strict
-- Your templates, as the host publishes them (src/plugins/live.rs): Zeron's
-- own first, then each one made here, which is in use, and how each stands.
--
--   state.now:get()   { state = "loading" | "ready", active = id, dir,
--                       list = { Template } }
--   state.previews:get()  each template's preview themes by its id: one a
--                         changed appearance (light, then dark), Zeron's
--                         own two for one that changes none
--   state.use(id)            a promise: the window changes to it, in place
--   state.create(name)       a promise of the new one's id (a copy of Zeron's)
--   state.duplicate(id)      a promise of the copy's id
--   state.delete(id)         a promise (Zeron's own can't be)
--   state.open_folder(id)    its folder in Finder
--   state.yours(t)           whether you (or your agents) made it
--   state.export(id, path)   a promise of `{ path, files }`: it, signed as
--                            yours, as one .zeron-template file
--   state.import(from)       from a file (`path`, or `{ path }`) or a link
--                            (`{ url }`: a .zeron-template's, or a GitHub
--                            repository or release with one): a promise of
--                            `{ installed = id, name }` (yours), or
--                            `{ pending = name }` (someone else's: the host
--                            opens Zeron's consent window for it)
--   state.check_updates()    a promise: each one from a package checked
--                            where it came from for a newer version
--   state.update(id)         a promise of `{ updated, name, version, from }`:
--                            the update found for it, in its place
--   state.review(name)       a waiting package's consent window
--   state.pending:get()      packages waiting for you to allow them

local palettes = require("@ds/palettes")

local M = {}

export type Template = {
	id: string,
	name: string,
	description: string?,
	author: string?,
	version: string?,
	-- The Zeron it was made from.
	zeron_version: string?,
	builtin: boolean,
	active: boolean,
	-- "ok" | "broken" (it can't run: Zeron's own shows) | "errors" (some of
	-- its files fall back) | "out_of_date" (made for another Zeron: Zeron
	-- has updated since)
	status: string,
	error: string?,
	-- Installed from someone's package: who signed it (`author.kind` "you",
	-- "verified" with `username`, or "unknown" with `reason`).
	-- `source`: where it came from (`{ kind, url?, ref? }`).
	imported: { key: string, author: any, source: any? }?,
	-- A newer version found where it came from (`{ version, refused? }`).
	update: { version: string, refused: string? }?,
	-- The colours its preview is drawn in (src/plugins/looks.rs): each
	-- appearance it changes, as its theme file and its palette over it.
	-- None for one that changes no colour of Zeron's.
	look: Look?,
}
export type Scheme = { theme: { type: string?, extends: string?, tokens: any }?, palette: any? }
export type Look = { stamp: string, light: Scheme?, dark: Scheme? }
-- A package waiting for you to allow it.
export type Pending = { pending: string, name: string, source: string?, files: number?, ignored: { string }? }
export type Now = { state: string, active: string?, dir: string?, list: { Template }?, pending: { Pending }? }

M.now = ui.shared("templates", { state = "loading" } :: Now)

M.list = ui.computed(function(): { Template }
	return M.now:get().list or {}
end)

M.pending = ui.computed(function(): { Pending }
	return M.now:get().pending or {}
end)

-- Each template's preview themes: its look registered once a change, each
-- appearance it changes as a theme of its own (its theme file over zgx's,
-- its palette over that or over Zeron's, through ds/palettes as the window
-- wears it), so its preview is drawn in its tokens as the window would be.
local registered: { [string]: string } = {}
local function register(name: string, stamp: string, spec: any)
	if registered[name] ~= stamp then
		registered[name] = stamp
		ui.theme.register(spec)
	end
end

local function wear(id: string, look: Look, base: { light: string, dark: string }): { string }
	local names = {}
	for _, scheme in { "light", "dark" } do
		local entry = (look :: any)[scheme] :: Scheme?
		if entry then
			local name = `preview-{id}-{scheme}`
			local under = (base :: any)[scheme]
			if entry.theme then
				local file = if entry.palette then `{name}-file` else name
				local theme = entry.theme :: any
				register(file, look.stamp, { name = file, type = theme.type or scheme, extends = theme.extends or `zgx-{scheme}`, tokens = theme.tokens or {} })
				under = file
			end
			if entry.palette then
				local p = table.clone(entry.palette)
				p.kind = scheme
				register(name, look.stamp, { name = name, type = scheme, extends = under, tokens = (palettes.tokens(p)) or {} })
			end
			table.insert(names, name)
		end
	end
	return names
end

M.previews = ui.state("templates_previews", {} :: { [string]: { string } })
ui.effect(function()
	local list = M.list:get()
	-- Zeron's own first: what an appearance a template leaves alone shows.
	-- (Its own theme names until the host has said its look.)
	local own = { light = "zeron-light", dark = "zeron-dark" }
	local out: { [string]: { string } } = {}
	for _, t in list do
		if t.id == "zeron" and t.look then
			local names = wear(t.id, t.look, own)
			if #names == 2 then
				own = { light = names[1], dark = names[2] }
			end
		end
	end
	for _, t in list do
		local names = if t.look and t.id ~= "zeron" then wear(t.id, t.look, own) else {}
		out[t.id] = if #names > 0 then names else { own.light, own.dark }
	end
	M.previews:set(out)
end)

-- Yours: made on this device, by you or an agent you asked (theirs say
-- "you", or nothing, written by hand), or installed from a package you
-- signed. Zeron's own is built in; someone else's says who.
function M.yours(t: Template): boolean
	if t.builtin then
		return false
	end
	if t.imported then
		return t.imported.author ~= nil and t.imported.author.kind == "you"
	end
	return t.author == nil or t.author == "" or t.author == "you"
end

function M.use(id: string): any
	return ui.call("templates.use", id)
end

function M.create(name: string): any
	return ui.call("templates.create", name)
end

function M.duplicate(id: string): any
	return ui.call("templates.duplicate", id)
end

function M.delete(id: string): any
	return ui.call("templates.delete", id)
end

function M.export(id: string, path: string): any
	return ui.call("templates.export", { id = id, path = path })
end

function M.import(from: string | { path: string?, url: string? }): any
	return ui.call("templates.import", from)
end

function M.check_updates(): any
	return ui.call("templates.check_updates", nil)
end

function M.update(id: string): any
	return ui.call("templates.update", id)
end

function M.review(name: string)
	ui.invoke("templates.review", name)
end

function M.open_folder(id: string)
	ui.invoke("templates.open_folder", id)
end

return M
