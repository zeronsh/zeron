-- A reply as the chat bridge lays it out (src/chat.rs `message`, handed in
-- as the shared `transcript_messages`), drawn by ds/transcript: for
-- src/visuals_tests.rs's check that display tools (`show_visual`,
-- `show_image`) show as their output alone, with no tool row.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local T = require("@ds/transcript")

local messages = ui.shared("transcript_messages", {} :: { any })

-- (Inside a component: the transcript keeps state of its own.)
return ui.component(function(): Node
	return ui.column {
		id = "conversation",
		w_full = true,
		h_full = true,
		T.Transcript { messages = messages },
	}
end)({})
