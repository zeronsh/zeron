-- A deploy token's scope: all your plugins (the first choice), all your
-- plugins and templates (which says what a template can do), or the ones
-- chosen, templates under their own heading. On ui/keys_fixture.luau.

local function settle(app: any)
	app:wait("#never-there", { timeout_ms = 300 })
	app:frames(10)
end

local function open(before: string?): any
	local app = mount("keys_fixture.luau")
	app:resize(900, 800)
	if before then
		app:command(before)
	end
	settle(app)
	app:click("#keys-new")
	settle(app)
	return app
end

local function choose(app: any, words: string)
	app:click("#key-scope-choice")
	settle(app)
	app:click(`role:menuitem "{words}"`)
	settle(app)
end

local function made(app: any, name: string)
	app:type('role:dialog >> role:textbox "Name"', name)
	app:click('role:dialog >> role:button "Create token"')
	settle(app)
end

local function calls(app: any): string
	return table.concat(app:state("store_stub_calls"), "\n")
end

test("all your plugins is the first choice, and names no kinds", function()
	local app = open()
	expect(app:tree("#key-scope-choice")).to_contain("All your plugins")
	expect(app:has("#key-scope-hint")).to_be(false)
	made(app, "CI")
	expect(calls(app)).to_contain("create_token:all:CI:")
end)

test("all your plugins and templates says what a template can do, and names both kinds", function()
	local app = open()
	choose(app, "All your plugins and templates")
	expect(app:tree("#key-scope-hint")).to_contain("A template can do anything Zeron can.")
	made(app, "CI")
	expect(calls(app)).to_contain("create_token:all:plugin+template:CI:")
	app:command("store_stub.signed_in_again")
	settle(app)
	app:click('role:dialog >> role:button "I\'ve copied it"')
	settle(app)
	expect(app:tree()).to_contain("All your plugins and templates: 3 today")
end)

test("chosen lists plugins, then templates under their heading, and takes either", function()
	local app = open()
	choose(app, "Chosen")
	local tree = app:tree("#key-scope")
	local tidy, heading, midnight = string.find(tree, "Tidy", 1, true), string.find(tree, "#key-scope-templates", 1, true), string.find(tree, "Midnight", 1, true)
	assert(tidy and heading and midnight, tree)
	expect(tidy < heading and heading < midnight).to_be(true)
	expect(tree).to_contain("Templates")
	expect(app:has("#key-scope-hint")).to_be(false)
	app:click('#key-scope >> role:checkbox "Midnight"')
	app:frames(10)
	expect(app:tree("#key-scope-hint")).to_contain("A template can do anything Zeron can.")
	app:click('#key-scope >> role:checkbox "Tidy"')
	settle(app)
	made(app, "CI")
	expect(calls(app)).to_contain("create_token:tidy,midnight:CI:")
end)

test("an author with no templates sees no heading for them", function()
	local app = open("no_templates")
	choose(app, "Chosen")
	expect(app:has("#key-scope-templates")).to_be(false)
	expect(app:tree("#key-scope")).to_contain("Focus timer")
end)

test("a token for plugins alone keeps its words", function()
	local app = mount("keys_fixture.luau")
	app:resize(900, 800)
	settle(app)
	expect(app:tree()).to_contain("All your plugins: 2 today")
end)

test("the choices are in Chinese too", function()
	local app = open("zh")
	choose(app, "你的所有插件和模板")
	expect(app:tree("#key-scope-hint")).to_contain("模板可以做 Zeron 能做的一切。")
	choose(app, "选定的")
	expect(app:tree("#key-scope-templates")).to_contain("模板")
end)
