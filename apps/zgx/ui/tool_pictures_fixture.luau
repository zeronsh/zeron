-- A reply's work (ds/activity) with a Read of a picture whose result the
-- engine kept a copy of (its `pictures`, the tool's `previewPaths`), a Read
-- of another that kept none, and a command whose result kept two, for
-- tests/tool_pictures.test.luau. The
-- pictures are read from where the transcript reads an agent's shown ones
-- (stubbed: each path is the picture, nothing read).
-- Here, beside app.luau, as a mounted script loads modules from its own
-- directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Activity = require("@ds/activity")
local Lightbox = require("@ds/lightbox")

local pictures = {
	resolve = function(src: string): any
		return { state = "ready", src = src, path = src, here = true }
	end,
	fetch = function(_src: string) end,
}

local TOOLS = {
	{
		id = "r1",
		kind = "read",
		verb = "Read",
		target = "test.png",
		pictures = { { path = "assets/composer/shell.png", name = "test.png" } },
	},
	{ id = "r2", kind = "read", verb = "Read", target = "old.png" },
	{
		id = "r3",
		kind = "exec",
		verb = "Ran",
		target = "shoot both",
		pictures = {
			{ path = "assets/composer/shell.png", name = "left.png" },
			{ path = "assets/composer/transcript.png", name = "right.png" },
		},
	},
}

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.4",
	ui.scroll {
		w_full = true,
		h_full = true,
		Activity {
			key = "work-pictures",
			pictures = pictures,
			tools = function(): { any }
				return TOOLS
			end,
		},
	},
	Lightbox.Host(),
}
