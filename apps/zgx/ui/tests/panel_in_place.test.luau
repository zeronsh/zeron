-- The right panel: its empty page opens each kind of tab; a file picked in
-- a file tab replaces a clean file and opens beside a dirty one; tabs keep
-- what they show across switches, sessions and a reload; closing one with
-- unsaved changes asks first. On the panel in ui/panel_fixture.luau, over
-- the made-up disk of ui/files_stub.luau.

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

local function open_panel(): any
	local app = mount("panel_fixture.luau")
	app:resize(900, 640)
	app:frames(10)
	expect(app:error()).to_be_nil()
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	return app
end

local function tab_titles(app: any): string
	if not app:has("#panel-strip") then
		return ""
	end
	local out: { string } = {}
	for name in app:tree("#panel-strip"):gmatch('tab "([^"]+)"') do
		table.insert(out, name)
	end
	return table.concat(out, ",")
end

local function open_files(app: any)
	app:click("#panel-open-files")
	expect(app:wait('role:treeitem "long.txt"', { timeout_ms = 15000 })).to_be(true)
end

-- Typed into a file's editor where its view's middle is (the editor holds
-- a window of the file, taller than the view: scrolled into it first).
local function type_into(app: any, name: string, text: string)
	app:wait('"never shown, only waited past"', { timeout_ms = 300 })
	app:scroll('"10"', 1200)
	app:frames(20)
	app:type(`role:textbox "{name}"`, text)
	app:frames(5)
end

local function pick(app: any, name: string)
	app:click(`role:treeitem "{name}"`)
	expect(app:wait(`role:textbox "{name}"`, { timeout_ms = 15000 })).to_be(true)
end

test("the empty page opens each kind of tab, and + goes back to it", function()
	local app = open_panel()
	expect(tab_titles(app)).to_be("")
	expect(app:has('role:button "New Tab"')).to_be(false)
	open_files(app)
	expect(tab_titles(app)).to_be("Files")
	app:click('role:button "New Tab"')
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	app:click("#panel-open-terminal")
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "Files,Terminal 1"
	end)).to_be(true)
end)

test("the empty page's titled rows each show their key, and the key opens it", function()
	local app = open_panel()
	expect(app:has('#panel-empty >> "Open"')).to_be(true)
	for _, k in { "B", "F", "T" } do
		expect(app:has(`#panel-empty >> "{k}"`)).to_be(true)
	end
	-- (Tab onto the page's rows: its keys work there too, as anywhere but
	-- in a text field: tests/panel_home.test.luau.)
	for _ = 1, 12 do
		if app:tree("#panel-empty"):find("focused", 1, true) then
			break
		end
		app:key("Tab")
		app:frames(2)
	end
	app:key("t")
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "Terminal 1"
	end)).to_be(true)
end)

test("a clean file is replaced; a dirty one keeps its tab and the next opens beside it", function()
	local app = open_panel()
	open_files(app)
	pick(app, "long.txt")
	expect(tab_titles(app)).to_be("long.txt")
	pick(app, "code.luau")
	expect(tab_titles(app)).to_be("code.luau")
	-- Edited: the next file opens in a tab of its own.
	type_into(app, "code.luau", "x")
	expect(app:has('#panel-strip >> "Unsaved changes"')).to_be(true)
	-- (Read out as what it is: a mark, not a stray label.)
	expect(app:has('#panel-strip >> role:img "Unsaved changes"')).to_be(true)
	app:click('role:treeitem "zeta.txt"')
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "code.luau,zeta.txt"
	end)).to_be(true)
	expect(app:wait('role:textbox "zeta.txt"', { timeout_ms = 15000 })).to_be(true)
	-- The edited one is as it was.
	app:click("#panel-strip >> role:tab \"code.luau\"")
	expect(app:wait('role:textbox "code.luau"', { timeout_ms = 15000 })).to_be(true)
	expect(app:text('role:textbox "code.luau"')).to_contain("x")
end)

