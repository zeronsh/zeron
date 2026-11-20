local function open(): any
	local app = mount("panel_context_fixture.luau")
	app:resize(1400, 900)
	app:frames(30)
	app:click('#shell-sidebar >> role:button "Fix the build"')
	app:command("side_chats")
	for _, command in { "context.file", "context.side", "context.other" } do
		app:command(command)
		app:frames(30)
	end
	expect(app:error()).to_be_nil()
	return app
end

local function snapshot(app: any): any
	app:command("context.snapshot")
	return app:state("context.snapshot")
end

for _, side in { false, true } do
	test(`panel context: file and saved side-chat tabs attach to the {if side then "side" else "main"} composer`, function()
		local app = open()
		local before = snapshot(app)
		local destination = if side then '.side-chat-tab >> role:textbox' else '#chat-composer >> role:textbox'
		app:drag('#panel-strip >> role:tab "notes.md"', destination)
		app:frames(20)
		app:drag('#panel-strip >> role:tab "Look into the flake"', destination)
		app:frames(20)
		local after = snapshot(app)
		local cargo = if side then after.side else after.main
		expect(#cargo).to_be(2)
		expect(cargo[1].rel).to_be("notes.md")
		expect(cargo[1].path).to_contain("/notes.md")
		expect(cargo[2].kind).to_be("chat")
		expect(cargo[2].chat).to_be("s1")
		expect(cargo[2].name).to_be("Look into the flake")
		expect(cargo[2].mark).to_be("claude")
		expect(#after.source).to_be(0)
		expect(#(if side then after.main else after.side)).to_be(0)
		expect(after.tabs.active).to_be(before.tabs.active)
		for i, tab in before.tabs.tabs do
			expect(after.tabs.tabs[i].id).to_be(tab.id)
		end
		-- Dropping the same references again keeps one chip per context.
		app:drag('#panel-strip >> role:tab "notes.md"', destination)
		app:drag('#panel-strip >> role:tab "Look into the flake"', destination)
		after = snapshot(app)
		expect(#(if side then after.side else after.main)).to_be(2)
		expect(app:error()).to_be_nil()
	end)
end

test("panel context: an unsent side-chat tab carries no chat reference", function()
	local app = open()
	app:command("context.unsent")
	app:frames(30)
	app:drag('#panel-strip >> role:tab "New side chat"', '#chat-composer >> role:textbox')
	expect(#snapshot(app).main).to_be(0)
	expect(app:error()).to_be_nil()
end)

test("panel context: a file tab carries its current file after another is picked", function()
	local app = open()
	app:click('#panel-strip >> role:tab "notes.md"')
	app:frames(30)
	app:command("context.pick")
	expect(app:wait('#panel-strip >> role:tab "long.txt"', { timeout_ms = 15000 })).to_be(true)
	app:drag('#panel-strip >> role:tab "long.txt"', '#chat-composer >> role:textbox')
	local cargo = snapshot(app).main
	expect(#cargo).to_be(1)
	expect(cargo[1].rel).to_be("long.txt")
	expect(cargo[1].path).to_contain("/long.txt")
	expect(app:error()).to_be_nil()
end)
