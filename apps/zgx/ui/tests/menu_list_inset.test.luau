-- A menu's list (ds/menu `List`) has the same room over its first line as
-- under its last, whatever it holds: one row, a few, a full list scrolled,
-- nothing (what an empty list says), a heading first. The composer's `@`
-- launcher (ds/mention_launcher) and its `/` and `$` lists
-- (ds/composer_mentions), on the chat page (ui/chat_fixture.luau). And the
-- launcher's bands line up: one left edge and one right edge.

local BOX = 'role:textbox "Message"'

local function settle(app: any)
	app:frames(30)
end

local function chat(): any
	local app = mount("chat_fixture.luau")
	app:resize(1000, 900)
	app:command("go_s2")
	settle(app)
	return app
end

local function box(line: string): (number, number, number, number)
	local x, y, w, h = line:match(" (%-?[%d.]+),(%-?[%d.]+) ([%d.]+)x([%d.]+)")
	return tonumber(x) :: number, tonumber(y) :: number, tonumber(w) :: number, tonumber(h) :: number
end

local function indent(line: string): number
	return #(line:match("^(%s*)") :: string)
end

-- The room over the list's first line and under its last one showing (a
-- scrolled list's last is cut by its scroll), each from the list's edge.
local function room(app: any): (number, number, string)
	local list_y, list_b, list_in = nil, nil, -1
	local view_b, view_in = nil, -1
	local first: number? = nil
	local last: number? = nil
	local what = ""
	for line in app:tree("#mention-popover"):gmatch("[^\n]+") do
		local d = indent(line)
		if list_y and d <= list_in then
			-- (Past the list.)
			break
		end
		if line:find("#mention-list", 1, true) then
			local _, y, _, h = box(line)
			list_y, list_b, list_in = y, y + h, d
		elseif list_y and d == list_in + 2 and line:find(" listbox ", 1, true) then
			local _, y, _, h = box(line)
			view_b, view_in = y + h, d
		elseif list_y and (d == list_in + 2 or d == view_in + 2) and not line:find(" hidden", 1, true) then
			local _, y, _, h = box(line)
			local bottom = y + h
			if view_b and d == view_in + 2 then
				if y >= view_b then
					continue
				end
				bottom = math.min(bottom, view_b)
			end
			first = first or y
			last = bottom
			what ..= `\n{line}`
		end
	end
	if not list_y or not first or not last then
		error(`no list lines in:\n{app:tree("#mention-popover")}`)
	end
	return first - (list_y :: number), (list_b :: number) - last, what
end

local function even(app: any, case: string)
	local over, under, what = room(app)
	expect(`{case}: over {over}, under {under}`).to_be(`{case}: over {over}, under {over}`)
	expect(math.abs(over - under) <= 0.5).to_be(true)
	if over > 8 then
		error(`{case}: {over} over the first line{what}`)
	end
end

test("the @ launcher's list: as much room over the first line as under the last, one row, three, a full list, none", function()
	local app = chat()
	app:command("drop_pictures")
	settle(app)
	app:type(BOX, "@")
	settle(app)
	-- All: a heading first, a full list scrolled.
	even(app, "all")
	-- Attachments: three rows.
	app:key("Shift+Tab")
	settle(app)
	even(app, "three rows")
	-- One row.
	app:type(BOX, "zgx")
	settle(app)
	even(app, "one row")
	-- None: what an empty scope says.
	app:type(BOX, "qqqq")
	settle(app)
	expect(app:has("#mention-empty")).to_be(true)
	even(app, "empty")
	-- A scope's few rows under a heading? All, narrowed to one section.
	app:fill(BOX, "@chat.rs")
	settle(app)
	app:key("Tab")
	settle(app)
	even(app, "a few files")
end)

test("the / and $ lists: the same, their heading first", function()
	local app = chat()
	app:type(BOX, "/")
	settle(app)
	even(app, "/ full")
	app:type(BOX, "comp")
	settle(app)
	even(app, "/ one")
	app:type(BOX, "qqqq")
	settle(app)
	even(app, "/ none")
	app:fill(BOX, "$")
	settle(app)
	even(app, "$")
end)

