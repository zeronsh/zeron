-- Ping: sends a ping to the same plugin on your other devices and shows
-- the round trip ("devices"; it must be installed and allowed there too),
-- from a button placed after the title bar's actions ("ui.add"). Each device
-- answers a ping with a pong carrying the ping's time back.

local zeron = require("./zeron")("ping")

local said = ui.state("said", "Ping")

zeron.on_message(function(message: any)
	local data = message.data
	if type(data) ~= "table" then
		return
	end
	if data.kind == "ping" then
		zeron.send(message.from.id, { kind = "pong", sent = data.sent })
	elseif data.kind == "pong" and type(data.sent) == "number" then
		local ms = math.max(0, math.floor(ui.now() - data.sent + 0.5))
		said:set(`{message.from.name} · {ms} ms`)
	end
end)

local function ping()
	said:set("Pinging…")
	zeron.send("*", { kind = "ping", sent = ui.now() }):after(function(results: any)
		-- No answer will come from devices it didn't reach: say so.
		local reached = false
		for _, outcome in results or {} do
			if outcome == "delivered" then
				reached = true
			end
		end
		if not reached then
			said:set("No other device has Ping open")
		end
	end, function(err: any)
		said:set(tostring(err.message or err))
	end)
end

zeron.place {
	at = "#titlebar-actions",
	after = function(): Node
		return ui.button {
			id = "ping-button",
			label = "Ping your devices",
			h = "$zeron.island.control",
			px = "$space.2",
			rounded = "$radius.full",
			cursor = "pointer",
			bg = "#00000000",
			color = "$text.secondary",
			hover = { bg = "$hover", color = "$text.primary" },
			active = { bg = "$pressed" },
			on_click = ping,
			ui.text {
				function(): string
					return said:get()
				end,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				tabular = true,
			},
		}
	end,
}