test("tabs keep what they show across switches, sessions and a reload", function()
	local app = open_panel()
	open_files(app)
	pick(app, "long.txt")
	app:click('role:button "New Tab"')
	open_files(app)
	pick(app, "zeta.txt")
	expect(tab_titles(app)).to_be("long.txt,zeta.txt")
	app:click('#panel-strip >> role:tab "long.txt"')
	expect(app:wait('role:textbox "long.txt"', { timeout_ms = 15000 })).to_be(true)
	-- Another session: its own tabs (none yet); back: these.
	app:set_state("test_panel_session", { id = "chat-2", cwd = "/proj" })
	expect(app:wait("#panel-empty", { timeout_ms = 15000 })).to_be(true)
	app:set_state("test_panel_session", { id = "chat-1", cwd = "/proj" })
	expect(app:wait('role:textbox "long.txt"', { timeout_ms = 15000 })).to_be(true)
	expect(tab_titles(app)).to_be("long.txt,zeta.txt")
	-- Made again, as a reload does: the same tabs, the same one showing.
	app:set_state("test_panel_generation", 2)
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "long.txt,zeta.txt" and app:has('role:textbox "long.txt"')
	end)).to_be(true)
	app:click('#panel-strip >> role:tab "zeta.txt"')
	expect(app:wait('role:textbox "zeta.txt"', { timeout_ms = 15000 })).to_be(true)
end)

test("a terminal tab survives a switch to another tab and back", function()
	local app = open_panel()
	app:click("#panel-open-terminal")
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "Terminal 1"
	end)).to_be(true)
	app:click('role:button "New Tab"')
	open_files(app)
	app:click('#panel-strip >> role:tab "Terminal 1"')
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "Terminal 1,Files" and not app:has("#panel-empty")
	end)).to_be(true)
end)

test("closing a tab with unsaved changes asks first; ⌘W closes the active one", function()
	local app = open_panel()
	open_files(app)
	pick(app, "code.luau")
	type_into(app, "code.luau", "x")
	app:hover('#panel-strip >> role:tab "code.luau"')
	app:hover('#panel-strip >> role:tab "code.luau"')
	app:click('#panel-strip >> role:button "Close Tab"')
	expect(app:wait('"Close this tab?"', { timeout_ms = 15000 })).to_be(true)
	-- (The dialog arrives; its buttons take a click once it has.)
	app:wait('"never shown, only waited past"', { timeout_ms = 400 })
	app:click('role:dialog >> role:button "Close"')
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "" and app:has("#panel-empty")
	end)).to_be(true)
	-- (The dialog leaves before anything under it takes a click.)
	app:wait('"Close this tab?"', { gone = true, timeout_ms = 15000 })
	app:wait('"never shown, only waited past"', { timeout_ms = 300 })
	-- A clean one goes at once.
	open_files(app)
	app:click("#panel-strip >> role:tab")
	app:key("cmd+w")
	expect(eventually(app, function(): boolean
		return app:has("#panel-empty")
	end)).to_be(true)
end)


