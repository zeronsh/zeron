-- Dictation: hold the microphone or its shortcut, speak, let go, and the words
-- land in the draft you can still edit (src/dictation.rs records and
-- transcribes; Settings → Voice turns it on). Anything that changes the draft
-- while it's listening throws the recording away, and Send waits for the words.
--
--   local dictation = Dictation.bind(draft, key, blocked, "main")
--   ds.Composer { dictation = dictation, … }
--
-- `key` names what the draft belongs to (another chat, another queued message:
-- another key), `blocked` says it can't be dictated into now.
local Button = require("@ds/button")
local Icon = require("@ds/icon")
local toast = require("@ds/toast")
local keys = require("@app/screens/shell/keys")
local prefs = require("@app/screens/settings/prefs")
local route = require("@app/screens/shell/route")

local M = {}
M.state = ui.shared("dictation", { phase = "idle" } :: any)
M.active = ui.computed(function(): boolean
	local p = M.state:get().phase
	return p == "starting" or p == "listening" or p == "finalizing"
end)
-- The voice bar's waveform: the microphone's loudness, one bar a 60 ms slot,
-- newest first (0 to 1), from the host as each slot fills while listening.
-- On its own, so what reads `state` doesn't run again at each slot.
M.levels = ui.shared("dictation_levels", {} :: { number })
-- starting, listening or finalizing (else what it last was): the waveform's
-- mode, apart from the rest of `state`.
M.phase = ui.computed(function(): string
	return M.state:get().phase
end)
local function active_for(id: string): any
	return ui.computed(function(): boolean
		return M.active:get() and (M.state:get().target or "main") == id
	end)
end

local function say(body: string?)
	toast.show { title = ui.t("chat.dictation.failed"):get(), body = body, tone = "warning" }
end
local function op(name: string)
	ui.call("dictation.call", { op = name }):after(function() end, function() end)
end

local binding: any = nil
local bindings: { any } = {}
local pending: any = nil
local send_pressed = false

local function valid(p: any): boolean
	if binding == nil or p.binding ~= binding or binding.key() ~= p.key or binding.blocked() then return false end
	if binding.value:peek() ~= p.text then return false end
	local el = binding.editor:peek()
	if el == nil then return false end
	local a, b = el:selection_bytes()
	return a == p.a and b == p.b
end
function M.cancel()
	local capturing = pending ~= nil or M.active:peek()
	pending = nil
	if capturing then op("cancel") end
end
function M.begin(serial: number, target: string?)
	if target then
		binding = nil
		for _, own in bindings do
			if own.id == target then binding = own; break end
		end
	end
	pending = nil
	if binding == nil or binding.blocked() then op("cancel") return end
	local el = binding.editor:peek()
	if el == nil then op("cancel") return end
	local a, b = el:selection_bytes()
	if a == nil or b == nil then op("cancel") return end
	el:focus()
	pending = { serial = serial, since = ui.now(), binding = binding, key = binding.key(), text = binding.value:peek(), a = a, b = b, send = false }
	ui.invoke("dictation.guard", { target = binding.id })
end
function M.before_send(target: string?): boolean
	if pending and target and pending.binding.id ~= target then
		M.cancel()
		return false
	end
	if pending and M.active:peek() then
		if not valid(pending) then M.cancel() return true end
		pending.send = true
		op("stop")
		return true
	end
	return M.active:peek()
end

-- Holding. A tap explains the gesture; it never inserts or sends anything.
local TAP = 300
local starting, held, since = false, false, 0
local from_shortcut = ui.state("dictation_shortcut_held", false)
local function finish()
	held = false
	from_shortcut:set(false)
	if starting or M.active:peek() then
		local tap = ui.now() - since < TAP
		op(if tap then "cancel" else "stop")
		if tap then toast.show { title = ui.t("chat.dictation.hold_hint"):get() } end
	end
end
local function start(combo: string?, target: string?)
	-- (Not on yet: Settings → Voice says how.)
	if not prefs.dictation:peek() then route.settings("voice") return end
	if starting or M.active:peek() then return end
	starting, held, since = true, true, ui.now()
	from_shortcut:set(combo ~= nil and string.find(combo, "+", 1, true) ~= nil)
	local device = prefs.dictation_input:peek()
	ui.call("dictation.call", { op = "start", device = if device == "system" then nil else device, hold = combo, target = target }):after(function(r: any)
		starting = false
		M.begin(r.serial, target)
		if not held then op(if ui.now() - since < TAP then "cancel" else "stop") end
	end, function(err: any)
		starting, held = false, false
		from_shortcut:set(false)
		local why = if type(err) == "table" then tostring(err.message) else tostring(err)
		if why == "no_model" then route.settings("voice") else say(why) end
	end)
