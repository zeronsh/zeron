-- The shell's shortcuts, one list (screens/shell/keys): Zeron's defaults,
-- each id and each key once, each in a group Settings lists.

local keys = require("../screens/shell/keys")

test("each shortcut is one command's, on one key, in a known group", function()
	local ids: { [string]: boolean } = {}
	local taken: { [string]: string } = {}
	local GROUPS = { Sessions = true, Panels = true, Go = true, App = true }
	for _, k in keys.LIST do
		expect(ids[k.id]).to_be(nil)
		ids[k.id] = true
		expect(`{k.key}: {taken[k.key] or "free"}`).to_be(`{k.key}: free`)
		taken[k.key] = k.id
		expect(GROUPS[k.group]).to_be(true)
		expect(keys.get(k.id)).to_be(k)
	end
end)

test("Zeron's defaults", function()
	local want = {
		["shell.toggle_sidebar"] = "cmd+b",
		["shell.toggle_files"] = "cmd+r",
		["shell.toggle_files_tab"] = "cmd+e",
		["shell.toggle_terminal"] = "cmd+j",
		["shell.model_picker"] = "cmd+/",
		["shell.archive"] = "cmd+shift+a",
		["shell.next_session"] = "ctrl+tab",
		["shell.previous_session"] = "ctrl+shift+tab",
		["shell.new_chat"] = "cmd+n",
		["shell.palette"] = "cmd+k",
	}
	for id, key in want do
		local k = keys.get(id)
		expect(k).never.to_be_nil()
		expect(`{id} {(k :: any).key}`).to_be(`{id} {key}`)
	end
	for i = 1, 9 do
		expect((keys.get(`shell.session_{i}`) :: any).key).to_be(`cmd+{i}`)
	end
end)

test("a shortcut another screen makes is listed as it makes it", function()
	-- (The browser tab's annotate: its own command, screens/panel/browser.)
	local k = keys.get("panel.annotate") :: any
	expect(k.key).to_be("cmd+shift+i")
	expect(k.group).to_be("Panels")
	expect(k.elsewhere).to_be(true)
end)
