-- The store's templates (screens/templates/store), answered here as the
-- engine's `Store*` calls would with `kind = "template"`: a made-up catalog of
-- 34, so Browse pages. What was asked is in `template_stub_calls`.
--   template_stub.hold / release   answers wait until released
--   template_stub.offline          every call fails as offline
--   template_stub.empty            the store has none
--   template_stub.published        Hot Pink is yours there, verified, then not
--   template_stub.problems         the next checks find what a template lacks
--   template_stub.few              the store has eight, a page

local store: any = require("@app/screens/templates/store")

local calls = ui.state("template_stub_calls", {} :: { string })
local holding, offline, empty, few = false, false, false, false
local held: { () -> () } = {}

-- Answers a beat later, from one clock of this script's (store_stub's): a
-- timer made where it's asked goes with what asked, drawn again meanwhile.
local due: { () -> () } = {}
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

local function answer(what: string, work: () -> any): any
	calls:push(what)
	local p: any = { settled = false, waiters = {} }
	local function settle()
		local ok, value = pcall(work)
		if offline then
			ok, value = false, { message = "store_unreachable: the store didn't answer" }
		end
		p.settled = true
		if ok then
			p.value = value
		else
			p.err = if type(value) == "table" then value else { message = tostring(value) }
		end
		for _, w in p.waiters do
			w()
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
			tell()
		else
			table.insert(p.waiters, tell)
		end
		return p
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

local catalog: { any } = {}
local function add(id: string, name: string, summary: string, author: string, installs: number, verified: boolean)
	table.insert(catalog, {
		kind = "template",
		id = id,
		name = name,
		summary = summary,
		author = { username = author },
		categories = {},
		keywords = { "dark" },
		cover = "preview.png",
		version = "1.2.0",
		installs = installs,
		verified = verified,
		featured = #catalog < 3,
	})
end
add("midnight", "Midnight", "Deep blues and one warm light.", "ada", 4210, true)
add("ledger", "Ledger", "Dense rows, ledger lines, figures that line up.", "grace", 3180, false)
add("airy", "Airy", "More room between things, and " .. string.rep("a long way of saying it ", 12), "lin", 960, true)
add("hostile", "{count} [b](https://x) " .. string.rep("W", 60), "", string.rep("n", 39), 3, false)
add("hot-pink", "Hot Pink", "Someone else's.", "lin", 2, false)
for i = 1, 29 do
	add(`made-up-{i}`, `Made up {i}`, "One of many.", "ada", 500 - i, i % 2 == 0)
end