-- The panel as laid out, node ids and focus aside, only what's in the
-- window (as the files pane's reload tests compare it).
local function layout_of(app: any): string
	local out: { string } = {}
	local skip_deeper: number? = nil
	-- (The strip, the tree and the file's header, each on its own: the
	-- whole panel's tree is cut short past the inspector's budget, an
	-- editor's lines filling it.)
	local parts: { string } = { app:tree("#panel-strip"), app:tree("role:tree") }
	if app:has("#file-viewer") then
		local header = {}
		for line in app:tree("#file-viewer"):gmatch("[^\n]+") do
			if line:find("#code-editor", 1, true) or line:find(" scroll ", 1, true) then
				break
			end
			table.insert(header, line)
		end
		table.insert(parts, table.concat(header, "\n"))
	end
	for line in table.concat(parts, "\n"):gmatch("[^\n]+") do
		local indent = #(line:match("^%s*") or "")
		if skip_deeper and indent > skip_deeper then
			continue
		end
		skip_deeper = nil
		if line:find("more (narrow with", 1, true) then
			continue
		end
		-- (A tooltip left from the pointer's last stop is no part of it.)
		if line:find('"Tooltip"', 1, true) then
			skip_deeper = indent
			continue
		end
		-- (An image arriving from its file is no motion: its row's place
		-- is what's compared.)
		if line:match("^%s*@%d+ img ") then
			continue
		end
		local plain = line:gsub("@%d+ ", ""):gsub(" focused", "")
		if plain:find("#code-editor textbox", 1, true) then
			-- (Its height is the window's, which follows the room measured.)
			plain = plain:gsub(' ="[^"]*"', ""):gsub("(%-?%d+,%-?%d+ %d+)x%d+", "%1")
			skip_deeper = indent
		end
		local y, h = plain:match(" %-?%d+,(%-?%d+) %d+x(%d+)")
		local top, tall = tonumber(y), tonumber(h)
		if not top or (top + (tall or 0) > 0 and top < 640) then
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
			return `line {i}: {la[i - 1] or ""} / {la[i] or "(none)"}  →  {lb[i - 1] or ""} / {lb[i] or "(none)"} / {lb[i + 1] or ""}`
		end
	end
	return ""
end

test("a reload keeps the panel exactly as it was: its tabs, the one showing, still", function()
	local app = open_panel()
	open_files(app)
	pick(app, "long.txt")
	app:click('role:button "New Tab"')
	app:click("#panel-open-terminal")
	app:click('#panel-strip >> role:tab "long.txt"')
	expect(app:wait('role:textbox "long.txt"', { timeout_ms = 15000 })).to_be(true)
	app:wait('"never shown, only waited past"', { timeout_ms = 700 })
	local before = layout_of(app)
	app:set_state("test_panel_generation", 2)
	-- (From the second frame: under heavy load a list may build a row it
	-- shows a frame late; nothing slides, fades, opens or closes.)
	app:frames(1)
	for frame = 2, 15 do
		app:frames(1)
		expect(`frame {frame}: {first_difference(before, layout_of(app))}`).to_be(`frame {frame}: `)
	end
end)

local function term_calls(app: any): string
	return app:state("test_term_calls") or ""
end

test("a terminal tab opens one engine terminal and keeps it across switches and a reload", function()
	local app = open_panel()
	app:click("#panel-open-terminal")
	expect(app:wait("#stub-terminal", { timeout_ms = 15000 })).to_be(true)
	expect(term_calls(app)).to_be("open:term-1,start:tab1:bridge term-1")
	app:click('role:button "New Tab"')
	open_files(app)
	app:click('#panel-strip >> role:tab "Terminal 1"')
	expect(app:wait("#stub-terminal", { timeout_ms = 15000 })).to_be(true)
	app:set_state("test_panel_generation", 2)
	expect(app:wait("#stub-terminal", { timeout_ms = 15000 })).to_be(true)
	-- One terminal, opened once, its session started once.
	expect(term_calls(app)).to_be("open:term-1,start:tab1:bridge term-1")
end)

test("closing a terminal with a program running asks first; an idle one closes at once", function()
	local app = open_panel()
	app:click("#panel-open-terminal")
	expect(app:wait("#stub-terminal", { timeout_ms = 15000 })).to_be(true)
	app:set_state("test_term_busy", true)
	app:click('#panel-strip >> role:tab "Terminal 1"')
	-- (Its engine terminal claimed from its bridge: there's something to ask
	-- about.)
	expect(eventually(app, function(): boolean
		local s = app:state("panel_sessions")["chat:chat-1"]
		return s ~= nil and s.tabs[1] ~= nil and s.tabs[1].terminal ~= nil
	end)).to_be(true)
	app:key("cmd+w")
	expect(app:wait('"Close this tab?"', { timeout_ms = 15000 })).to_be(true)
	expect(term_calls(app)).never.to_contain("close:")
	app:wait('"never shown, only waited past"', { timeout_ms = 400 })
	app:click('role:dialog >> role:button "Close"')
	expect(eventually(app, function(): boolean
		return string.find(term_calls(app), "close:term-1", 1, true) ~= nil and app:has("#panel-empty")
	end)).to_be(true)
	expect(term_calls(app)).to_contain("end:tab1")
	app:wait('"Close this tab?"', { gone = true, timeout_ms = 15000 })
	app:wait('"never shown, only waited past"', { timeout_ms = 300 })
	-- Idle: at once.
	app:set_state("test_term_busy", false)
	app:click("#panel-open-terminal")
	expect(app:wait("#stub-terminal", { timeout_ms = 15000 })).to_be(true)
	app:click('#panel-strip >> role:tab "Terminal 1"')
	app:key("cmd+w")
	expect(eventually(app, function(): boolean
		return string.find(term_calls(app), "close:term-2", 1, true) ~= nil
	end)).to_be(true)
end)

-- Where a tab starts, across the window and down it (from its line in
-- the tree).
local function tab_at(app: any): (number?, number?)
	for line in app:tree("#panel-strip"):gmatch("[^\n]+") do
		if line:find('tab "', 1, true) then
			local x, y = line:match(" (%-?%d+),(%-?%d+) %d+x%d+")
			return tonumber(x), tonumber(y)
		end
	end
	return nil, nil
end
local function tab_x(app: any): number?
	return (tab_at(app))
end

test("the app's ⌘W closes the panel's active tab only while the panel has the keyboard", function()
	local app = open_panel()
	open_files(app)
	-- (The tree's row just clicked has the keyboard.)
	pick(app, "code.luau")
	app:frames(4)
	app:command("test.close_focused_tab")
	expect(eventually(app, function(): boolean
		return app:state("test_panel_closed_focused") == "true" and app:has("#panel-empty")
	end)).to_be(true)
	-- The keyboard elsewhere: the panel leaves it be.
	open_files(app)
	app:click("#outside")
	app:frames(4)
	app:command("test.close_focused_tab")
	expect(eventually(app, function(): boolean
		return app:state("test_panel_closed_focused") == "false"
	end)).to_be(true)
	expect(tab_titles(app)).to_be("Files")
end)

test("closing the last tab closes the panel; one of two doesn't; a busy terminal's asks first", function()
	local app = open_panel()
	-- (Settings → General's "Close the panel with its last tab", on.)
	app:set_state("settings_close_panel_with_last_tab", true)
	-- Two tabs: closing one leaves the panel open.
	open_files(app)
	app:click('#panel-strip >> role:button "New Tab"')
	expect(app:wait("#panel-open-terminal", { timeout_ms = 15000 })).to_be(true)
	app:click("#panel-open-terminal")
	expect(app:wait("#stub-terminal", { timeout_ms = 15000 })).to_be(true)
	expect(eventually(app, function(): boolean
		local s = app:state("panel_sessions")["chat:chat-1"]
		return s ~= nil and s.tabs[2] ~= nil and s.tabs[2].terminal ~= nil
	end)).to_be(true)
	app:click('#panel-strip >> role:tab "Files"')
	app:key("cmd+w")
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "Terminal 1"
	end)).to_be(true)
	expect(app:state("test_panel_closed")).to_be(0)
	-- The last, a terminal with something running: it asks, then the panel
	-- closes with it.
	app:set_state("test_term_busy", true)
	app:click('#panel-strip >> role:tab "Terminal 1"')
	app:key("cmd+w")
	expect(app:wait('"Close this tab?"', { timeout_ms = 15000 })).to_be(true)
	expect(app:state("test_panel_closed")).to_be(0)
	app:wait('"never shown, only waited past"', { timeout_ms = 400 })
	app:click('role:dialog >> role:button "Close"')
	expect(eventually(app, function(): boolean
		return app:state("test_panel_closed") == 1
	end)).to_be(true)
	-- Opened again: its empty page.
	expect(app:has("#panel-empty")).to_be(true)
	expect(tab_titles(app)).to_be("")
end)

test("⌘↵ in the tree opens the file in a tab of its own; the one showing stays", function()
	local app = open_panel()
	open_files(app)
	pick(app, "code.luau")
	-- (The tree has the keyboard, on code.luau: down to the next file.)
	app:key("down")
	app:frames(5)
	app:key("cmd+enter")
	expect(eventually(app, function(): boolean
		local titles = tab_titles(app)
		return string.find(titles, ",", 1, true) ~= nil
	end)).to_be(true)
	local first, second = string.match(tab_titles(app), "^([^,]+),([^,]+)$")
	expect(first).to_be("code.luau")
	expect(second).never.to_be("code.luau")
	expect(second).never.to_be("Files")
end)

test("⌘↵ on the file showing keeps the one tab, active: a file is never open twice", function()
	local app = open_panel()
	open_files(app)
	pick(app, "code.luau")
	-- (The tree has the keyboard, on code.luau: the file showing, unedited.)
	app:key("cmd+enter")
	app:wait('"never shown, only waited past"', { timeout_ms = 400 })
	expect(tab_titles(app)).to_be("code.luau")
	local s = app:state("panel_sessions")["chat:chat-1"]
	expect(s.active).to_be(s.tabs[1].id)
end)

-- Moves the tree's selection `n` rows (down, or up for negative) without
-- opening anything: a click on a row opens it.
local function step(app: any, n: number)
	for _ = 1, math.abs(n) do
		app:key(if n > 0 then "down" else "up")
		app:frames(2)
	end
end

-- Two file tabs, zeta.txt then code.luau (the second showing). The tree's
-- rows: src, code.luau, long.txt, notes.md, zeta.txt.
local function two_tabs(app: any)
	open_files(app)
	pick(app, "zeta.txt")
	step(app, -3)
	app:key("cmd+enter")
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "zeta.txt,code.luau"
	end)).to_be(true)
end

local function active_is(app: any, n: number): boolean
	local s = app:state("panel_sessions")["chat:chat-1"]
	return s ~= nil and s.tabs[n] ~= nil and s.active == s.tabs[n].id
end

test("a tree pick of a file open in another tab shows that tab; the tab picked from stays as it was", function()
	local app = open_panel()
	two_tabs(app)
	-- Back in the first (zeta.txt); pick code.luau, open in the second.
	app:click('#panel-strip >> role:tab "zeta.txt"')
	expect(app:wait('role:textbox "zeta.txt"', { timeout_ms = 15000 })).to_be(true)
	app:click('role:treeitem "code.luau"')
	expect(eventually(app, function(): boolean
		return active_is(app, 2)
	end)).to_be(true)
	-- Still two tabs, the first still on zeta.txt.
	expect(tab_titles(app)).to_be("zeta.txt,code.luau")
end)

test("Open in New Tab on a file open already shows its tab; no new one", function()
	local app = open_panel()
	two_tabs(app)
	-- In the second (code.luau), ⌘↵ on zeta.txt (open in the first).
	expect(app:wait('role:textbox "code.luau"', { timeout_ms = 15000 })).to_be(true)
	app:click('role:treeitem "code.luau"')
	app:frames(3)
	step(app, 3)
	app:key("cmd+enter")
	expect(eventually(app, function(): boolean
		return active_is(app, 1)
	end)).to_be(true)
	expect(tab_titles(app)).to_be("zeta.txt,code.luau")
end)

test("a file open with unsaved edits is shown with its edits as they were", function()
	local app = open_panel()
	open_files(app)
	pick(app, "code.luau")
	type_into(app, "code.luau", "Q")
	-- Another file: past the edits, in a tab of its own.
	app:click('role:treeitem "zeta.txt"')
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "code.luau,zeta.txt"
	end)).to_be(true)
	-- From it, code.luau again: its tab, edits and all.
	expect(app:wait('role:textbox "zeta.txt"', { timeout_ms = 15000 })).to_be(true)
	app:click('role:treeitem "code.luau"')
	expect(eventually(app, function(): boolean
		return active_is(app, 1)
	end)).to_be(true)
	expect(app:wait('role:textbox "code.luau"', { timeout_ms = 15000 })).to_be(true)
	expect(app:text('role:textbox "code.luau"')).to_contain("Q")
	expect(tab_titles(app)).to_be("code.luau,zeta.txt")
end)

test("a file mentioned in a chat opens in its tab if open; per chat; never twice", function()
	local app = open_panel()
	two_tabs(app)
	-- zeta.txt, mentioned: its tab (the first), nothing new.
	app:set_state("test_open_path", "zeta.txt")
	app:command("test.open_file")
	expect(eventually(app, function(): boolean
		return active_is(app, 1)
	end)).to_be(true)
	expect(tab_titles(app)).to_be("zeta.txt,code.luau")
	-- Another chat: its own panel; zeta.txt open in chat-1's doesn't count.
	app:set_state("test_panel_session", { id = "chat-2", cwd = "/proj" })
	app:frames(5)
	app:command("test.open_file")
	expect(eventually(app, function(): boolean
		local s = app:state("panel_sessions")["chat:chat-2"]
		return s ~= nil and #s.tabs == 1
	end)).to_be(true)
	expect(#app:state("panel_sessions")["chat:chat-1"].tabs).to_be(2)
end)

test("what plugins place by is there: the panel, its strip, the files pane, its tree, header and file", function()
	local app = open_panel()
	expect(app:has("#panel")).to_be(true)
	expect(app:has("#panel-empty")).to_be(true)
	open_files(app)
	pick(app, "code.luau")
	for _, target in { "#panel-strip", "#files-pane", "#file-tree", "#files-header", "#file-viewer" } do
		expect(`{target}: {app:has(target)}`).to_be(`{target}: true`)
	end
	-- (`.panel-tab`, `.file-tab` and `.file-row` and what they provide are
	-- set on those nodes; zgx's test driver can't target a class yet.)
end)

test("⌃Tab from the shell steps the panel's tabs only while the panel has the keyboard", function()
	local app = open_panel()
	open_files(app)
	app:click('role:button "New Tab"')
	expect(app:wait("#panel-open-terminal", { timeout_ms = 15000 })).to_be(true)
	app:click("#panel-open-terminal")
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "Files,Terminal 1"
	end)).to_be(true)
	-- The keyboard in the panel (its first tab's tree): steps, round.
	app:click('#panel-strip >> role:tab "Files"')
	expect(app:wait('role:treeitem "long.txt"', { timeout_ms = 15000 })).to_be(true)
	app:click('role:treeitem "long.txt"')
	app:frames(4)
	app:command("test.step_tab")
	expect(eventually(app, function(): boolean
		local s = app:state("panel_sessions")["chat:chat-1"]
		return app:state("test_panel_stepped") == "true" and s.active == s.tabs[2].id
	end)).to_be(true)
	-- (The terminal turned to takes the keyboard on the next frame, as it
	-- does in the app: frames pass before the next click, as a person's do.)
	app:frames(4)
	-- The keyboard elsewhere: it leaves the tabs be (the shell steps sessions).
	app:click("#outside")
	app:frames(4)
	app:command("test.step_tab")
	expect(eventually(app, function(): boolean
		return app:state("test_panel_stepped") == "false"
	end)).to_be(true)
end)

test("a terminal opened or turned to takes the keyboard: typing reaches its shell, no click", function()
	local app = open_panel()
	-- Whether the terminal has the keyboard (it takes it as it shows, a
	-- moment after its tab does).
	local function terminal_has_keys(): boolean
		return string.find(app:tree("#stub-terminal"), "focused", 1, true) ~= nil
	end
	local function typed(key: string): boolean
		eventually(app, function(): boolean
			return app:has("#stub-terminal") and terminal_has_keys()
		end, 3000)
		app:key(key)
		return eventually(app, function(): boolean
			return string.find(term_calls(app), `key:{key}`, 1, true) ~= nil
		end, 3000)
	end
	-- Opened from the empty page.
	app:click("#panel-open-terminal")
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "Terminal 1"
	end)).to_be(true)
	expect(typed("a")).to_be(true)
	-- Turned to by its tab, the keyboard in a file tree.
	app:click('role:button "New Tab"')
	open_files(app)
	expect(app:wait('role:treeitem "long.txt"', { timeout_ms = 15000 })).to_be(true)
	app:click('role:treeitem "long.txt"')
	app:click('#panel-strip >> role:tab "Terminal 1"')
	expect(typed("b")).to_be(true)
	-- Stepped to with ⌃Tab from the file's tab (named for it now), the
	-- keyboard in its tree.
	app:click('#panel-strip >> role:tab "long.txt"')
	expect(app:wait('role:treeitem "long.txt"', { timeout_ms = 15000 })).to_be(true)
	app:click('role:treeitem "long.txt"')
	app:frames(4)
	app:command("test.step_tab")
	expect(typed("c")).to_be(true)
	-- ⌘J, the keyboard elsewhere: the panel shows its terminal, and it has
	-- the keyboard.
	app:click('#panel-strip >> role:tab "long.txt"')
	app:click("#outside")
	app:command("test.toggle_terminal")
	expect(typed("d")).to_be(true)
	-- A reload takes nothing: the keyboard elsewhere stays there.
	app:click("#outside")
	app:frames(4)
	app:set_state("test_panel_generation", 2)
	app:wait('"never shown, only waited past"', { timeout_ms = 300 })
	expect(typed("e")).to_be(false)
end)

