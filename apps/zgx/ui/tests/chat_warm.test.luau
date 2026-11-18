-- A new chat's agent started while its first message is still being written
-- (screens/chat/warm): only by the person's own new chat or keys, never a
-- page merely shown or an existing chat; the send takes it, under its id;
-- and leaving, clearing or writing elsewhere ends it. Never for a chat on
-- another device, on Cloud, or in a worktree made as it's sent. On
-- ui/chat_fixture.luau, which notes `warm:<new|chat>:<open|typed>`,
-- `warm_ask:<id>|<model>|<cwd>|<device>`, `warm_taken:<id>`, `unwarm:…` and
-- `warm_cleared:…`.

local BOX = 'role:textbox "Message"'

local function settle(app: any)
	app:frames(30)
end

local function calls(app: any): string
	return app:state("calls") or ""
end

local function fresh(): any
	local app = mount("chat_fixture.luau")
	app:resize(1200, 800)
	settle(app)
	return app
end

-- The ids warm starts were asked under, in order.
local function warm_ids(app: any): { string }
	local ids = {}
	for id in string.gmatch(calls(app), "warm_ask:([%w%-]+)|") do
		if ids[#ids] ~= id then
			table.insert(ids, id)
		end
	end
	return ids
end

local function count(app: any, word: string): number
	local n = 0
	for _ in string.gmatch(calls(app), word) do
		n += 1
	end
	return n
end

local function pick(app: any, row: string, option: string)
	app:click('role:button "Branch"')
	settle(app)
	app:click(`role:menuitem "{row}"`)
	settle(app)
	app:click(`role:option ~"{option}"`)
	settle(app)
	app:key("Escape")
	app:key("Escape")
	settle(app)
end

-- A new chat in the project's own folder (a new worktree is made as a chat
-- is sent: nothing to start in before).
local function local_checkout(app: any)
	app:command("go_home")
	settle(app)
	pick(app, "Checkout", "Local checkout Work in")
	-- (A chat shown between, so the next new chat is an arrival.)
	app:command("go_s2")
	settle(app)
end

