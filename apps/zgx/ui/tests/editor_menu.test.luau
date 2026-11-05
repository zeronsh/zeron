-- An editor's text menu (ds/editor_menu, in ui/editor_menu_fixture.luau with
-- the host's part stubbed): a secondary click in an editor opens the design
-- system's menu (its panel, rows, icons and keys), not zgx's own; its items
-- are enabled as zgx's were for each case, and each does its action
-- through the host.

local function settle(app: any)
	app:wait("#never-shown", { timeout_ms = 300 })
	app:frames(5)
end

local FULL = { selected = "hello", copyable = true, editable = true, clipboard = true, platform = true, looks_up = true }

local function with(changes: { [string]: any }): { [string]: any }
	local s = table.clone(FULL)
	for k, v in changes do
		s[k] = v
	end
	return s
end

-- Opens the menu on `editor` with the editor in `state`: each row's name and
-- whether it's enabled, in order.
local function open(app: any, editor: string, state: { [string]: any }): { { name: string, on: boolean } }
	app:key("Escape")
	settle(app)
	app:set_state("test_editor_state", state)
	app:frames(2)
	app:right_click(editor)
	settle(app)
	local rows = {}
	for line in app:tree("role:menu"):gmatch("[^\n]+") do
		local name = line:match('menuitem "([^"]*)"')
		if name then
			table.insert(rows, { name = name, on = not line:find(" disabled", 1, true) })
		end
	end
	return rows
end

local function names(rows: { { name: string, on: boolean } }): string
	local out = {}
	for _, r in rows do
		table.insert(out, r.name .. (if r.on then "" else " (off)"))
	end
	return table.concat(out, " | ")
end

local function mounted(): any
	local app = mount("editor_menu_fixture.luau")
	app:resize(600, 500)
	app:frames(5)
	return app
end

test("editor menu: a secondary click in an editor opens the design system's menu", function()
	local app = mounted()
	local rows = open(app, "#plain", FULL)
	expect(names(rows)).to_be("Look Up “hello” | Cut | Copy | Paste | Select All")
	-- Ours: the menus' panel width (232) and rows (28), with their keys.
	local panel = string.match(app:tree("role:menu"), "^[^\n]*") or ""
	expect(string.find(panel, " 232x", 1, true) ~= nil).to_be(true)
	expect(string.find(app:tree('role:menuitem "Cut"'), " 222x28", 1, true) ~= nil).to_be(true)
	expect(app:has('role:menuitem "Cut" >> "⌘X"')).to_be(true)
	expect(app:has('role:menuitem "Paste" >> "⌘V"')).to_be(true)
	-- A ds field has it too.
	rows = open(app, 'role:textbox "Field"', FULL)
	expect(rows[2].name).to_be("Cut")
	expect(app:error()).to_be(nil)
end)

test("editor menu: each item is enabled as zgx's own menu enabled it", function()
	local app = mounted()
	-- Nothing selected: no Look Up; Cut and Copy off; Paste and Select All on.
	expect(names(open(app, "#plain", with({ selected = "", copyable = false })))).to_be("Cut (off) | Copy (off) | Paste | Select All")
	-- Read-only, a selection: Copy on; Cut and Paste off.
	expect(names(open(app, "#plain", with({ editable = false })))).to_be("Look Up “hello” | Cut (off) | Copy | Paste (off) | Select All")
	-- A password field (copies nothing): Cut and Copy off, no Look Up.
	expect(names(open(app, "#plain", with({ selected = "", copyable = false })))).to_be("Cut (off) | Copy (off) | Paste | Select All")
	-- Nothing on the clipboard: Paste off...
	expect(names(open(app, "#plain", with({ clipboard = false })))).to_be("Look Up “hello” | Cut | Copy | Paste (off) | Select All")
	-- ...except in an editor that takes pictures (the composer's), with the
	-- place's own entries after a rule.
	expect(names(open(app, "#pictures", with({ clipboard = false })))).to_be("Look Up “hello” | Cut | Copy | Paste | Select All | Mention")
	-- Where the platform looks nothing up: no Look Up.
	expect(names(open(app, "#plain", with({ looks_up = false })))).to_be("Cut | Copy | Paste | Select All")
	expect(app:error()).to_be(nil)
end)

test("editor menu: each item does its action through the host", function()
	local app = mounted()
	for _, case in { { "Cut", "cut" }, { "Copy", "copy" }, { "Paste", "paste" }, { "Select All", "select_all" }, { "Look Up “hello”", "look_up" } } do
		app:set_state("test_editor_calls", "")
		open(app, "#plain", FULL)
		app:click(`role:menuitem "{case[1]}"`)
		settle(app)
		expect(app:state("test_editor_calls")).to_be(case[2] .. ",")
	end
	expect(app:error()).to_be(nil)
end)
