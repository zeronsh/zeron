-- Dictation policy and controls. The native host owns audio and draft safety.
local zeron = require("./zeron")("dictation")
local Button = require("@ds/button")
local Icon = require("@ds/icon")
local state = zeron.dictation_state()
local model = zeron.setting("model")
local input = zeron.setting("input")
local shortcut = zeron.setting("shortcut")
local starting = false
local downloading = false
local held = false
local since = 0
local from_shortcut = ui.state("dictation_shortcut_held", false)
local function failed(err: any)
	starting = false
	held = false
	from_shortcut:set(false)
	zeron.toast(if type(err) == "table" then tostring(err.message or "Dictation unavailable") else tostring(err))
end
local function call(op: string): any
	return zeron.dictation({ op = op, model = model:peek() })
end
local function active(): boolean
	local s = state:get()
	return s.phase == "starting" or s.phase == "listening" or s.phase == "finalizing"
end
local function refresh()
	call("installed"):after(function(ready: boolean)
		zeron.set_status("model", { badge = if ready then "Installed" else "Download required", tone = if ready then "success" else "neutral" })
	end, failed)
end
local function inputs()
	call("inputs"):after(function(list: { any })
		local options = { { value = "system", label = "System default" } }
		for _, d in list do table.insert(options, { value = d.id, label = d.name }) end
		zeron.set_options("input", options)
	end, failed)
end
local function finish()
	held = false
	from_shortcut:set(false)
	if starting or active() then
		-- A tap explains the gesture; it must never insert or send anything.
		local tap = ui.now() - since < 300
		call(if tap then "cancel" else "stop"):after(function() end, failed)
		if tap then zeron.toast("Hold the microphone or shortcut while speaking.") end
	end
end
local function start(combo: string?, target: string?)
	if starting or active() then return end
	starting = true
	held = true
	since = ui.now()
	from_shortcut:set(combo ~= nil and string.find(combo, "+", 1, true) ~= nil)
	local device = input:peek()
	zeron.dictation({ op = "start", model = model:peek(), device = if device == "system" then nil else device, hold = combo, target = target }):after(function()
		starting = false
		if not held then call(if ui.now() - since < 300 then "cancel" else "stop"):after(function() end, failed) end
	end, failed)
end
-- A Command chord may produce no key-up on macOS. Modifier state still
-- changes, including when the window loses focus.
ui.effect(function()
	local m = ui.modifiers
	local cmd = if string.find(ui.shortcut_text("cmd"), "⌘", 1, true) then m.meta:get() else m.control:get()
	local ctrl, alt, shift = m.control:get(), m.alt:get(), m.shift:get()
	if not from_shortcut:get() then return end
	local combo = shortcut:peek()
	if (string.find(combo, "cmd+", 1, true) and not cmd)
		or (string.find(combo, "ctrl+", 1, true) and not ctrl)
		or (string.find(combo, "alt+", 1, true) and not alt)
		or (string.find(combo, "shift+", 1, true) and not shift) then finish() end
end)
ui.command { id = "dictate", title = "Hold to dictate", menu = "Edit", key = function() return shortcut:get() end, run = function() start(shortcut:peek()) end }
zeron.on_setting_action(function(id: string)
	if id == "refresh-inputs" then inputs() return end
	local op = if id == "download" then "download" elseif id == "remove" then "remove" elseif id == "cancel-download" then "cancel" else nil
	if op then call(op):after(function() refresh() end, failed) end
end)
zeron.on_setting(function(id: string)
	if id == "model" then call("cancel"):after(function() refresh() end, failed) end
end)
ui.effect(function()
	local s = state:get()
	if s.phase == "downloading" then
		downloading = true
		zeron.set_status("download", { badge = `{math.floor((s.bytes or 0) / math.max(1, s.total or 1) * 100)}%`, text = "Downloading model…" })
	elseif s.phase == "ready" then
		downloading = false
		zeron.set_status("download", { badge = "Ready", tone = "success" })
		refresh()
	elseif s.phase == "cancelled" and downloading then
		downloading = false
		zeron.set_status("download", { badge = "Cancelled", tone = "neutral" })
	elseif s.phase == "failed" and downloading then
		downloading = false
		zeron.set_status("download", { text = s.error or "Download failed", tone = "danger" })
	end
end)
zeron.place { at = "slot:zeron.composer.dictation", after = function(): Node
	local target = zeron.context("dictation_target")
	local released = -1000
	return ui.row {
		items = "center", gap = "$space.2",
		ui.show(active, ui.text { function(): string
			local s = state:get()
			return if s.phase == "finalizing" then "Transcribing…" elseif s.phase == "starting" then "Starting…" else "Listening"
		end, text_size = "$font.size.xs", color = "$text.secondary" }),
		Button {
			id = "dictation-microphone", label = "Hold to dictate", tone = "ghost", shape = "pill", size = "sm", icon = true,
			on_press = function() start(nil, target) end,
			on_release = function() released = ui.now(); finish() end,
			on_key = function(e)
				if e.key == "Escape" then call("cancel"):after(function() end, failed); return true end
				if e.key == "Enter" or e.key == "Space" then if not e["repeat"] then start(e.key, target) end; return true end
				return false
			end,
			on_key_up = function(e)
				if e.key == "Enter" or e.key == "Space" then finish(); return true end
				return false
			end,
			-- Accessible Click has no held press: click once to start, again to finish.
			on_click = function()
				if ui.now() - released < 100 then return end
				if active() then call("stop"):after(function() end, failed) else start(nil, target) end
			end,
			Icon { name = "microphone", size = 18 },
		},
	}
end }
call("catalog"):after(function(models: { any })
	local options = {}
	for _, m in models do table.insert(options, { value = m.id, label = m.name }) end
	zeron.set_options("model", options)
end, failed)
refresh()
inputs()
