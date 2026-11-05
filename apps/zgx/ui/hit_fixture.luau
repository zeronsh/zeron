-- The main window as it is (A's ui/shell_fixture.luau), with a session's
-- transcript in it: the chat page's host stood in for (as
-- ui/chat_fixture.luau does), each session opening on a long conversation.
-- What src/hit_sweep_tests.rs moves the pointer over.

local store = require("@app/screens/chat/store")

-- A long conversation: more than a window's height.
local function long(n: number): { any }
	local out = {}
	for i = 1, n do
		table.insert(out, { id = `u{i}`, role = "user", text = `Question number {i}: how does part {i} work?` })
		table.insert(out, {
			id = `a{i}`,
			role = "assistant",
			text = `Part {i} works like this.\n\nIt reads its input, then it writes **its answer**, then it stops.`,
			model = "Opus",
		})
	end
	return out
end

local node = require("./shell_fixture")

local stub = store :: any
local opened: string? = nil
stub.open = function(id: string?)
	if id == opened then
		return
	end
	opened = id
	if not id then
		return
	end
	store.messages:set(long(30))
	store.draft:set { id = "", text = "" }
	store.session:set { id = id, state = "ready", title = "Fix the build", project = "atlas", model = "opus", effort = "high", access = "full", plan = false }
end

return node
