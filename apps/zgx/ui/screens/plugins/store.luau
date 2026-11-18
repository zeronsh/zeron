--!strict
-- The plugin store, as Settings → Plugins asks it: Browse, a plugin's page,
-- and publishing from Yours. Every call goes through here, so a fixture
-- answers them as the host would (ui/store_stub.luau). The engine is the
-- store's one client (its `Store*` calls, crates/rpc); the host only carries
-- them (src/plugins/catalog.rs). Installing from the store is
-- `state.install { store = id, version?, device? }`; a version the store
-- took down is turned back on with `state.set_off(id, false, device)`.
--
-- Each returns the host's promise, `:after(ok, err)`; `err.message` is an
-- outcome word ("offline", "signed_out", "not_found") or "<reason>: <words>"
-- (screens/plugins/words `failure`).
--
-- Reading the catalog:
--   store.search { q?, category?, keyword?, sort?, cursor?, limit?, featured? }
--       { items = { Card }, next_cursor? }   sort: "relevance" | "popular" |
--                                            "updated" | "new"
--   store.detail(id)          Detail (rejects "not_found")
--   store.categories()        { categories = { { id, name } } }
--   store.picture(id, version, path)  { src }: a picture of its package, as
--                             the engine keeps it here
--   store.icon(id, version, path)     { svg }: its icon, or nil
--   store.prereleases(id, on, device?)  whether that device takes its
--                             pre-releases
--   store.report(id, reason, text)
-- Your own:
--   store.mine()              { plugins = { Card + { listing, versions } } }
--   store.version_action(id, version, action)   "withdraw" | "yank" | "unyank"
--   store.prepare(id)         { id, version, hash, ok, problems = { { rule,
--                             field?, path?, words } }, warnings, listing,
--                             notes, package = { files, bytes }, ticket? }
--   store.publish(id, hash, ticket)  { id, version, status, checks }
--   store.keys(id)            { keys = { Key } }
--   store.create_key { id, name, expires? }   { key, secret } once: a fresh
--                             sign-in first when the store asks for one
--   store.cancel_step_up()    stops waiting for that sign-in
--   store.revoke_key(id, key)
--
-- Shapes (the plan's contract and its amendments,
-- docs/plans/2026-11-17-plugin-store.md):
--   Card    { id, name, summary, icon?, author = { username, badges = {
--             { id, name } } }, categories, keywords, version, updated_at,
--             installs, featured }
--   Detail  Card + { readme, screenshots = { path }, permissions (the consent
--             sheet's item: { asks, ui, commands, network_hosts, tools… }),
--             engines, homepage?, repository?, license?, versions = { Version },
--             package, compatible, incompatible? (why not, in words) }
--   Version { version, status, published_at, notes?, engines?, yanked?,
--             review? = { state, reason?, message?, decided_at? } }
--   Key     { key, name, created_at, last_used_at?, expires_at?, revoked_at? }

local M = {}

export type Badge = { id: string?, name: string? }
export type Author = { username: string?, badges: { Badge | string }? }
export type Card = {
	id: string,
	name: string,
	summary: string?,
	icon: string?,
	author: Author?,
	categories: { string }?,
	keywords: { string }?,
	version: string,
	updated_at: number?,
	installs: number?,
	featured: boolean?,
}
export type Version = {
	version: string,
	status: string,
	published_at: number?,
	notes: string?,
	engines: { zeron: string? }?,
	yanked: boolean?,
	prerelease: boolean?,
	review: { state: string?, reason: string?, message: string?, decided_at: number? }?,
}
export type Query = {
	q: string?,
	category: string?,
	keyword: string?,
	sort: string?,
	cursor: string?,
	limit: number?,
	featured: boolean?,
}

-- How many a page asks for (the store gives at most 50).
M.PAGE = 30

function M.search(query: Query): any
	return ui.call("plugins.store.search", query)
end

function M.detail(id: string): any
	return ui.call("plugins.store.detail", id)
end

function M.categories(): any
	return ui.call("plugins.store.categories", nil)
end

function M.picture(id: string, version: string, path: string): any
	return ui.call("plugins.store.picture", { id = id, version = version, path = path })
end

function M.icon(id: string, version: string, path: string): any
	return ui.call("plugins.store.icon", { id = id, version = version, path = path })
end

function M.prereleases(id: string, on: boolean, device: string?): any
	return ui.call("plugins.store.prereleases", { id = id, on = on, device = device })
end

function M.report(id: string, reason: string, text: string): any
	return ui.call("plugins.store.report", { id = id, reason = reason, text = text })
end

function M.mine(): any
	return ui.call("plugins.store.mine", nil)
end

function M.version_action(id: string, version: string, action: string): any
	return ui.call("plugins.store.version_action", { id = id, version = version, action = action })
end

function M.prepare(id: string): any
	return ui.call("plugins.store.prepare", id)
end

function M.publish(id: string, hash: string, ticket: string): any
	return ui.call("plugins.store.publish", { id = id, hash = hash, ticket = ticket })
end

function M.keys(id: string): any
	return ui.call("plugins.store.keys", id)
end

function M.create_key(args: { id: string, name: string, expires: number? }): any
	return ui.call("plugins.store.create_key", args)
end

function M.cancel_step_up()
	ui.invoke("plugins.store.cancel_step_up", nil)
end

function M.revoke_key(id: string, key: string): any
	return ui.call("plugins.store.revoke_key", { id = id, key = key })
end

-- What a report may say is wrong (the store's list).
M.REPORT_REASONS = { "broken", "misleading", "malware", "copyright", "other" }

-- A store id, as plugin ids are (the engine's manifest rule,
-- `^[a-z0-9][a-z0-9-]{1,39}$`): what a link may name, and nothing else.
function M.valid_id(id: any): boolean
	return type(id) == "string" and #id >= 2 and #id <= 40 and string.match(id, "^[a-z0-9][a-z0-9%-]*$") ~= nil
end

-- Its pictures, as the host has them here: by `id@version/path`, the file
-- to show, or false for one that can't come (not a picture of its
-- package, too big, offline). Read reactively, asked once.
local pictures = ui.state("store_pictures", {} :: { [string]: string | false })
local asking: { [string]: boolean } = {}

function M.picture_src(id: string, version: string, path: string): string?
	local key = `{id}@{version}/{path}`
	local have = pictures:get()[key]
	if have ~= nil then
		return have or nil
	end
	if not asking[key] then
		asking[key] = true
		M.picture(id, version, path):after(function(r: any)
			asking[key] = nil
			pictures:set_key(key, if type(r) == "table" and type(r.src) == "string" and r.src ~= "" then r.src else false)
		end, function()
			asking[key] = nil
			pictures:set_key(key, false)
		end)
	end
	return nil
end

-- Its icon's SVG, the same way: by `id@version`, or false for none.
local icons = ui.state("store_icons", {} :: { [string]: string | false })
local asking_icon: { [string]: boolean } = {}

function M.icon_svg(card: Card): string?
	local path = card.icon
	if type(path) ~= "string" or path == "" then
		return nil
	end
	local key = `{card.id}@{card.version}`
	local have = icons:get()[key]
	if have ~= nil then
		return have or nil
	end
	if not asking_icon[key] then
		asking_icon[key] = true
		local id, version = card.id, card.version
		M.icon(id, version, path):after(function(r: any)
			asking_icon[key] = nil
			icons:set_key(key, if type(r) == "table" and type(r.svg) == "string" and r.svg ~= "" then r.svg else false)
		end, function()
			asking_icon[key] = nil
			icons:set_key(key, false)
		end)
	end
	return nil
end

return M
