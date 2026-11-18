-- A store plugin's own words built to cost (a hostile README, a version's
-- notes): the page opens with no error, each frame's build bounded, and
-- what's past the budget is plain text after one quiet line
-- (screens/plugins/untrusted). zgx stops a script call past 50 ms, so a
-- text built in one long call would leave About undrawn.

local KIB = 1024

local function settle(app: any, ms: number?)
	app:wait("#never-there", { timeout_ms = ms or 400 })
	app:frames(10)
end

local function lines(n: number, f: (i: number) -> string): string
	local out = {}
	for i = 1, n do
		table.insert(out, f(i))
	end
	return table.concat(out, "\n")
end

-- Each shape at the store's README cap (512 KiB), as the web store's review
-- found them, and the honest worst: prose.
local function shapes(size: number): { any }
	local para = "A paragraph of ordinary prose, its words wrapping across the page as any README's do, with a [link](https://example.com) and some `code` in it.\n\n"
	local nested, total, i = {}, 0, 0
	while total < size do
		local line = string.rep("  ", i % 400) .. "- item\n"
		table.insert(nested, line)
		total += #line
		i += 1
	end
	local n = size // 5
	return {
		{ "setext headings", string.sub(string.rep("a\n=\n", size // 4 + 1), 1, size) },
		{ "escaped quotes", string.rep("&gt;", size // 4) },
		{ "quote depth", string.rep(">", size) },
		{ "open links", string.rep("[", n) .. "a" .. string.rep("](b)", n) },
		{ "emphasis run", string.rep("*a ", size // 3) },
		{ "nested lists", table.concat(nested) },
		{ "a table", "| a | b |\n|---|---|\n" .. lines(size // 16, function(k) return "| x" .. k .. " | y |" end) },
		{ "an open fence", "```\n" .. string.rep("code line\n", size // 10) },
		{ "prose", string.sub(string.rep(para, size // #para + 1), 1, size) },
		{ "a run of blanks", string.rep(" ", size // 2) .. string.rep(" \n", size // 4) },
	}
end

-- The page opened on Focus timer with `readme` (and `notes`) as its own.
local function page(readme: string, notes: string?): (any, number)
	local app = mount("plugins_store_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	app:set_state("store_stub_readme", readme)
	if notes then
		app:set_state("store_stub_notes", notes)
	end
	local t0 = os.clock()
	app:set_state("plugins_view", "store:focus-timer")
	app:wait("#store-readme", { timeout_ms = 5000 })
	app:frames(1)
	return app, (os.clock() - t0) * 1000
end

test("a README built to cost, every shape at the store's 512 KiB, opens its page with no error, About drawn", function()
	for _, s in shapes(512 * KIB) do
		local app = page(s[2])
		settle(app)
		expect(`{s[1]}: {app:error() or "no error"}`).to_be(`{s[1]}: no error`)
		expect(`{s[1]}: {app:has("#store-readme")}`).to_be(`{s[1]}: true`)
	end
end)

test("past its budget a README is plain text after one quiet line; within it, Markdown", function()
	-- (A setext README of 512 KiB: far past the pieces Markdown may take.)
	local app = page(string.sub(string.rep("a\n=\n", 131072 + 1), 1, 512 * KIB))
	settle(app)
	-- Show more until it's all there: the plain part, with its line.
	for _ = 1, 60 do
		if not app:has('#store-readme-more >> role:button "Show more"') then
			break
		end
		local y = tonumber((app:tree("#store-readme-more"):match(" [%d.]+,([%-%d.]+) "))) or 0
		app:scroll("#fixture-scroll", y - 450)
		app:frames(2)
		app:click('#store-readme-more >> role:button "Show more"')
		app:frames(3)
		expect(app:error()).to_be(nil)
	end
	expect(app:has('#store-readme-more >> role:button "Show more"')).to_be(false)
	expect(app:error()).to_be(nil)
	expect(app:count(".untrusted-plain-note")).to_be(1)
	expect(app:tree(".untrusted-plain-note")).to_contain("plain text past here")
	-- (An honest README stays Markdown throughout.)
	local honest = page("# Focus timer\n\nA 25 minute timer.\n\n## What it does\n\n- Counts down.\n")
	settle(honest)
	expect(honest:has(".untrusted-plain-note")).to_be(false)
end)

test("a 64 KiB note built to cost opens in full with no error, a piece a frame", function()
	local notes = string.sub(string.rep("a\n=\n", 16384 + 1), 1, 64 * KIB)
	local app = page("# Focus timer\n\nA timer.\n", notes)
	settle(app)
	app:scroll("#fixture-scroll", 1e6)
	settle(app)
	app:click(".store-notes-more[1]")
	settle(app, 800)
	expect(app:error()).to_be(nil)
end)

test("a piece drawn plain starts and ends on its own text, wherever the cut fell", function()
	-- (Headings a blank line apart, as `store_stub.hostile_readme`: the cut
	-- to plain falls on a blank line.)
	local app = page(string.sub(string.rep("Heading\n=======\n\n", 32768), 1, 512 * KIB))
	settle(app)
	for _ = 1, 60 do
		if app:has(".untrusted-plain") then
			break
		end
		local y = tonumber((app:tree("#store-readme-more"):match(" [%d.]+,([%-%d.]+) "))) or 0
		app:scroll("#fixture-scroll", y - 450)
		app:frames(2)
		app:click('#store-readme-more >> role:button "Show more"')
		app:frames(3)
	end
	expect(app:tree(".untrusted-plain[1]")).to_contain('text "Heading')
	expect(app:error()).to_be(nil)
end)
