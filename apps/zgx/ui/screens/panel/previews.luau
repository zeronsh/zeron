--!strict
-- The dev servers running for a session (the engine's previews: src/
-- previews.rs, the shared state `previews`), for whoever shows them: the
-- panel's home (its Ports) and a browser tab's address suggestions. The
-- host watches one chat at a time, so the watch is shared here: it's kept
-- while anyone uses it, the chat asked for last the one watched.
--
--   previews.use(chat)       watch `chat` while in use: gives back the
--                            release (call it as the user goes, an
--                            `ui.on_cleanup`)
--   previews.services(chat)  its services (tracked): { id, name, port, url,
--                            project, device, cwd, pid, started, process,
--                            command }
--   previews.looking(chat)   asked, no answer yet (tracked)
--   previews.trouble(chat)   why local previews can't open now (the
--                            engine's words, for a tooltip or the log;
--                            never shown as they are), else nil (tracked)
--   previews.hold(chat)      its services as a list shows them: settled,
--                            and kept a while once closed (`closed`); see
--                            `M.hold`

local M = {}

local shared = ui.shared("previews", nil :: any)

-- Who uses which chat now, in the order they asked (the last one wins).
local users: { { chat: string, n: number } } = {}
local watched: string? = nil

local function settle()
	local want = if #users > 0 then users[#users].chat else nil
	if want == watched then
		return
	end
	watched = want
	pcall(function()
		if want then
			ui.invoke("previews.watch", { chatId = want })
		else
			ui.invoke("previews.unwatch", nil)
		end
	end)
end

function M.use(chat: string): () -> ()
	local me = { chat = chat, n = 1 }
	table.insert(users, me)
	settle()
	local released = false
	return function()
		if released then
			return
		end
		released = true
		local at = table.find(users, me)
		if at then
			table.remove(users, at)
		end
		settle()
	end
end

local function of(chat: string?): any?
	local p = shared:get()
	if type(p) ~= "table" or chat == nil or p.chat ~= chat then
		return nil
	end
	return p
end

function M.services(chat: string?): { any }
	local p = of(chat)
	return if p and type(p.services) == "table" then p.services else {}
end

function M.looking(chat: string?): boolean
	local p = of(chat)
	return p ~= nil and p.ready ~= true
end

function M.trouble(chat: string?): string?
	local p = of(chat)
	if p and type(p.error) == "string" and p.error ~= "" then
		return p.error
	end
	return nil
end

-- How long a port stays open before it's listed (a test's server, open a
-- moment, never is), and how long one listed stays after it closes (a dev
-- server restarting doesn't blink out and back).
M.SETTLE_MS = 5000
M.GRACE_MS = 10000
-- (The hold's clock: its timer ticks this often while a port is settling
-- or closing, and only then.)
local TICK_MS = 250

-- `chat`'s services as a list shows them (call it while building: it keeps
-- state): each once it's been open `SETTLE_MS`, and on for `GRACE_MS`
-- after it closes, `closed` set. What's open as the chat's first answer
-- comes, its process older than the settling, shows at once (it was open
-- before anyone looked). A port is its service on its port: a test
-- reopening its server on a new port each run is a new one each time.
-- Tracked: gives back { service..., closed: true? }, in the order each
-- first came.
function M.hold(chat: () -> string?): () -> { any }
	local shown = ui.use_state({} :: { any })
	-- Something's settling or closing: the clock runs.
	local busy = ui.use_state(false)
	local clock = 0
	local seq = 0
	local seeded = false
	local said = ""
	local held: { [string]: { sv: any, open: boolean, shown: boolean, due: number?, order: number } } = {}
	local function publish()
		local list = {}
		local pending = false
		for _, e in held do
			if e.shown then
				table.insert(list, e)
			end
			if e.due then
				pending = true
			end
		end
		table.sort(list, function(a, b)
			return a.order < b.order
		end)
		local out = {}
		local says = {}
		for _, e in list do
			local row = table.clone(e.sv)
			row.closed = if e.open then nil else true
			table.insert(out, row)
			local sv = e.sv
			table.insert(says, `{sv.id}:{sv.port}:{e.open}:{sv.name}:{sv.url}:{sv.device}:{sv.process}:{sv.command}`)
		end
		-- (Only what changes what it shows: the clock's ticks set nothing.)
		local sig = table.concat(says, "\n")
		if sig ~= said then
			said = sig
			shown:set(out)
		end
		busy:set(pending)
	end
	ui.effect(function()
		local p = of(chat())
		if p == nil or p.ready ~= true then
			-- (Nothing known of it now, another chat's or none: nothing held,
			-- and its next answer is a first one again.)
			table.clear(held)
			seeded = false
			publish()
			return
		end
		local first = not seeded
		seeded = true
		local now = ui.now()
		local here: { [string]: boolean } = {}
		for _, sv in (if type(p.services) == "table" then p.services else {}) :: { any } do
			local key = `{sv.id}:{sv.port}`
			here[key] = true
			local e = held[key]
			if e == nil then
				seq += 1
				local started = tonumber(sv.started) or 0
				local at_once = M.SETTLE_MS <= 0 or (first and started > 0 and now - started >= M.SETTLE_MS)
				held[key] = {
					sv = sv,
					open = true,
					shown = at_once,
					due = if at_once then nil else clock + M.SETTLE_MS,
					order = seq,
				}
			else
				e.sv = sv
				if not e.open then
					-- Back while closing: open, as it was.
					e.open = true
					e.due = nil
				end
			end
		end
		for key, e in held do
			if not here[key] and e.open then
				e.open = false
				if e.shown and M.GRACE_MS > 0 then
					e.due = clock + M.GRACE_MS
				else
					held[key] = nil
				end
			end
		end
		publish()
	end)
	-- Each port's time come: listed, or gone.
	ui.effect(function()
		if not busy:get() then
			return
		end
		ui.every(TICK_MS, function()
			clock += TICK_MS
			for key, e in held do
				if e.due and clock >= e.due then
					e.due = nil
					if e.open then
						e.shown = true
					else
						held[key] = nil
					end
				end
			end
			publish()
		end)
	end)
	return function(): { any }
		return shown:get()
	end
end

return M