store.search = function(q: any): any
	return answer(`search:q={q.q or ""},sort={q.sort or ""},cursor={q.cursor or ""}{if q.featured then ",featured" else ""}{if q.verified then ",verified" else ""}`, function(): any
		local found: { any } = {}
		for _, c in (if empty then {} elseif few then table.move(catalog, 1, 8, 1, {}) else catalog) :: { any } do
			if (not q.featured or c.featured) and (not q.verified or c.verified) and (q.q == nil or q.q == "" or string.find(string.lower(c.name), string.lower(q.q), 1, true)) then
				table.insert(found, c)
			end
		end
		local from = tonumber(q.cursor) or 0
		local page: { any } = {}
		for i = from + 1, math.min(#found, from + (q.limit or 30)) do
			table.insert(page, found[i])
		end
		return { items = page, total = #found, next_cursor = if from + #page < #found then tostring(from + #page) else nil }
	end)
end
store.picture = function(id: string, version: string, path: string): any
	return answer(`picture:{id}@{version}/{path}`, function(): any
		if id == "ledger" then
			error({ message = "not_a_picture" }, 0)
		end
		return { src = if #id % 2 == 0 then "assets/art/plate-3-sunrise.jpg" else "assets/art/plate-2-bluehour.jpg" }
	end)
end

local function entry(id: string): any
	for _, c in catalog do
		if c.id == id then
			return c
		end
	end
	error({ message = "not_in_store" }, 0)
end
local NOW = 1795132800000
local function versions_of(id: string): { any }
	return {
		{ version = "1.2.0", status = "listed", published_at = NOW, notes = "Warmer light.", verified = entry(id).verified },
		{ version = "1.1.0", status = "listed", published_at = NOW - 1e9, notes = "First.", verified = true, verified_at = NOW - 1e8 },
	}
end
store.detail = function(id: string, _for: string?): any
	return answer(`detail:{id}`, function(): any
		local d = table.clone(entry(id))
		d.readme = `# {d.name}\n\nA whole interface, in one colour.`
		d.screenshots = { "screenshots/one.png", "screenshots/two.png", "screenshots/three.png", "screenshots/four.png" }
		d.versions = versions_of(id)
		d.verified_version = if d.verified then "1.2.0" else "1.1.0"
		d.compatible = id ~= "airy"
		d.incompatible = if id == "airy" then "Made from Zeron 9.9.9" else nil
		d.license = "MIT"
		return d
	end)
end
store.versions = function(id: string, _cursor: string?): any
	return answer(`versions:{id}`, function(): any
		return { versions = versions_of(id), total = 2 }
	end)
end
store.version = function(id: string, version: string): any
	return answer(`version:{id}:{version}`, function(): any
		for _, v in versions_of(id) do
			if v.version == version then
				return v
			end
		end
		error({ message = "not_found" }, 0)
	end)
end
store.report = function(id: string, reason: string, _text: string): any
	return answer(`report:{id}:{reason}`, function(): any
		return {}
	end)
end
store.prereleases = function(id: string, on: boolean): any
	return answer(`prereleases:{id}:{on}`, function(): any
		return {}
	end)
end
store.set_updates = function(id: string?, prefer: string?): any
	return answer(`updates:{id or ""}:{prefer or ""}`, function(): any
		return {}
	end)
end

-- Yours there, and publishing one (`PrepareTemplatePublish`, `PublishTemplate`).
local mine: { any } = {}
local problems = false
store.mine = function(): any
	return answer("mine", function(): any
		return { publisher_id = "p_1", templates = table.clone(mine) }
	end)
end
store.prepare = function(id: string): any
	return answer(`prepare:{id}`, function(): any
		return {
			id = id,
			name = "Hot Pink",
			version = "1.1.0",
			hash = `{id}-1.1.0`,
			ok = not problems,
			problems = if problems
				then {
					{ rule = "missing_listing", field = "preview", words = "no preview.png" },
					{ rule = "bad_listing", field = "categories", words = "categories" },
					{ rule = "protected_file", path = "app.luau", words = "protected" },
					{ rule = "missing_readme", words = "no README.md" },
					{ rule = "protected_file", path = "screens/APP~1.LUA", words = "APP~1.LUA is a short name" },
					{ rule = "bad_package", field = "id", words = "Its id isn't one the store takes" },
					{ rule = "bad_package", path = "screens/nul", words = "a device name" },
					{ rule = "bad_listing", field = "files", path = "screenshots/a.txt", words = "A file there isn't a picture" },
				}
				else {},
			warnings = {},
			package = { files = 6, bytes = 12000 },
			ticket = if problems then nil else "ticket-1.1.0",
			store_checked = true,
		}
	end)
end
store.publish = function(id: string, hash: string, ticket: string): any
	return answer(`publish:{id}:{hash}:{ticket}`, function(): any
		mine = { { id = id, name = "Hot Pink", version = "1.1.0", versions = { { version = "1.1.0", status = "listed", verified = false, published_at = NOW, via = "device", review = { decided = false } } } } }
		return { ok = true, id = id, version = "1.1.0", status = "listed", verified = false, checks = {}, url = `https://zeron.example/templates/{id}` }
	end)
end
store.version_action = function(id: string, version: string, action: string): any
	return answer(`version_action:{id}:{version}:{action}`, function(): any
		return { ok = true }
	end)
end

for name, run in
	{
		hold = function()
			holding = true
		end,
		release = function()
			holding = false
			local now = held
			held = {}
			for _, f in now do
				f()
			end
		end,
		offline = function()
			offline = true
		end,
		empty = function()
			empty = true
		end,
		problems = function()
			problems = true
		end,
		published = function()
			mine = {
				{
					id = "hot-pink",
					name = "Hot Pink",
					version = "1.0.0",
					versions = { { version = "1.0.0", status = "listed", verified = true, verified_at = NOW - 1e8, published_at = NOW - 1e9, via = "device", review = { decided = true, decision = "verified", decided_at = NOW - 1e8 } } },
				},
			}
		end,
		few = function()
			few = true
		end,
	} :: { [string]: () -> () }
do
	ui.command { id = `template_stub.{name}`, title = `Template store: {name}`, run = run }
end

return {}
