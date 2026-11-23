-- The gallery's Orchestration pages: an orchestrator's hue across the
-- wheel, a dump's receipt (with a button that routes a new one, so its
-- parts light one after another), Needs you, the Map (a report's pulse on
-- a button), Machines and Comms, each drawn from
-- account/orchestration_sample. What the pages do is said under them.

local ds = require("@ds")
local hue = require("../ds/orch_hue")
local data = require("../ds/orch_data")
local Receipt = require("../ds/orch_receipt")
local Asks = require("../ds/orch_asks")
local OrchMap = require("../ds/orch_map")
local Machines = require("../ds/orch_machines")
local Comms = require("../ds/orch_comms")
local sample_of = require("../account/orchestration_sample")

local sample = sample_of.at(os.time())
local said = ui.state("orch_demo_said", "")
local now = ui.state("orch_demo_now", sample.now :: any)
local comms = ui.state("orch_demo_comms", sample.comms :: { any })
local sessions_now = (function()
	local top, children = {}, {}
	for _, c0 in sample.chats do
		local c = table.clone(c0)
		if c.agent == "codex" then
			c.agent = "openai"
		end
		if c.parent then
			children[c.parent] = children[c.parent] or {}
			table.insert(children[c.parent], c)
		else
			table.insert(top, c)
		end
	end
	return { chats = top, children = children, devices = sample.devices }
end)()
local chats = data.chats(sessions_now)

local function note(): Node
	return ui.text { function(): string
		return said:get()
	end, text_size = "$font.size.sm", color = "$text.tertiary" }
end

