-- The new-chat page keeps its own settings: what was last chosen there
-- (agent and model, effort, Fast, the model's options, access, plan, and
-- where: device, project, checkout, branch), never a chat's. They come back
-- after a chat is opened or made, and after a relaunch, with the page's
-- draft and what it carries. (Except where: New chat after a chat was open
-- starts in that chat's project, on its device.) (A chat's own composer keeps its own.) On the
-- chat fixture: s1 runs Opus/high/full access, s2 Sonnet/medium/edits.

local function settle(app: any)
	app:frames(30)
end

local function go(app: any, where: string)
	app:command(where)
	settle(app)
end

-- What the composer shows, in one line.
local function shown(app: any): string
	local o = app:state("chat_options") or {}
	local opts: { string } = {}
	for k, v in o do
		table.insert(opts, `{k}={v}`)
	end
	table.sort(opts)
	return `{app:state("chat_model") or "-"}|{app:state("chat_effort") or "-"}|{tostring(app:state("chat_fast"))}|{table.concat(opts, ",")}|{app:state("chat_access") or "-"}|{tostring(app:state("chat_plan"))}|{app:state("chat_device") or "-"}|{app:state("chat_project") or "-"}|{app:state("chat_checkout") or "-"}|{app:state("chat_branch") or "-"}`
end

local function open(): any
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	go(app, "go_new")
	return app
end

-- The new page's settings, as a person picks them: Sonnet on low, edits
-- only, plan on, in Notes, in its folder.
local function pick(app: any)
	app:set_state("chat_model", "sonnet")
	app:set_state("chat_effort", "low")
	app:set_state("chat_access", "edits")
	app:set_state("chat_plan", true)
	app:set_state("chat_project", "notes")
	app:set_state("chat_checkout", "local")
	settle(app)
	-- (Its branch as the project's refs come: the one checked out.)
	app:wait("#never-shown", { timeout_ms = 200 })
	settle(app)
end

test("the new page's settings come back after visiting a chat", function()
	local app = open()
	pick(app)
	local mine = shown(app)
	go(app, "go_s1")
	expect(app:state("chat_model")).to_be("opus")
	go(app, "go_new")
	-- (All but where: that's the chat last open's, Long chat in Atlas.)
	expect(shown(app)).to_be((mine:gsub("|notes|local|%-$", "|atlas|local|main")))
end)

test("a model never picked on the new page doesn't become the last chat's", function()
	local app = open()
	-- Nothing picked here: the catalog's first (Opus), as a reload shows.
	local mine = shown(app)
	expect(app:state("chat_model")).to_be("opus")
	-- The last chat seen runs Sonnet: not taken.
	go(app, "go_s1")
	go(app, "go_s2")
	go(app, "go_new")
	expect(shown(app)).to_be(mine)
end)

test("a model's options picked in a chat stay that chat's", function()
	local app = open()
	pick(app)
	local mine = shown(app)
	go(app, "go_s2")
	app:set_state("chat_model", "fusion")
	app:set_state("chat_options", { lead = "sol" })
	settle(app)
	go(app, "go_new")
	-- (All but where: that's the chat last open's, Short chat in Atlas.)
	expect(shown(app)).to_be((mine:gsub("|notes|local|%-$", "|atlas|local|main")))
end)

test("a chat made from the new page leaves the new page's settings as they were", function()
	local app = open()
	pick(app)
	local mine = shown(app)
	app:type('role:textbox "Message"', "Start something")
	app:key("Enter")
	settle(app)
	expect(app:state("calls")).to_contain("send:new1:Start something")
	-- The chat made: picked differently in it, and another chat seen.
	app:set_state("chat_model", "opus")
	app:set_state("chat_access", "full")
	settle(app)
	go(app, "go_s1")
	go(app, "go_new")
	-- (All but where: that's the chat last open's, Long chat in Atlas.)
	expect(shown(app)).to_be((mine:gsub("|notes|local|%-$", "|atlas|local|main")))
end)

test("the new page's project, picked there, outlasts the project the route last asked for", function()
	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	-- A new chat asked for in Atlas (a project's "+").
	go(app, "go_home")
	expect(app:state("chat_project")).to_be("atlas")
	app:set_state("chat_project", "notes")
	settle(app)
	-- A chat visited (Long chat, in Atlas): New chat starts where it was,
	-- not in the project last picked on the new page.
	go(app, "go_s1")
	go(app, "go_new")
	expect(app:state("chat_project")).to_be("atlas")
	-- Asked for in a project again: that one.
	go(app, "go_home")
	expect(app:state("chat_project")).to_be("atlas")
end)

test("the new page's draft and what it carries come back after visiting a chat", function()
	local app = open()
	app:type('role:textbox "Message"', "Half a thought")
	app:command("attach_passage")
	settle(app)
	local cargo = #(app:state("chat.composer.cargo") or {})
	expect(cargo > 0).to_be(true)
	go(app, "go_s1")
	expect(app:text('role:textbox "Message"')).to_be("")
	go(app, "go_new")
	expect(app:text('role:textbox "Message"')).to_be("Half a thought")
	expect(#(app:state("chat.composer.cargo") or {})).to_be(cargo)
end)

test("the new page's settings, draft and what it carries survive a relaunch", function()
	local first = open()
	pick(first)
	local mine = shown(first)
	first:type('role:textbox "Message"', "Half a thought")
	first:command("attach_passage")
	settle(first)
	local cargo = #(first:state("chat.composer.cargo") or {})
	go(first, "go_s1")
	go(first, "go_new")

	local app = mount("chat_fixture.luau")
	app:resize(900, 700)
	go(app, "go_new")
	-- (Where is the chat last open's before the relaunch, Long chat in Atlas.)
	expect(shown(app)).to_be((mine:gsub("|notes|local|%-$", "|atlas|local|main")))
	expect(app:text('role:textbox "Message"')).to_be("Half a thought")
	expect(#(app:state("chat.composer.cargo") or {})).to_be(cargo)
end)
