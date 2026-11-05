-- Selected text's context menu: selectable text that opens the design
-- system's menu (ds/context_menu), not zgx's own, so a secondary click on a
-- selection looks like every other menu. zgx gives a selectable node its own
-- menu only if it has none, so this is a `ContextMenu` that is the selectable
-- node. Look Up, Copy and Select All do what zgx's did, through the host
-- (scripts can't reach them), and `more` adds what the place offers.
--
-- Off the selected text it opens nothing, and the click goes on to what's
-- around it, as zgx's does.
--
-- `SelectionMenu { w_full = true, more = function(s) return { … } end, content }`:
-- other props go to the box, as ContextMenu's do. `more` gets the selection
-- (`read`'s), and its entries follow a rule.

local ContextMenu = require("@self/context_menu")
local locale = require("@self/locale")

local M = {}

-- (A plugin lent the design system has its own copy of this module, and
-- reads neither: the app keeps the selection to itself. There its menu
-- never opens, and a secondary click goes on to what's around it.)
local has_selection, selection = pcall(ui.shared, "selection", nil :: any)
-- Whether the platform looks words up.
local has_looks_up, looks_up = pcall(ui.shared, "looks_up", false)

-- The selection now: its `text`, and its first and last lines in the window
-- (`top`, `bottom`), either missing while its row is scrolled away. Nil with
-- nothing selected.
function M.read(): any
	-- (A host built before `read_selection` has none.)
	if not has_selection or not pcall(ui.invoke, "read_selection", nil) then
		return nil
	end
	local s = selection:peek()
	return if s and string.find(s.text, "%S") then s else nil
end

-- Ends the selection.
function M.clear()
	pcall(ui.invoke, "clear_selection", nil)
end

-- Whether window point `x`, `y` is on the selection: past its start on its
-- first line, short of its end on its last, anywhere between. (zgx's own test
-- isn't open to a host: this counts the gaps between its lines.)
function M.on(s: any, x: number, y: number): boolean
	local t, b = s.top, s.bottom
	if not t or not b or y < t.y or y >= b.y + b.h then
		return false
	end
	if y < t.y + t.h and x < t.x then
		return false
	end
	if y >= b.y and x > b.x + b.w then
		return false
	end
	return true
end

-- A Look Up's name, as AppKit's: the text in quotes, cut short.
local function look_up_label(text: string): string
	local line = string.gsub(string.gsub(text, "%s+", " "), "^ ", "")
	local short = if (utf8.len(line) or 0) > 24 then string.sub(line, 1, (utf8.offset(line, 25) :: number) - 1) .. "…" else line
	return locale.s("ds.selection_menu.look_up", { text = short })
end
-- (Shared with ds/editor_menu: one Look Up wording.)
M.look_up_label = look_up_label

local function entries(s: any, more: ((s: any) -> { any })?): { any }
	local list: { any } = {}
	if has_looks_up and looks_up:peek() then
		table.insert(list, {
			label = look_up_label(s.text),
			icon = "search",
			on_select = function()
				pcall(ui.invoke, "look_up_selection", nil)
			end,
		})
	end
	table.insert(list, {
		label = locale.s("common.copy"),
		icon = "copy",
		shortcut = "cmd+c",
		on_select = function()
			pcall(ui.invoke, "copy_selection", nil)
		end,
	})
	table.insert(list, {
		label = locale.s("ds.selection_menu.select_all"),
		icon = "wrap-text",
		shortcut = "cmd+a",
		on_select = function()
			pcall(ui.invoke, "select_all", nil)
		end,
	})
	local added = if more then more(s) else {}
	if #added > 0 then
		table.insert(list, { separator = true })
		for _, entry in added do
			table.insert(list, entry)
		end
	end
	return list
end

return setmetatable(M, {
	__call = function(_, props: { [any]: any }): Node
		local box = table.clone(props)
		local more = box.more
		box.more = nil
		box.selectable = true
		box.label = box.label or locale.t("ds.selection_menu.selection")
		box.items = function(at: any): { any }?
			local s = M.read()
			if not s or not M.on(s, at.x, at.y) then
				return nil
			end
			return entries(s, more)
		end
		return ContextMenu(box)
	end,
}) :: any
