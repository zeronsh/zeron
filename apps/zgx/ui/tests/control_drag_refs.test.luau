-- Dragging anything in the Control room onto Capture's composer references
-- it (ds/orch_ref): a chip on the Brain dump's draft (its token, its kind's
-- glyph), once each, sent with the dump as its `refs`, never as a file. On
-- ui/control_page_fixture.luau (orchestration_sample; ui/control_world).

-- Capture's bar, at the foot of every tab.
local FIELD = "#control-float >> role:textbox"

-- The Brain dump's chips (its draft's, shown or kept).
local function chips(app: any): { any }
	if app:state("chat.composer.here") == "orc-brain" then
		return app:state("chat.composer.cargo") or {}
	end
	local d = (app:state("chat.composer.drafts") or {})["orc-brain"]
	return (d and d.cargo) or {}
end

local function refs_of(app: any): { any }
	local out = {}
	for _, c in chips(app) do
		if c.kind == "ref" then
			table.insert(out, c)
		end
	end
	return out
end

local function calls(app: any): string
	return tostring(app:state("calls") or "")
end

-- To a tab of the Control room's.
local function tab(app: any, id: string)
	app:set_state("control_tab", id)
	app:frames(30)
end

-- On the Map tab, in the Topology (still drawn, no longer offered: set once
-- the tab is built, which opens a view it doesn't offer on the Graph).
local function tall(): any
	local app = mount("control_page_fixture.luau")
	app:resize(1440, 1400)
	app:frames(30)
	tab(app, "map")
	app:set_state("control_map", { variant = "topology", window = 15 })
	app:frames(30)
	return app
end

-- Dragged from `source` onto the composer: the chip it left (the one
-- reference chip there).
local function dropped(app: any, source: string): any
	expect(app:has(source)).to_be(true)
	app:drag(source, FIELD)
	app:frames(10)
	expect(app:error()).to_be_nil()
	local refs = refs_of(app)
	expect(#refs).to_be(1)
	return refs[1]
end

local FLOAT_FIELD = FIELD
local BOARD_CARD = '#control-board-tab >> .orch-card "Reboot Moscow node"'

local function expect_ref(chip: any, kind: string, id: string, token: string)
	expect(chip.ref.kind).to_be(kind)
	expect(chip.ref.id).to_be(id)
	expect(chip.name).to_be(token)
end

test("drag refs: a Board card, on the Board tab, dragged into Capture's bar is referenced by its id and title", function()
	local app = tall()
	tab(app, "board")
	expect(app:has(FLOAT_FIELD)).to_be(true)
	app:drag(BOARD_CARD, FLOAT_FIELD)
	app:frames(10)
	expect(app:error()).to_be_nil()
	local refs = refs_of(app)
	expect(#refs).to_be(1)
	local chip = refs[1]
	expect_ref(chip, "card", "it-moscow", '@card:"Reboot Moscow node"')
	expect(chip.ref.context.state).to_be("review")
	expect(chip.ref.context.orchestrator).to_be("orc-vpn")
	-- The same draft on the Map tab: the bar is the same everywhere.
	tab(app, "map")
	expect(#refs_of(app)).to_be(1)
end)

test("drag refs: a Progress row is its chat, with its note", function()
	local app = tall()
	tab(app, "notes")
	local chip = dropped(app, "#progress-full-th-vpn-merge")
	expect_ref(chip, "chat", "th-vpn-merge", '@chat:"Merge phase-1 fixes"')
	expect(chip.ref.context.note).to_contain("Merge queued")
end)

test("drag refs: a Map thread, its orchestrator and machine with it", function()
	local app = tall()
	local chip = dropped(app, '#orch-map-field >> role:button "Moscow node reboot plan"')
	expect_ref(chip, "chat", "th-vpn-moscow", '@chat:"Moscow node reboot plan"')
	expect(chip.ref.context.orchestrator).to_be("orc-vpn")
	expect(chip.ref.context.device).to_be("dev-desk")
end)

test("drag refs: a Map orchestrator, with its goal", function()
	local app = tall()
	local chip = dropped(app, '#orch-map-field >> role:button "Exnomic VPN"')
	expect_ref(chip, "orchestrator", "orc-vpn", '@orchestrator:"Exnomic VPN"')
	expect(chip.ref.context.goal).to_be("Phase 2 migration, safely.")
end)

test("drag refs: a Map machine, by its name", function()
	local app = tall()
	local chip = dropped(app, "#orch-map-field >> .orch-machine-head[2]")
	expect_ref(chip, "device", "dev-mac", "@device:macbook")
	expect(chip.ref.context.os).to_be("macos")
end)

test("drag refs: a Q&A entry, by its question", function()
	local app = tall()
	tab(app, "notes")
	local chip = dropped(app, '#orch-qa >> .orch-qa-entry "ask the vpn guy whether moscow rebooted fine"')
	expect(chip.ref.kind).to_be("qa")
	expect(chip.name).to_be('@qa:"ask the vpn guy whether moscow…"')
	expect(chip.ref.context.dump ~= nil).to_be(true)
	expect(chip.ref.context.chat).to_be("orc-vpn")
end)

test("drag refs: a Needs-you ask, by its question, its options with it", function()
	local app = tall()
	-- (From Needs you floating over the Map.)
	local chip = dropped(app, "#control-needs-float >> .orch-need-question[1]")
	expect(chip.ref.kind).to_be("ask")
	expect(chip.name).to_contain("@ask:\"Reboot the Moscow node")
	expect(chip.ref.context.options).to_be("Approve / Not tonight")
end)

test("drag refs: the same thing dropped twice is one chip; another is a second", function()
	local app = tall()
	tab(app, "board")
	app:drag(BOARD_CARD, FLOAT_FIELD)
	app:frames(10)
	app:drag(BOARD_CARD, FLOAT_FIELD)
	app:frames(10)
	expect(#refs_of(app)).to_be(1)
	tab(app, "map")
	app:drag('#orch-map-field >> role:button "Exnomic VPN"', FIELD)
	app:frames(10)
	expect(#refs_of(app)).to_be(2)
	-- Taken off with its ×, on the chip over the draft.
	app:hover('#control-float >> ~"@orchestrator:"')
	app:frames(5)
	app:click('#control-float >> role:button ~"Remove @orchestrator:"')
	app:frames(10)
	expect(#refs_of(app)).to_be(1)
end)

test("drag refs: sent, they go with the dump as its refs, never as files", function()
	local app = tall()
	tab(app, "board")
	app:drag(BOARD_CARD, FLOAT_FIELD)
	app:frames(10)
	tab(app, "map")
	app:drag('#orch-map-field >> role:button "Moscow node reboot plan"', FIELD)
	app:frames(10)
	app:type(FIELD, "same as these")
	app:key("Enter")
	app:frames(5)
	local sent: any = app:state("last_dump")
	expect(calls(app)).to_contain("dump:same as these")
	expect(calls(app)).never.to_contain("dump-file:")
	expect(calls(app)).to_contain("dump-ref:card:it-moscow,dump-ref:chat:th-vpn-moscow")
	expect(sent).never.to_be_nil()
	expect(sent.attachments).to_be_nil()
	expect(#sent.refs).to_be(2)
	expect(sent.refs[1].kind).to_be("card")
	expect(sent.refs[1].id).to_be("it-moscow")
	expect(sent.refs[1].label).to_be("Reboot Moscow node")
	expect(sent.refs[2].kind).to_be("chat")
	expect(sent.refs[2].id).to_be("th-vpn-moscow")
	-- The draft's chips gone with it.
	expect(#refs_of(app)).to_be(0)
end)

test("drag refs: a dump with nothing referenced has no refs", function()
	local app = tall()
	app:type(FIELD, "just words")
	app:key("Enter")
	app:frames(5)
	local sent: any = app:state("last_dump")
	expect(sent).never.to_be_nil()
	expect(sent.refs).to_be_nil()
end)

test("drag refs: Capture says Drop to reference while one is over it", function()
	local app = tall()
	-- At rest: nothing over the bar.
	expect(app:has("#lane-drop-ref-control-capture-float")).to_be(false)
	app:command("fixture.drag_refs")
	app:frames(10)
	expect(app:has("#lane-drop-ref-control-capture-float")).to_be(true)
	-- Its words on their own accent pill, over the washed, ringed zone.
	expect(app:has("#lane-drop-ref-control-capture-float-label")).to_be(true)
	expect(app:has('#lane-drop-ref-control-capture-float-label >> "Drop to reference"')).to_be(true)
	-- (The chips there under it.)
	expect(#refs_of(app)).to_be(3)
end)

test("drag refs: the Map's Machines and Timeline views drag their threads too", function()
	local app = tall()
	app:set_state("control_map", { variant = "machines", window = 15 })
	app:frames(20)
	local chip = dropped(app, '#control-map >> .orch-mc-row "Moscow node reboot plan"')
	expect_ref(chip, "chat", "th-vpn-moscow", '@chat:"Moscow node reboot plan"')
	app:set_state("control_map", { variant = "timeline", window = 15 })
	app:frames(20)
	app:drag('#control-map >> .orch-tl-lane "Exnomic VPN"', FIELD)
	app:frames(10)
	local refs = refs_of(app)
	expect(#refs).to_be(2)
	expect(refs[2].ref.kind).to_be("orchestrator")
	expect(refs[2].ref.id).to_be("orc-vpn")
end)

test("drag refs: dropped on the bar beside the files already on it, the chip is on the same draft", function()
	local app = mount("control_float_fixture.luau")
	app:resize(1440, 900)
	app:frames(30)
	app:command("fixture.float")
	app:frames(30)
	app:set_state("control_tab", "map")
	app:frames(30)
	app:set_state("control_map", { variant = "topology", window = 15 })
	app:frames(30)
	app:drag('#orch-map-field >> role:button "Moscow node reboot plan"', FIELD)
	app:frames(10)
	-- (No app:error() check: the shell's fixture asks a host action, `appearance`,
	-- a test host doesn't have, as it mounts.)
	local refs = refs_of(app)
	expect(#refs).to_be(1)
	expect_ref(refs[1], "chat", "th-vpn-moscow", '@chat:"Moscow node reboot plan"')
	-- (The files already on it kept, beside it.)
	expect(#chips(app)).to_be(3)
end)

-- The Graph: a node's grip (under the pointer, at its edge) drags a
-- reference to it; a plain drag of the node still places it.
local function graph(): any
	local app = tall()
	app:set_state("control_map", { variant = "graph", window = 15 })
	app:frames(90)
	return app
end

local function grip_drag(app: any, node: string): any
	app:hover(`#control-map >> {node}`)
	app:frames(5)
	expect(app:has("#orch-graph-grip")).to_be(true)
	return dropped(app, "#orch-graph-grip")
end

test("drag refs: the Graph's thread, by its grip", function()
	local app = graph()
	local chip = grip_drag(app, '.orch-graph-thread ~"Moscow node reboot plan"')
	expect_ref(chip, "chat", "th-vpn-moscow", '@chat:"Moscow node reboot plan"')
	expect(chip.ref.context.orchestrator).to_be("orc-vpn")
	-- (Not moved by it.)
	expect((app:state("control_map_graph_pins") or {})["th-vpn-moscow"]).to_be_nil()
end)

test("drag refs: the Graph's orchestrator, by its grip", function()
	local app = graph()
	local chip = grip_drag(app, '.orch-graph-orchestrator ~"Exnomic VPN"')
	expect_ref(chip, "orchestrator", "orc-vpn", '@orchestrator:"Exnomic VPN"')
end)

test("drag refs: the Graph's machine, by its grip", function()
	local app = graph()
	-- (A machine is hit by its tab, drawn on the canvas: the pointer there.)
	app:set_state("control_map_graph_hover", "dev:dev-mac")
	app:frames(5)
	expect(app:has("#orch-graph-grip")).to_be(true)
	local chip = dropped(app, "#orch-graph-grip")
	expect_ref(chip, "device", "dev-mac", "@device:macbook")
end)

test("drag refs: a Graph node dragged plainly still moves, no chip made", function()
	local app = graph()
	app:drag('#control-map >> .orch-graph-thread ~"Moscow node reboot plan"', '#control-map >> .orch-graph-device ~"Cloud"')
	app:frames(30)
	expect((app:state("control_map_graph_pins") or {})["th-vpn-moscow"] ~= nil).to_be(true)
	expect(#refs_of(app)).to_be(0)
end)
