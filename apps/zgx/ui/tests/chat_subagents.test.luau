-- A reply's subagents as their own block at its top (ds/subagents), from
-- every harness: its spawns (Claude Code's here) and the chats its agent
-- made through Zeron as subagents (a Codex one), joined on the chat page
-- (screens/chat/subagents). Open while any runs, folded once all are done
-- unless the reader chose; Stop while one runs (its call, or its chat's
-- turn), Open once done (its lane); the Zeron call that made a chat leaves
-- the reply's tool group. On ui/chat_fixture.luau.

local function open(): any
	local app = mount("chat_fixture.luau")
	app:resize(1000, 800)
	app:command("go_s2")
	app:frames(30)
	app:command("subagents_running")
	app:frames(40)
	return app
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

-- A node's identity in the tree, by a pattern on its line: `@id`.
local function node(app: any, pattern: string): string?
	for line in app:tree():gmatch("[^\n]+") do
		if line:find(pattern, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

test("running subagents from two harnesses show open at the reply's top, out of its tool group", function()
	local app = open()
	expect(app:has('role:button "3 subagents · 3 working"')).to_be(true)
	-- A row each: Claude Code's spawns and the chat made on Codex.
	expect(app:has('role:group "Map the parser"')).to_be(true)
	expect(app:has('role:group "Check the tests"')).to_be(true)
	expect(app:has('role:group "Review the API"')).to_be(true)
	-- Model and what it's on, while running.
	expect(app:has('~"haiku · Explore"')).to_be(true)
	expect(app:has('~"gpt-5.5 · Reading routes.ts"')).to_be(true)
	-- Each has its Stop, and each row whose lane is known opens it.
	expect(app:has('role:button "Stop Map the parser"')).to_be(true)
	expect(app:has('role:button "Stop Review the API"')).to_be(true)
	expect(app:has('role:button "Open Map the parser"')).to_be(true)
	expect(app:has('role:button "Open Review the API"')).to_be(true)
	expect(app:has('role:button "Open Check the tests"')).to_be(false)
	-- The tool group counts the command alone: the Zeron call is the block's.
	expect(app:has('role:button "Ran 1 command"')).to_be(true)
	expect(app:has('~"called 1 tool"')).to_be(false)
	-- The block sits above the group.
	local block, group = nil :: number?, nil :: number?
	for line in app:tree():gmatch("[^\n]+") do
		if not block and line:find('button "3 subagents', 1, true) then
			block = tonumber(line:match(" %-?%d+,(%-?%d+) "))
		elseif not group and line:find('button "Ran 1 command"', 1, true) then
			group = tonumber(line:match(" %-?%d+,(%-?%d+) "))
		end
	end
	expect(block ~= nil and group ~= nil and block < group).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("subagents all done fold themselves and say done; the reader's choice is kept", function()
	local app = open()
	app:command("subagents_done")
	app:frames(40)
	expect(app:has('role:button "3 subagents · done"')).to_be(true)
	expect(app:has('role:group "Map the parser"')).to_be(false)
	-- Opened by hand, it shows each one's result and Open (where it has a lane).
	app:click('role:button "3 subagents · done"')
	app:frames(30)
	expect(app:has('~"Found 3 callers of parse()."')).to_be(true)
	expect(app:has('~"The API looks fine."')).to_be(true)
	expect(app:has('role:button "Open Map the parser"')).to_be(true)
	expect(app:has('role:button "Open Review the API"')).to_be(true)
	-- (Its doc unknown: nothing to open.)
	expect(app:has('role:button "Open Check the tests"')).to_be(false)
	expect(app:has('role:button "Stop Map the parser"')).to_be(false)
	-- Made again (a reload), it stays as the reader left it.
	app:command("remake_session")
	app:frames(40)
	expect(app:has('role:group "Map the parser"')).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("a running subagent folded by hand stays folded", function()
	local app = open()
	app:click('role:button "3 subagents · 3 working"')
	app:frames(30)
	expect(app:has('role:group "Map the parser"')).to_be(false)
	app:command("subagent_step")
	app:frames(20)
	expect(app:has('role:group "Map the parser"')).to_be(false)
	expect(app:has('role:button "3 subagents · 3 working"')).to_be(true)
end)

test("a subagent's Stop asks for that spawn's call alone, or interrupts that chat", function()
	local app = open()
	app:click('role:button "Stop Map the parser"')
	app:frames(10)
	expect(calls(app)).to_contain("stop_call:s2:sp1")
	expect(app:has('role:button "Stop Map the parser"')).to_be(false)
	expect(app:has('role:group "Map the parser" >> "Stopping"')).to_be(true)
	app:click('role:button "Stop Review the API"')
	app:frames(10)
	expect(calls(app)).to_contain("interrupt:kid1")
	-- A refused stop gives the row its Stop back.
	app:command("stop_unsupported")
	app:click('role:button "Stop Check the tests"')
	app:frames(10)
	expect(calls(app)).to_contain("stop_call:s2:sp2")
	expect(app:has('role:button "Stop Check the tests"')).to_be(true)
	expect(app:error()).to_be(nil)
end)

test("Open opens a finished subagent's lane: a spawn's doc, a chat's own", function()
	local app = open()
	app:command("subagents_done")
	app:frames(30)
	app:click('role:button "3 subagents · done"')
	app:frames(30)
	app:click('role:button "Open Map the parser"')
	app:frames(10)
	expect(calls(app)).to_contain("lane:s2:subagent:s2--sub--1")
	app:click('role:button "Open Review the API"')
	app:frames(10)
	expect(calls(app)).to_contain("lane:s2:subagent:kid1")
end)

-- A subagent's row (its line names it alone, no kind before its label).
local function row_node(app: any, title: string): string?
	for line in app:tree():gmatch("[^\n]+") do
		local id = line:match('^%s*@(%d+) "' .. title .. '"')
		if id then
			return id
		end
	end
	return nil
end

test("subagents' live changes reach their rows in place", function()
	local app = open()
	local row = row_node(app, "Map the parser")
	local chat_row = row_node(app, "Review the API")
	local header = node(app, 'button "3 subagents')
	expect(row ~= nil and chat_row ~= nil and header ~= nil).to_be(true)
	-- The chat moves on: its second line follows, its row the same.
	app:command("subagent_step")
	app:frames(20)
	expect(app:has('~"gpt-5.5 · Checking the auth middleware"')).to_be(true)
	expect(row_node(app, "Review the API")).to_be(chat_row)
	-- A third joins: a row of its own, the others untouched.
	app:command("subagent_joins")
	app:frames(30)
	expect(app:has('role:button "4 subagents · 4 working"')).to_be(true)
	expect(app:has('role:group "Read the docs"')).to_be(true)
	expect(row_node(app, "Map the parser")).to_be(row)
	expect(node(app, 'button "4 subagents')).to_be(header)
	-- All done: the same rows, now done.
	app:command("subagents_done")
	app:frames(5)
	expect(row_node(app, "Map the parser")).to_be(row)
	expect(app:error()).to_be(nil)
end)

test("a subagent's whole row opens its lane while it runs; its Stop only stops", function()
	local app = open()
	-- Its title, its second line, its face: each is in the row's press.
	expect(app:has('role:button "Open Map the parser" >> "Map the parser"')).to_be(true)
	expect(app:has('role:button "Open Map the parser" >> "haiku · Explore"')).to_be(true)
	app:click('role:button "Open Map the parser"')
	app:frames(10)
	expect(calls(app)).to_contain("lane:s2:subagent:s2--sub--1")
	app:click('role:button "Open Review the API"')
	app:frames(10)
	expect(calls(app)).to_contain("lane:s2:subagent:kid1")
	-- Stop is its own press: it stops, and opens nothing.
	local function lanes_opened(): number
		return select(2, string.gsub(calls(app), "lane:", ""))
	end
	local before = lanes_opened()
	app:click('role:button "Stop Map the parser"')
	app:frames(10)
	expect(calls(app)).to_contain("stop_call:s2:sp1")
	expect(lanes_opened()).to_be(before)
	-- No lane known (its doc not stamped yet): the row opens nothing.
	expect(app:has('role:button "Open Check the tests"')).to_be(false)
	expect(app:error()).to_be(nil)
end)

test("a subagent's row is a key stop: Enter opens its lane", function()
	local app = open()
	local found = false
	for _ = 1, 40 do
		app:key("Tab")
		app:frames(3)
		for line in app:tree():gmatch("[^\n]+") do
			if line:find(" focused", 1, true) and line:find('button "Open Review the API"', 1, true) then
				found = true
			end
		end
		if found then
			break
		end
	end
	expect(found).to_be(true)
	app:key("Enter")
	app:frames(10)
	expect(calls(app)).to_contain("lane:s2:subagent:kid1")
end)
