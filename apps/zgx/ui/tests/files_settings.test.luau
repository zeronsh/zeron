-- Settings → Files, read where it applies: the tree leaves dotfiles and
-- ignored files out unless "Show hidden and ignored files" is on (turning
-- it lists them in place); Word wrap is the editor's wrapping as a file
-- opens, and as it changes; Autosave saves a moment after the typing
-- stops, and only then. On the files pane in ui/files_fixture.luau (its
-- disk ui/files_stub.luau).

local EDITOR = 'role:textbox "long.txt"'

local function settle(app: any)
	app:frames(30)
end

-- Time passing for the autosave's timer. (A test's time is its frames',
-- 1/60 s each, which a wait moves on too: `settle`'s 30 frames are half a
-- second of it.)
local function wait_ms(app: any, ms: number)
	app:wait('"never shown, only waited past"', { timeout_ms = ms })
end

local function calls(app: any): string
	return app:state("test_files_calls") or ""
end

local function open_pane(): any
	local app = mount("files_fixture.luau")
	app:resize(900, 600)
	settle(app)
	expect(app:wait('role:treeitem "src"', { timeout_ms = 15000 })).to_be(true)
	return app
end

local function open_long(app: any)
	app:click('role:treeitem "long.txt"')
	expect(app:wait(EDITOR, { timeout_ms = 15000 })).to_be(true)
	settle(app)
end

test("the tree leaves dotfiles and ignored files out, and lists them when asked, in place", function()
	local app = open_pane()
	app:command("disk_hidden_files")
	-- (Heard as a watch event: the folder read again.)
	local _, before = (calls(app) .. ","):gsub("list:,", "")
	for _ = 1, 100 do
		local _, now = (calls(app) .. ","):gsub("list:,", "")
		if now > before then
			break
		end
		wait_ms(app, 50)
	end
	settle(app)
	expect(app:wait('role:treeitem "zeta.txt"', { timeout_ms = 15000 })).to_be(true)
	expect(app:has('role:treeitem ".env"')).to_be(false)
	expect(app:has('role:treeitem "build"')).to_be(false)
	local zeta = app:tree('role:treeitem "zeta.txt"'):match("@(%d+)")
	-- (The folder's new read landed, filtered.)
	wait_ms(app, 300)
	settle(app)
	expect(app:has('role:treeitem ".env"')).to_be(false)
	app:set_state("settings_files_show_all", true)
	expect(app:wait('role:treeitem ".env"', { timeout_ms = 3000 })).to_be(true)
	expect(app:has('role:treeitem "build"')).to_be(true)
	-- (Listed again from what was read: nothing read again, the rest kept.)
	expect(app:tree('role:treeitem "zeta.txt"'):match("@(%d+)")).to_be(zeta)
	app:set_state("settings_files_show_all", false)
	expect(app:wait('role:treeitem ".env"', { gone = true, timeout_ms = 3000 })).to_be(true)
end)

test("Word wrap is how a file opens, and the editor follows it as it changes", function()
	local app = open_pane()
	app:set_state("settings_files_word_wrap", true)
	open_long(app)
	expect(app:tree('role:checkbox "Wrap Lines"')).never.to_contain("unchecked")
	app:set_state("settings_files_word_wrap", false)
	settle(app)
	expect(app:tree('role:checkbox "Wrap Lines"')).to_contain("unchecked")
end)

test("Autosave saves a moment after the typing stops; off, nothing saves on its own", function()
	local app = open_pane()
	open_long(app)
	app:type(EDITOR, "x")
	settle(app)
	wait_ms(app, 500)
	expect(calls(app)).never.to_contain("write:long.txt")

	app:set_state("settings_files_autosave", true)
	app:set_state("settings_files_autosave_delay", 300)
	app:type(EDITOR, "y")
	-- (A few frames, well short of the 300 ms delay.)
	app:frames(5)
	-- Not before the delay…
	expect(calls(app)).never.to_contain("write:long.txt")
	-- …and once it's past.
	local saved = false
	for _ = 1, 40 do
		wait_ms(app, 50)
		if string.find(calls(app), "write:long.txt", 1, true) then
			saved = true
			break
		end
	end
	expect(saved).to_be(true)
end)