test("/terminal opens a new terminal each time, with the keyboard; /files shows the one files tab", function()
	local app = open_panel()
	app:command("test.show_terminal")
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "Terminal 1"
	end)).to_be(true)
	app:command("test.show_terminal")
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "Terminal 1,Terminal 2"
	end)).to_be(true)
	-- The new one in front, and its shell has the keyboard.
	expect(eventually(app, function(): boolean
		return app:has("#stub-terminal") and string.find(app:tree("#stub-terminal"), "focused", 1, true) ~= nil
	end)).to_be(true)
	app:key("q")
	expect(eventually(app, function(): boolean
		return string.find(term_calls(app), "key:q", 1, true) ~= nil
	end)).to_be(true)
	-- /files twice: one files tab, shown.
	app:command("test.show_files")
	app:command("test.show_files")
	expect(eventually(app, function(): boolean
		return tab_titles(app) == "Terminal 1,Terminal 2,Files"
	end)).to_be(true)
	app:frames(10)
	expect(tab_titles(app)).to_be("Terminal 1,Terminal 2,Files")
end)

test("a file opened at a line shows with the caret on it, in a new tab or the one it's in; a range is selected", function()
	local app = open_panel()
	local function status(): string
		return if app:has('role:button "Go to Line"') then app:text('role:button "Go to Line"') else ""
	end
	-- A new tab, at line 200 (a link to `long.txt:200`).
	app:set_state("test_open_path", "long.txt")
	app:set_state("test_open_at", { line = 200 })
	app:command("test.open_file")
	expect(eventually(app, function(): boolean
		return string.find(status(), "Ln 200,", 1, true) ~= nil
	end)).to_be(true)
	expect(tab_titles(app)).to_be("long.txt")
	-- The tab it's in already, at another line and column.
	app:set_state("test_open_at", { line = 50, column = 3 })
	app:command("test.open_file")
	expect(eventually(app, function(): boolean
		return string.find(status(), "Ln 50, Col 3", 1, true) ~= nil
	end)).to_be(true)
	expect(tab_titles(app)).to_be("long.txt")
	-- A range (`long.txt:10-12`): its lines selected.
	app:set_state("test_open_at", { line = 10, end_line = 12 })
	app:command("test.open_file")
	expect(eventually(app, function(): boolean
		return string.find(status(), "selected", 1, true) ~= nil
	end)).to_be(true)
	-- A plain number works as a line too.
	app:set_state("test_open_at", 7)
	app:command("test.open_file")
	expect(eventually(app, function(): boolean
		return string.find(status(), "Ln 7,", 1, true) ~= nil
	end)).to_be(true)
end)

