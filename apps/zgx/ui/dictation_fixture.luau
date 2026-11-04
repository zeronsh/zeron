-- Real composer/editor with deterministic host events; never opens a microphone.
local ds = require("@ds")
ds.setup()
local D = require("@app/screens/chat/dictation")
local value = ui.state("dictation_draft", "")
local key = ui.state("dictation_target", "one")
local sent = ui.state("dictation_sent", "")
local editor = ui.ref()
local serial = 0
local function send(text: string) sent:set(text); value:set("") end
D.bind(editor, value, function() return key:get() end, function() return false end, send)
ui.command { id = "begin", run = function()
	serial += 1
	D.begin("dictation", serial)
	D.state:set({ owner = "dictation", serial = serial, phase = "listening" })
end }
ui.command { id = "final", run = function() D.state:set({ owner = "dictation", serial = serial, phase = "final", text = "你好 🌎" }) end }
ui.command { id = "empty", run = function() D.state:set({ owner = "dictation", serial = serial, phase = "final", text = " " }) end }
ui.command { id = "send_other", run = function() D.before_send("another-composer") end }
ui.command { id = "cancel", run = D.cancel }
ui.command { id = "move", run = function() key:set("two") end }
ui.command { id = "fail", run = function() D.state:set({ owner = "dictation", serial = serial, phase = "failed", error = "Unavailable" }) end }
return ui.column {
	w_full = true, h_full = true, justify = "end", p = "$space.4",
	ds.Composer {
		running = ui.state("dictation_running", false),
		model = ui.state("dictation_agent_model", nil :: string?),
		effort = ui.state("dictation_effort", nil :: string?), harnesses = {}, models = {},
		editor = editor, value = value, on_send = send, before_send = D.before_send,
		on_editor_focus = function() D.focus(editor) end, on_editor_change = D.changed, on_editor_caret = D.caret, on_editor_blur = D.blur,
		on_send_press = D.send_press, dictating = D.active, on_cancel_dictation = D.cancel,
		notice = D.notice,
	},
}
