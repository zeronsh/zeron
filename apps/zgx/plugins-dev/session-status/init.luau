-- Session status: a word in a chat's header for what its session is doing
-- ("ui.add"), after its title (#page-title, in #session-header, which gives
-- the session). It reads the session there with
-- zeron.need: placed somewhere with no session (the title bar, say), it
-- draws nothing and Settings → Plugins says why. The session's status is
-- "sessions.read"'s; without it the record is only an id, and the chip
-- says nothing.

local zeron = require("./zeron")("session-status")

local WORDS: { [string]: string } = {
	working = "Working",
	input = "Waiting for you",
	approval = "Needs your approval",
	failed = "Stopped",
	done = "Done",
}

zeron.place {
	at = "#page-title",
	after = function(): Node
		-- The session here, by its id; its record read as it changes.
		local id = zeron.need("session").id
		return ui.text {
			id = "session-status",
			function(): string
				return WORDS[zeron.context_of("session", id).status or ""] or ""
			end,
			text_size = "$font.size.sm",
			color = "$text.secondary",
			tabular = true,
		}
	end,
}