test("opening a new chat starts no agent: only typing in it does", function()
	local app = fresh()
	local_checkout(app)
	app:command("go_home")
	settle(app)
	expect(calls(app)).never.to_contain("warm:new")
	app:command("go_s2")
	settle(app)
	app:command("go_new_by_hand")
	settle(app)
	expect(calls(app)).never.to_contain("warm:new")
	app:type(BOX, "F")
	settle(app)
	expect(calls(app)).to_contain("warm:new:typed")
	expect(calls(app)).never.to_contain("warm:new:open")
	expect(#warm_ids(app)).to_be(1)
end)

test("typing into a new chat warms it, and the send goes into that agent, as typed", function()
	local app = fresh()
	local_checkout(app)
	app:command("go_home")
	settle(app)
	app:type(BOX, "Fix the parser")
	settle(app)
	expect(calls(app)).to_contain("warm:new:typed")
	local ids = warm_ids(app)
	expect(#ids).to_be(1)
	app:key("Enter")
	settle(app)
	-- (Made with the warm start's id, not one of its own, and its message
	-- what was typed.)
	expect(calls(app)).to_contain(`warm_taken:{ids[1]}`)
	expect(calls(app)).to_contain(`send:{ids[1]}:Fix the parser`)
	expect(calls(app)).never.to_contain("unwarm:")
	-- One agent for the whole draft: renewed, never restarted per key.
	expect(#warm_ids(app)).to_be(1)
end)

test("an existing chat's agent never starts early, viewed or typed into", function()
	local app = fresh()
	app:command("go_s1")
	settle(app)
	app:type(BOX, "And the lexer")
	settle(app)
	expect(calls(app)).never.to_contain("warm")
end)

test("writing in another chat stops the new chat's agent (one per window)", function()
	local app = fresh()
	local_checkout(app)
	app:command("go_home")
	settle(app)
	app:type(BOX, "Draft")
	settle(app)
	expect(calls(app)).to_contain("warm:new:typed")
	app:command("go_s1")
	settle(app)
	expect(calls(app)).never.to_contain("unwarm:")
	app:type(BOX, "x")
	settle(app)
	expect(calls(app)).to_contain("unwarm:new")
end)

test("leaving a new chat with its draft empty stops its agent", function()
	local app = fresh()
	local_checkout(app)
	app:command("go_new_by_hand")
	settle(app)
	app:type(BOX, "F")
	settle(app)
	expect(calls(app)).to_contain("warm:new:typed")
	app:key("Backspace")
	settle(app)
	app:command("go_s1")
	settle(app)
	expect(calls(app)).to_contain("unwarm:new")
end)

test("a cleared draft is handed to the host to stop, and typing again asks again", function()
	local app = fresh()
	local_checkout(app)
	app:command("go_home")
	settle(app)
	app:type(BOX, "x")
	settle(app)
	app:key("Backspace")
	settle(app)
	expect(calls(app)).to_contain("warm_cleared:new")
	local before = count(app, "warm_ask:")
	app:type(BOX, "y")
	settle(app)
	-- (The host may have stopped it meanwhile: asked again at once.)
	expect(count(app, "warm_ask:") > before).to_be(true)
end)

test("a change of model while it waits starts another with it", function()
	local app = fresh()
	local_checkout(app)
	app:command("go_home")
	settle(app)
	app:type(BOX, "Draft")
	settle(app)
	local first = string.match(calls(app), "warm_ask:[%w%-]+|([^|]*)|")
	local other = if first == "opus" then "sonnet" else "opus"
	app:set_state("chat_model", other)
	settle(app)
	local models = {}
	for model in string.gmatch(calls(app), "warm_ask:[%w%-]+|([^|]*)|") do
		models[model] = true
	end
	expect(models[first]).to_be(true)
	expect(models[other]).to_be(true)
	-- (The same id: one new chat, its agent replaced.)
	expect(#warm_ids(app)).to_be(1)
end)

test("a new chat in a worktree made as it's sent starts nothing early", function()
	local app = fresh()
	app:command("go_new_by_hand")
	settle(app)
	app:type(BOX, "Branch off")
	settle(app)
	expect(calls(app)).never.to_contain("warm:new")
end)

test("a new chat that will run on another device starts nothing early", function()
	local app = fresh()
	app:command("go_home")
	settle(app)
	pick(app, "Device", "Build box")
	pick(app, "Checkout", "Local checkout Work in")
	app:type(BOX, "On the box")
	settle(app)
	-- (Typed into a local checkout there: only the device keeps it cold.)
	expect(calls(app)).to_contain("refs:/srv/borealis:box")
	expect(calls(app)).never.to_contain("warm_ask:")
end)

test("a new chat on Cloud starts nothing early", function()
	local app = fresh()
	app:command("cloud_stub.offer")
	settle(app)
	app:command("go_home")
	settle(app)
	pick(app, "Device", "Cloud")
	app:type(BOX, "On Cloud")
	settle(app)
	expect(calls(app)).never.to_contain("warm_ask:")
end)

test("a send whose chat isn't made stops the agent started for it", function()
	local app = fresh()
	local_checkout(app)
	app:command("go_home")
	settle(app)
	app:type(BOX, "Fix the parser")
	settle(app)
	local ids = warm_ids(app)
	app:command("create_fails")
	app:key("Enter")
	settle(app)
	expect(calls(app)).to_contain(`create_failed_for:{ids[1]}`)
	expect(calls(app)).to_contain("unwarm:new")
end)

test("a new chat in no project warms its agent here, in its own folder", function()
	local app = fresh()
	app:command("go_home")
	settle(app)
	app:click('role:button "Branch"')
	settle(app)
	app:click('role:menuitem "Project"')
	settle(app)
	app:click('~"Don\'t work in a project"')
	settle(app)
	app:key("Escape")
	app:key("Escape")
	settle(app)
	app:type(BOX, "Just a question")
	settle(app)
	expect(calls(app)).to_contain("warm:new:typed")
	local ids = warm_ids(app)
	app:key("Enter")
	settle(app)
	expect(calls(app)).to_contain(`send:{ids[1]}:Just a question`)
end)
