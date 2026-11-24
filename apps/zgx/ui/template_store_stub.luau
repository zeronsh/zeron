-- The store's templates (screens/templates/store), answered here as the
-- engine's `Store*` calls would with `kind = "template"`: a made-up catalog of
-- 34, so Browse pages. What was asked is in `template_stub_calls`.
--   template_stub.hold / release   answers wait until released
--   template_stub.offline          every call fails as offline
--   template_stub.empty            the store has none

local store: any = require("@app/screens/templates/store")

local calls = ui.state("template_stub_calls", {} :: { string })
local holding, offline, empty = false, false, false
local held: { () -> () } = {}

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
		ui.after(16, settle)
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
for i = 1, 30 do
	add(`made-up-{i}`, `Made up {i}`, "One of many.", "ada", 500 - i, i % 2 == 0)
end

store.search = function(q: any): any
	return answer(`search:q={q.q or ""},sort={q.sort or ""},cursor={q.cursor or ""}{if q.featured then ",featured" else ""}{if q.verified then ",verified" else ""}`, function(): any
		local found: { any } = {}
		for _, c in (if empty then {} else catalog) :: { any } do
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
	} :: { [string]: () -> () }
do
	ui.command { id = `template_stub.{name}`, title = `Template store: {name}`, run = run }
end

return {}
