-- An editor's context menu (a field, a text area, the composer, the code
-- editor): the design system's (ds/context_menu's panel and rows, opened by
-- ds/menu_at), not zgx's own, so a secondary click in text looks like every
-- other menu. zgx gives an editor its own menu only when it has none, so
-- this sets the editor's `context_menu`; the host does what zgx's menu did
-- (src/editor_menu.rs: Look Up, Cut, Copy, Paste, Select All, each enabled
-- as zgx's are), for the editor at the click.
--
--   ui.textarea(EditorMenu.on({ … }))                 -- a field's props
--   EditorMenu.on(props, { takes_paste = true, more = function(state) … end })
--
-- `takes_paste`: the editor hears pastes with no text (a picture: the
-- composer's), so Paste stays on with no text on the clipboard. `more`: the
-- place's own entries after a rule, given what the menu knows (`State`).
-- An editor with a `context_menu` of its own keeps it.
--
-- (Not yet: the spelling checker's guesses for a misspelled word, which zgx's
-- own menu had. zgx keeps them to itself: zgx-asks/editor-menu-items.)

local MenuAt = require("@self/menu_at")
local SelectionMenu = require("@self/selection_menu")
local locale = require("@self/locale")

local M = {}

-- What the host says of the editor the menu is about.
export type State = {
	selected: string,
	copyable: boolean,
	editable: boolean,
	clipboard: boolean,
	platform: boolean,
	looks_up: boolean,
}
export type Options = { takes_paste: boolean?, more: ((state: State) -> { any })? }

-- (A plugin lent the design system has no host for it: its editors keep
-- zgx's own menu.)
local has_said, said = pcall(ui.shared, "editor_menu", nil :: any)

local function run(action: string): () -> ()
	return function()
		pcall(ui.invoke, `editor_menu.{action}`, nil)
	end
end

-- The menu's entries for an editor in `state`, as zgx's own menu enables
-- them: Look Up with something selected (where the platform looks words
-- up); Cut with a selection to copy in an editor that edits; Copy with a
-- selection (a password field's copies nothing); Paste in one that edits,
-- with text on the clipboard (or any paste, when it takes pictures).
function M.entries(state: State, options: Options?): { any }
	local o = options or {}
	local list: { any } = {}
	if state.looks_up and state.copyable then
		table.insert(list, {
			label = SelectionMenu.look_up_label(state.selected),
			icon = "search",
			on_select = run("look_up"),
		})
		table.insert(list, { separator = true })
	end
	table.insert(list, {
		label = locale.s("ds.editor_menu.cut"),
		icon = "scissors",
		shortcut = "cmd+x",
		disabled = not (state.copyable and state.editable and state.platform),
		on_select = run("cut"),
	})
	table.insert(list, {
		label = locale.s("common.copy"),
		icon = "copy",
		shortcut = "cmd+c",
		disabled = not (state.copyable and state.platform),
		on_select = run("copy"),
	})
	table.insert(list, {
		label = locale.s("ds.editor_menu.paste"),
		icon = "clipboard",
		shortcut = "cmd+v",
		disabled = not state.editable or (not state.clipboard and o.takes_paste ~= true),
		on_select = run("paste"),
	})
	table.insert(list, {
		label = locale.s("ds.selection_menu.select_all"),
		icon = "wrap-text",
		shortcut = "cmd+a",
		on_select = run("select_all"),
	})
	local added = if o.more then o.more(state) else {}
	if #added > 0 then
		table.insert(list, { separator = true })
		for _, entry in added do
			table.insert(list, entry)
		end
	end
	return list
end

-- The menu on an editor's props (changed in place, and returned).
function M.on(props: { [any]: any }, options: Options?): { [any]: any }
	if not has_said or props.context_menu ~= nil then
		return props
	end
	props.context_menu = function(e: PointerEvent): { Node }?
		-- (The editor at the click, or the one with focus from the keys.)
		if not pcall(ui.invoke, "editor_menu.at", { x = e.x, y = e.y }) then
			return nil
		end
		local state = said:peek()
		if not state then
			return nil
		end
		return MenuAt.show(e, M.entries(state, options))
	end
	return props
end

return M
