-- Settings → Templates → Yours: where each of yours stands in the store, and
-- publishing one (screens/plugins/store_publish, in a template's words), on
-- the page in ui/templates_fixture.luau, the store in
-- ui/template_store_stub.luau.

local function calls(app: any): string
	return table.concat(app:state("template_stub_calls"), "\n")
end

local function settle(app: any)
	app:wait("#never-there", { timeout_ms = 300 })
	app:frames(10)
end

local function yours(before: string?): any
	local app = mount("templates_fixture.luau")
	app:resize(1000, 900)
	app:frames(10)
	if before then
		app:command(before)
	end
	app:click('role:tab "Yours"')
	settle(app)
	return app
end

local function publishing(before: string?): any
	local app = yours(before)
	app:click('#template-hot-pink >> role:link "Publish…"')
	app:wait("#publish-checks", { timeout_ms = 3000 })
	settle(app)
	return app
end

test("each of yours says where it stands in the template store, with Publish…; Installed's rows are as they were", function()
	local app = yours()
	expect(calls(app)).to_contain("mine")
	expect(app:tree("#template-hot-pink")).to_contain("Not in the template store")
	expect(app:has('#template-hot-pink >> role:link "Publish…"')).to_be(true)
	expect(string.lower(app:tree())).never.to_contain("plugin")
	app = yours("template_stub.published")
	expect(app:tree("#template-hot-pink")).to_contain("1.0.0 in the template store, verified")
	expect(app:tree("#template-night")).to_contain("Not in the template store")
	app:click('role:tab "Installed"')
	settle(app)
	expect(app:tree()).never.to_contain("Publish…")
	expect(app:tree()).never.to_contain("in the template store")
end)

test("one taken down says so there, and offers no Publish…", function()
	local app = yours("templates.taken_down")
	expect(app:has('#template-night >> role:link "Publish…"')).to_be(false)
	expect(app:has('#template-hot-pink >> role:link "Publish…"')).to_be(true)
end)

test("Publish sends exactly what was checked, with its ticket, and it's listed, not verified yet", function()
	local app = publishing()
	local tree = app:tree()
	for _, said in { "Publish Hot Pink", "A README", "A description", "A preview picture", "A version the store takes", "The Zeron it's made for", "Newer than the last", "6 files are sent", "Nothing sent yet." } do
		expect(tree).to_contain(said)
	end
	for _, never in { "plugin", "An icon", "A category", "before anyone else can install" } do
		expect(tree).never.to_contain(never)
	end
	app:click("#publish-submit")
	settle(app)
	expect(calls(app)).to_contain("publish:hot-pink:hot-pink-1.1.0:ticket-1.1.0")
	expect(app:tree()).to_contain("Not verified yet")
	app:click('role:button "Back to Templates"')
	settle(app)
	expect(app:tree("#template-hot-pink")).to_contain("1.1.0 in the template store, not verified yet")
end)

test("what's missing is said in a template's words, and nothing can be sent", function()
	for _, where in { "en", "zh-Hans" } do
		local app = yours("template_stub.problems")
		app:command(`locale_{where}`)
		app:set_state("templates_view", "publish:hot-pink")
		app:wait("#publish-checks", { timeout_ms = 3000 })
		settle(app)
		local tree = app:tree("#publish-checks")
		if where == "en" then
			expect(tree).to_contain("Add a picture of it, preview.png")
			expect(tree).to_contain("A template has no “categories”.")
			expect(tree).to_contain("This file is Zeron's own")
			expect(tree).to_contain("account/sessions.luau")
			-- (One sentence a cause: the engine's where Zeron's is of another.)
			expect(tree).to_contain("APP~1.LUA is a short name")
			expect(tree).to_contain("Its id isn't one the store takes")
			expect(tree).to_contain("It holds a file Zeron doesn't write for a template")
			expect(tree).to_contain("A file there isn't a picture")
		else
			expect(tree).to_contain("模板没有“categories”")
			expect(tree).to_contain("preview.png")
		end
		expect(tree).never.to_contain("zeron-plugin.json")
		expect(app:tree("#publish-submit")).to_contain("disabled")
	end
end)
