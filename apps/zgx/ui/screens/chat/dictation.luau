-- The host owns draft safety. No plugin receives the draft, selection or editor.
local M = {}
M.state = ui.shared("dictation", { phase = "idle" } :: any)
M.active = ui.computed(function(): boolean
	local p = M.state:get().phase
	return p == "starting" or p == "listening" or p == "finalizing"
end)
function M.active_for(id: string): any
	return ui.computed(function(): boolean
		local s = M.state:get()
		return M.active:get() and (s.target or "main") == id
	end)
end
M.notice = ui.state("dictation_notice", nil :: any)
function M.notice_for(id: string): any
    return ui.computed(function(): any
        local n = M.notice:get()
        return if n and (n.target or "main") == id then n else nil
    end)
end
local binding: any = nil
local bindings: { any } = {}
local pending: any = nil
local send_pressed = false
local function control(op: string)
	ui.invoke("dictation.control", { op = op })
end
function M.send_press()
	send_pressed = true
	ui.after(0, function() send_pressed = false end)
end
function M.blur()
	if not send_pressed and pending then M.cancel() end
end
function M.key_up(e: any): boolean
	local s = M.state:peek()
	if pending and s.hold then
		local key = string.match(s.hold, "([^+]+)$") or s.hold
		if string.lower(e.key) ~= string.lower(key) then return false end
		control(if ui.now() - (pending.since or 0) < 300 then "cancel" else "stop")
		return true
	end
	return false
end
function M.cancel()
	local capturing = pending ~= nil or M.active:peek()
	pending = nil
	if capturing then control("cancel") end
end
local function valid(p: any): boolean
	if binding == nil or p.binding ~= binding or binding.key() ~= p.key or binding.blocked() then return false end
	if binding.value:peek() ~= p.text then return false end
	local el = binding.field:peek()
	if el == nil then return false end
	local a, b = el:selection_bytes()
	return a == p.a and b == p.b
end
function M.focus(field: any)
	for _, own in bindings do
		if own.field == field and binding ~= own then
			if pending then M.cancel() end
			binding = own
			return
		end
	end
end
function M.begin(owner: string, serial: number, target: string?)
	if target then
		local found = false
		for _, own in bindings do
			if own.id == target then binding = own; found = true; break end
		end
		if not found then M.cancel(); return end
	end
	pending = nil
	M.notice:set(nil)
	if binding == nil or binding.blocked() then control("cancel") return end
	local el = binding.field:peek()
	if el == nil then control("cancel") return end
	local a, b = el:selection_bytes()
	if a == nil or b == nil then control("cancel") return end
	el:focus()
	pending = { owner = owner, serial = serial, since = ui.now(), binding = binding, key = binding.key(), text = binding.value:peek(), a = a, b = b, send = false }
	ui.invoke("dictation.guard", { target = binding.id })
end
function M.caret()
	if pending and not valid(pending) then M.cancel() end
end
function M.changed()
	if pending then M.cancel() end
end
function M.before_send(target: string?): boolean
    if pending and target and pending.binding.id ~= target then
        M.cancel()
        return false
    end
	if pending and M.active:peek() then
		if not valid(pending) then M.cancel() return true end
		pending.send = true
		control("stop")
		return true
	end
	return M.active:peek()
end
function M.bind(field: any, value: any, key: () -> string, blocked: () -> boolean, send: (string) -> (), id: string?)
	local own = { field = field, value = value, key = key, blocked = blocked, send = send, id = id }
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
end
ui.effect(function()
	local s = M.state:get()
	local p = pending
	if p == nil then return end
	if s.owner ~= p.owner or s.serial ~= p.serial then M.cancel() return end
	if s.phase == "final" then
		local ok = valid(p)
		pending = nil
		if not ok then control("cancel") return end
		local text = s.text or ""
		if not string.find(text, "%S") then
			M.notice:set({ tone = "warning", text = "No speech recognized. Your draft is unchanged.", target = p.binding.id, dismissable = true })
			control("cancel")
			return
		end
		local el = p.binding.field:peek()
		-- One paste is one native undo operation, including selection replacement.
		local applied = el and el:replace_selection(text)
		if applied and p.send then
			-- Native paste publishes its bound state after this effect returns.
			local a, b = math.min(p.a, p.b), math.max(p.a, p.b)
			local expected = string.sub(p.text, 1, a) .. text .. string.sub(p.text, b + 1)
			ui.after(0, function()
				if binding == p.binding and binding.key() == p.key and not binding.blocked() and binding.value:peek() == expected then
					binding.send(expected)
				end
			end)
		end
		control("cancel") -- Retire the transient transcript after consuming it.
	elseif s.phase == "failed" or s.phase == "cancelled" then
		pending = nil
		if s.phase == "failed" then M.notice:set({ tone = "warning", text = s.error or "Dictation failed. Your draft is safe.", target = p.binding.id, dismissable = true }) end
	end
end)
return M