end
-- A Command chord may produce no key-up on macOS. Modifier state still
-- changes, including when the window loses focus.
ui.effect(function()
	if not from_shortcut:get() then return end
	local m = ui.modifiers
	local cmd = if string.find(ui.shortcut_text("cmd"), "⌘", 1, true) then m.meta:get() else m.control:get()
	local ctrl, alt, shift = m.control:get(), m.alt:get(), m.shift:get()
	local combo = keys.combo("shell.dictate")
	if (string.find(combo, "cmd+", 1, true) and not cmd)
		or (string.find(combo, "ctrl+", 1, true) and not ctrl)
		or (string.find(combo, "alt+", 1, true) and not alt)
		or (string.find(combo, "shift+", 1, true) and not shift) then finish() end
end)
local command = keys.get("shell.dictate") :: any
ui.command {
	id = command.id, title = keys.title(command), menu = command.menu, key = keys.key(command.id),
	run = function() start(keys.combo(command.id)) end,
}
local function Mic(target: string): Node
	local released = -1000
	return Button {
		id = "dictation-microphone", label = ui.t("chat.dictation.hold"), tone = "ghost", shape = "pill", size = "sm", icon = true,
		on_press = function() start(nil, target) end,
		on_release = function() released = ui.now(); finish() end,
		on_key = function(e)
			if e.key == "Escape" then M.cancel(); return true end
			if e.key == "Enter" or e.key == "Space" then if not e["repeat"] then start(e.key, target) end; return true end
			return false
		end,
		on_key_up = function(e)
			if e.key == "Enter" or e.key == "Space" then finish(); return true end
			return false
		end,
		-- An accessible click has no held press: once to start, again to finish.
		on_click = function()
			if ui.now() - released < 100 then return end
			if M.active:peek() then op("stop") else start(nil, target) end
		end,
		Icon { name = "microphone", size = 18 },
	}
end

-- What `ds.Composer` takes as `dictation`.
function M.bind(value: any, key: () -> string, blocked: () -> boolean, id: string): any
	local own: any = {
		editor = ui.ref(), value = value, key = key, blocked = blocked, id = id,
		dictating = active_for(id),
		mic = function(): Node return Mic(id) end,
		levels = M.levels, phase = M.phase,
		status = function(): string
			local p = M.state:get().phase
			return ui.t(if p == "finalizing" then "chat.dictation.transcribing" elseif p == "starting" then "chat.dictation.starting" else "ds.composer.listening"):get()
		end,
		cancel = M.cancel,
		on_change = function() if pending then M.cancel() end end,
		on_caret = function() if pending and not valid(pending) then M.cancel() end end,
		-- (Pressing Send takes the focus; that's no reason to stop.)
		on_blur = function() if not send_pressed and pending then M.cancel() end end,
		on_send_press = function()
			send_pressed = true
			ui.after(0, function() send_pressed = false end)
		end,
		on_key_up = function(e: any): boolean
			local hold = M.state:peek().hold
			if not (pending and hold) or type(e.key) ~= "string" then return false end
			if string.lower(e.key) ~= string.lower(string.match(hold, "([^+]+)$") or hold) then return false end
			finish()
			return true
		end,
		before_send = function(): boolean return M.before_send(id) end,
		-- `send`: the composer's own, set as it's built.
	}
	own.on_focus = function()
		if binding ~= own then
			if pending then M.cancel() end
			binding = own
		end
	end
	table.insert(bindings, own)
	if binding == nil then binding = own end
	ui.effect(function()
		value:get()
		key()
		local no = blocked()
		if pending and pending.binding == own and (no or not valid(pending)) then M.cancel() end
	end)
	ui.effect(function()
		if ui.window.active:get() == false and pending then M.cancel() end
	end)
	ui.on_cleanup(function()
		local at = table.find(bindings, own)
		if at then table.remove(bindings, at) end
		if binding == own then
			binding = nil
			M.cancel()
		end
	end)
	return own
end

ui.effect(function()
	local s = M.state:get()
	local p = pending
	if p == nil then return end
	if s.serial ~= p.serial then M.cancel() return end
	if s.phase == "final" then
		local ok = valid(p)
		pending = nil
		if not ok then op("cancel") return end
		local text = s.text or ""
		if not string.find(text, "%S") then
			toast.show { title = ui.t("chat.dictation.no_speech"):get(), tone = "warning" }
			op("cancel")
			return
		end
		-- One paste is one native undo operation, selection replaced included.
		local applied = p.binding.editor:peek():replace_selection(text)
		if applied and p.send then
			-- (The paste reaches the draft's state after this effect. Retiring
			-- the words waits too: it runs this effect again, which would stop
			-- the timer.)
			local a, b = math.min(p.a, p.b), math.max(p.a, p.b)
			local expected = string.sub(p.text, 1, a) .. text .. string.sub(p.text, b + 1)
			ui.after(0, function()
				if binding == p.binding and binding.key() == p.key and not binding.blocked() and binding.value:peek() == expected then
					binding.send()
				end
				op("cancel")
			end)
			return
		end
		op("cancel") -- The words are in the draft: nothing keeps them.
	elseif s.phase == "failed" or s.phase == "cancelled" then
		pending = nil
		if s.phase == "failed" then say(s.error) end
	end
end)
return M