local function receipt(): Node
	return ui.column {
		w_full = true,
		gap = "$space.4",
		ui.row {
			gap = "$space.2",
			ds.Button { tone = "secondary", size = "sm", on_click = function()
				local n = table.clone(now:peek())
				local dumps = table.clone(n.dumps)
				local text = "ship the map pulse today and ask the vpn crew if the reboot plan is final"
				table.insert(dumps, 1, { id = `dp-demo-{os.clock()}`, text = text, state = "routing", routes = {}, device = "dev-desk", created = os.time() })
				n.dumps = dumps
				now:set(n)
				ui.after(700, function()
					local m = table.clone(now:peek())
					local ds2 = table.clone(m.dumps)
					local d = table.clone(ds2[1])
					d.state = "routed"
					d.routes = {
						{ id = "a", excerpt = "ship the map pulse today", span = { from = 0, to = 24 }, target = { kind = "orchestrator", id = "orc-orch" }, intent = "task", state = "delivered", candidates = {} },
						{ id = "b", excerpt = "ask the vpn crew if the reboot plan is final", span = { from = 29, to = #text }, target = { kind = "orchestrator", id = "orc-vpn" }, intent = "question", state = "pending", candidates = {} },
					}
					ds2[1] = d
					m.dumps = ds2
					now:set(m)
				end)
			end, "Dump something" },
		},
		Receipt {
			dump = function(): any
				return now:get().dumps[1]
			end,
			now = function(): any
				return now:get()
			end,
			chats = function(): { [string]: any }
				return chats
			end,
			on_reroute = function(_d: string, r: string, t: any)
				said:set(`Moved {r} to {t.kind} {t.id or ""}`)
			end,
			on_open = function(t: any)
				said:set(`Opened {t.kind} {t.id or ""}`)
			end,
			text_size = "$font.size.lg",
			line_height = "$font.line.lg",
		},
		ds.SectionTitle { "Unsure" },
		Receipt {
			dump = function(): any
				return now:get().dumps[#now:get().dumps]
			end,
			now = function(): any
				return now:get()
			end,
			chats = function(): { [string]: any }
				return chats
			end,
			on_reroute = function(_d: string, r: string, t: any)
				said:set(`Moved {r} to {t.kind} {t.id or ""}`)
			end,
			on_open = function(t: any)
				said:set(`Opened {t.kind} {t.id or ""}`)
			end,
		},
		note(),
	}
end

local function hues(): Node
	local list = {}
	for h = 0, 330, 30 do
		table.insert(list, h)
	end
	return ui.column {
		w_full = true,
		gap = "$space.4",
		ui.row {
			gap = "$space.4",
			flex_wrap = true,
			ui.each(list, function(h: number): Node
				return ui.column {
					gap = "$space.1",
					items = "center",
					ui.div { w = 44, h = 22, rounded = "$radius.full", bg = hue.wash(h), items = "center", justify = "center", hue.Dot { hue = h, size = 9 } },
					ui.div { w = 44, h = 2, rounded = "$radius.full", bg = ui.raw(hue.line(h), "a hue's line") },
					ui.text { tostring(h), tabular = true, text_size = "$font.size.xs", color = "$text.tertiary" },
				}
			end, { direction = "row", gap = "$space.4", flex_wrap = true }),
		},
		ui.row {
			gap = "$space.2",
			items = "center",
			hue.Dot { hue = 196, size = 9, pulse = function(): boolean
				return true
			end },
			ui.text { "Breathing while its crew works", text_size = "$font.size.sm", color = "$text.secondary" },
		},
	}
end

local function asks(): Node
	return ui.column {
		w = 420,
		gap = "$space.3",
		Asks {
			needs = function(): { any }
				return data.needs(now:get(), sessions_now)
			end,
			name_of = function(id: string?): string?
				return data.name_of(now:get(), chats, id)
			end,
			on_answer = function(a: any, choice: any)
				said:set(`Answered {a.id}: {choice.text or (a.options[choice.option or 0] or {}).label or ""}`)
				local n = table.clone(now:peek())
				local list = {}
				for _, x in n.asks do
					if x.id == a.id then
						x = table.clone(x)
						x.state = "answered"
					end
					table.insert(list, x)
				end
				n.asks = list
				ui.after(300, function()
					now:set(n)
				end)
			end,
			on_dismiss = function(a: any)
				said:set(`Dismissed {a.id}`)
			end,
			on_open = function(id: string)
				said:set(`Opened {id}`)
			end,
		},
		ds.Button { tone = "ghost", size = "sm", on_click = function()
			now:set(sample.now)
		end, "Ask again" },
		note(),
	}
end

local function map(): Node
	return ui.column {
		w_full = true,
		gap = "$space.4",
		ds.Button { tone = "secondary", size = "sm", on_click = function()
			comms:push({ id = `demo-{os.clock()}`, kind = "report", from = "th-ui-control", to = "orc-orch", summary = "Map pulse in", at = os.time() })
		end, "A report" },
		OrchMap.Map {
			world = function(): any
				return { now = now:get(), chats = chats, devices = sample.devices, loads = sample.loads, here = "dev-desk" }
			end,
			comms = function(): { any }
				return comms:get()
			end,
			on_open_chat = function(id: string)
				said:set(`Opened {id}`)
			end,
			on_open_orchestrator = function(id: string)
				said:set(`Opened orchestrator {id}`)
			end,
		},
		note(),
	}
end

local function machines(): Node
	return ui.column {
		w = 420,
		Machines {
			devices = function(): { any }
				return sample.devices
			end,
			loads = function(): { [string]: any }
				return sample.loads
			end,
			chats = function(): { [string]: any }
				return chats
			end,
			here = function(): string?
				return "dev-desk"
			end,
			hue_of = function(id: string): number?
				return data.hue_of(now:get(), chats, id)
			end,
			on_open_chat = function(id: string)
				said:set(`Opened {id}`)
			end,
		},
	}
end

local function comms_page(): Node
	return ui.column {
		w = 420,
		Comms {
			comms = function(): { any }
				return comms:get()
			end,
			name_of = function(id: string): string?
				return data.name_of(now:get(), chats, id)
			end,
			hue_of = function(id: string): number?
				return data.hue_of(now:get(), chats, id)
			end,
			on_open = function(id: string)
				said:set(`Opened {id}`)
			end,
		},
	}
end

return {
	{ id = "orch-hue", title = "Orchestrator hue", about = "Each orchestrator's one colour, at one lightness and chroma per scheme: its dot, its wash, its line.", demo = hues },
	{ id = "orch-receipt", title = "Dump receipt", about = "What was dumped, each part underlined in where it went's hue, a chip per part. Dump something: its parts light one after another as they're placed. A chip's menu opens or moves it; an unsure part offers its likely places.", demo = receipt },
	{ id = "orch-asks", title = "Needs you", about = "Asks and chats awaiting input, urgent first, answered in place; an answered one slides out.", demo = asks },
	{ id = "orch-map", title = "Map", about = "Machines as lanes, orchestrators as stations, crew threads as pills on their hue lines; a report runs a pulse along its line.", demo = map },
	{ id = "orch-machines", title = "Machines", about = "Each device's CPU as it moves, its memory, and the tests and builds on it now.", demo = machines },
	{ id = "orch-comms", title = "Comms", about = "What passes between agents and you, newest first.", demo = comms_page },
}
