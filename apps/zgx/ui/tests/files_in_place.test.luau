-- The session page's files pane follows the disk in place: a file made or
-- deleted on disk reaches the tree as a row, the folders open and the
-- scroll where they were, every other row the same node; a save leaves the
-- editor as it was, caret and scroll; a file changed under unsaved edits,
-- or a save that finds it changed, asks which to keep, the edits kept; and
-- another session in the same folder keeps the tree, one in another folder
-- shows that folder's. On the pane in ui/files_fixture.luau, its host's
-- calls stubbed over a disk made up there.

-- A node's identity, by its kind and label (or text): `@id`.
local function node(app: any, kind: string?, label: string, scope: string?): string?
	local pattern = if kind then ` {kind} "{label}"` else ` "{label}"`
	for line in app:tree(scope):gmatch("[^\n]+") do
		if line:find(pattern, 1, true) then
			return line:match("@(%d+)")
		end
	end
	return nil
end

local function settle(app: any)
	app:frames(30)
end

-- Whether `ok()` comes true within `ms` milliseconds of real time (not
-- frames: a loaded machine takes longer between them), asking as time
-- passes. What follows a reload (reads, a draft taken up, a place gone
-- back to) is waited for this way, never assumed.
local function eventually(app: any, ok: () -> boolean, ms: number?): boolean
	local tries = math.ceil((ms or 15000) / 50)
	for _ = 1, tries do
		if ok() then
			return true
		end
		app:wait('"never shown, only waited past"', { timeout_ms = 50 })
	end
	return ok()
end

local function open_pane(): any
	local app = mount("files_fixture.luau")
	app:resize(900, 600)
	settle(app)
	expect(app:error()).to_be_nil()
	expect(app:wait('role:treeitem "src"', { timeout_ms = 15000 })).to_be(true)
	return app
end

local function calls(app: any): string
	return app:state("test_files_calls") or ""
end

test("folders are read as they open, not before", function()
	local app = open_pane()
	expect(calls(app)).to_contain("list:")
	expect(calls(app)).never.to_contain("list:src")
	app:click('role:treeitem "src"')
	expect(app:wait('role:treeitem "a01.txt"', { timeout_ms = 15000 })).to_be(true)
	expect(calls(app)).to_contain("list:src")
	expect(calls(app)).never.to_contain("list:src/deep")
	expect(calls(app)).to_contain("watch:")
end)

