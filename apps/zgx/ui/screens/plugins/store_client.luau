--!strict
-- The store, as Settings asks it of one kind of thing (its plugins, its
-- templates): Browse, a thing's page, and publishing from Yours.
--
--   make(calls, keys)   `calls` what the host names its calls by
--                       ("plugins.store"), `keys` what this kind's state is
--                       named by, so neither kind's is the other's
--
-- The plugins' is screens/plugins/store. Every call goes through here, so a
-- fixture answers as the host would (ui/store_stub.luau). The engine is the
-- store's one client (its `Store*` calls, crates/rpc); the host only carries
-- them (src/plugins/catalog.rs). Installing from the store is
-- `state.install { store = id, version?, device? }`; a version the store
-- took down is turned back on with `state.set_off(id, false, device)`.
--
-- Each returns the host's promise, `:after(ok, err)`; `err.message` is an
-- outcome word ("offline", "signed_out") or the engine's "<reason>: <words>"
-- (its reason words: docs/plugins.md "Reason words")
-- (screens/plugins/words `failure`).
--
-- Reading the catalog:
--   store.search { q?, category?, keyword?, sort?, cursor?, limit?, featured? }
--       { items = { Card }, next_cursor? }   sort: "relevance" | "popular" |
--                                            "updated" | "new"
--   store.detail(id, for_zeron?)  Detail, its first page of versions
--                             (`versions_next_cursor` for more), whether it's
--                             verified and its newest verified version; whether
--                             it works with Zeron `for_zeron` (the device
--                             shown's), else this one (rejects "not_in_store")
--   store.versions(id, cursor?, limit?)  { versions, total, next_cursor? }
--   store.version(id, version)  Version
--   store.categories()        { categories = { { id, name, count? } } }
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
--   store.tokens()            { tokens = { Token } }, the account's deploy
--                             tokens (never a token itself)
--   store.create_token { name, scope, expires? }   { token = Token, secret },
--                             `secret` the token this once: a fresh sign-in first
--                             (`scope`: { all = true } or { plugins = { ids } })
--   store.cancel_step_up()    stops waiting for that sign-in
--   store.revoke_token(id)
--
-- Shapes (the plan's contract and its amendments,
-- docs/plans/2026-11-17-plugin-store.md):
--   Card    { id, name, summary, icon?, author = { username, badges = {
--             { id, name } } }, categories, keywords, version, updated_at,
--             installs, featured }
--   Detail  Card + { readme, screenshots = { path }, permissions (the consent
--             sheet's item: { asks, ui, commands, network_hosts, tools… }),
--             engines, homepage?, repository?, license?, versions = { Version },
--             package, compatible, incompatible? (why not, in words),
--             installed? }
--   Version { version, status, published_at, notes?, engines?, yanked?,
--             review? = { state, reason?, message?, decided_at? } }
--   Token   { id, short, name, scope, created_at, last_used_at?,
--             last_used_plugin?, expires_at?, revoked_at? }

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
	-- Whether the store has verified `version` (its newest listed one), and
	-- the newest version it has verified (nil: none yet).
	verified: boolean?,
	verified_version: string?,
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
	-- Whether the store has verified it, and when.
	verified: boolean?,
	verified_at: number?,
}
export type Query = {
	q: string?,
	category: string?,
	keyword: string?,
	-- One publisher's plugins (a username), and whether only plugins whose
	-- newest version is verified.
	author: string?,
	verified: boolean?,
	sort: string?,
	cursor: string?,
	limit: number?,
	featured: boolean?,
}

export type Scope = { all: boolean?, plugins: { string }? }

local C = {}

function C.make(calls: string, keys: string)
	local M = {}

	-- How many a page asks for (the store gives at most 50).
	M.PAGE = 30

	function M.search(query: Query): any
		return ui.call(calls .. ".search", query)
	end

	function M.detail(id: string, for_zeron: string?): any
		return ui.call(calls .. ".detail", if for_zeron then { id = id, for_zeron = for_zeron } else id)
	end

	-- A page of a plugin's versions, newest first (`cursor` from the detail's
	-- `versions_next_cursor`, then each page's `next_cursor`), and one version.
	function M.versions(id: string, cursor: string?, limit: number?): any
		return ui.call(calls .. ".versions", { id = id, cursor = cursor, limit = limit })
	end
	function M.version(id: string, version: string): any
		return ui.call(calls .. ".version", { id = id, version = version })
	end

	function M.categories(): any
		return ui.call(calls .. ".categories", nil)
	end

	function M.picture(id: string, version: string, path: string): any
		return ui.call(calls .. ".picture", { id = id, version = version, path = path })
	end

	function M.icon(id: string, version: string, path: string): any
		return ui.call(calls .. ".icon", { id = id, version = version, path = path })
	end

	function M.prereleases(id: string, on: boolean, device: string?): any
		return ui.call(calls .. ".prereleases", { id = id, on = on, device = device })
	end

	-- Which updates a plugin takes on the device shown ("newest", the default,
	-- or "verified"); with no id, the device's own choice (nil: the default).
	function M.set_updates(id: string?, prefer: ("newest" | "verified")?, device: string?): any
		return ui.call(calls .. ".updates", { id = id, prefer = prefer, device = device })
	end

	function M.report(id: string, reason: string, text: string): any
		return ui.call(calls .. ".report", { id = id, reason = reason, text = text })
	end

	function M.mine(): any
		return ui.call(calls .. ".mine", nil)
	end

	function M.version_action(id: string, version: string, action: string): any
		return ui.call(calls .. ".version_action", { id = id, version = version, action = action })
	end

	function M.prepare(id: string): any
		return ui.call(calls .. ".prepare", id)
	end

	function M.publish(id: string, hash: string, ticket: string): any
		return ui.call(calls .. ".publish", { id = id, hash = hash, ticket = ticket })
	end

	-- The account's deploy tokens ({ tokens }: each { id, short, name, scope,
	-- created_at, last_used_at, last_used_plugin, expires_at }; never the token).
	function M.tokens(): any
		return ui.call(calls .. ".tokens", nil)
	end

	-- A new one, after a fresh sign-in ({ ok, token = { id, short, name,
	-- scope, expires_at, created_at }, secret }: `secret` the token, `zdt_…`,
	-- this once).
	function M.create_token(args: { name: string, scope: Scope, expires: number? }): any
		return ui.call(calls .. ".create_token", args)
	end

	-- Stops waiting for that sign-in, and the engine lets go of it.
	function M.cancel_step_up()
		ui.call(calls .. ".cancel_step_up", nil):after(function() end, function() end)
	end

	-- A secret on the clipboard (a deploy token): kept by no clipboard history
	-- or cloud, and cleared in a minute if it's still there (src/secrets.rs).
	function M.copy_secret(text: string): any
		return ui.call("secret.copy", { text = text })
	end

	function M.revoke_token(id: string): any
		return ui.call(calls .. ".revoke_token", { id = id })
	end

	-- What a report may say is wrong (the store's list).
	M.REPORT_REASONS = { "broken", "misleading", "malware", "copyright", "other" }

	-- A store id, as plugin ids are (the engine's manifest rule,
	-- `^[a-z0-9][a-z0-9-]{1,39}$`): what a link may name, and nothing else.
	-- Whether a failure says you aren't signed in to the store now: no session
	-- on this device (`signed_out`), or one the store no longer takes (a 401
	-- or 403 with nothing more, `unauthenticated` / `forbidden`): most often a
	-- session that ended while the page was open.
	local SIGNED_OUT = { signed_out = true, unauthenticated = true, forbidden = true }
	function M.signed_out(message: any): boolean
		local m = if type(message) == "table" then message.message else message
		if type(m) ~= "string" then
			return false
		end
		local word = string.match(m, "^([%l_]+): ") or string.match(m, "^([%l_]+)$")
		return word ~= nil and SIGNED_OUT[word] == true
	end

	function M.valid_id(id: any): boolean
		return type(id) == "string" and #id >= 2 and #id <= 40 and string.match(id, "^[a-z0-9][a-z0-9%-]*$") ~= nil
	end

	-- Its pictures, as the engine keeps them here: by `id@version/path`, the
	-- file to show, or false for one that isn't (not a picture of its package).
	-- Read reactively, asked once; one that couldn't come for now (offline)
	-- is asked again the next time it's drawn. At most `PICTURES_MOST` of one
	-- version's are ever asked for (a README of thousands of pictures shows
	-- the rest as their links).
	local pictures = ui.state(keys .. "_pictures", {} :: { [string]: string | false })
	local asking: { [string]: boolean } = {}
	local asked_of: { [string]: number } = {}
	M.PICTURES_MOST = 24

	-- Whether a failure says what the picture is (it never will be one), not
	-- just that it can't come now.
	local function lasting(err: any): boolean
		local m = if type(err) == "table" then err.message else err
		return m ~= "offline" and not M.signed_out(m) and not (type(m) == "string" and string.match(m, "^rate_limited"))
	end

	function M.picture_src(id: string, version: string, path: string): (string | false)?
		local key = `{id}@{version}/{path}`
		local have = pictures:get()[key]
		if have ~= nil then
			return have
		end
		if asking[key] then
			return nil
		end
		local of = `{id}@{version}`
		if (asked_of[of] or 0) >= M.PICTURES_MOST then
			return false
		end
		asked_of[of] = (asked_of[of] or 0) + 1
		asking[key] = true
		M.picture(id, version, path):after(function(r: any)
			asking[key] = nil
			pictures:set_key(key, if type(r) == "table" and type(r.src) == "string" and r.src ~= "" then r.src else false)
		end, function(err: any)
			asking[key] = nil
			if lasting(err) then
				pictures:set_key(key, false)
			else
				asked_of[of] = math.max(0, (asked_of[of] or 1) - 1)
			end
		end)
		return nil
	end

	-- Its icon's SVG, the same way: by `id@version`, or false for none.
	local icons = ui.state(keys .. "_icons", {} :: { [string]: string | false })
	local asking_icon: { [string]: boolean } = {}

	function M.icon_svg(card: Card): string?
		local path = card.icon
		-- (An SVG of its package: a path in it, as the host checks again.)
		if type(path) ~= "string" or path == "" or #path > 512 or not string.match(path, "%.svg$") or string.find(path, "%.%.") or string.find(path, "^/") or string.find(path, "[:%%\\]") then
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
end

return C
