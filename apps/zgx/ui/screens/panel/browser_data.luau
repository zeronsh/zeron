-- The browser's settings and what it keeps, this Mac's, across launches:
-- what Settings → Browser sets (screens/settings/browser) and the panel's
-- browser tab reads (screens/panel/browser).
--
--   data.engine:get()            the search engine's id ("google", …)
--   data.engine_url()            its address, `%s` for the words (followed)
--   data.ENGINES                 { id, name, url }, in the order offered
--   data.default_zoom:get()      a new site's zoom (1 = 100%)
--   data.zoom_of(host)           a site's zoom (followed), else the default
--   data.set_zoom(host, z)       a site's zoom kept (the default forgets it)
--   data.step_zoom(z, by)        the next level up (1) or down (-1) from z
--   data.ZOOMS                   the levels zoom steps through
--   data.reset_zooms()           every site back to the default
--   data.inspectable:get()       pages opened may be inspected (Web Inspector)
--   data.keep_history:get()      pages visited are kept (History)
--   data.visit(profile, url, title?)  a visit, kept (the newest first, one per address)
--   data.retitle(profile, url, title) its title, once the page says it
--   data.set_icon(profile, url, path)  its icon, once the tab has fetched it
--   data.icon_of(profile, host)  an icon a visit to that site has, if any
--   data.site_zoom_list()        the sites off the default zoom: { host, zoom }, by host
--   data.site_name(host)         the name a site's initial is taken from ("apple")
--   data.history(profile)        that profile's visits, newest first (followed)
--   data.forget(profile, url)    one visit let go
--   data.clear_history(profile, since?)  visits since `since` (os.time), else all
--   data.suggest(profile, text, n)  up to n visits matching typed text, the best first
--
-- History is kept by the pages' cookie profile (one per account: the tab's
-- `profile_name`), so one account's visits never show for another.

local M = {}

-- Who's signed in (ui/account/auth), for the pages' cookie profile: one
-- per account, as the shipping app keeps a profile per workspace identity
-- (crates/ui browser/profile.rs), so sign-ins never bleed between accounts;
-- signed out, the device's own.
local auth: any = nil
do
	local ok, found = pcall(function(): any
		return (require :: any)("@account/auth")
	end)
	if ok then
		auth = found
	end
end

-- The profile now (followed): nil while the launch is still reading the
-- Keychain, as a page made then would take the device's profile for the
-- whole run and the account's sign-ins would seem lost.
function M.profile(): string?
	local ok, account = pcall(function(): any
		return if auth and auth.account then auth.account:get() else nil
	end)
	if ok and type(account) == "table" then
		if account.state == "restoring" then
			return nil
		end
		local user = account.user
		if type(user) == "table" and type(user.id) == "string" and user.id ~= "" then
			return `zeron-{user.id}`
		end
	end
	return "zeron-local"
end

-- `icon`: the page's icon, a file the host fetched (src/favicons.rs), once
-- the tab has found it.
export type Visit = { url: string, title: string?, at: number, count: number?, icon: string? }

M.ENGINES = {
	{ id = "google", name = "Google", url = "https://www.google.com/search?q=%s" },
	{ id = "duckduckgo", name = "DuckDuckGo", url = "https://duckduckgo.com/?q=%s" },
	{ id = "bing", name = "Bing", url = "https://www.bing.com/search?q=%s" },
	{ id = "brave", name = "Brave Search", url = "https://search.brave.com/search?q=%s" },
	{ id = "kagi", name = "Kagi", url = "https://kagi.com/search?q=%s" },
	{ id = "ecosia", name = "Ecosia", url = "https://www.ecosia.org/search?q=%s" },
}

M.engine = ui.persist("browser_search_engine", "google")
M.default_zoom = ui.persist("browser_default_zoom", 1)
M.inspectable = ui.persist("browser_inspectable", true)
M.keep_history = ui.persist("browser_keep_history", true)
-- Each site's zoom, by host (only those off the default).
local zooms = ui.persist("browser_site_zoom", {} :: { [string]: number })
M.site_zooms = zooms
-- Visits, by profile, newest first.
local visits = ui.persist("browser_history", {} :: { [string]: { Visit } })

