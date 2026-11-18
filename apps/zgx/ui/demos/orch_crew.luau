-- The gallery's Crew page: an orchestrator's threads as its page lists them
-- (ds/orch_crew), from the sample world (account/orchestration_sample):
-- Orchestrators' four, one waiting on you, and Exnomic VPN's two, in its
-- own hue. A card says what it opened.

local ds = require("@ds")
local OrchCrew = require("@ds/orch_crew")
-- (Unshifted: `sample.at()` shifts each todo's `done` too.)
local sample = require("@account/orchestration_sample")

local chats: { [string]: any } = {}
for _, c in sample.chats do
	chats[c.id] = c
end
local names: { [string]: string } = {}
for _, d in sample.devices do
	names[d.id] = d.name
end
local hues: { [string]: number } = {}
for _, o in sample.now.orchestrators do
	hues[o.id] = o.hue
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

local said = ui.state("orch_crew_demo_said", "Pick a thread.")

local function crew(orc: string, columns: number?): Node
	return OrchCrew {
		members = members_of(orc),
		hue = function(): number?
			return hues[orc]
		end,
		device_name = function(id: string): string?
			return names[id]
		end,
		on_open = function(chat: string)
			said:set(`Opened “{(chats[chat] or {}).title or chat}”`)
		end,
		now = function(): number
			return sample.NOW
		end,
		columns = columns,
	}
end

return {
	id = "orch-crew",
	title = "Crew",
	about = "An orchestrator's threads on its page: the one that needs you first, then those working; each card its status, activity, todo, machine, model and last report, opening its thread.",
	demo = function(): Node
		return ui.column {
			w_full = true,
			gap = "$space.5",
			ds.Text { said, style = "caption", tone = "secondary" },
			ui.column { w_full = true, max_w = 640, gap = "$space.2", ds.Text { "Orchestrators", style = "label", tone = "tertiary" }, crew("orc-orch") },
			ui.column { w_full = true, max_w = 440, gap = "$space.2", ds.Text { "Exnomic VPN", style = "label", tone = "tertiary" }, crew("orc-vpn", 1) },
			ui.column { w_full = true, max_w = 440, gap = "$space.2", ds.Text { "Empty", style = "label", tone = "tertiary" }, crew("orc-none") },
		}
	end,
}
