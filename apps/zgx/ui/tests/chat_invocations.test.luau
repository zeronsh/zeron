-- The composer's `/` commands and `$` skills (screens/chat/invocations),
-- on the chat page (ui/chat_fixture.luau, the host's list stubbed): each
-- list comes from the agent's, read as a `/` or `$` opens; it narrows as
-- the name is typed; the agent's settings gate `$` and fold the skills
-- into `/`; a pick lands as its token and goes out as its reference.

local function settle(app: any)
	app:frames(30)
end

local function chat(): any
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	app:command("go_s2")
	settle(app)
	return app
end

local function reads(app: any): number
	local n = 0
	for _ in string.gmatch(app:state("calls") or "", "invocations:") do
		n += 1
	end
	return n
end

local PREFS = "screens/settings/prefs.luau::settings_skill_completion"

test("`/` lists the agent's commands, read as it opens, narrowing as the name is typed", function()
	local app = chat()
	expect(reads(app)).to_be(0)
	app:type("role:textbox", "/")
	settle(app)
	expect(app:state("calls")).to_contain("invocations:claude-code:s2")
	expect(app:has('role:option "/review"')).to_be(true)
	expect(app:has('role:option "/compact"')).to_be(true)
	expect(app:has('~"Review the diff · <branch>"')).to_be(true)
	-- (Separate by default: the skills wait for `$`.)
	expect(app:has('role:option "Code Review"')).to_be(false)
	app:type("role:textbox", "rev")
	settle(app)
	expect(app:has('role:option "/review"')).to_be(true)
	expect(app:has('role:option "/compact"')).to_be(false)
	-- Typed further at, no new read: one per opening.
	expect(reads(app)).to_be(1)
end)

test("`$` lists the skills; with the agent's `$` off it's plain text, and with `/` not separate, `/` lists them too", function()
	local app = chat()
	app:type("role:textbox", "$")
	settle(app)
	expect(app:has('role:option "Code Review"')).to_be(true)
	expect(app:has('role:option "/review"')).to_be(false)
	app:key("Escape")
	app:fill("role:textbox", "")
	settle(app)
	app:set_state(PREFS, { ["claude-code"] = { dollar = false, separate = false } })
	settle(app)
	app:type("role:textbox", "$")
	settle(app)
	expect(app:has('role:option "Code Review"')).to_be(false)
	app:fill("role:textbox", "/")
	settle(app)
	expect(app:has('role:option "/review"')).to_be(true)
	expect(app:has('role:option "Code Review"')).to_be(true)
	app:set_state(PREFS, {})
end)

test("a command picked lands as its token and is sent as its reference; one typed by hand stays words", function()
	local app = chat()
	app:type("role:textbox", "/rev")
	settle(app)
	app:key("Enter")
	settle(app)
	expect(app:text("role:textbox")).to_be("/review ")
	app:type("role:textbox", "the branch, then /compact by hand")
	settle(app)
	app:key("Escape")
	app:key("Enter")
	settle(app)
	-- (It goes to the host as it joins the transcript.)
	expect(app:wait('~"the branch"', { timeout_ms = 2000 })).to_be(true)
	local calls = app:state("calls")
	expect(calls).to_contain("send:s2:[/review](zeron-invoke:7b226b696e64223a22636f6d6d616e64222c226e616d65223a22726576696577227d) the branch, then /compact by hand")
end)

test("Zeron's own commands end the `/` list, named clear of the agent's, and only a chat's in a chat", function()
	local app = chat()
	app:type("role:textbox", "/")
	settle(app)
	expect(app:has('role:option "/model"')).to_be(true)
	expect(app:has('role:option "/stop"')).to_be(true)
	expect(app:has('~"Zeron: open settings"')).to_be(true)
	-- The agent has a `/new`: it keeps it, and Zeron's is `/zeron:new`.
	expect(app:has('role:option "/new"')).to_be(true)
	expect(app:has('role:option "/zeron:new"')).to_be(true)
	-- A new chat (no session): none of a chat's own.
	app:key("Escape")
	app:fill("role:textbox", "")
	app:command("go_home")
	settle(app)
	app:type("role:textbox", "/")
	settle(app)
	expect(app:has('role:option "/model"')).to_be(true)
	expect(app:has('role:option "/stop"')).to_be(false)
	expect(app:has('role:option "/rename"')).to_be(false)
end)

test("one of Zeron's own picked runs at once, its token gone and the rest kept; a draft of only it runs as it's sent", function()
	local app = chat()
	app:type("role:textbox", "Keep this /sto")
	settle(app)
	app:key("Enter")
	settle(app)
	expect(app:state("calls")).to_contain("interrupt:s2")
	expect(app:text("role:textbox")).to_be("Keep this ")
	expect(app:state("calls")).never.to_contain("send:")
	app:fill("role:textbox", "/stop")
	settle(app)
	app:key("Escape")
	app:key("Enter")
	settle(app)
	local _, n = string.gsub(app:state("calls"), "interrupt:s2", "")
	expect(n).to_be(2)
	expect(app:state("calls")).never.to_contain("send:")
	expect(app:text("role:textbox")).to_be("")
end)

-- (`/rename` opens the sidebar's rename, which reads the host's places:
-- the fixture has none, so it's left to the app.)
test("every one of Zeron's own runs from the chat page without an error", function()
	-- (The fixture's agent has a `/new` of its own, so Zeron's is
	-- `/zeron:new`: `/new` goes to the agent, as it should.)
	for _, name in { "model", "resume", "settings", "files", "terminal", "zeron:new" } do
		local app = chat()
		app:type("role:textbox", `/{name}`)
		settle(app)
		app:key("Escape")
		app:key("Enter")
		settle(app)
		expect(app:error()).to_be_nil()
		expect(app:state("calls")).never.to_contain("send:")
	end
end)
