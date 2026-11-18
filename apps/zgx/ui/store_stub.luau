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
--   store_stub.one_shot    Focus timer has one screenshot (store_stub.three_shots: three)
--   store_stub.id_reserved the plugin's id is one the store keeps (its site's pages)
--   store_stub.no_total    lists come without `total` (how many match)
--   store_stub.no_notes    the engine has no notes on your plugin's folder
--   store_stub.html_and_footnote  your plugin's README has HTML and math, its CHANGELOG a footnote (advice)
--   store_stub.hostile_readme  Focus timer's README: 512 KiB of setext headings (before its page opens)
--   store_stub.many_keys   the account has ten deploy keys: names of every length,
--                          one ending today, one tomorrow, one in five days
--   store_stub.long_featured   the first featured says 300 characters of itself, the second nothing

local store = require("@app/screens/plugins/store")

local calls = ui.state("store_stub_calls", {} :: { string })
local holding = false
local offline = false
local fail_next = false
-- Each later page starts five before where the last ended, as a store
-- whose order moved under the reader would.
local overlap = false
-- The engine's notes on your plugin's folder (its changelog has nothing for
-- the version); `store_stub.no_notes`, none.
local notes = true
-- Its README's HTML and math, and its CHANGELOG's footnote, said
-- (`store_stub.html_and_footnote`).
local typed = false
-- Its name calls it verified, its description says it comes from Zeron
-- (`store_stub.claims`): a problem and a warning, the engine's English.
local claims = false
-- Lists without `total`, as a store from before it counted them.
local no_total = false

-- A promise as the host's: settled a beat later, from one clock the script
-- keeps while answers are due (none runs with nothing to answer, so a
-- window idling with the stub in it runs no script); or, held, once
-- released. `await` works in a task (ui.paged's load).
local due: { () -> () } = {}
local held: { () -> () } = {}
local ticking = false
local function tick()
	local now = due
	due = {}
	for _, f in now do
		f()
	end
	if #due > 0 then
		ui.after(16, tick)
	else
		ticking = false
	end
end
local function soon(f: () -> ())
	table.insert(due, f)
	if not ticking then
		ticking = true
		ui.after(16, tick)
	end
end

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
		word = "store_unreachable: the store didn't answer"
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
		soon(settle)
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
			soon(tell)
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
		-- (Two in three verified, the featured among them; of the rest, every
		-- other has an older version that is.)
		verified = i % 3 ~= 2,
		verified_version = if i % 3 ~= 2 then `1.{i % 4}.0` elseif i % 2 == 0 then "1.0.0" else nil,
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
			{ version = card.version, status = "listed", verified = card.verified == true, verified_at = if card.verified then card.updated_at else nil, published_at = card.updated_at, notes = "- Faster.\n- Fewer words." },
			{ version = "1.0.0", status = "listed", verified = card.verified_version ~= nil, verified_at = if card.verified_version then (card.updated_at :: number) - 29 * DAY else nil, published_at = (card.updated_at :: number) - 30 * DAY, notes = "- The first one." },
		},
	}
	table.insert(entries, e)
	by_id[id] = e
end

