-- The Control room's composer floating over the page, for its pictures:
-- the Map or the Board, a draft of one line or six, three pictures on it;
-- and what floats with it, Capture's history and Needs you, on every tab.
--   control_composer@2530x1378|!theme_dark|!fx.map|!fx.lines1|+500|~map-1line
local built = require("./control_fixture")
local function draft(s: string)
	require("@app/screens/chat/lane_tab").draft("orc-brain"):set(s)
end
ui.command { id = "fx.map", title = "Fixture: the Map", run = function()
	require("@app/screens/control/page").show_tab("map")
end }
ui.command { id = "fx.board", title = "Fixture: the Board", run = function()
	require("@app/screens/control/page").show_tab("board")
end }
ui.command { id = "fx.notes", title = "Fixture: Notes", run = function()
	require("@app/screens/control/page").show_tab("notes")
end }
ui.command { id = "fx.chat", title = "Fixture: Chat", run = function()
	require("@app/screens/control/page").show_tab("chat")
end }
ui.command { id = "fx.lines1", title = "Fixture: a draft of one line", run = function()
	draft("a lot of empty space on the map in control room.")
end }
ui.command { id = "fx.lines6", title = "Fixture: a draft of six lines", run = function()
	draft("a lot of empty space on the map in control room.\n\nweird spacing when composer gets expanded, should not shift elements on the page\nbut just get on top?\n\nand the more you increase the number of lines, larger the gap.")
end }
ui.command { id = "fx.attach3", title = "Fixture: three pictures on the draft", run = function()
	require("@app/screens/chat/shared").attach_files({
		{ name = "zgx.png", path = "assets/favicons/zgx.png" },
		{ name = "zeron.png", path = "assets/favicons/zeron.png" },
		{ name = "marigold.png", path = "assets/favicons/marigold.png" },
	}, "orc-brain")
end }
ui.command { id = "fx.history", title = "Fixture: the history opened", run = function()
	require("@app/screens/control/page").history_panel:set(true)
end }
ui.command { id = "fx.needs_shut", title = "Fixture: Needs you minimised", run = function()
	require("@app/screens/control/page").needs_min:set(true)
end }
ui.command { id = "fx.needs_open", title = "Fixture: Needs you opened", run = function()
	require("@app/screens/control/page").needs_min:set(false)
end }
return built