-- A footer key's label as ds/kbd gives it: macOS's glyph (↑ said "Up
-- Arrow", ↵ as it is), elsewhere the platform's word ("Up", "Enter").
local function key_label(tree: string, mac: string, other: string): string
	return if tree:find(`img "{mac}"`, 1, true) then mac else other
end

-- The x of a printed line's box, its right edge, by what it says.
local function edges(tree: string, needle: string): (number?, number?)
	for line in tree:gmatch("[^\n]+") do
		if line:find(needle, 1, true) then
			local x, _, w = box(line)
			return x, x + w
		end
	end
	return nil, nil
end

test("the launcher's bands line up: the trigger, the tabs, the headings, the tiles and the keys start on one edge; the count and the ↵ end on one", function()
	local app = chat()
	app:command("drop_pictures")
	settle(app)
	app:type(BOX, "@")
	settle(app)
	local tree = app:tree("#mention-launcher")
	local panel = edges(tree, "#mention-launcher") :: number
	-- (A tab's label sits a row's padding into its pill, as a heading's
	-- text into its row and a tile into its row.)
	local tab = edges(tree, 'tab "All"') :: number
	local heading = edges(tree, 'text "Files"') :: number
	local up = edges(tree, `img "{key_label(tree, "Up Arrow", "Up")}"`) :: number
	local option = edges(tree, 'option "chat.rs"') :: number
	expect(`tab pill {tab - panel}`).to_be(`tab pill {option - panel}`)
	expect(`heading {heading - panel}`).to_be(`heading {up - panel}`)
	-- The trigger's slot is the tiles' column (its glyph centred in it).
	local at, at_end = edges(tree, 'text "@"')
	local enter = `img "{key_label(tree, "↵", "Enter")}"`
	local tile = (edges(tree, enter))
	expect(tile).never.to_be_nil()
	expect(math.abs(((at :: number) + (at_end :: number)) / 2 - (up + 9)) <= 1).to_be(true)
	-- The right: the count's end, the ↵'s end.
	local _, count_end = edges(tree, " results")
	local _, key_end = edges(tree, enter)
	expect(`count ends {count_end}`).to_be(`count ends {key_end}`)
	-- Nothing typed: what to type, quiet, on the names' line.
	expect(tree).to_contain('text "Search files, chats, attachments"')
	local hint = edges(tree, "Search files") :: number
	local name = edges(tree, 'text "chat"') :: number
	expect(`hint at {hint}`).to_be(`hint at {name}`)
end)

test("a long file name is cut before its extension, never clipped; a narrow window narrows the panel to its least", function()
	local app = chat()
	app:command("attach_odd")
	settle(app)
	app:type(BOX, "@very")
	settle(app)
	local tree = app:tree("#mention-launcher")
	local _, row_end = edges(tree, "option \"a-very-long")
	local ext, ext_end = edges(tree, 'text ".txt"')
	expect(ext).never.to_be_nil()
	expect((ext_end :: number) <= (row_end :: number)).to_be(true)
	app:key("Escape")
	app:resize(380, 900)
	settle(app)
	app:fill(BOX, "")
	app:type(BOX, "@")
	settle(app)
	local x, right = edges(app:tree("#mention-popover"), "#mention-popover")
	expect(`width {(right :: number) - (x :: number)}`).to_be("width 348")
end)

-- A printed line's box, by what it says.
local function find(tree: string, needle: string): (number, number, number, number)
	for line in tree:gmatch("[^\n]+") do
		if line:find(needle, 1, true) then
			return box(line)
		end
	end
	error(`no {needle} in:\n{tree}`)
end

local function centred(what: string, y: number, h: number, mid: number)
	local c = y + h / 2
	expect(`{what} centre {c} on {mid}: {math.abs(c - mid) <= 0.5}`).to_be(`{what} centre {c} on {mid}: true`)
end

test("the launcher's top, its tabs and its keys are each centred on their band's middle", function()
	local app = chat()
	app:type(BOX, "@")
	settle(app)
	local tree = app:tree("#mention-launcher")
	local _, top = find(tree, "#mention-launcher")
	-- The top band: its inset, then a row's height; its middle half a row in.
	local inset = 4
	local row = 28
	local mid = top + inset + row / 2
	local _, ay, _, ah = find(tree, 'text "@"')
	local _, hy, _, hh = find(tree, 'text "Search files, chats, attachments"')
	local _, cy, _, ch = find(tree, " results")
	centred("@", ay, ah, mid)
	centred("hint", hy, hh, mid)
	centred("count", cy, ch, mid)
	-- (As much room over the hint as under it, in its row.)
	local over, under = hy - (top + inset), (top + inset + row) - (hy + hh)
	expect(`hint over {over}, under {under}`).to_be(`hint over {over}, under {over}`)
	-- A tab: its count on the pill's middle.
	local _, ty, _, th = find(tree, 'tab "Files"')
	local _, ny, _, nh = find(tree, 'text "4"')
	centred("tab count", ny, nh, ty + th / 2)
	-- The keys: each word on its cap's middle.
	local _, ky, _, kh = find(tree, 'img "Tab"')
	local _, wy, _, wh = find(tree, 'text "Switch tab"')
	centred("Switch tab", wy, wh, ky + kh / 2)
	-- (The footer's ↵: the last, under the lit row's.)
	local ey, eh = 0, 0
	for line in tree:gmatch("[^\n]+") do
		if line:find(`img "{key_label(tree, "↵", "Enter")}"`, 1, true) then
			local _, y, _, h = box(line)
			ey, eh = y, h
		end
	end
	local _, iy, _, ih = find(tree, 'text "Insert"')
	centred("Insert", iy, ih, ey + eh / 2)
end)