-- The store's listing limits, each at its worst (plan amendment 22): a
-- name of 64 with no spaces, a description of 300, two categories, eight
-- keywords of 32, eight screenshots, a README of 512 KB, twelve versions
-- with notes of 64 KB each.
do
	local name = string.sub("Superlongpluginnamewithnospacesanywhereatalljusttokeepgoingonandon", 1, 64)
	local summary = string.sub(string.rep("Every word of this description is here to fill all three hundred characters the store allows. ", 4), 1, 300)
	local keywords = {}
	for i = 1, 8 do
		table.insert(keywords, string.sub(string.rep(string.char(96 + i), 32), 1, 32))
	end
	local shots = {}
	for i = 1, 8 do
		table.insert(shots, `shots/{i}.png`)
	end
	local para = "A paragraph of the longest README the store takes, its words wrapping across the page as any prose does. "
	local lines = { `# {name}` }
	local size = 0
	local n = 0
	while size < 512 * 1024 - 200 do
		n += 1
		local line = if n % 40 == 0 then `## Part {n // 40}` else para
		table.insert(lines, "")
		table.insert(lines, line)
		size += #line + 2
	end
	local notes = string.sub(string.rep("- A change in this version, said in one line of the changelog.\n", 1100), 1, 64 * 1024)
	local card: store.Card = {
		id = "extremes",
		name = name,
		summary = summary,
		icon = "icon.svg",
		author = { id = "u_extreme", username = "averylongusernamex", badges = { { id = "maintainer", name = "Maintainer" } } },
		categories = { "developer-tools", "integrations" },
		keywords = keywords,
		version = "10.20.30",
		updated_at = NOW,
		installs = 9_876_543,
		featured = false,
	}
	local e = {
		card = card,
		detail = {
			readme = table.concat(lines, "\n"),
			screenshots = shots,
			permissions = { asks = { "ui.add" } },
			compatible = true,
			license = "Apache-2.0 WITH LLVM-exception OR MIT OR BSD-3-Clause-Clear",
			homepage = "https://example.com/" .. string.rep("a", 200),
			repository = "https://github.com/averylongusernamex/" .. name,
			package = { bytes = 49_000_000, sha256 = string.rep("b", 64), hash = "extremes-hash" },
		},
		versions = {},
	}
	for i = 0, 11 do
		table.insert(e.versions, { version = `10.20.{30 - i}`, status = "listed", verified = i % 2 == 1, verified_at = if i % 2 == 1 then NOW - i * DAY + DAY // 2 else nil, published_at = NOW - i * DAY, notes = notes })
	end
	table.insert(entries, 1, e)
	by_id.extremes = e
end

-- Reacher, as the consent fixture has it (its hosts rated near, named
-- and any), its asks the store's consent item: the page and the sheet say
-- it in one wording.
do
	local e = by_id["api-probe"]
	e.card.name = "Reacher"
	e.detail.permissions = {
		asks = { "network" },
		network_hosts = { "localhost:8080", "box.example.ts.net", "*.example.org" },
		network_local = { "localhost:8080" },
		network_named = { "box.example.ts.net" },
	}
end

-- A README of forty pictures (a page asks for only so many).
do
	local lines = { "# Screenshot tray" }
	for i = 1, 40 do
		table.insert(lines, "")
		table.insert(lines, `![shot {i}](shots/s{i}.png)`)
	end
	by_id["screenshot-tray"].detail.readme = table.concat(lines, "\n")
end

-- Words that look like templates, as anyone may write them: drawn as they are.
do
	local b = by_id["haiku-commit"]
	b.card.name = "Haiku {commit}"
	b.card.keywords = { "{oops}", "daily" }
	b.card.author = { username = "mara", badges = { { id = "x", name = "{badge}" } } }
	by_id["rubber-duck"].card.keywords = { "{oops}", "review" }
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
		"See [Your chats](zeron-chat:abc), [your key](zeron-file:/Users/you/.ssh/id_ed25519), [Allow](zeron-invoke:allow) and [a picture](zeron-image:1).",
		"",
		"> See Your chats, your key, Allow and a picture.",
		"",
		"<script>zeron.decide(true)</script>",
		"",
		"## What it does",
		"",
		"- Counts down from 25 minutes.",
		"- Says when it's done.",
		"",
		"# Questions",
		"",
		"### Does it ring?",
		"",
		"Once, softly.",
	}, "\n")
	f.detail.screenshots = { "shots/header.png", "shots/done.png" }
	f.detail.permissions = { asks = { "ui.add", "notifications", "network" }, network_hosts = { "api.example.com" } }
	f.detail.engines = { zeron = ">=0.3.0" }
	f.versions = {
		{ version = "1.2.0", status = "listed", verified = true, verified_at = NOW - DAY, published_at = NOW - 2 * DAY, notes = "- Pauses when you leave the chat.\n- Says when it's done." },
		{ version = "1.1.0", status = "yanked", yanked = true, yanked_by = "superseded", superseded_by = "1.2.0", verified = false, published_at = NOW - 20 * DAY, notes = "- A bell." },
		{ version = "1.0.0", status = "listed", verified = true, verified_at = NOW - 59 * DAY, published_at = NOW - 60 * DAY, notes = "- The first one." },
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
		if ok and q.verified then
			ok = c.verified == true
		end
		if ok and q.author and q.author ~= "" then
			ok = (c.author :: any).username == q.author
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
	-- (`total`: how many the query matches, as the store counts them.)
	return { items = items, next_cursor = if from + limit < #out then tostring(from + limit) else nil, total = if no_total then nil else #out }
end

local function entry(id: string): Entry
	local e = by_id[id]
	if not e then
		error({ message = "not_in_store: no plugin by that id in the store" }, 0)
	end
	return e
end

local stub = store :: any
stub.search = function(q: store.Query): any
	note(`search:q={q.q or ""},category={q.category or ""},keyword={q.keyword or ""},sort={q.sort or ""},cursor={q.cursor or ""}{if q.featured then ",featured" else ""}{if q.author then `,author={q.author}` else ""}{if q.verified then ",verified" else ""}`)
	local asked = table.clone(q)
	return answer(function(): any
		return search(asked)
	end)
end
-- A README, and a latest version's notes, a test sets for Focus timer (a
-- hostile author's text: `store_stub_readme`, `store_stub_notes`).
local given_readme = ui.state("store_stub_readme", "")
local given_notes = ui.state("store_stub_notes", "")
-- (A detail's first page of versions, as the store gives it.)
local VERSIONS_PAGE = 5
-- The store's superseding rule, kept by every list here so each is one the
-- store could hold: publishing a version stops new installs of every older
-- listed one it hasn't verified ("superseded", by the one published next).
-- A release is superseded by the next release; a pre-release by the next
-- version of either kind. Newest first.
local function supersede(versions: { any })
	local function pre(v: any): boolean
		return string.find(v.version, "-", 1, true) ~= nil
	end
	for i, v in versions do
		if v.status == "listed" and v.verified ~= true then
			for j = i - 1, 1, -1 do
				local newer = versions[j]
				if pre(v) or not pre(newer) then
					v.status = "yanked"
					v.yanked = true
					v.yanked_by = "superseded"
					v.superseded_by = newer.version
					break
				end
			end
		end
	end
end
for _, e in entries do
	supersede(e.versions)
end

-- Which updates each plugin takes here ("newest", the default, or
-- "verified"), as the store page reads it (`installed.updates`).
local updates_pref: { [string]: string } = {}
stub.detail = function(id: string, for_zeron: string?): any
	note(if for_zeron then `detail:{id}:for_zeron={for_zeron}` else `detail:{id}`)
	return answer(function(): any
		local e = entry(id)
		local d: any = card_of(e)
		for k, v in e.detail do
			d[k] = v
		end
		-- (What this device has of it, as the engine adds: which updates it
		-- takes here.)
		d.installed = { updates = updates_pref[id] or "newest" }
		-- (Its first page of versions, newest first; the rest by cursor.)
		d.versions = table.move(e.versions, 1, math.min(VERSIONS_PAGE, #e.versions), 1, {})
		d.versions_next_cursor = if #e.versions > VERSIONS_PAGE then tostring(VERSIONS_PAGE) else nil
		if id == "focus-timer" and given_readme:peek() ~= "" then
			d.readme = given_readme:peek()
		end
		if id == "focus-timer" and given_notes:peek() ~= "" and d.versions[1] then
			local first = table.clone(d.versions[1])
			first.notes = given_notes:peek()
			d.versions = table.clone(d.versions)
			d.versions[1] = first
		end
		return d
	end)
end
stub.versions = function(id: string, cursor: string?, limit: number?): any
	note(`versions:{id}:{cursor or ""}`)
	return answer(function(): any
		local e = entry(id)
		local from = tonumber(cursor or "0") or 0
		local n = math.min(limit or 20, 50)
		local out = table.move(e.versions, from + 1, math.min(from + n, #e.versions), 1, {})
		return { versions = out, total = #e.versions, next_cursor = if from + n < #e.versions then tostring(from + n) else nil }
	end)
end
-- One version's record, as the store gives it; one it hasn't, `not_found`.
stub.version = function(id: string, version: string): any
	note(`version:{id}:{version}`)
	return answer(function(): any
		for _, v in entry(id).versions do
			if v.version == version then
				return v
			end
		end
		error({ message = "not_found" }, 0)
	end)
end
stub.categories = function(): any
	note("categories")
	return answer(function(): any
		-- (Each with how many listed plugins it holds, as the store counts.)
		local out = {}
		for _, c in CATEGORIES do
			local n = 0
			for _, e in entries do
				if table.find(e.card.categories or {}, c.id) then
					n += 1
				end
			end
			table.insert(out, { id = c.id, name = c.name, count = n })
		end
		return { categories = out }
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
-- (What the engine keeps of it, as the host shares it.)
local store_updates = ui.shared("plugins.store_updates", { device = "newest", plugins = {} } :: any)
if store_updates:peek() == nil then
	store_updates:set({ device = "newest", plugins = {} })
end
stub.set_updates = function(id: string?, prefer: string?): any
	note(`updates:{id or "device"}:{prefer or "default"}`)
	return answer(function(): any
		local now = table.clone(store_updates:peek() or { device = "newest", plugins = {} })
		now.plugins = table.clone(now.plugins or {})
		if id then
			updates_pref[id] = prefer
			now.plugins[id] = prefer
		else
			now.device = prefer or "newest"
		end
		store_updates:set(now)
		return { prefer = prefer or "newest" }
	end)
end
stub.prereleases = function(id: string, on: boolean): any
	note(`prereleases:{id}:{tostring(on)}`)
	return answer(function(): any
		return {}
	end)
end
-- Your own: Pomodoro clock (yours, on this device) in the store: 1.0.0
-- verified; 1.0.5, never verified, replaced by 1.1.0 (superseded: no new
-- installs); 1.1.0, listed, a reviewer declined to verify it, their words.
local mine: { [string]: any } = {
	["pomodoro-clock"] = {
		id = "pomodoro-clock",
		name = "Pomodoro clock",
		version = "1.0.0",
		listing = "listed",
		author = { username = "you", badges = {} },
		versions = {
			{ version = "1.1.0", status = "listed", verified = false, published_at = NOW - 3 * DAY, via = "device", review = { decided = true, decision = "declined", reason = "asks_too_much", message = "It reads every chat's messages but only needs the title. Ask for less.", decided_at = NOW - 2 * DAY } },
			{ version = "1.0.5", status = "yanked", yanked_by = "superseded", superseded = { at = NOW - 3 * DAY, by = "1.1.0" }, verified = false, published_at = NOW - 9 * DAY, via = "device", review = { decided = false } },
			{ version = "1.0.0", status = "listed", verified = true, verified_at = NOW - 19 * DAY, published_at = NOW - 20 * DAY, via = "device", review = { decided = true, decision = "verified", decided_at = NOW - 19 * DAY } },
		},
	},
}
-- The account's deploy keys, each with its scope (all its plugins, or
-- some) and the plugin it last published.
local keys: { any } = {
	{ key = "k_gh", name = "GitHub Actions", scope = { all = true }, created_at = NOW - 10 * DAY, last_used_at = NOW - 2 * DAY, last_used_plugin = "pomodoro-clock" },
}
-- What the engine finds checking your plugin's folder; `store_stub.fixed`
-- makes it all there.
local problems: { any } = {
	{ rule = "missing_listing", field = "icon", words = "The store needs an icon" },
	{ rule = "missing_readme", words = "README.md is missing" },
}
local prepared_version = "1.2.0"
-- Whether the engine could ask the store which versions it has.
local store_checked = true
-- (Tests: an answer the engine can't give, to see the page stays honest.)
local force_problem = ui.state("store_stub_force_problem", false)
local signed_in_again = false

-- (Tests: your own account's calls, yours, a version sent, a plugin's keys,
-- failing with this word as the engine gives it, alone: a session that
-- ended says `unauthenticated` or `forbidden`.)
local account_fails = ui.state("store_stub_account_fails", "")
local function refused_as_you()
	local word = account_fails:peek()
	if word ~= "" then
		error({ message = word }, 0)
	end
end

stub.mine = function(): any
	note("mine")
	return answer(function(): any
		refused_as_you()
		local out = {}
		for _, p in mine do
			table.insert(out, p)
		end
		return { plugins = out }
	end)
end
stub.prepare = function(id: string): any
	note(`prepare:{id}`)
	return answer(function(): any
		local ok = #problems == 0 and not claims
		return {
			id = id,
			name = "Pomodoro clock",
			version = prepared_version,
			hash = `{id}-{prepared_version}`,
			ok = ok,
			-- (Each answer alone: not having asked the store, it gives no
			-- newer-than answer.)
			problems = (function(): { any }
				local out = {}
				if claims then
					table.insert(out, { rule = "bad_listing", field = "name", words = "Its name says \"verified\": only the store marks a plugin verified" })
				end
				for _, p in problems do
					if store_checked or force_problem:peek() or (p.rule ~= "not_newer" and p.rule ~= "version_exists") then
						table.insert(out, p)
					end
				end
				return out
			end)(),
			warnings = (function(): { any }
				local out = {}
				if notes then
					table.insert(out, { rule = "no_notes", words = "CHANGELOG.md has no section for 1.2.0" })
				end
				if claims then
					table.insert(out, { rule = "claims_zeron", field = "description", words = "Its description says \"by Zeron\": only Zeron's own plugins may, and the store refuses it from anyone else" })
				end
				if typed then
					table.insert(out, { rule = "raw_html", path = "README.md", line = 3, words = "README.md line 3 has HTML; the store shows it as text. Use Markdown for pictures and headings." })
					table.insert(out, { rule = "footnote", path = "CHANGELOG.md", line = 7, words = "CHANGELOG.md line 7 has a footnote; the store shows it as text. Write the note into the text instead." })
					table.insert(out, { rule = "math", path = "README.md", line = 12, words = "README.md line 12 has math; the store's website shows it as typed (Zeron itself draws it)." })
				end
				return out
			end)(),
			package = { files = 4, bytes = 9120 },
			ticket = if ok then `ticket-{prepared_version}` else nil,
			store_checked = store_checked,
			note = if store_checked then nil else "Couldn't ask the store which versions it has: the store has the last word on whether this one is newer",
		}
	end)
end
stub.publish = function(id: string, hash: string, ticket: string): any
	note(`publish:{id}:{hash}:{ticket}`)
	return answer(function(): any
		refused_as_you()
		local p = mine[id]
		if not p then
			p = { id = id, name = "Pomodoro clock", version = prepared_version, versions = {} }
			mine[id] = p
		end
		-- Listed at once, not verified; every older listed one that isn't
		-- verified takes no new installs from now (superseded).
		local superseded = {}
		for _, v in p.versions do
			if v.status == "listed" and v.verified ~= true then
				v.status = "yanked"
				v.yanked_by = "superseded"
				v.superseded = { at = NOW, by = prepared_version }
				table.insert(superseded, v.version)
			end
		end
		table.insert(p.versions, 1, { version = prepared_version, status = "listed", verified = false, published_at = NOW, via = "device", review = { decided = false } })
		return { ok = true, id = id, version = prepared_version, status = "listed", verified = false, superseded = superseded, url = `https://zeron.example/plugins/{id}` }
	end)
end
stub.version_action = function(id: string, version: string, action: string): any
	note(`version:{id}:{version}:{action}`)
	return answer(function(): any
		for _, v in mine[id].versions do
			if v.version == version then
				-- (Its author's own yank and unyank: never a superseded one's.)
				if action == "yank" then
					v.status = "yanked"
					v.yanked_by = "author"
				elseif action == "unyank" and v.yanked_by == "author" then
					v.status = "listed"
					v.yanked_by = nil
				end
			end
		end
		return {}
	end)
end
stub.keys = function(): any
	note("keys")
	return answer(function(): any
		refused_as_you()
		return { keys = table.clone(keys) }
	end)
end
-- A new key needs a fresh sign-in: the first ask waits until
-- `store_stub.signed_in_again`, as the host's StepUp does.
stub.create_key = function(args: any): any
	local scope = if args.scope and args.scope.all then "all" else table.concat((args.scope and args.scope.plugins) or {}, ",")
	note(`create_key:{scope}:{args.name}:{if args.expires then "expires" else "never"}`)
	local p: any = { settled = false, waiters = {} }
	local function settle()
		p.settled = true
		local k = { key = `k_{#keys + 1}`, name = args.name, scope = args.scope, created_at = NOW, expires_at = args.expires }
		table.insert(keys, k)
		p.value = { ok = true, key = k.key, secret = "zpk_live_3f9a1c7e0b2d4f6a8c1e3a5b7d9f0e2c" }
		for _, w in p.waiters do
			w()
		end
	end
	local function check()
		if signed_in_again then
			settle()
			return
		end
		ui.after(50, check)
	end
	ui.after(50, check)
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
			tell()
		else
			table.insert(p.waiters, tell)
		end
	end
	return p
end
stub.cancel_step_up = function()
	note("cancel_step_up")
end
stub.revoke_key = function(key: string): any
	note(`revoke_key:{key}`)
	return answer(function(): any
		local out = {}
		for _, k in keys do
			if k.key ~= key then
				table.insert(out, k)
			end
		end
		keys = out
		return { revoked = true }
	end)
end
ui.command {
	id = "store_stub.never_published",
	title = "Store: none of Pomodoro clock's versions sent yet",
	run = function()
		mine["pomodoro-clock"] = nil
	end,
}
ui.command {
	id = "store_stub.store_unchecked",
	title = "Store: the engine couldn't ask the store which versions it has",
	run = function()
		store_checked = false
	end,
}
ui.command {
	id = "store_stub.not_newer",
	title = "Store: the version isn't newer than one sent",
	run = function()
		table.insert(problems, { rule = "not_newer", field = "version", words = "1.2.0 isn't newer than 1.3.0" })
	end,
}
-- (An id the store keeps for its site's own pages, as the engine refuses it.)
ui.command {
	id = "store_stub.id_reserved",
	title = "Store: your plugin's id is one the store keeps",
	run = function()
		table.insert(problems, { rule = "id_reserved", field = "id", words = "\"all\" is kept for the store's own pages: pick another id" })
	end,
}
ui.command {
	id = "store_stub.fixed",
	title = "Store: your plugin's folder has all the store needs",
	run = function()
		problems = {}
	end,
}
ui.command {
	id = "store_stub.signed_in_again",
	title = "Store: the fresh sign-in lands",
	run = function()
		signed_in_again = true
	end,
}

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
			soon(f)
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
-- (For a picture: a hostile author's README, setext headings at the
-- store's 512 KiB, Markdown to its budget and plain text past it.)
ui.command {
	id = "store_stub.hostile_readme",
	title = "Store: Focus timer's README is 512 KiB of setext headings",
	run = function()
		given_readme:set(string.sub(string.rep("Heading\n=======\n\n", 32768), 1, 512 * 1024))
	end,
}
ui.command {
	id = "store_stub.html_and_footnote",
	title = "Store: your plugin's README has HTML and math, its CHANGELOG a footnote",
	run = function()
		typed = true
	end,
}
ui.command {
	id = "store_stub.claims",
	title = "Store: your plugin's name says verified, its description says it's from Zeron",
	run = function()
		claims = true
	end,
}
ui.command {
	id = "store_stub.no_notes",
	title = "Store: nothing to note on your plugin's folder",
	run = function()
		notes = false
	end,
}
ui.command {
	id = "store_stub.many_keys",
	title = "Store: ten deploy keys",
	run = function()
		local names = {
			"GitHub Actions",
			"CI",
			"Release job on the build machine in the office",
			string.sub(string.rep("Averylongnamewithnospacesatall", 3), 1, 64),
			"Nightly",
			"GitLab",
			"Buildkite pipeline",
			"Ends today",
			"Ends tomorrow",
			"Ends in five days",
		}
		local list = {}
		for i, name in names do
			-- (A key as the store names it: its public half, base64url.)
			local key = string.sub(string.rep(string.format("k%02dAbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_", i), 2), 1, 43)
			-- (Ends from this very day, as the page reads it: noon of today,
			-- of tomorrow, of five days on.)
			local t = os.date("*t") :: any
			local noon = os.time({ year = t.year, month = t.month, day = t.day, hour = 12 }) * 1000
			local ends: number? = nil
			if name == "Ends today" then
				ends = noon
			elseif name == "Ends tomorrow" then
				ends = noon + DAY
			elseif name == "Ends in five days" then
				ends = noon + 5 * DAY
			elseif i % 3 == 0 then
				ends = noon + 90 * DAY
			end
			table.insert(list, {
				key = key,
				name = name,
				scope = if i % 2 == 0 then { all = true } else { plugins = { "pomodoro-clock" } },
				created_at = NOW - i * DAY,
				last_used_at = if i % 2 == 0 then NOW - DAY else nil,
				last_used_plugin = if i % 2 == 0 then "pomodoro-clock" else nil,
				expires_at = ends,
			})
		end
		keys = list
	end,
}
ui.command {
	id = "store_stub.long_featured",
	title = "Store: one featured says all it may, another nothing",
	run = function()
		local n = 0
		for _, e in entries do
			if e.card.featured then
				n += 1
				if n == 1 then
					e.card.summary = string.sub(string.rep("All it may say of itself, to the store's last character. ", 6), 1, 300)
				elseif n == 2 then
					e.card.summary = ""
				end
			end
		end
	end,
}
ui.command {
	id = "store_stub.no_total",
	title = "Store: lists don't say how many match",
	run = function()
		no_total = true
	end,
}
ui.command {
	id = "store_stub.one_shot",
	title = "Store: Focus timer has one screenshot",
	run = function()
		by_id["focus-timer"].detail.screenshots = { "shots/header.png" }
	end,
}
ui.command {
	id = "store_stub.three_shots",
	title = "Store: Focus timer has three screenshots",
	run = function()
		by_id["focus-timer"].detail.screenshots = { "shots/header.png", "shots/done.png", "shots/break.png" }
	end,
}
ui.command {
	id = "store_stub.bump",
	title = "Store: Focus timer 1.3.0 is out",
	run = function()
		local f = by_id["focus-timer"]
		f.card.version = "1.3.0"
		-- (Listed at once; not verified yet, 1.2.0 still the verified one.)
		f.card.verified = false
		f.card.verified_version = "1.2.0"
		table.insert(f.versions, 1, { version = "1.3.0", status = "listed", verified = false, published_at = NOW, notes = "- Longer breaks." })
		supersede(f.versions)
	end,
}

return {
	calls = calls,
	entries = entries,
	by_id = by_id,
	answer = answer,
	note = note,
}
