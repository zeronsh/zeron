-- A new chat's agent started while its first message is still being written
-- (the engine's warm start, src/chat.rs `warm`), so Enter has it answering at
-- once instead of first starting it. It is started for the id the chat will
-- be made with; nothing of the chat exists until Enter (no chat, no
-- transcript, no title). The agent asks its model nothing until Enter. Only
-- a new chat's: an existing chat's agent is never started early.
--
-- Only the person's own typing starts one: the first key in a new chat's
-- draft. Not opening a new chat (most are opened and left, or opened long
-- before anything is written: an agent waiting meanwhile held some 100 MB),
-- a launch, a window put back, a draft restored, or anything a plugin,
-- template, link or agent does. Typing a message takes longer than the
-- agent's start, so it is ready by Enter all the same. And never for a chat that will run on another device, on
-- Cloud, or in a worktree made as it's sent (the composer's `request` is nil
-- then).
--
-- It stops when:
--   the draft is cleared and left empty for 10 s (the host's `warm_cleared`);
--   the window moves on with that draft empty;
--   another chat's draft is typed into (one per window);
--   the window closes, or the app quits (the engine stops them all then);
--   a typed draft is left alone (the engine's idle stop, 2 min after the
--     last keystroke it heard of).
-- A change of agent, model, effort, folder or access while it waits starts
-- another in its place.
--
-- (No timers here: a timer an effect makes ends when that effect runs
-- again, as these do on every key. The host keeps the time.)

local store = require("@app/screens/chat/store")

local M = {}

-- While the person types, the engine hears of it this often at most.
local RENEW_MS = 10000

export type Bind = {
	-- The chat the composer is on (nil: a new chat's).
	session: () -> string?,
	-- The draft, and the chat it is ("" a new chat's: it changes with the
	-- text, after the route, as a move swaps drafts).
	text: any,
	owner: () -> string,
	-- The send as it stands (`store.Request`'s fields: model, effort,
	-- options, access, plan, cwd, device), read so a change to any is heard;
	-- nil when it can't warm (an existing chat, on Cloud or another device,
	-- in a worktree made as it's sent, no agent picked).
	request: () -> any?,
}

export type Warm = {
	-- The id the new chat is to be made with, taken by its send (nil: none).
	take: () -> string?,
	-- Stop it (a send that can't use it: Cloud; a send that failed).
	stop: () -> (),
}

-- The draft's length, without copying it.
local function length(s: any): number
	if (s :: any).len ~= nil then
		return s:len()
	end
	return #s:get()
end

function M.bind(b: Bind): Warm
	-- (Per composer: plain values, not states, so nothing redraws for them.)
	-- The id it's asked under (nil: none).
	local id: string? = nil
	local typed = false
	-- The ask last sent, to send a changed one only; and when.
	local sent = ""
	local sent_at = 0
	local cleared = false
	-- The chat the draft's length was last seen on ("" a new chat's), and
	-- that length: a key is a change of it on the same chat (a draft put
	-- back is none).
	local seen_on: string? = nil
	local seen_len = 0

	local function stop()
		if id then
			store.unwarm(id)
		end
		id, typed, sent, cleared = nil, false, "", false
	end

	-- Ask the host for it (again when the ask changed, to renew it, or when
	-- it was first typed into).
	local function ask(req: any, force: boolean)
		if req == nil then
			stop()
			return
		end
		local key = `{req.model or ""}|{req.effort or ""}|{tostring(req.fast)}|{req.access or ""}|{tostring(req.plan)}|{req.cwd or ""}|{req.device or ""}`
		local keys = {}
		for k, v in (req.options or {}) :: { [string]: any } do
			table.insert(keys, `{k}={tostring(v)}`)
		end
		table.sort(keys)
		key ..= "|" .. table.concat(keys, ",")
		if id ~= nil and key == sent and not force then
			return
		end
		if id == nil then
			id = store.new_id()
		end
		sent, sent_at = key, ui.now()
		store.warm(id :: string, req, typed)
	end

	-- Typed into (a key), cleared, or left.
	ui.effect(function()
		local on = b.owner()
		local n = length(b.text)
		if on ~= seen_on then
			-- Arrived (its draft as it was left): no key. Left the new chat
			-- with its draft empty: its agent goes.
			if id ~= nil and seen_on == "" and seen_len == 0 then
				stop()
			end
			seen_on, seen_len = on, n
			return
		end
		if n == seen_len then
			return
		end
		seen_len = n
		if on ~= "" then
			-- Writing in another chat: the new chat's agent goes (one per
			-- window, and only while its message is being written).
			if id ~= nil then
				stop()
			end
			return
		end
		if n > 0 then
			local was_cleared = cleared
			cleared = false
			-- First typed, typed again after a clear (the host may have
			-- stopped it), or the engine due to hear it's still typed into.
			if id == nil or not typed or was_cleared or ui.now() - sent_at >= RENEW_MS then
				local first = not typed or was_cleared
				typed = true
				ask(b.request(), first or id ~= nil)
			end
		elseif id ~= nil and not cleared then
			cleared = true
			store.warm_cleared(id :: string)
		end
	end)

	-- A change of agent, model, effort, folder or access while it waits.
	ui.effect(function()
		local req = b.request()
		if id ~= nil and b.session() == nil then
			ask(req, false)
		end
	end)

	ui.on_cleanup(stop)

	return {
		take = function(): string?
			local taken = id
			if taken ~= nil then
				-- (A start or stop still on its way for it is moot now.)
				store.warm_taken(taken)
			end
			id, typed, sent, cleared = nil, false, "", false
			return taken
		end,
		stop = stop,
	}
end

return M
