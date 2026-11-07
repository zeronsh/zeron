--!strict
-- `VAL-WIN-010` (a): a fixture whose manifest declares no permissions at
-- all. It asks for a native window once, is refused with a sentence naming
-- `imgui.windows`, and stays loaded: a refused permission is not a crash and
-- no window may appear.
--
-- Run it (the log line is the refusal's text):
--   ZERON_ZGX_DEV_PLUGINS=<this folder> ZERON_ZGX_PLUGIN_LOG=1 \
--     target/debug/zeron-zgx.exe gallery

local zeron = require("./zeron")("imgui-win-probe-noperm")

local function say(what: string)
	print(`probe-noperm: {what}`)
end

local ok, why = pcall(function()
	zeron.imgui.open_window { id = "x", title = "Denied" }
end)

say(`imgui refused={tostring(not ok)} error={tostring(why)}`)

-- Still running: this timer is the proof the plugin was not withdrawn.
local beats = 0
ui.every(1000, function()
	beats += 1
	say(`alive {beats}`)
end)
