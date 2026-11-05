-- A reply's activity group between its words, the reply selectable as the
-- transcript's is: what src/chat.rs's selection tests press on. Here, beside
-- app.luau, as a mounted script loads modules from its own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Activity = require("@ds/activity")
local Markdown = require("@ds/markdown")

local live = ui.shared("activity_live", false)
return ui.column {
	selectable = true,
	w_full = true,
	h_full = true,
	p = "$space.6",
	gap = "$space.3",
	Markdown { text = "Some reply text before the tools, long enough to select a word in it." },
	Activity {
		tools = function()
			return {
				{ id = "t1", kind = "read", verb = "Read", target = "chat.rs" },
				{ id = "t2", kind = "exec", verb = "Ran", target = "cargo test" },
			}
		end,
		live = function()
			return live:get()
		end,
	},
	Markdown { text = "And more words after them, which a stray press could select." },
}
