--!strict
-- The plugin store, as Settings → Plugins asks it: Browse, a plugin's page,
-- and publishing from Yours. Every call goes through here, so a fixture
-- answers them as the host would (ui/store_stub.luau). The host reads the
-- catalog from Zeron's servers (src/plugins/catalog.rs: public, no sign-in
-- needed; your own plugins with your session, which never reaches a
-- script); the engine installs, publishes and makes deploy keys, on the
-- device each call names (`device`, nil: this one).
--
-- Each returns the host's promise, `:after(ok, err)`; `err.message` is an
-- outcome word ("offline", "signed_out", "not_found") or the engine's
-- "<reason>: <words>" (screens/plugins/words `failure`).
--
-- Reading the catalog:
--   store.search { q?, category?, keyword?, sort?, cursor?, limit?, featured? }
--       { items = { Card }, next_cursor? }   sort: "relevance" | "popular" |
--                                            "updated" | "new"
--   store.detail(id)          Detail (rejects "not_found")
--   store.versions(id)        { versions = { Version } }
--   store.categories()        { categories = { { id, name } } }
--   store.picture(id, version, path)  { src }: a picture of its package, kept
--                             here to show (only pictures, only its files)
--   store.icon(id, version, path)     { svg }: its icon, as an installed
--                             plugin's (an SVG of its package), or nil
-- Installing (state.install/update carry the rest):
--   store.unblock(id, device?)  a version taken down by Zeron, back on
-- Your own:
--   store.mine()              { plugins = { Mine } }  every version and its
--                             review
--   store.check(id)           { version, last?, checks = { Check } } before
--                             publishing (the engine reads its folder)
--   store.publish(id)         { id, version, status, checks }
--   store.withdraw(id, version), store.yank(id, version),
--   store.unyank(id, version)
--   store.keys(id)            { keys = { Key } }
--   store.create_key { id, name, expires? }   { key = Key, secret } once:
--                             asks for a fresh sign-in first
--   store.revoke_key(id, key)
--   store.report(id, reason, text)
--
-- Shapes (the plan's contract, docs/plans/2026-11-17-plugin-store.md):
--   Card    { id, name, summary, icon?, author = { username, verified,
--             badges = { "zeron"? } }, categories, keywords, version,
--             updated_at, installs, featured }
--   Detail  Card + { readme, screenshots = { path }, permissions = { string },
--             and what the consent sheet reads of them (ui, tools,
--             commands, network_hosts…), engines = { zeron? }, homepage?,
--             repository?, license?, versions = { Version }, package =
--             { bytes, sha256, hash } }
--   Version { version, status, published_at, notes?, engines?, yanked?,
--             prerelease?, reason?, message? }  status: "pending" |
--             "approved" | "rejected" | "withdrawn" | "yanked" | "removed"
--   Check   { id, ok, detail? }   id: "readme" | "icon" | "category" |
--             "description" | "semver" | "newer"
--   Key     { key, name, created_at, last_used_at?, expires_at? }

local M = {}

export type Author = { username: string?, verified: boolean?, badges: { string }? }
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
	reason: string?,
	message: string?,
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

function M.versions(id: string): any
	return ui.call("plugins.store.versions", id)
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

function M.unblock(id: string, device: string?): any
	return ui.call("plugins.store.unblock", { id = id, device = device })
end

function M.mine(): any
	return ui.call("plugins.store.mine", nil)
end

function M.check(id: string): any
	return ui.call("plugins.store.check", id)
end

function M.publish(id: string): any
	return ui.call("plugins.store.publish", id)
end

function M.withdraw(id: string, version: string): any
	return ui.call("plugins.store.withdraw", { id = id, version = version })
end

function M.yank(id: string, version: string): any
	return ui.call("plugins.store.yank", { id = id, version = version })
end

function M.unyank(id: string, version: string): any
	return ui.call("plugins.store.unyank", { id = id, version = version })
end

function M.keys(id: string): any
	return ui.call("plugins.store.keys", id)
end

function M.create_key(args: { id: string, name: string, expires: number? }): any
	return ui.call("plugins.store.create_key", args)
end

function M.revoke_key(id: string, key: string): any
	return ui.call("plugins.store.revoke_key", { id = id, key = key })
end

function M.report(id: string, reason: string, text: string): any
	return ui.call("plugins.store.report", { id = id, reason = reason, text = text })
end

-- A store id, as plugin ids are (the engine's manifest rule): what a link
-- may name, and nothing else.
function M.valid_id(id: any): boolean
	return type(id) == "string" and #id >= 1 and #id <= 64 and string.match(id, "^[a-z0-9][a-z0-9%-]*$") ~= nil
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
