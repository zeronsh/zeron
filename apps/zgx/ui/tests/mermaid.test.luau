-- Mermaid fences (ds/mermaid) in a reply streaming in (ui/mermaid_fixture
-- .luau, a stand-in for the host's drawing): a fence still being written
-- is its code and asks for nothing; once it closes it's drawn, a flowchart
-- and a sequence diagram alike; Show source and Show diagram swap them in
-- the same frame; a click opens the diagram in the lightbox; one that
-- can't be drawn stays code, with why under it.

local function open_reply(): any
	local app = mount("mermaid_fixture.luau")
	app:resize(900, 900)
	app:command("test.stub")
	app:frames(4)
	expect(app:error()).to_be_nil()
	return app
end

local function diagrams(app: any): number
	return app:count('role:button "View diagram"')
end

test("a fence still being written is its code, and nothing is drawn until it closes", function()
	local app = open_reply()
	app:command("test.append")
	app:frames(20)
	expect(app:has('~"flowchart TD"')).to_be(true)
	expect(app:has('"Copy code"')).to_be(true)
	expect(app:count('role:button "Show source"')).to_be(0)
	expect(diagrams(app)).to_be(0)
	app:command("test.append")
	app:frames(20)
	expect(diagrams(app)).to_be(0)
	expect(app:state("test_mermaid_calls")).to_be(0)
	-- The fence closes (and a second diagram arrives whole): both drawn.
	app:command("test.append")
	expect(app:wait('role:button "View diagram"', { timeout_ms = 2000 })).to_be(true)
	app:frames(20)
	expect(diagrams(app)).to_be(2)
	expect(app:count('role:button "Show source"')).to_be(2)
	expect(app:state("test_mermaid_calls")).to_be(2)
	expect(app:error()).to_be_nil()
end)

test("Show source and Show diagram swap the diagram and its code in its frame", function()
	local app = open_reply()
	app:command("test.all")
	expect(app:wait('role:button "View diagram"', { timeout_ms = 2000 })).to_be(true)
	app:frames(20)
	expect(diagrams(app)).to_be(2)
	app:click('role:button "Show source"[1]')
	app:frames(10)
	expect(diagrams(app)).to_be(1)
	expect(app:count('role:button "Show diagram"')).to_be(1)
	expect(app:has('~"flowchart TD"')).to_be(true)
	app:click('role:button "Show diagram"')
	app:frames(10)
	expect(diagrams(app)).to_be(2)
	expect(app:count('role:button "Show diagram"')).to_be(0)
	-- Drawn once each: the swap asks for nothing again.
	expect(app:state("test_mermaid_calls")).to_be(3)
	expect(app:error()).to_be_nil()
end)

test("a diagram opens in the lightbox, on the fence's fill", function()
	local app = open_reply()
	app:command("test.all")
	expect(app:wait('role:button "View diagram"', { timeout_ms = 2000 })).to_be(true)
	app:frames(20)
	app:click('role:button "View diagram"[1]')
	app:frames(90)
	local open = app:state("ds/lightbox.luau::open")
	expect(open).never.to_be_nil()
	expect(open.items[1].src).to_be("assets/composer/transcript.png")
	expect(open.items[1].name).to_be("Mermaid diagram")
	app:key("Escape")
	app:frames(120)
	expect(app:state("ds/lightbox.luau::open")).to_be_nil()
	expect(app:error()).to_be_nil()
end)

test("a fence that can't be drawn stays code, with why under it", function()
	local app = open_reply()
	app:command("test.all")
	expect(app:wait('~"Couldn\'t draw this diagram"', { timeout_ms = 2000 })).to_be(true)
	expect(app:has('~"Parse error on line 1"')).to_be(true)
	expect(app:has('~"not a diagram at all"')).to_be(true)
	expect(app:error()).to_be_nil()
end)

test("a fence being written is told from one that has closed", function()
	local app = open_reply()
	-- Open with its last line, open with nothing in it yet, closed, closed
	-- with words after it, closed by tildes.
	expect(app:state("test_mermaid_writing")).to_be("true true false false false")
end)
