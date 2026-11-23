-- The gallery's Crew updates page: what an orchestrator's crew reported, as
-- its transcript shows the engine's `crewReports` message (ds/orch_reports),
-- on the sample world's crew (account/orchestration_sample): three rows,
-- the rest folded, a report filed to see it join, and a short card.

local ds = require("@ds")
local OrchReports = require("../ds/orch_reports")
local sample = require("../account/orchestration_sample").at(os.time())

local titles: { [string]: string } = {}
for _, c in sample.chats do
	titles[c.id] = c.title
end
local function title_of(chat: string): string?
	return titles[chat]
end

local first: { any } = {}
for _, m in sample.now.crew do
	if m.last_report and m.orchestrator_id == "orc-orch" then
		local r = m.last_report
		table.insert(first, { id = r.id, from_chat = m.chat_id, item_id = m.item_id, state = r.state, summary = r.summary, at = r.at })
	end
end
table.insert(first, { id = "rp-d1", from_chat = "th-ui-control", item_id = "it-control", state = "done", summary = "Map widget lands; board next", at = os.time() - 60 })
table.insert(first, { id = "rp-d2", from_chat = "th-engine-state", item_id = "it-engine", state = "message", summary = "Which registry kind owns dumps' 50-row cap?", at = os.time() - 20 })
local reports = ui.state("orch_reports_demo", first)
local said = ui.state("orch_reports_demo_said", "Pick a report.")

local FILED = {
	{ from_chat = "th-ui-orch", state = "progress", summary = "Crew card states drawn; dialogs next" },
	{ from_chat = "th-engine-flow", state = "failed", summary = "MCP role filter rejects crew on a cold start" },
	{ from_chat = "th-ui-control", state = "blocked", summary = "Map needs device loads before it can lay lanes" },
}

return {
	id = "orch-reports",
	title = "Crew updates",
	about = "What an orchestrator's crew reported, in its transcript: a row a report (its thread, how it stands, its one line, when), three at first and the rest folded; a row opens its thread.",
	demo = function(): Node
		return ui.column {
			w_full = true,
			gap = "$space.5",
			ui.row {
				gap = "$space.2",
				flex_wrap = true,
				ds.Button { tone = "secondary", size = "sm", on_click = function()
					local now = table.clone(reports:peek())
					local f = FILED[(#now % #FILED) + 1]
					table.insert(now, { id = `rp-d{#now + 1}`, from_chat = f.from_chat, state = f.state, summary = f.summary, at = os.time() })
					reports:set(now)
				end, "File a report" },
				ds.Button { tone = "secondary", size = "sm", on_click = function()
					reports:set(first)
				end, "Reset" },
			},
			ui.column {
				w_full = true,
				max_w = 720,
				OrchReports {
					reports = reports,
					title_of = title_of,
					hue = function(): number?
						return 196
					end,
					on_open = function(chat: string)
						said:set(`Opened “{titles[chat] or chat}”`)
					end,
				},
			},
			ui.column {
				w_full = true,
				max_w = 720,
				OrchReports {
					reports = { first[4], first[5] },
					title_of = title_of,
					hue = function(): number?
						return 32
					end,
					on_open = function(chat: string)
						said:set(`Opened “{titles[chat] or chat}”`)
					end,
				},
			},
			ui.text { function(): string
				return said:get()
			end, literal = true, text_size = "$font.size.sm", color = "$text.tertiary" },
		}
	end,
}
