-- Editors with the design system's text menu (ds/editor_menu), for
-- tests/editor_menu.test.luau. The host's part (src/editor_menu.rs: what the
-- editor at the click can do, and doing it) is stubbed here: the editor's
-- state is what the test sets in `test_editor_state`, and each action asked
-- for is noted in `test_editor_calls`.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local EditorMenu = require("@ds/editor_menu")
local MenuAt = require("@ds/menu_at")

local state = ui.state("test_editor_state", {
	selected = "hello",
	copyable = true,
	editable = true,
	clipboard = true,
	platform = true,
	looks_up = true,
})
local calls = ui.state("test_editor_calls", "")
local said = ui.shared("editor_menu", nil :: any)
local function note(what: string): () -> ()
	return function()
		calls:append(what .. ",")
	end
end
ui.actions {
	["editor_menu.at"] = function()
		said:set(state:peek())
	end,
	["editor_menu.cut"] = note("cut"),
	["editor_menu.copy"] = note("copy"),
	["editor_menu.paste"] = note("paste"),
	["editor_menu.select_all"] = note("select_all"),
	["editor_menu.look_up"] = note("look_up"),
}

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.4",
	gap = "$space.4",
	theme = "zeron-dark",
	bg = "$zeron.menu.solid",
	MenuAt.Host {},
	ui.textarea(EditorMenu.on {
		id = "plain",
		label = "Plain",
		value = ui.state("test_editor_plain", "hello world"),
		w = 320,
		h = 60,
	}),
	ui.textarea(EditorMenu.on({
		id = "pictures",
		label = "Pictures",
		value = ui.state("test_editor_pictures", ""),
		w = 320,
		h = 60,
	}, {
		takes_paste = true,
		more = function()
			return { { label = "Mention", icon = "chat-round-line", on_select = note("mention") } }
		end,
	})),
	ds.TextField { value = ui.state("test_editor_field", "a field"), label = "Field", w = 320 },
}