-- Most visits kept a profile (the oldest go first).
M.MOST_VISITS = 500

-- A search of your own: its address, `%s` where the words go.
M.custom_engine = ui.persist("browser_custom_search", "")

-- Whether a search address can be used: http(s), a host, and `%s` past the
-- host (in its path or query, never naming the site itself).
function M.valid_template(t: string): boolean
	local scheme, rest = string.match(t, "^(https?)://(.+)$")
	if not scheme or not rest then
		return false
	end
	local host = string.match(rest, "^([^/?#]+)")
	if not host or string.find(host, "%", 1, true) or string.find(host, "@", 1, true) then
		return false
	end
	return string.find(rest, "%s", 1, true) ~= nil
end

function M.engine_url(): string
	local id = M.engine:get()
	if id == "custom" then
		local t = M.custom_engine:get()
		return if M.valid_template(t) then t else M.ENGINES[1].url
	end
	for _, e in M.ENGINES do
		if e.id == id then
			return e.url
		end
	end
	return M.ENGINES[1].url
end

-- (Read in the locale shown: "Custom" is a word of its own.)
function M.engine_options(): { { value: string, label: string } }
	local out = {}
	for _, e in M.ENGINES do
		table.insert(out, { value = e.id, label = e.name })
	end
	table.insert(out, { value = "custom", label = ui.t("settings.browser.search_engine.custom"):get() })
	return out
end

-- The levels a browser's ⌘+ and ⌘− step through.
M.ZOOMS = { 0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3 }

