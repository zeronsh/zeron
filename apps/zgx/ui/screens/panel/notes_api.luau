--!strict
-- A main chat's notes, from the host (src/notes.rs), one place, so a fixture
-- stands in for them.
--
--   api.watch(chat)               watched while in use: gives back the
--                                 release (an `ui.on_cleanup`'s)
--   api.report(chat)              what the engine said last (tracked): { text,
--                                 hash, epoch, seq }, { unsupported = true }
--                                 (an engine from before notes), { error },
--                                 or nil (nothing yet)
--   api.set(chat, base_hash, text)  a promise: { kind = "applied", hash,
--                                 epoch, seq } | { kind = "conflict", current }
--                                 | { kind = "tooLarge", limit }; refused
--                                 "unsupported" by an engine (or a host) from
--                                 before notes, else in the engine's words

local M = {}

local shared = ui.shared("session_notes", nil :: any)

-- A host without the calls (one from before notes, a fixture's) is an
-- engine from before them.
local host_has_notes = true

function M.watch(chat: string): () -> ()
	local ok = pcall(ui.invoke, "notes.watch", { chatId = chat })
	if not ok then
		host_has_notes = false
	end
	local released = false
	return function()
		if released then
			return
		end
		released = true
		pcall(ui.invoke, "notes.unwatch", { chatId = chat })
	end
end

function M.report(chat: string): any?
	if not host_has_notes then
		return { unsupported = true }
	end
	local all = shared:get()
	return if type(all) == "table" then all[chat] else nil
end

function M.set(chat: string, base_hash: string, text: string): any
	local ok, promise = pcall(ui.call, "notes.set", { chatId = chat, baseHash = base_hash, text = text })
	if ok then
		return promise
	end
	return {
		after = function(_self: any, _done: any, failed: ((any) -> ())?)
			if failed then
				failed({ message = "unsupported" })
			end
		end,
	}
end

return M
