-- How threads' work is going (ds/orch_progress), each variant on the
-- sample's rows (account/orchestration_sample's `progress`): the full
-- units a widget lists, the one-line strips (plain and lean) and the chips,
-- for tests/orch_progress.test.luau and audit shots. The clock is the
-- state `now` (Unix seconds) and the rows the state `rows`, which a test
-- may set; what's asked is noted in `calls`, pins in `pins`.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Unit = require("@ds/orch_progress")
local Block = require("@ds/orch_block")
local sample = require("@account/orchestration_sample")

local now = ui.state("now", sample.NOW)
local rows = ui.state("rows", sample.progress :: { [string]: any })
local calls = ui.state("calls", "")
local pins = ui.state("pins", {} :: { [string]: boolean })
local resting = ui.state("resting", {} :: { [string]: boolean })

local chats: { [string]: any } = {}
for _, c in sample.chats do
	chats[c.id] = c
end
local CONTEXT: { [string]: string } = {
	["orc-orch"] = "Orchestrator · desk",
	["th-engine-state"] = "Orchestrators · desk",
	["th-engine-flow"] = "Orchestrators · desk",
	["th-ui-control"] = "Orchestrators · macbook",
	["th-ui-orch"] = "Orchestrators · macbook",
	["th-vpn-merge"] = "Exnomic VPN · desk",
	["th-vpn-moscow"] = "Exnomic VPN · desk",
	["th-ci-cache"] = "CI speed · Cloud",
}
-- Full: the widget's first four; strips and chips: all of them.
local FULL = { "th-engine-state", "th-engine-flow", "th-ui-control", "th-ui-orch" }
local ALL = { "th-engine-state", "th-engine-flow", "th-ui-control", "th-ui-orch", "th-vpn-merge", "th-vpn-moscow", "th-ci-cache", "orc-orch" }

local function note(what: string)
	calls:set(if calls:peek() == "" then what else `{calls:peek()},{what}`)
end
local function set(map: State<{ [string]: boolean }>, id: string, on: boolean)
	local copy = table.clone(map:peek())
	copy[id] = if on then true else nil
	map:set(copy)
end

local function unit(id: string, variant: any, lean: boolean?): Node
	return Unit.Progress {
		id = `fx-{variant}{if lean then "-lean" else ""}-{id}`,
		variant = variant,
		lean = lean,
		row = function(): any
			return rows:get()[id]
		end,
		chat = function(): any
			return chats[id]
		end,
		context = function(): string
			return CONTEXT[id] or ""
		end,
		now = function(): number
			return now:get()
		end,
		on_check = function()
			if resting:peek()[id] then
				return
			end
			note(`check:{id}`)
			set(resting, id, true)
			ui.after(15000, function()
				set(resting, id, false)
			end)
		end,
		cooling = function(): boolean
			return resting:get()[id] == true
		end,
		on_open = function()
			note(`open:{id}`)
		end,
		pinned = function(): boolean
			return pins:get()[id] == true
		end,
		on_pin = function(on: boolean)
			note(`pin:{id}:{if on then "on" else "off"}`)
			set(pins, id, on)
		end,
	}
end

local function list(ids: { string }, make: (id: string) -> Node, gap: string?): Node
	local nodes: { any } = {}
	for i, id in ids do
		if i > 1 and gap == "rule" then
			table.insert(nodes, ui.div { w_full = true, h = 1, bg = "$border.subtle" })
		end
		table.insert(nodes, make(id))
	end
	return ui.column { w_full = true, gap = if gap == "rule" then "$space.3" else "$space.1", table.unpack(nodes) }
end

ui.command { id = "theme_light", title = "Light theme", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-light")
end }
ui.command { id = "theme_dark", title = "Dark theme", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-dark")
end }
-- A minute on.
ui.command { id = "fixture.later", title = "A minute later", run = function()
	now:set(now:peek() + 60)
end }

return ui.component(function(): Node
	return ui.scroll {
		id = "orch-progress-fixture",
		w_full = true,
		h_full = true,
		bg = "$background",
		ui.column {
			w_full = true,
			p = "$space.6",
			gap = "$space.4",
			items = "center",
			ui.row {
				w_full = true,
				max_w = 1200,
				gap = "$space.4",
				items = "start",
				flex_wrap = true,
				ui.column {
					id = "fx-full",
					grow = 1,
					basis = 360,
					min_w = 0,
					Block.Block {
						title = "Progress",
						list(FULL, function(id: string): Node
							return unit(id, "full")
						end, "rule"),
					},
				},
				ui.column {
					id = "fx-strips",
					grow = 1,
					basis = 360,
					min_w = 0,
					gap = "$space.4",
					Block.Block {
						title = "Strips",
						list(ALL, function(id: string): Node
							return unit(id, "strip")
						end),
					},
					Block.Block {
						title = "Lean",
						list({ "th-engine-state", "th-ui-control", "th-engine-flow" }, function(id: string): Node
							return unit(id, "strip", true)
						end),
					},
					Block.Block {
						title = "Chips",
						ui.row {
							id = "fx-chips",
							w_full = true,
							gap = "$space.3",
							flex_wrap = true,
							table.unpack((function(): { Node }
								local out = {}
								for _, id in ALL do
									table.insert(out, unit(id, "chip"))
								end
								return out
							end)()),
						},
					},
				},
			},
		},
	}
end)({})
