-- Session words: session-status (a word in a chat's header for what its
-- session is doing), in Zeron's language. Its words are its own,
-- locales/<locale>.json (en.json the source), shown with zeron.t: they
-- follow Settings → General → Language as it changes, and a language the
-- plugin lacks falls back to its English.

local zeron = require("./zeron")("session-words")

local KEYS: { [string]: string } = {
	working = "status.working",
	input = "status.input",
	approval = "status.approval",
	failed = "status.failed",
	done = "status.done",
}

zeron.place {
	at = "#page-title",
	after = function(): Node
		local id = zeron.need("session").id
		return ui.text {
			id = "session-words",
			function(): string
				local key = KEYS[zeron.context_of("session", id).status or ""]
				return if key then zeron.s(key) else ""
			end,
			text_size = "$font.size.sm",
			color = "$text.secondary",
		}
	end,
}