-- Where a row is, from the top of the window: its `y`.
local function row_y(app: any, name: string): string?
	-- (The tree's own: the whole window's is cut short past a budget, and
	-- an open editor's gutter spends it.)
	return app:tree("role:tree"):match(`treeitem "{name}" %d+,(%-?%d+) `)
end

test("a file made or deleted on disk reaches the tree in place", function()
	local app = open_pane()
	app:click('role:treeitem "src"')
	expect(app:wait('role:treeitem "a01.txt"', { timeout_ms = 15000 })).to_be(true)
	settle(app)
	app:click('role:treeitem "deep"')
	expect(app:wait('role:treeitem "d01.txt"', { timeout_ms = 15000 })).to_be(true)
	settle(app)
	-- Scrolled past most of deep's files: its last ones and src's first.
	app:scroll('role:treeitem "d05.txt"', 520)
	settle(app)
	local kept = node(app, "treeitem", "d25.txt")
	local below = node(app, "treeitem", "a06.txt")
	local deep = node(app, "treeitem", "deep")
	local y = row_y(app, "d25.txt")
	expect(kept).never.to_be_nil()
	expect(below).never.to_be_nil()

	app:command("disk_add")
	expect(app:wait('role:treeitem "a05b.txt"', { timeout_ms = 15000 })).to_be(true)
	settle(app)
	-- The rows that were there are the same nodes, where they were; the
	-- folders still open.
	expect(`d25 @{node(app, "treeitem", "d25.txt")}`).to_be(`d25 @{kept}`)
	expect(`a06 @{node(app, "treeitem", "a06.txt")}`).to_be(`a06 @{below}`)
	expect(`deep @{node(app, "treeitem", "deep")}`).to_be(`deep @{deep}`)
	expect(row_y(app, "d25.txt")).to_be(y)
	-- Only the folder that changed was read again.
	expect(calls(app)).never.to_contain("list:src/deep,list:src/deep")

	app:command("disk_remove")
	expect(app:wait('role:treeitem "a03.txt"', { gone = true, timeout_ms = 15000 })).to_be(true)
	settle(app)
	expect(`d25 @{node(app, "treeitem", "d25.txt")}`).to_be(`d25 @{kept}`)
	expect(`a06 @{node(app, "treeitem", "a06.txt")}`).to_be(`a06 @{below}`)
	expect(row_y(app, "d25.txt")).to_be(y)
	expect(app:has('role:treeitem "a05b.txt"')).to_be(true)
end)

-- The editor: one textbox holding a window of the file, its gutter's
-- numbers beside it. The line at the top of the view is the lowest number
-- showing.
local EDITOR = 'role:textbox "long.txt"'
local function top_line(app: any): string?
	local best, best_y = nil, math.huge
	for line in app:tree("#file-viewer"):gmatch("[^\n]+") do
		local n, y = line:match('@%d+ "(%d+)" %d+,(%-?%d+) ')
		local at = if y then tonumber(y) else nil
		if n and at and not line:find("hidden", 1, true) and at >= 40 and at < best_y then
			best, best_y = n, at
		end
	end
	return best
end
local function editor_text(app: any): string
	return app:text(EDITOR)
end
local function caret(app: any): string
	local from, to = app:selection(EDITOR)
	return `{from}..{to}`
end
-- Where the caret is in the file, as the status bar says it ("Ln 24, Col 35"):
-- the textbox's own selection counts from its window, which a new editor
-- may start elsewhere.
local function caret_in_file(app: any): string?
	if not app:has('~"Ln "') then
		return nil
	end
	return string.match(app:text('~"Ln "'), "Ln %d+, Col %d+")
end

local function open_long(app: any)
	app:click('role:treeitem "long.txt"')
	expect(app:wait(EDITOR, { timeout_ms = 15000 })).to_be(true)
	settle(app)
end

-- Scrolled into the file, a letter typed where the view's middle is.
local function type_in(app: any, text: string)
	app:scroll('"10"', 1200)
	settle(app)
	app:type(EDITOR, text)
	app:frames(5)
end

test("a save keeps the editor's caret, scroll and nodes", function()
	local app = open_pane()
	open_long(app)
	type_in(app, "x")
	local before, top = caret(app), top_line(app)
	local editor = node(app, "textbox", "long.txt")
	expect(top).never.to_be("1")
	expect(app:has('"Unsaved changes"')).to_be(true)

	app:key("cmd+s")
	expect(eventually(app, function(): boolean
		return string.find(calls(app), "write:long.txt", 1, true) ~= nil and not app:has('"Unsaved changes"')
	end)).to_be(true)
	expect(app:has('"Changed on disk"')).to_be(false)
	expect(caret(app)).to_be(before)
	expect(top_line(app)).to_be(top)
	expect(`editor @{node(app, "textbox", "long.txt")}`).to_be(`editor @{editor}`)
	-- Typing on goes where the caret was.
	app:type(EDITOR, "y")
	app:frames(5)
	expect(editor_text(app)).to_contain("xy")
end)

test("a file changed on disk under unsaved edits asks which to keep", function()
	local app = open_pane()
	open_long(app)
	type_in(app, "Q")
	app:command("disk_edit")
	expect(app:wait('"Changed on disk"', { timeout_ms = 15000 })).to_be(true)
	-- Mine stays until chosen.
	expect(editor_text(app)).to_contain("Q")
	app:click('role:button "Reload"')
	expect(eventually(app, function(): boolean
		return not app:has('"Changed on disk"')
	end)).to_be(true)
	expect(editor_text(app)).never.to_contain("Q")
end)

test("a save that finds the file changed keeps the edits and asks", function()
	local app = open_pane()
	open_long(app)
	type_in(app, "Q")
	app:command("disk_edit_unheard")
	app:key("cmd+s")
	expect(app:wait('"Changed on disk"', { timeout_ms = 15000 })).to_be(true)
	expect(editor_text(app)).to_contain("Q")
	-- Keeping mine and saving again writes over it.
	app:click('role:button "Keep Mine"')
	settle(app)
	expect(app:has('"Unsaved changes"')).to_be(true)
	app:type(EDITOR, "R")
	app:frames(5)
	app:key("cmd+s")
	expect(eventually(app, function(): boolean
		local _, n = calls(app):gsub("write:long.txt", "")
		return n == 2
	end)).to_be(true)
	local _, writes = calls(app):gsub("write:long.txt", "")
	expect(writes).to_be(2)
	expect(app:has('"Changed on disk"')).to_be(false)
	expect(app:has('"Unsaved changes"')).to_be(false)
end)

test("another session in the folder keeps the tree; another folder shows its own", function()
	local app = open_pane()
	app:click('role:treeitem "src"')
	expect(app:wait('role:treeitem "a01.txt"', { timeout_ms = 15000 })).to_be(true)
	local pane = node(app, "treeitem", "src")
	local row = node(app, "treeitem", "a01.txt")

	app:set_state("test_files_session", { id = "chat-2", cwd = "/proj" })
	settle(app)
	expect(`src @{node(app, "treeitem", "src")}`).to_be(`src @{pane}`)
	expect(`a01 @{node(app, "treeitem", "a01.txt")}`).to_be(`a01 @{row}`)

	app:set_state("test_files_session", { id = "chat-3", cwd = "/other" })
	expect(app:wait('role:treeitem "readme.txt"', { timeout_ms = 15000 })).to_be(true)
	expect(app:has('role:treeitem "src"')).to_be(false)

	-- Back: the folder as it was left, src open.
	app:set_state("test_files_session", { id = "chat-1", cwd = "/proj" })
	expect(app:wait('role:treeitem "src"', { timeout_ms = 15000 })).to_be(true)
	expect(app:wait('role:treeitem "a01.txt"', { timeout_ms = 15000 })).to_be(true)
end)

test("unsaved edits outlive a visit to another folder", function()
	local app = open_pane()
	open_long(app)
	type_in(app, "Q")
	expect(app:has('"Unsaved changes"')).to_be(true)
	app:set_state("test_files_session", { id = "chat-3", cwd = "/other" })
	expect(app:wait('role:treeitem "readme.txt"', { timeout_ms = 15000 })).to_be(true)
	app:set_state("test_files_session", { id = "chat-1", cwd = "/proj" })
	expect(eventually(app, function(): boolean
		return app:has(EDITOR) and app:has('"Unsaved changes"')
	end)).to_be(true)
	-- (Back where it was typed: the editor's place is kept too.)
	expect(eventually(app, function(): boolean
		return string.find(editor_text(app), "Q", 1, true) ~= nil
	end)).to_be(true)
	expect(calls(app)).never.to_contain("write:")
end)

test("a reload brings the pane back as it was, unsaved edits and all", function()
	local app = open_pane()
	open_long(app)
	type_in(app, "Q")
	expect(app:has('"Unsaved changes"')).to_be(true)
	app:click('role:treeitem "src"')
	expect(app:wait('role:treeitem "a01.txt"', { timeout_ms = 15000 })).to_be(true)
	settle(app)
	app:scroll('role:treeitem "a05.txt"', 260)
	settle(app)
	local tree_y = row_y(app, "a20.txt")
	expect(tree_y).never.to_be_nil()
	local at, top = caret_in_file(app), top_line(app)
	expect(at).never.to_be_nil()
	-- At once: no pause for the edit to be kept on its own.
	app:set_state("test_files_generation", 2)
	-- The pane made again: its folder read again, its rows back where they
	-- were; the edit taken up again; the editor where it stood. Each waited
	-- for until it holds.
	expect(eventually(app, function(): boolean
		return row_y(app, "a20.txt") == tree_y
	end)).to_be(true)
	expect(eventually(app, function(): boolean
		return app:has(EDITOR) and app:has('"Unsaved changes"')
	end)).to_be(true)
	expect(eventually(app, function(): boolean
		return top_line(app) == top and caret_in_file(app) == at
	end)).to_be(true)
	expect(editor_text(app)).to_contain("Q")
	expect(calls(app)).never.to_contain("write:")
	-- And it saves as before (the header's Save: nothing has focus in a
	-- pane just made).
	app:click('role:button "Save"')
	expect(eventually(app, function(): boolean
		return string.find(calls(app), "write:long.txt", 1, true) ~= nil and not app:has('"Unsaved changes"')
	end)).to_be(true)
end)

test("deleting asks first, and says it's for good", function()
	local app = open_pane()
	app:click('role:treeitem "zeta.txt"')
	app:frames(5)
	app:key("cmd+backspace")
	expect(app:wait('"Delete for good?"', { timeout_ms = 15000 })).to_be(true)
	expect(calls(app)).never.to_contain("delete:")
	app:click('role:button "Delete"')
	expect(eventually(app, function(): boolean
		return string.find(calls(app), "delete:zeta.txt", 1, true) ~= nil
	end)).to_be(true)
	expect(app:has('role:treeitem "zeta.txt"')).to_be(false)
end)

test("the filter finds files in folders not read yet", function()
	local app = open_pane()
	app:type('role:textbox', "d07")
	expect(app:wait('role:treeitem "d07.txt"', { timeout_ms = 15000 })).to_be(true)
	expect(calls(app)).to_contain("search:d07")
end)

test("a file deleted on disk under unsaved edits keeps them, and Save makes it again", function()
	local app = open_pane()
	open_long(app)
	type_in(app, "Q")
	app:command("disk_delete")
	expect(app:wait('"Deleted on disk"', { timeout_ms = 15000 })).to_be(true)
	expect(editor_text(app)).to_contain("Q")
	expect(app:has('"Unsaved changes"')).to_be(true)
	app:click('role:button "Save"[2]')
	expect(app:wait('"Deleted on disk"', { gone = true, timeout_ms = 15000 })).to_be(true)
	expect(eventually(app, function(): boolean
		return string.find(calls(app), "write:long.txt", 1, true) ~= nil
	end)).to_be(true)
	expect(calls(app)).to_contain("create:long.txt")
	expect(app:has('"Unsaved changes"')).to_be(false)
	expect(editor_text(app)).to_contain("Q")
end)

test("Discard lets a deleted file's edits go", function()
	local app = open_pane()
	open_long(app)
	type_in(app, "Q")
	app:command("disk_delete")
	expect(app:wait('"Deleted on disk"', { timeout_ms = 15000 })).to_be(true)
	app:click('role:button "Discard"')
	expect(app:wait(EDITOR, { gone = true, timeout_ms = 15000 })).to_be(true)
	expect(calls(app)).never.to_contain("write:")
	expect(calls(app)).never.to_contain("create:")
end)

test("a file made goes through the host and shows in its folder", function()
	local app = open_pane()
	app:click('role:treeitem "src"')
	expect(app:wait('role:treeitem "a01.txt"', { timeout_ms = 15000 })).to_be(true)
	app:command("make_file")
	expect(app:wait('role:treeitem "0-made.txt"', { timeout_ms = 15000 })).to_be(true)
	expect(calls(app)).to_contain("create:src/0-made.txt")
end)

test("a row's menu reveals the file here, and carries plugins' items", function()
	local app = open_pane()
	app:command("plugin_item")
	app:click('role:treeitem "zeta.txt"')
	app:frames(5)
	app:key("shift+F10")
	expect(app:wait('"Reveal in Finder"', { timeout_ms = 15000 })).to_be(true)
	expect(app:has('"Line Counter"')).to_be(true)
	expect(app:has('"Count Lines"')).to_be(true)
	-- (A menu takes no pick in its first moments, and runs one a moment
	-- after: time passes, not only frames.)
	app:wait('"nothing"', { timeout_ms = 400 })
	app:click('"Reveal in Finder"')
	expect(eventually(app, function(): boolean
		return string.find(calls(app), "reveal:zeta.txt", 1, true) ~= nil
	end)).to_be(true)
end)

test("a row's menu adds the file, or each of the selection, to the chat", function()
	local app = open_pane()
	app:click('role:treeitem "zeta.txt"')
	app:frames(5)
	app:key("shift+F10")
	expect(app:wait('"Add to Chat"', { timeout_ms = 15000 })).to_be(true)
	-- (A menu takes no pick in its first moments.)
	app:wait('"nothing"', { timeout_ms = 400 })
	app:click('"Add to Chat"')
	expect(eventually(app, function(): boolean
		return string.find(calls(app), "add:zeta.txt@/proj", 1, true) ~= nil
	end)).to_be(true)
	-- A selection of two: both, at once.
	app:key("shift+Down")
	app:frames(5)
	app:key("shift+F10")
	expect(app:wait('"Add to Chat"', { timeout_ms = 15000 })).to_be(true)
	app:wait('"nothing"', { timeout_ms = 400 })
	app:click('"Add to Chat"')
	expect(eventually(app, function(): boolean
		return string.find(calls(app), "add:zeta.txt+zwide.txt@/proj", 1, true) ~= nil
	end)).to_be(true)
	-- And a folder (the shipping app offers it on folders too).
	app:click('role:treeitem "src"')
	app:frames(5)
	app:key("shift+F10")
	expect(app:wait('"Add to Chat"', { timeout_ms = 15000 })).to_be(true)
	app:wait('"nothing"', { timeout_ms = 400 })
	app:click('"Add to Chat"')
	expect(eventually(app, function(): boolean
		return string.find(calls(app), "add:src@/proj", 1, true) ~= nil
	end)).to_be(true)
end)

test("a picture on another device shows from its bytes, read from there", function()
	local app = open_pane()
	app:command("disk_remote_images")
	expect(app:wait('role:treeitem "shot.png"', { timeout_ms = 15000 })).to_be(true)
	app:click('role:treeitem "shot.png"')
	expect(eventually(app, function(): boolean
		return string.find(calls(app), "image:shot.png@c1", 1, true) ~= nil
	end)).to_be(true)
	expect(app:wait('role:img "shot.png"', { timeout_ms = 15000 })).to_be(true)
	-- One that can't come says why.
	app:click('role:treeitem "away.png"')
	expect(app:wait('"The device is offline."', { timeout_ms = 15000 })).to_be(true)
	expect(app:has('role:img "away.png"')).to_be(false)
end)

test("a Markdown preview shows its pictures, read from the device, and follows links to files", function()
	local app = open_pane()
	app:set_state("test_files_away", true)
	app:command("disk_md_links")
	expect(app:wait('role:treeitem "docs"', { timeout_ms = 15000 })).to_be(true)
	app:click('role:treeitem "docs"')
	expect(app:wait('role:treeitem "guide.md"', { timeout_ms = 15000 })).to_be(true)
	-- (Its row is there as the folder starts to open: opened, then clicked.)
	app:frames(30)
	app:click('role:treeitem "guide.md"')
	expect(app:wait('"Guide"', { timeout_ms = 15000 })).to_be(true)
	-- The picture, beside the document, read from where it is.
	expect(eventually(app, function(): boolean
		return string.find(calls(app), "image:shot.png@c1", 1, true) ~= nil
	end)).to_be(true)
	expect(app:wait('role:button "View shot"', { timeout_ms = 15000 })).to_be(true)
	-- A link to a file of the folder opens it. (Under the picture: scrolled
	-- to.)
	app:scroll('"Guide"', 2000)
	app:frames(10)
	app:click('"the notes"')
	expect(eventually(app, function(): boolean
		return string.find(calls(app), "read:notes.md", 1, true) ~= nil
	end)).to_be(true)
	expect(app:wait('"Notes"', { timeout_ms = 15000 })).to_be(true)
end)

test("a Markdown preview's task boxes tick the file's own tasks", function()
	local app = open_pane()
	-- (Its picture read through the stub: the made-up disk isn't there.)
	app:set_state("test_files_away", true)
	app:command("disk_md_links")
	expect(app:wait('role:treeitem "docs"', { timeout_ms = 15000 })).to_be(true)
	app:click('role:treeitem "docs"')
	expect(app:wait('role:treeitem "guide.md"', { timeout_ms = 15000 })).to_be(true)
	-- (Its row is there as the folder starts to open: opened, then clicked.)
	app:frames(30)
	app:click('role:treeitem "guide.md"')
	expect(app:wait('role:checkbox "Done"', { timeout_ms = 15000 })).to_be(true)
	expect(app:count('role:checkbox "Done"')).to_be(2)
	-- The boxes stand for the text's `[ ]` and `[x]`, not drawn as glyphs.
	expect(app:has('"☐ Write it"')).to_be(false)
	-- The first ticked: the file's first task, not the code block's.
	-- (The preview fades in: time passes before it takes a click.)
	app:wait('"never shown, only waited past"', { timeout_ms = 400 })
	app:click('role:checkbox "Done"[1]')
	app:frames(10)
	app:click('role:radio "Source"')
	local source = 'role:textbox "guide.md"'
	expect(app:wait(source, { timeout_ms = 15000 })).to_be(true)
	local text = app:text(source)
	expect(text).to_contain("- [x] Write it")
	expect(text).to_contain("- [x] Plan it")
	expect(text).to_contain("- [ ] not a task")
	expect(app:has('"Unsaved changes"') or string.find(app:tree("role:tree"), "guide.md", 1, true) ~= nil).to_be(true)
end)

test("a Markdown preview's code block turns on Fit content, one choice for all, kept", function()
	local app = open_pane()
	app:set_state("test_files_away", true)
	app:command("disk_md_links")
	expect(app:wait('role:treeitem "docs"', { timeout_ms = 15000 })).to_be(true)
	app:click('role:treeitem "docs"')
	expect(app:wait('role:treeitem "guide.md"', { timeout_ms = 15000 })).to_be(true)
	-- (Its row is there as the folder starts to open: opened, then clicked.)
	app:frames(30)
	app:click('role:treeitem "guide.md"')
	expect(app:wait('role:checkbox "Fit content"', { timeout_ms = 15000 })).to_be(true)
	expect(app:state("markdown_code_fit")).to_be(false)
	app:wait('"never shown, only waited past"', { timeout_ms = 400 })
	app:click('role:checkbox "Fit content"')
	expect(eventually(app, function(): boolean
		return app:state("markdown_code_fit") == true
	end)).to_be(true)
	expect(app:tree('role:checkbox "Fit content"')).to_contain("checked")
end)

test("a comment left on a line is staged for the chat and stands under it until deleted", function()
	local app = open_pane()
	open_long(app)
	-- The caret on line 3, then the editor's menu: Add a Comment.
	app:click(EDITOR)
	app:key("cmd+up")
	app:key("down")
	app:key("down")
	app:frames(4)
	app:key("shift+F10")
	expect(app:wait('"Add a Comment"', { timeout_ms = 15000 })).to_be(true)
	app:wait('"never shown, only waited past"', { timeout_ms = 400 })
	app:click('"Add a Comment"')
	expect(app:wait('"long.txt:3"', { timeout_ms = 15000 })).to_be(true)
	app:type('role:textbox "Comment"', "Why twice?")
	app:key("cmd+enter")
	expect(eventually(app, function(): boolean
		return string.find(calls(app), "comment:long.txt:3:Why twice?", 1, true) ~= nil
	end)).to_be(true)
	-- It stands on its line (sent with the next message): your name and
	-- what you said, no time and no "Pending".
	expect(app:wait('role:article "You: Why twice?"', { timeout_ms = 15000 })).to_be(true)
	expect(app:has('"Pending"')).to_be(false)
	expect(app:has('role:article "You: Why twice?" >> "just now"')).to_be(false)
	expect(app:has('"long.txt:3"')).to_be(false)
	-- Deleted from its card: unstaged, gone.
	app:hover('role:article "You: Why twice?"')
	app:frames(4)
	app:click('role:article "You: Why twice?" >> role:button "Delete"')
	expect(eventually(app, function(): boolean
		return string.find(calls(app), "comment-remove:comment-1", 1, true) ~= nil
	end)).to_be(true)
	expect(app:wait('role:article "You: Why twice?"', { gone = true, timeout_ms = 15000 })).to_be(true)
end)

test("a folder's unsaved edits, and the folder shown, outlast many folders opened after them", function()
	local app = open_pane()
	app:command("many_folders")
	expect(eventually(app, function(): boolean
		return app:state("test_files_kept") ~= nil and app:state("test_files_kept") ~= ""
	end)).to_be(true)
	expect(app:state("test_files_kept")).to_be("edited,shared:0,pane:true")
	-- The pane's own folder, shown all along, still lists its files.
	expect(app:has('role:treeitem "long.txt"')).to_be(true)
end)

test("quick ⌘+ presses each take a zoom step, from where the zoom is going", function()
	local app = open_pane()
	app:command("disk_remote_images")
	expect(app:wait('role:treeitem "shot.png"', { timeout_ms = 15000 })).to_be(true)
	app:click('role:treeitem "shot.png"')
	expect(app:wait('role:img "shot.png"', { timeout_ms = 15000 })).to_be(true)
	app:wait('"never shown, only waited past"', { timeout_ms = 400 })
	app:click('role:img "shot.png"')
	app:key("cmd+1")
	expect(eventually(app, function(): boolean
		return app:has('"100%"')
	end)).to_be(true)
	app:wait('"never shown, only waited past"', { timeout_ms = 400 })
	-- Two at once: two steps (125%, then 150%), not the first one twice.
	app:key("cmd+=")
	app:key("cmd+=")
	expect(eventually(app, function(): boolean
		return app:has('"150%"')
	end, 3000)).to_be(true)
end)

test("a reload reads nothing again: the tree and the file show from what was kept", function()
	local app = open_pane()
	open_long(app)
	app:click('role:treeitem "src"')
	expect(app:wait('role:treeitem "a01.txt"', { timeout_ms = 15000 })).to_be(true)
	expect(eventually(app, function(): boolean
		return string.find(calls(app), "read:long.txt", 1, true) ~= nil
	end)).to_be(true)
	-- (Settled: opening a file scrolls the tree to it, which takes a moment.)
	local settled_at = ""
	expect(eventually(app, function(): boolean
		local now = app:tree("role:tree")
		local same = now == settled_at
		settled_at = now
		return same
	end)).to_be(true)
	-- The rows that show now.
	local shown: { string } = {}
	for name, y in app:tree("role:tree"):gmatch('treeitem "([^"]+)" %d+,(%-?%d+)') do
		-- (Files in view: a folder may stand in the stack at the top
		-- instead, and rows past the view's foot are built as needed.)
		local top = tonumber(y) or -1
		-- (The upper part of the view: on its first frame a list builds rows
		-- for the height it expects, settling its foot a frame later.)
		if string.find(name, ".", 1, true) and top >= 40 and top + 26 <= 400 then
			table.insert(shown, name)
		end
	end
	expect(#shown > 10).to_be(true)
	local before = calls(app)
	app:set_state("test_files_generation", 2)
	-- Its first frame: the same rows, and the file's text, already there.
	app:frames(1)
	for _, name in shown do
		expect(`{name} {app:has(`role:treeitem "{name}"`)}`).to_be(`{name} true`)
	end
	expect(app:has(EDITOR)).to_be(true)
	expect(editor_text(app)).to_contain("long line 1")
	-- And after a while, nothing listed or read: only the watch asked for
	-- again, and found kept.
	app:wait('"never shown, only waited past"', { timeout_ms = 500 })
	local since = string.sub(calls(app), #before + 1)
	expect(since).never.to_contain("list:")
	expect(since).never.to_contain("read:")
	expect(since).to_contain("watch:")
end)

test("a change on disk while the pane is made again still arrives", function()
	local app = open_pane()
	app:click('role:treeitem "src"')
	expect(app:wait('role:treeitem "a01.txt"', { timeout_ms = 15000 })).to_be(true)
	app:set_state("test_files_generation", 2)
	app:command("disk_add")
	expect(app:wait('role:treeitem "a05b.txt"', { timeout_ms = 15000 })).to_be(true)
end)

test("width changes rebuild nothing: the pane's nodes stay, no line is coloured again", function()
	local app = open_pane()
	app:click('role:treeitem "code.luau"')
	expect(app:wait('role:textbox "code.luau"', { timeout_ms = 15000 })).to_be(true)
	app:wait('"never shown, only waited past"', { timeout_ms = 400 })
	local editor = node(app, "textbox", "code.luau")
	local name = node(app, "text", "code.luau", "#file-viewer")
	local row = node(app, "treeitem", "zeta.txt")
	local toggle = node(app, "button", "Toggle File Tree")
	expect(editor).never.to_be_nil()
	expect(row).never.to_be_nil()
	app:command("count_highlights")
	local before = app:state("test_files_highlights")
	expect(before > 0).to_be(true)
	-- A drag's worth of widths, the tree beside the file throughout.
	for _, w in { 880, 850, 820, 790, 760, 730, 700, 730, 780, 840, 900 } do
		app:resize(w, 600)
		app:frames(1)
	end
	settle(app)
	expect(`editor @{node(app, "textbox", "code.luau")}`).to_be(`editor @{editor}`)
	expect(`name @{node(app, "text", "code.luau", "#file-viewer")}`).to_be(`name @{name}`)
	expect(`row @{node(app, "treeitem", "zeta.txt")}`).to_be(`row @{row}`)
	expect(`toggle @{node(app, "button", "Toggle File Tree")}`).to_be(`toggle @{toggle}`)
	app:command("count_highlights")
	expect(app:state("test_files_highlights") - before).to_be(0)
end)

-- The pane as laid out, its node ids left out: what a reload must not
-- change, frame by frame.
-- (Read in parts, each its own dump: a dump of the whole pane is cut off
-- after a budget of nodes, and where it cuts follows how many there are
-- (an image still loading adds one), not what shows; a part cut off fails.)
-- (The file's editor holds a window of lines far taller than the view, its
-- gutter's numbers with it: its frame says where it stands.)
local LAYOUT_PARTS = {
	"#file-viewer-head",
	"#file-viewer >> role:alert",
	"#code-editor",
	"#code-viewer",
	"#files-tree-beside",
	"#files-tree-float",
	'role:button "Toggle File Tree"',
}

local layout_lines: (dump: string) -> string

local function layout_of(app: any): string
	local out: { string } = {}
	-- (No file open: the viewer's own words, all of it, small.)
	local parts = table.clone(LAYOUT_PARTS)
	if not app:has("#code-editor") and not app:has("#code-viewer") then
		table.insert(parts, 1, "#file-viewer")
	end
	for _, part in parts do
		if app:has(part) then
			local dump = app:tree(part)
			assert(not dump:find("more (narrow with", 1, true), `{part}'s dump was cut off`)
			table.insert(out, `[{part}]`)
			table.insert(out, layout_lines(dump))
		end
	end
	return table.concat(out, "\n")
end

-- A dump's lines that count: where things are and whether they show.
function layout_lines(dump: string): string
	local out: { string } = {}
	local skip_deeper: number? = nil
	for line in dump:gmatch("[^\n]+") do
		local indent = #(line:match("^%s*") or "")
		if skip_deeper and indent > skip_deeper then
			continue
		end
		skip_deeper = nil
		-- (The inspector's cut-off note: where it cuts follows how many
		-- nodes there are, not what shows.)
		if line:find("more (narrow with", 1, true) then
			continue
		end
		-- (A tooltip comes by the pointer resting, on the clock, reload or
		-- not: left out with what it holds.)
		if line:find('dialog "Tooltip"', 1, true) then
			skip_deeper = indent
			continue
		end
		-- (Nor what doesn't show: a closed drawer's rows, hidden off the
		-- edge, may be built a frame later under load; nobody sees them.)
		if line:find(" hidden$") then
			skip_deeper = indent
			continue
		end
		-- (Focus aside: a node made again doesn't hold it; this is about
		-- where things are and whether they show.)
		local plain = line:gsub("@%d+ ", ""):gsub(" focused", "")
		-- The editor: its frame, not the window of lines it holds (one made
		-- again builds that window round the same place, spread
		-- differently out of sight); the gutter's numbers say which show.
		if plain:find("#code-editor textbox", 1, true) then
			-- (Its height is the window's, which follows the room measured.)
			plain = plain:gsub(' ="[^"]*"', ""):gsub("(%-?%d+,%-?%d+ %d+)x%d+", "%1")
			skip_deeper = indent
		end
		-- And only what's in the window.
		local y, h = plain:match(" %-?%d+,(%-?%d+) %d+x(%d+)")
		local top, tall = tonumber(y), tonumber(h)
		if not top or (top + (tall or 0) > 0 and top < 600) then
			table.insert(out, plain)
		end
	end
	return table.concat(out, "\n")
end

local function first_difference(a: string, b: string): string
	local la, lb = {}, {}
	for l in a:gmatch("[^\n]+") do
		table.insert(la, l)
	end
	for l in b:gmatch("[^\n]+") do
		table.insert(lb, l)
	end
	for i = 1, math.max(#la, #lb) do
		if la[i] ~= lb[i] then
			return `line {i}: {la[i] or "(none)"}  →  {lb[i] or "(none)"}`
		end
	end
	return ""
end

-- Settled, made again: every frame after shows the pane exactly as it
-- was, nothing sliding, fading, opening or closing.
local function reload_holds_still(app: any, generation: number)
	-- Settled first: two looks 200 ms apart the same (under load a row or
	-- a file may still be arriving; 15 s at most).
	local before = layout_of(app)
	for _ = 1, 75 do
		app:wait('"never shown, only waited past"', { timeout_ms = 200 })
		local again = layout_of(app)
		if again == before then
			break
		end
		before = again
	end
	app:set_state("test_files_generation", generation)
	-- From the first frame, exactly as it was: nothing missing, nothing
	-- anywhere it wasn't, nothing sliding, fading, opening or closing.
	for frame = 1, 15 do
		app:frames(1)
		local now = layout_of(app)
		expect(`frame {frame}: {first_difference(before, now)}`).to_be(`frame {frame}: `)
	end
	app:wait('"never shown, only waited past"', { timeout_ms = 400 })
	expect(first_difference(before, layout_of(app))).to_be("")
end

test("a reload keeps the narrow pane's drawer closed, still", function()
	local app = open_pane()
	app:resize(560, 600)
	open_long(app)
	reload_holds_still(app, 2)
end)

test("a reload keeps the wide pane, tree beside the file, still", function()
	local app = open_pane()
	open_long(app)
	reload_holds_still(app, 2)
end)

test("a reload keeps a conflict's question, the edits and the banner, still", function()
	local app = open_pane()
	open_long(app)
	type_in(app, "Q")
	app:command("disk_edit_unheard")
	app:key("cmd+s")
	expect(app:wait('"Changed on disk"', { timeout_ms = 15000 })).to_be(true)
	reload_holds_still(app, 2)
	expect(app:has('"Changed on disk"')).to_be(true)
	expect(editor_text(app)).to_contain("Q")
	-- And it's still answered as before.
	app:click('role:button "Keep Mine"')
	expect(eventually(app, function(): boolean
		return app:has('"Unsaved changes"')
	end)).to_be(true)
end)

test("a reload keeps a deleted file's banner and its edits, still", function()
	local app = open_pane()
	open_long(app)
	type_in(app, "Q")
	app:command("disk_delete")
	expect(app:wait('"Deleted on disk"', { timeout_ms = 15000 })).to_be(true)
	reload_holds_still(app, 2)
	expect(app:has('"Deleted on disk"')).to_be(true)
	expect(editor_text(app)).to_contain("Q")
end)

-- Scrolled, the caret left at the file's start: made again, the editor
-- shows the same lines from the first frame. (Its new window's text once
-- put the textarea's caret at its end, and the view followed that caret
-- ~40 lines down; ds/code_editor tells the selection again.)
test("a reload keeps a scrolled file on the lines it showed", function()
	local app = open_pane()
	open_long(app)
	app:scroll('"10"', 1200)
	settle(app)
	-- (The place is told once a scroll rests a moment.)
	app:wait('"never shown, only waited past"', { timeout_ms = 400 })
	local top = top_line(app)
	expect(top).never.to_be_nil()
	expect(top).never.to_be("1")
	app:set_state("test_files_generation", 2)
	for frame = 1, 15 do
		app:frames(1)
		expect(`frame {frame}: {top_line(app)}`).to_be(`frame {frame}: {top}`)
	end
	app:wait('"never shown, only waited past"', { timeout_ms = 400 })
	expect(top_line(app)).to_be(top)
end)

-- The floating tree's box, from its line: x, y, w, h (nil while hidden).
local function float_box(app: any): (number?, number?, number?, number?)
	for line in app:tree("#files-pane"):gmatch("[^\n]+") do
		if line:find("#files-tree-float", 1, true) and not line:find(" hidden$") then
			local x, y, w, h = line:match(" (%-?%d+),(%-?%d+) (%d+)x(%d+)")
			return tonumber(x), tonumber(y), tonumber(w), tonumber(h)
		end
	end
	return nil, nil, nil, nil
end

test("git's statuses mark the tree's files, a folder not read yet by what's inside it, and the open file's header", function()
	local app = open_pane()
	app:set_state("files.git", { ["here:/proj"] = { ["src/a01.txt"] = "modified", ["code.luau"] = "added" } })
	-- code.luau: its letter; src (not opened yet): a mark for what's inside.
	expect(eventually(app, function(): boolean
		return app:has('role:treeitem "code.luau" >> "A"')
	end)).to_be(true)
	expect(app:has('role:treeitem "src" >> "Holds modified files"')).to_be(true)
	-- Opened: its header says it too.
	app:click('role:treeitem "code.luau"')
	expect(app:wait('role:textbox "code.luau"', { timeout_ms = 15000 })).to_be(true)
	expect(eventually(app, function(): boolean
		return app:has('#file-viewer-head >> "A"')
	end)).to_be(true)
	-- A status that changes changes the marks in place; a clean tree has none.
	app:set_state("files.git", { ["here:/proj"] = { ["code.luau"] = "modified" } })
	expect(eventually(app, function(): boolean
		return app:has('role:treeitem "code.luau" >> "M"') and app:has('#file-viewer-head >> "M"')
	end)).to_be(true)
	-- (Nothing changed inside src any more: no mark.)
	expect(app:has('role:treeitem "src" >> "Holds modified files"')).to_be(false)
	app:set_state("files.git", { ["here:/proj"] = {} })
	expect(eventually(app, function(): boolean
		return not app:has('role:treeitem "code.luau" >> "M"') and not app:has('#file-viewer-head >> "M"')
	end)).to_be(true)
end)

test("changes on disk in quick succession all reach the tree", function()
	local app = open_pane()
	app:click('role:treeitem "src"')
	expect(app:wait('role:treeitem "a01.txt"', { timeout_ms = 15000 })).to_be(true)
	settle(app)
	-- (Each frame lands while the last one's re-read is still waiting.)
	app:command("disk_burst")
	expect(eventually(app, function(): boolean
		return app:has('role:treeitem "01-new.txt"') and app:has('role:treeitem "04-new.txt"')
	end)).to_be(true)
end)


-- No file open, too little room for one beside the tree: the tree has the pane,
-- until a file is chosen. (Its width, and the words that ask for a file.)
test("with no file and no room for one, the tree takes the whole pane until a file is chosen", function()
	local app = mount("files_fixture.luau")
	local function width(target: string): number
		local w = (app:tree(target):match("[^\n]+") or ""):match(" %-?%d+,%-?%d+ (%d+)x%d+")
		return tonumber(w) or 0
	end
	app:resize(420, 700)
	app:frames(60)
	expect(app:error()).to_be_nil()
	local pane = width("#files-pane")
	expect(pane < 288 + 240).to_be(true)
	expect(math.abs(width("#files-tree-beside") - pane) <= 1).to_be(true)
	-- A file chosen: the file has the whole pane, the tree closed
	-- (tests/files_narrow.test.luau has the rest of the narrow pane).
	app:click('"notes.md"')
	app:frames(60)
	expect(math.abs(width("#file-viewer") - pane) <= 1).to_be(true)
	local x = tonumber(((app:tree("#files-tree-beside"):match("[^\n]+") or ""):match(" (%-?%d+),%-?%d+ "))) or 0
	expect(x + width("#files-tree-beside") <= 1).to_be(true)
	-- Roomy, the tree's own width beside the file's side, and what that side says.
	app:resize(1000, 700)
	app:frames(60)
	expect(width("#files-tree-beside")).to_be(288)
	expect(app:error()).to_be_nil()
end)

test("a roomy pane with no file asks for one, and offers no keys", function()
	local app = mount("files_fixture.luau")
	app:resize(1000, 700)
	app:frames(60)
	local viewer = app:tree("#file-viewer")
	expect(viewer).to_contain("Choose a file to see it here")
	for _, word in { "Filter", "Move", "Open" } do
		expect(string.find(viewer, `"{word}"`, 1, true)).to_be(nil)
	end
end)
