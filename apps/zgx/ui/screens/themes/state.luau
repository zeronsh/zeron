--!strict
-- Themes, as the host lists them (src/themes.rs): Zeron's built in, each
-- template's own, and your imported VS Code colour themes, with the one
-- chosen for each scheme. A theme is only colours, so it's worn over any
-- template (ds/palettes, ds/tokens): a scheme with none chosen wears the
-- running template's own theme, else Zeron's.
--
--   state.entries:get()       every theme ({ Entry }), built in first
--   state.ready()             whether the host has listed them
--   state.palette(id)         a variant's palette
--   state.has(e, kind)        the variant of `kind` a theme has (its id), if any
--   state.worn(kind)          what that scheme wears now (a variant's id)
--   state.showing()           the schemes that show: both while following
--                             the system, else the fixed one
--   state.fills(e)            the schemes it's worn for now ({ light?, dark? })
--   state.full(e)             it's worn for every scheme it can be, of those showing
--   state.use(e)              worn for each scheme it has; with a fixed
--                             scheme it lacks, the scheme it has is shown
--   state.use_for(kind, id)   variant `id` worn for that scheme alone
--   state.import(path)        a theme file or an extension's folder brought in
--   state.rename(id, name), state.remove(id)   an imported one's
--   state.previews:get()      the themes each one's swatch halves wear, by id
--                             (kept while `state.swatches()`'s owner lives)
--   state.error, state.busy   an import's error, and one under way

local ds = require("@ds")
local palettes = require("@ds/palettes")
local templates = require("@app/screens/templates/state")
local appearance = require("@app/screens/shell/appearance")

local M = {}

export type Entry = {
	id: string,
	name: string,
	-- "builtin" | "template" | "imported"
	source: string,
	variants: { string },
	-- A template's: its id.
	template: string?,
	-- An imported one's: whether it compiled cleanly, and what's wrong.
	status: { ok: boolean, message: string? }?,
}

-- What the host does, where a test may stand in for it: each a promise
-- (`:after(ok, err)`) but remove.
M.host = {
	import = function(path: string): any
		return ui.call("themes.import", path)
	end,
	remove = function(id: string)
		ui.invoke("themes.remove", id)
	end,
	rename = function(id: string, name: string): any
		return ui.call("themes.rename", { id = id, name = name })
	end,
	choose = function(slots: { [string]: string }): any
		return ui.call("themes.choose", slots)
	end,
}

M.entries = ui.computed(function(): { Entry }
	return (palettes.library():get().entries or {}) :: { Entry }
end)

function M.ready(): boolean
	return palettes.library():get().state == "ready"
end

M.palette = palettes.find

-- The first variant of `kind` theme `e` has.
function M.has(e: Entry, kind: string): string?
	for _, id in e.variants do
		local p = palettes.find(id)
		if p and p.kind == kind then
			return id
		end
	end
	return nil
end

-- The running template's own theme, listed (none for the default).
local function template_theme(): Entry?
	local active = templates.now:get().active
	for _, e in M.entries:get() do
		if e.source == "template" and e.template == active then
			return e
		end
	end
	return nil
end

-- What scheme `kind` wears now: the theme chosen for it, else the running
-- template's own, else Zeron's.
function M.worn(kind: string): string
	local chosen = (palettes.library():get().chosen or {})[kind]
	local p = palettes.find(chosen)
	if p and p.kind == kind then
		return p.id
	end
	local own = template_theme()
	local suggested = if own then M.has(own, kind) else nil
	return suggested or `zeron-{kind}`
end

function M.showing(): { string }
	local scheme = appearance.scheme:get()
	if scheme == "light" or scheme == "dark" then
		return { scheme }
	end
	return { "light", "dark" }
end

function M.fills(e: Entry): { [string]: boolean }
	local out = {}
	for _, kind in M.showing() do
		local id = M.has(e, kind)
		if id and M.worn(kind) == id then
			out[kind] = true
		end
	end
	return out
end

-- Worn for every scheme showing that it has a variant for (at least one).
function M.full(e: Entry): boolean
	local any = false
	for _, kind in M.showing() do
		if M.has(e, kind) then
			if M.worn(kind) ~= M.has(e, kind) then
				return false
			end
			any = true
		end
	end
	return any
end

-- What went wrong asking the host, as words.
function M.said(err: any): string
	if type(err) == "table" and err.message then
		return tostring(err.message)
	end
	return tostring(err)
end

local function choose(slots: { [string]: string })
	local p: any = M.host.choose(slots)
	if p ~= nil then
		p:after(function(_: any) end, function(err: any)
			ds.toast.show { title = ui.t("themes.page.couldnt_use"):get(), body = M.said(err), tone = "warning" }
		end)
	end
end

function M.use(e: Entry)
	local slots: { [string]: string } = {}
	for _, kind in { "light", "dark" } do
		slots[kind] = M.has(e, kind) :: any
	end
	if next(slots) == nil then
		return
	end
	-- A fixed scheme it has no variant for: the one it has is shown.
	local scheme = appearance.scheme:peek()
	if (scheme == "light" or scheme == "dark") and not slots[scheme] then
		appearance.scheme:set(if slots.dark then "dark" else "light")
	end
	choose(slots)
end

function M.use_for(kind: string, id: string)
	choose({ [kind] = id })
end

-- An import's error, in the shipping app's words, until the next import.
M.error = ui.state("themes_import_error", nil :: string?)
M.busy = ui.state("themes_importing", false)

-- `path` brought in: a promise of the entry it made (`{ entry, variants,
-- skipped }`). Its error stays on the page until the next try.
function M.import(path: string): any
	M.error:set(nil)
	M.busy:set(true)
	local p: any = M.host.import(path)
	p:after(function(done: any)
		M.busy:set(false)
		if done and done.skipped and #done.skipped > 0 then
			M.error:set(ui.t("themes.page.skipped", { names = table.concat(done.skipped, "; ") }):get())
		end
	end, function(err: any)
		M.busy:set(false)
		M.error:set(M.said(err))
	end)
	return p
end

function M.rename(id: string, name: string): any
	return M.host.rename(id, name)
end

-- An imported theme gone (a scheme it's worn for goes back: the host's).
function M.remove(id: string)
	M.host.remove(id)
end

-- Each variant's swatch theme: its palette over Zeron's theme of its
-- scheme, registered once a change (Zeron's own are worn as they are).
local registered: { [string]: string } = {}
local function swatch_theme(p: palettes.Palette): string
	if p.id == "zeron-light" or p.id == "zeron-dark" then
		return p.id
	end
	local name = `swatch-{p.id}`
	local stamp = `{p.background}|{p.text}|{p.accent}|{table.concat(p.syntax or {}, ",")}`
	if registered[name] ~= stamp then
		registered[name] = stamp
		ui.theme.register { name = name, type = p.kind :: any, extends = `zeron-{p.kind}`, tokens = palettes.tokens(table.clone(p)) }
	end
	return name
end

-- The themes each theme's swatch halves wear (light, then dark), by its id:
-- kept by `swatches()`, which the page runs while it's open.
M.previews = ui.state("themes_previews", {} :: { [string]: { string } })

function M.swatches()
	ui.effect(function()
		local out: { [string]: { string } } = {}
		for _, e in M.entries:get() do
			local names = {}
			for _, kind in { "light", "dark" } do
				local p = palettes.find(M.has(e, kind))
				if p then
					table.insert(names, swatch_theme(p))
				end
			end
			out[e.id] = names
		end
		M.previews:set(out)
	end)
end

return M
