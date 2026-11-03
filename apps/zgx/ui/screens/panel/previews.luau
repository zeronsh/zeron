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
--                            project, device, cwd, remote }
--   previews.looking(chat)   asked, no answer yet (tracked)
--   previews.trouble(chat)   why local previews can't open now (the
--                            engine's words, for a tooltip or the log;
--                            never shown as they are), else nil (tracked)

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

return M
