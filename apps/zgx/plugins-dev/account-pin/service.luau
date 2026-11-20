-- Account pin's service: run by Zeron's engine on each device that allowed
-- it, with no window open. Right before a chat's first run, Zeron asks
-- (`run.preparing`) which saved account it should run on; this answers with
-- the one that has the fewest chats pinned to it, so new chats take turns
-- across your accounts. The active account is never switched: other chats
-- and your own terminals keep running on it.
--
-- The count comes from `zeron.pins()`, not from memory, so the service can
-- stop between chats. Zeron asks one chat at a time, so each sees the last
-- one's pin. A chat it doesn't answer for runs on the active account.

local zeron = require("./zeron")("account-pin")

local PROVIDER = "claude-code"

-- The saved account (that Zeron can run a chat on, and that's signed in)
-- with the fewest chats pinned to it: the first, in Zeron's order, on a tie.
local function fewest(): (string?, string?)
	local accounts = zeron.accounts({ provider = PROVIDER }):await()
	local count: { [string]: number } = {}
	for _, pin in zeron.pins({ provider = PROVIDER }):await() do
		count[pin.account] = (count[pin.account] or 0) + 1
	end
	local best: any = nil
	for _, a in accounts do
		if a.pinnable and not a.signed_out and (best == nil or (count[a.account] or 0) < (count[best.account] or 0)) then
			best = a
		end
	end
	if best == nil then
		return nil, nil
	end
	return best.account, best.email
end

-- What a `run.preparing` listener returns is its answer: `{ account }`.
zeron.on("run.preparing", function(e)
	local account, email = fewest()
	if account == nil then
		return nil
	end
	zeron.emit("pinned", { chat = e.chat, to = email or account })
	return { account = account }
end, { provider = PROVIDER })

-- A saved account that went away takes its chats' pins with it: told here
-- so the window can say so.
zeron.on("account.unpinned", function(e)
	if e.reason == "removed" then
		zeron.emit("lost", { account = e.account, chat = e.chat })
	end
end, { provider = PROVIDER })
