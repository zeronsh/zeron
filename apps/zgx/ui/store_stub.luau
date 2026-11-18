-- The plugin store (screens/plugins/store), answered here as the engine's
-- `Store*` calls would (crates/rpc): no engine in a test host. A made-up catalog of 75
-- plugins, so Browse pages (30 at a time), searches, filters and sorts as
-- the store does; each plugin's page, its versions, its pictures; your own
-- plugins' versions and reviews; deploy keys.
--
-- What was asked is in `store_stub_calls` ("search:q=…", "detail:<id>", …).
-- `ui.command`s change what the next calls do:
--   store_stub.hold        answers wait until store_stub.release
--   store_stub.release     every answer waiting, given
--   store_stub.offline     every call fails as offline, until store_stub.online
--   store_stub.fail_next   the next search, page or plugin's page fails with the store's words
--   store_stub.bump        Focus timer gets 1.3.0 in the catalog (in place)
--   store_stub.overlap     each later page repeats the last five of the one before

local store = require("@app/screens/plugins/store")

local calls = ui.state("store_stub_calls", {} :: { string })
local holding = false
local offline = false
local fail_next = false
-- Each later page starts five before where the last ended, as a store
-- whose order moved under the reader would.
local overlap = false

-- A promise as the host's: settled a beat later, from one clock the script
-- keeps; or, held, once released. `await` works in a task (ui.paged's load).
local due: { () -> () } = {}
local held: { () -> () } = {}
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

-- (`quiet`: a call `fail_next` passes over, a picture or an icon, so it
-- lands on the next search or page.)
local function answer(work: () -> any, quiet: boolean?): any
	local p: any = { settled = false, waiters = {} }
	local function settle()
		local ok, value = pcall(work)
		p.settled = true
		if ok then
			p.value = value
		else
			p.err = if type(value) == "table" then value else { message = tostring(value) }
		end
		for _, w in p.waiters do
			w()
		end
		p.waiters = {}
	end
	local word: string? = nil
	if offline then
		word = "offline"
	elseif fail_next and not quiet then
		fail_next = false
		word = "rate_limited: the store is busy; try again in a minute"
	end
	if word then
		local w = word :: string
		work = function(): any
			error({ message = w }, 0)
		end
	end
	if holding then
		table.insert(held, settle)
	else
		table.insert(due, settle)
	end
	function p.after(_self: any, ok: ((any) -> ())?, bad: ((any) -> ())?)
		local function tell()
			if p.err then
				if bad then
					bad(p.err)
				end
			elseif ok then
				ok(p.value)
			end
		end
		if p.settled then
			table.insert(due, tell)
		else
			table.insert(p.waiters, tell)
		end
	end
	function p.await(_self: any): any
		while not p.settled do
			ui.sleep(16):await()
		end
		if p.err then
			error(p.err, 0)
		end
		return p.value
	end
	function p.cancel(_self: any) end
	return p
end

local function note(what: string)
	calls:push(what)
end

-- The catalog.
local WORDS = {
	"Focus timer", "Branch badge", "Linear issues", "Review notes", "Sentry traces", "Postgres peek", "Playwright check",
	"Figma frames", "Slack ping", "Datadog graphs", "Daily standup", "Commit coach", "Diff stats", "Todo sweep",
	"Release notes", "Theme shuffle", "Word count", "Pomodoro", "Clipboard shelf", "Snippet drawer", "Session tags",
	"Quiet hours", "Cost meter", "Model switcher", "Lint lens", "Test runner", "Coverage map", "Docker deck",
	"Kube glance", "Log tail", "Weather chip", "Calendar strip", "Mail digest", "Jira bridge", "Notion export",
	"Gist share", "Code tour", "Emoji react", "Mermaid live", "Regex lab", "JSON lens", "Color picker",
	"Screenshot tray", "Voice notes", "Translate", "Spell check", "Read aloud", "Focus music", "Break reminder",
	"Git blame", "PR checklist", "Issue triage", "Changelog bot", "Dependency watch", "License check",
	"Secret scan", "API probe", "GraphQL deck", "SQL notebook", "CSV view", "Chart maker", "Uptime chip",
	"Deploy button", "Feature flags", "A/B notes", "Bug bash", "Pair timer", "Rubber duck", "Haiku commit",
	"Retro board", "Meeting notes", "Agent stats", "Token tally", "Prompt library",
}
-- (The store's fixed list, the plan's amendment 4.)
local CATEGORIES = {
	{ id = "agents", name = "Agents" },
	{ id = "appearance", name = "Appearance" },
	{ id = "chat", name = "Chat" },
	{ id = "developer-tools", name = "Developer tools" },
	{ id = "git", name = "Git" },
	{ id = "integrations", name = "Integrations" },
	{ id = "productivity", name = "Productivity" },
	{ id = "utilities", name = "Utilities" },
}
local FOCUS_SVG = [[<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="13" r="8" fill="none" stroke="currentColor" stroke-width="2"/><path d="M12 9v4l2 2M9 2h6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>]]

local function slug(name: string): string
	return (string.gsub(string.gsub(string.lower(name), "[^%w]+", "-"), "^%-+", ""))
end

type Entry = {
	card: store.Card,
	detail: { [string]: any },
	versions: { store.Version },
}
local entries: { Entry } = {}
local by_id: { [string]: Entry } = {}
local DAY = 86400 * 1000
local NOW = 1_794_873_600_000 -- the 17th of November, 2026

for i, name in WORDS do
	local id = slug(name)
	local category = CATEGORIES[(i - 1) % #CATEGORIES + 1].id
	local card: store.Card = {
		id = id,
		name = name,
		summary = `{name} for your chats: a short line saying what it does.`,
		icon = if i % 3 == 0 then nil else "icon.svg",
		author = {
			username = ({ "ada", "grace", "linus", "mara" })[i % 4 + 1],
			badges = if i % 5 == 0 then { { id = "maintainer", name = "Maintainer" } } else {},
		},
		categories = { category },
		keywords = { category, if i % 2 == 0 then "review" else "daily" },
		version = `1.{i % 4}.0`,
		updated_at = NOW - i * DAY,
		installs = (80 - i) * 137 + (i % 7) * 3,
		featured = i == 1 or i == 4 or i == 9,
	}
	local e: Entry = {
		card = card,
		detail = {
			readme = `# {name}\n\n{card.summary}\n\nIt keeps out of your way.`,
			screenshots = {},
			permissions = { asks = { "ui.add" } },
			engines = { zeron = ">=0.3.0" },
			compatible = true,
			license = "MIT",
			homepage = `https://example.com/{id}`,
			repository = `https://github.com/{(card.author :: any).username}/{id}`,
			package = { bytes = 24_000 + i * 310, sha256 = string.rep("a", 64), hash = `{id}-hash` },
		},
		versions = {
			{ version = card.version, status = "approved", published_at = card.updated_at, notes = "- Faster.\n- Fewer words." },
			{ version = "1.0.0", status = "approved", published_at = (card.updated_at :: number) - 30 * DAY, notes = "- The first one." },
		},
	}
	table.insert(entries, e)
	by_id[id] = e
end

-- Focus timer: a page with everything on it.
do
	local f = by_id["focus-timer"]
	f.card.icon = "icon.svg"
	f.card.author = { username = "ada", badges = { { id = "maintainer", name = "Maintainer" } } }
	f.detail.repository = "https://github.com/ada/focus-timer"
	f.card.version = "1.2.0"
	f.card.keywords = { "focus", "timer", "pomodoro" }
	f.detail.readme = table.concat({
		"# Focus timer",
		"",
		"A **25 minute** timer in a chat's header. Start it, and it counts down.",
		"",
		"![The timer in a chat's header](shots/header.png)",
		"",
		"![A picture from the web](https://example.com/tracker.png)",
		"",
		"Read [the guide](https://example.com/guide) for more.",
		"",
		"<script>zeron.decide(true)</script>",
		"",
		"## What it does",
		"",
		"- Counts down from 25 minutes.",
		"- Says when it's done.",
	}, "\n")
	f.detail.screenshots = { "shots/header.png", "shots/done.png" }
	f.detail.permissions = { asks = { "ui.add", "notifications", "network" }, network_hosts = { "api.example.com" } }
	f.detail.engines = { zeron = ">=0.3.0" }
	f.versions = {
		{ version = "1.2.0", status = "approved", published_at = NOW - 2 * DAY, notes = "- Pauses when you leave the chat.\n- Says when it's done." },
		{ version = "1.1.0", status = "approved", published_at = NOW - 20 * DAY, notes = "- A bell." },
		{ version = "1.0.0", status = "approved", published_at = NOW - 60 * DAY, notes = "- The first one." },
	}
end
-- One that needs a later Zeron: the engine says so, in its words.
by_id["cost-meter"].detail.engines = { zeron = ">=9.0.0" }
by_id["cost-meter"].detail.compatible = false
by_id["cost-meter"].detail.incompatible = "Needs Zeron 9.0.0 or later; this device has 0.4.2."


local function card_of(e: Entry): store.Card
	local c = table.clone(e.card)
	c.author = table.clone(e.card.author :: any)
	return c
end

-- The search, as the store answers it: matched, filtered, sorted, paged.
local function search(q: store.Query): any
	local needle = string.lower(q.q or "")
	local out: { Entry } = {}
	for _, e in entries do
		local c = e.card
		local hay = string.lower(`{c.name} {c.summary or ""} {table.concat(c.keywords or {}, " ")} {(c.author :: any).username}`)
		local ok = needle == "" or string.find(hay, needle, 1, true) ~= nil
		if ok and q.category and q.category ~= "" then
			ok = table.find(c.categories or {}, q.category) ~= nil
		end
		if ok and q.keyword and q.keyword ~= "" then
			ok = table.find(c.keywords or {}, q.keyword) ~= nil
		end
		if ok and q.featured then
			ok = c.featured == true
		end
		if ok then
			table.insert(out, e)
		end
	end
	local sort = q.sort or (if needle ~= "" then "relevance" else "popular")
	table.sort(out, function(a: Entry, b: Entry): boolean
		if sort == "updated" then
			return (a.card.updated_at :: number) > (b.card.updated_at :: number)
		elseif sort == "new" then
			return (a.versions[#a.versions].published_at :: number) > (b.versions[#b.versions].published_at :: number)
		elseif sort == "relevance" then
			local an = string.find(string.lower(a.card.name), needle, 1, true) ~= nil
			local bn = string.find(string.lower(b.card.name), needle, 1, true) ~= nil
			if an ~= bn then
				return an
			end
		end
		return (a.card.installs :: number) > (b.card.installs :: number)
	end)
	local from = tonumber(q.cursor or "0") or 0
	if overlap and from > 0 then
		from = math.max(0, from - 5)
	end
	local limit = math.min(q.limit or 30, 50)
	local items = {}
	for i = from + 1, math.min(from + limit, #out) do
		table.insert(items, card_of(out[i]))
	end
	return { items = items, next_cursor = if from + limit < #out then tostring(from + limit) else nil }
end

local function entry(id: string): Entry
	local e = by_id[id]
	if not e then
		error({ message = "not_found" }, 0)
	end
	return e
end

local stub = store :: any
stub.search = function(q: store.Query): any
	note(`search:q={q.q or ""},category={q.category or ""},keyword={q.keyword or ""},sort={q.sort or ""},cursor={q.cursor or ""}{if q.featured then ",featured" else ""}`)
	local asked = table.clone(q)
	return answer(function(): any
		return search(asked)
	end)
end
stub.detail = function(id: string): any
	note(`detail:{id}`)
	return answer(function(): any
		local e = entry(id)
		local d: any = card_of(e)
		for k, v in e.detail do
			d[k] = v
		end
		d.versions = e.versions
		return d
	end)
end
stub.categories = function(): any
	note("categories")
	return answer(function(): any
		return { categories = CATEGORIES }
	end, true)
end
-- Pictures: the package's own, as files kept here (made-up: the app's own
-- art stands in); anything else can't come.
stub.picture = function(id: string, version: string, path: string): any
	note(`picture:{id}@{version}/{path}`)
	return answer(function(): any
		local e = entry(id)
		if string.find(path, "^https?:") or not table.find(e.detail.screenshots or {}, path) then
			error({ message = "not_a_picture" }, 0)
		end
		return { src = if path == "shots/done.png" then "assets/art/plate-3-sunrise.jpg" else "assets/art/plate-2-bluehour.jpg" }
	end, true)
end
stub.icon = function(id: string, _version: string, _path: string): any
	return answer(function(): any
		entry(id)
		return { svg = FOCUS_SVG }
	end, true)
end

stub.report = function(id: string, reason: string, text: string): any
	note(`report:{id}:{reason}:{text}`)
	return answer(function(): any
		entry(id)
		return {}
	end)
end
stub.prereleases = function(id: string, on: boolean): any
	note(`prereleases:{id}:{tostring(on)}`)
	return answer(function(): any
		return {}
	end)
end
-- (The engine's `compatible` judged again, as for an older Zeron.)
ui.command {
	id = "store_stub.old_zeron",
	title = "Store: this Zeron is too old for Focus timer",
	run = function()
		local f = by_id["focus-timer"]
		f.detail.compatible = false
		f.detail.incompatible = "Needs Zeron 0.3.0 or later; this device has 0.2.0."
	end,
}

ui.command {
	id = "store_stub.hold",
	title = "Store: answers wait",
	run = function()
		holding = true
	end,
}
ui.command {
	id = "store_stub.release",
	title = "Store: answers given",
	run = function()
		holding = false
		local now = held
		held = {}
		for _, f in now do
			table.insert(due, f)
		end
	end,
}
ui.command {
	id = "store_stub.offline",
	title = "Store: offline",
	run = function()
		offline = true
	end,
}
ui.command {
	id = "store_stub.online",
	title = "Store: back online",
	run = function()
		offline = false
	end,
}
ui.command {
	id = "store_stub.fail_next",
	title = "Store: the next call fails",
	run = function()
		fail_next = true
	end,
}
ui.command {
	id = "store_stub.overlap",
	title = "Store: later pages overlap",
	run = function()
		overlap = true
	end,
}
ui.command {
	id = "store_stub.bump",
	title = "Store: Focus timer 1.3.0 is out",
	run = function()
		local f = by_id["focus-timer"]
		f.card.version = "1.3.0"
		table.insert(f.versions, 1, { version = "1.3.0", status = "approved", published_at = NOW, notes = "- Longer breaks." })
	end,
}

return {
	calls = calls,
	entries = entries,
	by_id = by_id,
	answer = answer,
	note = note,
}
