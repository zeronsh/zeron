-- The host owns draft safety. No plugin receives the draft, selection or editor.
local M = {}
M.state = ui.shared("dictation", { phase = "idle" } :: any)
M.active = ui.computed(function(): boolean
    local p = M.state:get().phase
    return p == "starting" or p == "listening" or p == "finalizing"
end)
M.notice = ui.state("dictation_notice", nil :: any)
local binding: any = nil
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
        control("stop")
        return true
    end
    return false
end
function M.cancel()
    pending = nil
    control("cancel")
end
local function valid(p: any): boolean
    if binding == nil or p.binding ~= binding or binding.key() ~= p.key or binding.blocked() then return false end
    if binding.value:peek() ~= p.text then return false end
    local el = binding.field:peek()
    if el == nil then return false end
    local a, b = el:selection_bytes()
    return a == p.a and b == p.b
end
function M.begin(owner: string, serial: number)
    pending = nil
    M.notice:set(nil)
    if binding == nil or binding.blocked() then control("cancel") return end
    local el = binding.field:peek()
    if el == nil then control("cancel") return end
    local a, b = el:selection_bytes()
    if a == nil or b == nil then control("cancel") return end
    el:focus()
    pending = { owner = owner, serial = serial, binding = binding, key = binding.key(), text = binding.value:peek(), a = a, b = b, send = false }
end
function M.caret()
    if pending and not valid(pending) then M.cancel() end
end
function M.changed()
    if pending then M.cancel() end
end
function M.before_send(): boolean
    if pending and M.active:peek() then
        if not valid(pending) then M.cancel() return true end
        pending.send = true
        control("stop")
        return true
    end
    return M.active:peek()
end
function M.bind(field: any, value: any, key: () -> string, blocked: () -> boolean, send: (string) -> ())
    local own = { field = field, value = value, key = key, blocked = blocked, send = send }
    binding = own
    ui.effect(function()
        value:get()
        key()
        local no = blocked()
        if pending and (no or not valid(pending)) then M.cancel() end
    end)
    ui.effect(function()
        if ui.window.active:get() == false and pending then M.cancel() end
    end)
    ui.on_cleanup(function()
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
            M.notice:set({ tone = "warning", text = "No speech recognized. Your draft is unchanged.", dismissable = true })
            return
        end
        local el = p.binding.field:peek()
        -- One paste is one native undo operation, including selection replacement.
        local applied = el and el:replace_selection(text)
        if applied and p.send then p.binding.send(p.binding.value:peek()) end
        control("cancel") -- Retire the transient transcript after consuming it.
    elseif s.phase == "failed" or s.phase == "cancelled" then
        pending = nil
        if s.phase == "failed" then M.notice:set({ tone = "warning", text = s.error or "Dictation failed. Your draft is safe.", dismissable = true }) end
    end
end)
return M