function M.step_zoom(z: number, by: number): number
	if by > 0 then
		for _, level in M.ZOOMS do
			if level > z + 0.001 then
				return level
			end
		end
		return M.ZOOMS[#M.ZOOMS]
	end
	for i = #M.ZOOMS, 1, -1 do
		if M.ZOOMS[i] < z - 0.001 then
			return M.ZOOMS[i]
		end
	end
	return M.ZOOMS[1]
end

-- A zoom as a percentage: "125%".
function M.percent(z: number): string
	return `{math.floor(z * 100 + 0.5)}%`
end

function M.zoom_options(): { { value: number, label: string } }
	local out = {}
	for _, z in M.ZOOMS do
		table.insert(out, { value = z, label = M.percent(z) })
	end
	return out
end

function M.zoom_of(host: string?): number
	local z = if host then zooms:get()[host] else nil
	return z or M.default_zoom:get()
end

function M.set_zoom(host: string?, z: number)
	if not host then
		return
	end
	local keep = if math.abs(z - M.default_zoom:peek()) < 0.001 then nil else z
	if zooms:peek()[host] ~= keep then
		zooms:set_key(host, keep)
	end
end

function M.reset_zooms()
	zooms:set({})
end

function M.zoomed_sites(): number
	local n = 0
	for _ in zooms:get() do
		n += 1
	end
	return n
end

local function kept(profile: string): { Visit }
	return visits:peek()[profile] or {}
end

local function keep(profile: string, list: { Visit }?)
	visits:set_key(profile, if list and #list > 0 then list else nil)
end

-- Not worth keeping: no page, or not a web page.
local function keepable(url: string?): boolean
	return type(url) == "string" and (string.sub(url, 1, 7) == "http://" or string.sub(url, 1, 8) == "https://")
end

function M.visit(profile: string, url: string, title: string?)
	if not M.keep_history:peek() or not keepable(url) then
		return
	end
	local now = os.time()
	local list = kept(profile)
	local head = list[1]
	-- (The same page again, a reload or its own redirect: one visit.)
	if head and head.url == url then
		local same = table.clone(head)
		same.at = now
		if title and title ~= "" then
			same.title = title
		end
		local next = table.clone(list)
		next[1] = same
		keep(profile, next)
		return
	end
	local next: { Visit } = { { url = url, title = if title and title ~= "" then title else nil, at = now, count = 1 } }
	for _, v in list do
		if v.url == url then
			next[1].count = (v.count or 1) + 1
			next[1].icon = v.icon
			if not next[1].title then
				next[1].title = v.title
			end
		elseif #next < M.MOST_VISITS then
			table.insert(next, v)
		end
	end
	keep(profile, next)
end

function M.retitle(profile: string, url: string, title: string)
	if title == "" then
		return
	end
	local list = kept(profile)
	for i, v in list do
		if v.url == url then
			if v.title ~= title then
				local next = table.clone(list)
				local one = table.clone(v)
				one.title = title
				next[i] = one
				keep(profile, next)
			end
			return
		end
	end
end

function M.set_icon(profile: string, url: string, icon: string?)
	local list = kept(profile)
	for i, v in list do
		if v.url == url then
			if v.icon ~= icon then
				local next = table.clone(list)
				local one = table.clone(v)
				one.icon = icon
				next[i] = one
				keep(profile, next)
			end
			return
		end
	end
end

local function host_of(url: string): string?
	return string.match(url, "^%a[%w+.-]*://([^/?#:]+)")
end

function M.icon_of(profile: string, host: string): string?
	for _, v in M.history(profile) do
		if v.icon and host_of(v.url) == host then
			return v.icon
		end
	end
	return nil
end

-- A site's name for its initial: the label before its suffix
-- ("developer.apple.com" → "apple", "localhost" → "localhost").
function M.site_name(host: string): string
	local labels = {}
	for part in string.gmatch(string.lower(host), "[^.]+") do
		table.insert(labels, part)
	end
	if #labels >= 2 and not string.match(host, "^%d+%.%d+%.%d+%.%d+$") then
		-- (A two-letter country code after a short second level: "co.uk".)
		local n = #labels
		if #labels[n] == 2 and #labels[n - 1] <= 3 and n >= 3 then
			return labels[n - 2]
		end
		return labels[n - 1]
	end
	return labels[1] or host
end

function M.site_zoom_list(): { { host: string, zoom: number } }
	local out = {}
	for host, z in zooms:get() do
		table.insert(out, { host = host, zoom = z })
	end
	table.sort(out, function(a, b)
		return a.host < b.host
	end)
	return out
end

function M.history(profile: string): { Visit }
	return visits:get()[profile] or {}
end

function M.forget(profile: string, url: string)
	local next: { Visit } = {}
	for _, v in kept(profile) do
		if v.url ~= url then
			table.insert(next, v)
		end
	end
	keep(profile, next)
end

function M.clear_history(profile: string, since: number?)
	if not since then
		keep(profile, nil)
		return
	end
	local next: { Visit } = {}
	for _, v in kept(profile) do
		if v.at < since then
			table.insert(next, v)
		end
	end
	keep(profile, next)
end

-- What an address is shown as in a list: no scheme, no lone trailing slash.
function M.short(url: string): string
	local s = string.gsub(url, "^https?://", "")
	s = string.gsub(s, "^www%.", "")
	if string.sub(s, -1) == "/" and not string.find(string.sub(s, 1, -2), "/", 1, true) then
		s = string.sub(s, 1, -2)
	end
	return s
end

-- Visits matching typed text: those whose address (past its scheme and
-- www.) starts with it first, then any whose address or title holds every
-- word; the more visited and the newer first within each.
function M.suggest(profile: string, text: string, n: number): { Visit }
	local q = string.lower(string.match(text, "^%s*(.-)%s*$") or "")
	if q == "" then
		return {}
	end
	local words = {}
	for w in string.gmatch(q, "%S+") do
		table.insert(words, w)
	end
	local starts: { Visit } = {}
	local holds: { Visit } = {}
	for _, v in M.history(profile) do
		local short = string.lower(M.short(v.url))
		if string.sub(short, 1, #q) == q then
			table.insert(starts, v)
		else
			local hay = `{short} {string.lower(v.title or "")}`
			local all = true
			for _, w in words do
				if not string.find(hay, w, 1, true) then
					all = false
					break
				end
			end
			if all then
				table.insert(holds, v)
			end
		end
	end
	local function rank(a: Visit, b: Visit): boolean
		local ca, cb = a.count or 1, b.count or 1
		if ca ~= cb then
			return ca > cb
		end
		return a.at > b.at
	end
	table.sort(starts, rank)
	table.sort(holds, rank)
	local out: { Visit } = {}
	for _, list in { starts, holds } do
		for _, v in list do
			if #out >= n then
				return out
			end
			table.insert(out, v)
		end
	end
	return out
end

return M
