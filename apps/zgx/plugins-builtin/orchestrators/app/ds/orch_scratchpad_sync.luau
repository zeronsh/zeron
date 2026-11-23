-- The Scratchpad's autosave: a value the person edits (its text, its
-- drawing) kept in step with the engine's copy, with no Save button.
--
--   local c = Sync.channel("text", "", function(v) return v end)
--   c.value          State: what the editor shows (the person's)
--   c.edit(v)        an edit: shown at once, saved ~800 ms after the last
--   c.save_now()     saved now (a blur, a close)
--   c.flush(done)    saved now, `done` once it has gone (or failed)
--   c.heard(v)       the engine's copy as the watch has it now
--   c.adopted        State: counts the engine's copies taken in
--   c.saver          (v) -> promise of a rev: set by what shows it
--   c.remote         () -> the engine's copy, for an ack that came late
--   c.busy           () -> boolean: mid-gesture (no remote change adopted)
--   c.early()        edited before the engine's first copy came (kept,
--                    and saved over it once it does)
--   Sync.offline     State: the engine (or the app) has no Scratchpad, so
--                    nothing saves: an edit reads "failed" ("Not saved")
--
-- A remote change (another device, the Brain dump) shows at once while the
-- person has nothing unsaved; with edits of their own, theirs stay and are
-- saved over it (its history keeps the other). Its state, `c.status`:
-- "saved", "saving" (an edit not gone yet, or going), "syncing" (gone, the
-- watch not showing it yet) or "failed" (kept, tried again with the next
-- edit or blur). The channels share one named state per key, so the
-- compact widget and the full page are one editor.

local M = {}

-- After the last edit, before it is saved.
M.DEBOUNCE = 800

export type Channel = {
	value: State<any>,
	base: State<string?>,
	status: State<string>,
	adopted: State<number>,
	edit: (v: any) -> (),
	save_now: () -> (),
	flush: (done: () -> ()) -> (),
	heard: (remote: any) -> (),
	dirty: () -> boolean,
	saver: ((v: any) -> any)?,
	remote: (() -> any)?,
	busy: () -> boolean,
	early: () -> boolean,
}

local channels: { [string]: Channel } = {}

M.offline = ui.state("ds_scratchpad_offline", false)

function M.channel(key: string, empty: any, sig: (any) -> string): Channel
	if channels[key] then
		return channels[key]
	end
	local value = ui.state(`ds_scratchpad_{key}`, empty)
	-- (The engine's copy the value agrees with, as its signature; nil before
	-- the first frame.)
	local base = ui.state(`ds_scratchpad_{key}_base`, nil :: string?)
	local status = ui.state(`ds_scratchpad_{key}_status`, "saved")
	-- (Counts the engine's copies taken in: what shows them follows it.)
	local adopted = ui.state(`ds_scratchpad_{key}_adopted`, 0)
	local flying = false
	local again = false
	-- (Edited before the engine's first copy: the person's, kept.)
	local early = false
	local timer: (() -> ())? = nil
	local waiting: { () -> () } = {}
	local c: Channel = nil :: any

	local function cancel()
		if timer then
			timer()
			timer = nil
		end
	end
	local function settle()
		if flying then
			return
		end
		local list = waiting
		waiting = {}
		for _, f in list do
			f()
		end
	end
	local function dirty(): boolean
		local b = base:peek()
		return b ~= nil and sig(value:peek()) ~= b
	end
	local function save_now()
		cancel()
		local b = base:peek()
		local save = c.saver
		if b == nil or save == nil then
			return settle()
		end
		if flying then
			again = true
			return
		end
		local v = value:peek()
		local s = sig(v)
		if s == b then
			if status:peek() == "failed" or status:peek() == "saving" then
				status:set("saved")
			end
			return settle()
		end
		flying = true
		status:set("saving")
		save(v):after(function(_rev: any)
			flying = false
			base:set(s)
			local get = c.remote
			local remote = if get then get() else nil
			status:set(if remote ~= nil and sig(remote) == s then "saved" else "syncing")
			if again or sig(value:peek()) ~= s then
				again = false
				save_now()
			else
				settle()
			end
		end, function(_err: any)
			flying = false
			again = false
			status:set("failed")
			settle()
		end)
	end

	c = ({
		value = value,
		base = base,
		status = status,
		adopted = adopted,
		dirty = dirty,
		save_now = save_now,
		busy = function(): boolean
			return false
		end,
		early = function(): boolean
			return early
		end,
		edit = function(v: any)
			if base:peek() == nil then
				early = true
			end
			value:set(v)
			cancel()
			timer = ui.after(M.DEBOUNCE, save_now)
		end,
		flush = function(done: () -> ())
			table.insert(waiting, done)
			if flying then
				return
			end
			save_now()
		end,
		heard = function(remote: any)
			-- (A frame after the ack has it.)
			if status:peek() == "syncing" then
				status:set("saved")
			end
			local rs = sig(remote)
			local b = base:peek()
			if b == nil then
				base:set(rs)
				if early then
					-- Typed before the engine could answer: theirs, saved now.
					early = false
					if sig(value:peek()) ~= rs then
						save_now()
					end
					return
				end
				value:set(remote)
				adopted:set(adopted:peek() + 1)
				return
			end
			if rs == b then
				return
			end
			local vs = sig(value:peek())
			if rs == vs then
				base:set(rs)
				return
			end
			-- Nothing of the person's unsaved: theirs shows. Else the person's
			-- stays, and is saved over it.
			if vs == b and not flying and not c.busy() then
				value:set(remote)
				base:set(rs)
				adopted:set(adopted:peek() + 1)
			end
		end,
	} :: any) :: Channel
	channels[key] = c
	return c
end

-- How the channels stand together, as a word: "failed", "saving",
-- "syncing" or "saved" (the first that any of them is).
function M.state(list: { Channel }): string
	local out = "saved"
	local offline = M.offline:get()
	-- (Nothing saves: what shows is this app's alone.)
	if offline then
		for _, c in list do
			c.value:get()
		end
		return "failed"
	end
	for _, c in list do
		local s = c.status:get()
		local b = c.base:get()
		-- (Read for its changes: an edit not gone yet is "saving".)
		local v = c.value:get()
		if s == "failed" or (offline and (c.early() or (b ~= nil and v ~= nil and c.dirty()))) then
			return "failed"
		end
		if b == nil and c.early() then
			out = "saving"
		end
		if s == "saving" or (b ~= nil and v ~= nil and c.dirty()) then
			out = "saving"
		elseif s == "syncing" and out == "saved" then
			out = "syncing"
		end
	end
	return out
end

return M