test("Escape in a file's editor takes the keyboard to the file's tab (Tab indents there)", function()
	local app = open_panel()
	open_files(app)
	pick(app, "long.txt")
	local editor = 'role:textbox "long.txt"'
	expect(app:wait(editor, { timeout_ms = 15000 })).to_be(true)
	app:click(editor)
	expect(app:tree(editor)).to_contain("focused")
	app:key("Escape")
	expect(eventually(app, function(): boolean
		return string.find(app:tree('#panel-strip >> role:tab "long.txt"'), "focused", 1, true) ~= nil
	end)).to_be(true)
end)

-- A tab dragged along the strip takes another's place as it crosses that
-- one's middle, the strip making room as it goes; let go, it stays there.
test("a tab dragged over another takes its place, and stays when dropped", function()
	local app = open_panel()
	open_files(app)
	pick(app, "long.txt")
	app:click('role:button "New Tab"')
	open_files(app)
	pick(app, "zeta.txt")
	expect(tab_titles(app)).to_be("long.txt,zeta.txt")
	app:drag('#panel-strip >> role:tab "zeta.txt"', '#panel-strip >> role:tab "long.txt"')
	app:frames(20)
	expect(tab_titles(app)).to_be("zeta.txt,long.txt")
	expect(app:error()).to_be_nil()
end)

