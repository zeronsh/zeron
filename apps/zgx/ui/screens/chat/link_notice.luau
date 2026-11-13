-- What the composer says of the chat's connection, as Zeron's does
-- (crates/ui composer.rs `link_notice`): this device's own link first
-- (offline, reconnecting), then whether the chat's host can be reached.
-- Only something to send (`intent`: a draft, a send in flight, queued
-- messages) earns the clause saying when it will go; an empty composer
-- never claims a message is waiting, and says nothing of an unreachable
-- host. A chat on Cloud (`cloud`) says its machine's own state in the host's
-- place (screens/chat/cloud_machine): a machine asleep isn't unreachable, and
-- one that can never wake mustn't promise the message will go.
--
--   link_notice({ connection = "reconnecting", intent = true })
--     → { text = "Reconnecting — messages will send once …", spinner = true }

local M = {}

-- A message in the locale shown (chat.link_notice.*). A test requires this
-- module alone, in a VM with no `ui`: there, the English it says.
local function say(key: string, english: string, args: { [string]: any }?): string
	if (ui :: any) == nil then
		return english
	end
	return ui.t(`chat.link_notice.{key}`, args):get()
end

export type Link = {
	-- This device's connection (account/sessions' `connection`).
	connection: string?,
	-- The chat's host is another device, and it's offline.
	host_unreachable: boolean?,
	-- Its name, for "Can't reach …".
	host: string?,
	intent: boolean?,
	-- The chat's on Cloud: its machine's line, if it has one to say.
	cloud: { line: Notice? }?,
}

export type Notice = { text: string, tone: string?, spinner: boolean?, action: string? }

function M.of(link: Link): Notice?
	local intent = link.intent == true
	local c = link.connection
	if c == "offline" then
		return {
			text = if intent
				then say("offline_intent", "Offline — messages will send when you're back online.")
				else say("offline", "Offline"),
			tone = "warning",
		}
	end
	if c == "reconnecting" then
		return {
			text = if intent
				then say("reconnecting_intent", "Reconnecting — messages will send once the connection recovers.")
				else say("reconnecting", "Reconnecting…"),
			spinner = true,
		}
	end
	if link.cloud then
		return link.cloud.line
	end
	if link.host_unreachable and intent then
		local host = link.host or say("the_host", "the host")
		return { text = say("unreachable", `Can't reach {host} — messages will send when it's reachable.`, { host = host }) }
	end
	return nil
end

return M
