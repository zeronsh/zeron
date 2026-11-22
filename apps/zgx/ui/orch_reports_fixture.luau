-- Crew updates (ds/orch_reports) on their own, and in a transcript as the
-- chat bridge hands an orchestrator's `crewReports` message over (src/chat.rs
-- `message`: role "crew_reports"), for tests/orch_reports.test.luau. The
-- reports are the sample world's crew (account/orchestration_sample); a
-- thread opened is noted in `opened`.

ui.theme.strict("warn")
-- (Orchestrators on, as these are its: off is the default everywhere.)
require("@account/plugins").set({ orchestrators = true })
local ds = require("@ds")
ds.setup()
local T = require("@ds/transcript")
local OrchReports = require("@ds/orch_reports")
local sample = require("@account/orchestration_sample").at(os.time())

local opened = ui.state("opened", "")
local titles: { [string]: string } = {}
for _, c in sample.chats do
	titles[c.id] = c.title
end

-- Every crew report the sample has, newest last, and one more each from
-- two threads: five.
local reports: { any } = {}
for _, m in sample.now.crew do
	if m.last_report and m.orchestrator_id == "orc-orch" then
		local r = m.last_report
		table.insert(reports, { id = r.id, from_chat = m.chat_id, item_id = m.item_id, state = r.state, summary = r.summary, at = r.at })
	end
end
table.insert(reports, { id = "rp-x1", from_chat = "th-ui-control", item_id = "it-control", state = "done", summary = "Map widget lands; board next", at = os.time() - 60 })
table.insert(reports, { id = "rp-x2", from_chat = "th-engine-state", item_id = "it-engine", state = "failed", summary = "Registry migration test fails on an empty profile", at = os.time() - 20 })
local shown = ui.state("reports", reports)

local messages = ui.state("orch_reports_messages", {
	{ id = "u1", role = "user", text = "Ship the orchestrator page.", at = "Nov 12, 6:00 PM" },
	{ id = "w1", role = "crew_reports", text = "", reports = reports },
} :: { any })

local function open(chat: string)
	opened:set(chat)
end

return ui.component(function(): Node
	return ui.row {
		id = "orch-reports-fixture",
		w_full = true,
		h_full = true,
		bg = "$background",
		ui.column {
			id = "card-side",
			grow = 1,
			min_w = 0,
			p = "$space.4",
			OrchReports {
				reports = shown,
				title_of = function(chat: string): string?
					return titles[chat]
				end,
				on_open = open,
				hue = function(): number?
					return 196
				end,
			},
		},
		ui.column {
			id = "conversation",
			grow = 1,
			min_w = 0,
			h_full = true,
			T.Transcript {
				messages = messages,
				orchestrators = function(): any
					return require("@account/plugins").module("orchestrators")
				end,
				chat_of = function(chat: string): any
					return if titles[chat] then { title = titles[chat] } else nil
				end,
				open_link = function(target: string)
					opened:set(target)
				end,
			},
		},
	}
end)({})