test("by default the panel stays open on its home when its last tab closes", function()
	local app = open_panel()
	open_files(app)
	expect(tab_titles(app)).to_be("Files")
	app:click('#panel-strip >> role:tab "Files"')
	app:key("cmd+w")
	expect(eventually(app, function(): boolean
		return app:has("#panel-empty")
	end)).to_be(true)
	app:frames(30)
	-- (Settings → General's "Close the panel with its last tab" is off.)
	expect(app:state("test_panel_closed")).to_be(0)
end)

-- A box from the test tree's first line under `target`: x, y, w, h.
local function bounds(app: any, target: string): (number, number, number, number)
	local line = string.match(app:tree(target), "[^\n]+") or ""
	local x, y, w, h = line:match(" (%-?%d+),(%-?%d+) (%d+)x(%d+)")
	return tonumber(x) or 0, tonumber(y) or 0, tonumber(w) or 0, tonumber(h) or 0
end

test("with more tabs than fit, + stays in view at the strip's end; with few, it follows the last tab", function()
	local app = open_panel()
	open_files(app)
	app:frames(20)
	-- Few: right after the last tab.
	local tx, _, tw = bounds(app, '#panel-strip >> role:tab "Files"')
	local nx = bounds(app, 'role:button "New Tab"')
	expect(nx >= tx + tw and nx < tx + tw + 24).to_be(true)
	-- Many: terminals until they run past the strip.
	for _ = 1, 8 do
		app:click('role:button "New Tab"')
		expect(app:wait("#panel-open-terminal", { timeout_ms = 15000 })).to_be(true)
		app:click("#panel-open-terminal")
		app:frames(10)
	end
	-- The strip scrolled back to its start (as to the first tab), where a
	-- "+" after the last tab would be out of view.
	app:set_state("panel_strip_offset", 0)
	app:frames(30)
	local sx, _, sw = bounds(app, "#panel-strip")
	local px, _, pw = bounds(app, 'role:button "New Tab"')
	-- In view, near the strip's right end, not scrolled off past it.
	expect(px > 0 and px + pw <= sx + sw).to_be(true)
	expect(px + pw > sx + sw - 120).to_be(true)
end)

-- A plugin's tab asked for again (Git's Changes, from its home row or a
-- link) brings forward the one the session has rather than making a
-- second; another kind (History) is a tab of its own.
test("a plugin's tab asked for again is brought forward, not made twice; another kind is its own tab", function()
	local app = open_panel()
	app:command("test.open_git")
	expect(eventually(app, function()
		return tab_titles(app) == "Changes"
	end)).to_be(true)
	app:command("test.open_git_history")
	expect(eventually(app, function()
		return tab_titles(app) == "Changes,History"
	end)).to_be(true)
	expect(active_is(app, 2)).to_be(true)
	app:command("test.open_git")
	app:frames(10)
	expect(tab_titles(app)).to_be("Changes,History")
	expect(eventually(app, function()
		return active_is(app, 1)
	end)).to_be(true)
	app:command("test.open_git_history")
	app:frames(10)
	expect(tab_titles(app)).to_be("Changes,History")
	expect(app:error()).to_be_nil()
end)

-- A plugin's tab with a key (Git's commit tabs): found by its key, not
-- its title, so two commits of one subject are two tabs, and the same one
-- asked again is brought forward.
test("a plugin tab with a key is found by it: two of one title are two tabs, the same one comes forward", function()
	local app = open_panel()
	app:command("test.open_commit_a")
	app:command("test.open_commit_b")
	expect(eventually(app, function()
		return tab_titles(app) == "Fix the build,Fix the build"
	end)).to_be(true)
	expect(active_is(app, 2)).to_be(true)
	app:command("test.open_commit_a")
	app:frames(10)
	expect(tab_titles(app)).to_be("Fix the build,Fix the build")
	expect(eventually(app, function()
		return active_is(app, 1)
	end)).to_be(true)
	expect(app:error()).to_be_nil()
end)
