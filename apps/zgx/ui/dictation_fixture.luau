-- Real composer/editor with deterministic host events; never opens a microphone.
local ds = require("@ds")
ds.setup()
local D = require("@app/screens/chat/dictation")
local prefs = require("@app/screens/settings/prefs")
prefs.dictation:set(true)
local value = ui.state("dictation_draft", "")
local key = ui.state("dictation_target", "one")
local sent = ui.state("dictation_sent", "")
local serial = 0
local function send(text: string) sent:set(text); value:set("") end
local dictation = D.bind(value, function() return key:get() end, function() return false end, "main")
ui.command { id = "begin", run = function()
	serial += 1
	D.begin(serial)
	D.state:set({ serial = serial, phase = "listening", target = "main" })
end }
ui.command { id = "final", run = function() D.state:set({ serial = serial, phase = "final", text = "你好 🌎" }) end }
ui.command { id = "empty", run = function() D.state:set({ serial = serial, phase = "final", text = " " }) end }
ui.command { id = "send_other", run = function() D.before_send("another-composer") end }
ui.command { id = "cancel", run = D.cancel }
ui.command { id = "move", run = function() key:set("two") end }
-- The voice bar's waveform: the host's levels (newest first) and its phases.
ui.command { id = "listen", run = function() D.state:set({ serial = serial, phase = "listening", target = "main" }) end }
ui.command { id = "levels", run = function() D.levels:set({ 1, 0.5, 0 }) end }
ui.command { id = "starting", run = function() D.state:set({ serial = serial, phase = "starting", target = "main" }) end }
ui.command { id = "finalizing", run = function() D.state:set({ serial = serial, phase = "finalizing", target = "main" }) end }
ui.command { id = "fail", run = function() D.state:set({ serial = serial, phase = "failed", error = "Unavailable" }) end }
return ui.column {
	w_full = true, h_full = true, justify = "end", p = "$space.4",
	ds.Composer {
		running = ui.state("dictation_running", false),
		model = ui.state("dictation_agent_model", nil :: string?),
		effort = ui.state("dictation_effort", nil :: string?), harnesses = {}, models = {},
		dictation = dictation, value = value, on_send = send,
	},
}
