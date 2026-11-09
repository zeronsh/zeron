-- Real shell, panel tabs and composers over the in-memory files host.
require("@app/files_stub")
local shell = require("@app/shell_fixture")
local Panel = require("@app/screens/panel")
local lanes = require("@app/screens/chat/lanes")
local shared = require("@app/screens/chat/shared")
local state = require("@app/screens/panel/state")

ui.command { id = "context.file", title = "Open context file", run = function()
	Panel.open_file("a1", "notes.md")
end }
ui.command { id = "context.side", title = "Open source side chat", run = function()
	lanes.open("a1", { kind = "side_chat", id = "s1", title = "Look into the flake" })
end }
ui.command { id = "context.other", title = "Open destination side chat", run = function()
	lanes.open("a1", { kind = "side_chat", id = "s2", title = "Write the changelog" })
end }
ui.command { id = "context.unsent", title = "Open unsent side chat", run = function()
	lanes.new_side_chat("a1")
end }
ui.command { id = "context.pick", title = "Pick another file in the existing tab", run = function()
	local Files = require("@app/screens/files")
	for _, tab in state.of("chat:a1").tabs do
		if tab.kind == "files" then
			Files.show_in_tab(tab.id, "long.txt")
		end
	end
end }
local snapshot = ui.state("context.snapshot", {} :: any)
ui.command { id = "context.snapshot", title = "Read context drafts and tabs", run = function()
	snapshot:set({
		main = shared.peek_cargo("a1"),
		side = shared.peek_cargo("s2"),
		source = shared.peek_cargo("s1"),
		tabs = state.of("chat:a1"),
	})
end }

return shell
