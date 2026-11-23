-- A dump's receipt alone (ds/orch_receipt), the sample's newest, for the
-- tests that select and copy its text.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Receipt = require("@ds/orch_receipt")
local data = require("@ds/orch_data")
local sample = require("@account/orchestration_sample")

local chats: { [string]: any } = {}
for _, c in sample.chats do
	chats[c.id] = c
end

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.6",
	bg = "$background",
	Receipt {
		dump = function(): any
			return sample.now.dumps[1]
		end,
		now = function(): any
			return sample.now
		end,
		chats = function(): { [string]: any }
			return data.chats({ chats = sample.chats })
		end,
		on_reroute = function() end,
		on_open = function() end,
	},
}
