-- An orchestrator's crew (ds/orch_crew) on the sample world
-- (account/orchestration_sample): orc-orch's four threads, for
-- tests/orch_crew.test.luau. A thread opened is noted in `opened`; the
-- members are the state `crew_members`, which a test may set.
-- (The sample unshifted: `sample.at()` shifts each todo's `done` too.)

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local OrchCrew = require("@orchestrators/ds/orch_crew")
local sample = require("@orchestrators/account/orchestration_sample")

local opened = ui.state("opened", "")

local chats: { [string]: any } = {}
for _, c in sample.chats do
	chats[c.id] = c
end
local names: { [string]: string } = {}
for _, d in sample.devices do
	names[d.id] = d.name
end

local function members_of(orc: string): { any }
	local out = {}
	for _, m in sample.now.crew do
		if m.orchestrator_id == orc then
			table.insert(out, { member = m, chat = chats[m.chat_id] })
		end
	end
	return out
end

local members = ui.state("crew_members", members_of("orc-orch"))

return ui.component(function(): Node
	return ui.column {
		id = "orch-crew-fixture",
		w_full = true,
		h_full = true,
		p = "$space.4",
		bg = "$background",
		OrchCrew {
			members = members,
			hue = function(): number?
				return 196
			end,
			device_name = function(id: string): string?
				return names[id]
			end,
			on_open = function(chat: string)
				opened:set(chat)
			end,
			now = function(): number
				return sample.NOW
			end,
		},
	}
end)({})
