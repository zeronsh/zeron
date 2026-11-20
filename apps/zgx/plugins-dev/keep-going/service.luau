-- Keep going's service: run by Zeron's engine on each device that allowed
-- it, with no window open. When a chat's turn finishes and its reply's last
-- line is the marker, it sends that chat "continue", as you would from the
-- composer. An agent asks for it by ending its reply with the marker.
--
-- A chat that asks every time would loop: Zeron stops a plugin whose own
-- messages keep setting each other off (16 in a row), and its row in
-- Settings → Plugins says so.

local zeron = require("./zeron")("keep-going")

local MARKER = "[continue]"

-- Whether `reply`'s last non-blank line is the marker.
local function asks(reply: string?): boolean
	if type(reply) ~= "string" then
		return false
	end
	local last = ""
	for line in string.gmatch(reply, "[^\n]+") do
		if string.match(line, "%S") then
			last = line
		end
	end
	return string.match(last, "^%s*(.-)%s*$") == MARKER
end

zeron.on("turn.finished", function(e)
	if e.outcome ~= "completed" or not asks(e.reply) then
		return
	end
	local sent = zeron.send_message({ chat = e.chat, text = "continue" }):await()
	zeron.emit("continued", { chat = e.chat, delivery = sent.delivery })
end)
