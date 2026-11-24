-- Settings → Templates → Browse: the store's templates (ui/template_store_stub
-- answers as it would), pictures first in a grid, on the page in
-- ui/templates_fixture.luau. The same screen as the plugins' (store_browse),
-- each with its own state.

local function open(before: string?, w: number?): any
	local app = mount("templates_fixture.luau")
	app:resize(w or 1000, 900)
	app:frames(10)
	if before then
		app:command(before)
	end
	app:click('role:tab "Browse"')
	if not before then
		app:wait("#store-cards", { timeout_ms = 3000 })
	end
	app:frames(20)
	return app
end

local function calls(app: any): string
	return table.concat(app:state("template_stub_calls"), "\n")
end

local function box(line: string): { x: number, y: number, w: number, h: number }
	local x, y, w, h = line:match("(%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
	return { x = tonumber(x) or 0, y = tonumber(y) or 0, w = tonumber(w) or 0, h = tonumber(h) or 0 }
end

-- Each card in the list, by where it is.
local function cards(app: any): { { x: number, y: number, w: number, h: number } }
	local out = {}
	for line in app:tree("#store-cards"):gmatch("[^\n]+") do
		if line:match("^%s*@%d+ button ") then
			table.insert(out, box(line))
		end
	end
	return out
end

test("the store isn't asked until Browse is looked at, and is asked for templates", function()
	local app = mount("templates_fixture.luau")
	app:resize(1000, 900)
	app:frames(20)
	expect(calls(app)).to_be("")
	app:click('role:tab "Browse"')
	app:wait("#store-cards", { timeout_ms = 3000 })
	expect(calls(app)).to_contain("search:q=,sort=popular,cursor=")
	expect(calls(app)).to_contain("picture:midnight@1.2.0/preview.png")
end)

test("what a template is stands once over the list, and a card says who made it and whether it's verified", function()
	local app = open()
	expect(app:count("#store-fact")).to_be(1)
	expect(app:tree("#store-fact")).to_contain("A template runs as Zeron itself")
	local tree = app:tree("#store-cards")
	expect(tree).to_contain("Midnight")
	expect(tree).to_contain("@ada")
	expect(tree).to_contain("Verified")
	expect(tree).to_contain("{count} [b](https://x)")
	expect(app:tree("#store-list")).to_contain("34 templates")
	-- None of a plugin's: no categories, no plugins' words.
	expect(app:has("#store-category")).to_be(false)
	expect(app:tree()).to_contain("Search templates")
	expect(app:tree()).never.to_contain("plugin")
end)

test("the cards are one size, in rows across the column, at every width", function()
	for w, across in { [1000] = 2, [720] = 2, [420] = 1 } do
		local app = open(nil, w)
		local list = cards(app)
		assert(#list >= 4, app:tree("#store-cards"))
		local column = box(app:tree("#store-cards"))
		local first_row = 0
		for _, c in list do
			expect(math.abs(c.w - list[1].w) < 1).to_be(true)
			expect(math.abs(c.h - list[1].h) < 1).to_be(true)
			expect(c.x >= column.x - 0.5 and c.x + c.w <= column.x + column.w + 0.5).to_be(true)
			if math.abs(c.y - list[1].y) < 1 then
				first_row += 1
			end
		end
		expect(first_row).to_be(across)
	end
end)

test("one installed from the store says so, whatever its folder's name", function()
	local app = open()
	expect(app:tree("#store-cards")).never.to_contain("Installed")
	app:command("templates.from_store")
	app:frames(20)
	expect(app:tree("#store-cards")).to_contain("Installed")
end)

test("a search and Verified only ask again, and more come as the end shows", function()
	local app = open()
	app:scroll("#fixture-scroll", 20000)
	app:wait("#never-there", { timeout_ms = 400 })
	app:frames(10)
	expect(calls(app)).to_contain("cursor=30")
	expect(#cards(app)).to_be(34)
	app = open()
	app:click('#store-verified-only >> role:checkbox "Verified only"')
	app:wait("#never-there", { timeout_ms = 400 })
	app:frames(10)
	expect(calls(app)).to_contain(",verified")
	expect(app:tree("#store-cards")).never.to_contain("Ledger")
end)

test("while it loads, offline and with none, it says so in its own words", function()
	local app = open("template_stub.hold")
	expect(app:has("#store-loading")).to_be(true)
	app:command("template_stub.release")
	app:wait("#store-cards", { timeout_ms = 3000 })
	app = open("template_stub.offline")
	app:wait("#never-there", { timeout_ms = 400 })
	app:frames(10)
	expect(app:tree()).to_contain("Can't reach the template store")
	app = open("template_stub.empty")
	app:wait("#never-there", { timeout_ms = 400 })
	app:frames(10)
	expect(app:tree()).to_contain("No templates in the store yet")
end)
