-- Crew threads are named in every Map view, never a bare dot
-- (ds/orch_name): a just-created one by the title it was given, before its
-- first turn ends; one whose chat isn't heard yet by its role; one with no
-- title of its own by its board item's title; one with nothing to go by as
-- "Crew thread" and its id. On ui/orch_map_fixture.luau's `map_crew_names`.

local Name = require("@ds/orch_name")
local model = require("@ds/orch_map_model")

local MAP = "#control-map"
local NAMES = {
	"Board card declutter", -- its own title, its turn done
	"End-of-turn question asks", -- its own title, its first turn running
	"hover card fix", -- its chat not heard yet: its role
	"Crew threads in sidebar", -- no title of its own, no role: its item
	"Crew thread 9f3a1c", -- nothing: the last resort
}

local function open(variant: string): any
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 2400)
	app:frames(10)
	app:command("map_crew_names")
	app:command("map_only")
	app:command(`map_{variant}`)
	app:frames(30)
	return app
end

test("crew names: the rule, a chat's own title first", function()
	local member = { chat_id = "c1", role = "end-of-turn asks", item_id = "i1" }
	local item = { id = "i1", title = "Turn-end asks" }
	-- A just-created crew thread: its row names it (`name`), nothing else
	-- needed.
	local row = { id = "c1", parent = "o", name = "End-of-turn question asks", title = "End-of-turn question asks", status = "working" }
	expect(Name.thread(row, member, item, "c1")).to_be("End-of-turn question asks")
	-- Its chat not heard yet: its role, then its item.
	expect(Name.thread(nil, member, item, "c1")).to_be("end-of-turn asks")
	expect(Name.thread(nil, { chat_id = "c1", role = "  " }, item, "c1")).to_be("Turn-end asks")
	-- A side chat's row without a title says what it last said as `title`:
	-- a crew thread isn't called that, but by its role.
	local untitled = { id = "c1", parent = "o", title = "[Message from Zeron chat]" }
	expect(Name.thread(untitled, member, item, "c1")).to_be("end-of-turn asks")
	-- (Nothing to go by: the last resort, its id in it. Its words are
	-- checked in the app, below.)
	expect(Name.thread(untitled, { chat_id = "c1" }, nil, "c1abcdef")).to_be(Name.unnamed("c1abcdef"))
	expect(Name.unnamed("c1abcdef")).to_contain("c1abcd")
	-- Not crew: a chat's own title, else its host's word.
	expect(Name.thread({ id = "x", title = "Fix the build" }, nil, nil, "x")).to_be("Fix the build")
	expect(Name.thread(untitled, nil, nil, "c1")).to_be("[Message from Zeron chat]")
	-- Never empty.
	expect(Name.thread(nil, nil, nil, "")).never.to_be("")
	-- Looked up: an orchestrator by its name.
	local now = { orchestrators = { { id = "o", name = "Zeron fixes" } }, crew = { member }, items = { item } }
	expect(Name.of(now, {}, "o")).to_be("Zeron fixes")
	expect(Name.of(now, {}, "c1")).to_be("end-of-turn asks")
	expect(Name.of(now, { c1 = row }, "c1")).to_be("End-of-turn question asks")
end)

test("crew names: the model names every crew thread, heard or not", function()
	local t = os.time()
	local world = {
		now = {
			orchestrators = { { id = "o", name = "Zeron fixes", hue = 20 } },
			crew = {
				{ chat_id = "new", orchestrator_id = "o", role = "end-of-turn asks" },
				{ chat_id = "gone", orchestrator_id = "o", role = "", item_id = "i", joined = t },
			},
			items = { { id = "i", title = "Hover cards", order = 1 } },
		},
		chats = {
			o = { id = "o", title = "Zeron fixes", device = "d" },
			new = { id = "new", parent = "o", name = "End-of-turn question asks", title = "End-of-turn question asks", device = "d", status = "working", created = t, updated = t },
		},
		devices = { { id = "d", name = "honeypot-1", here = true } },
		here = "d",
	}
	local f = model.fleet(world)
	expect(f.threads.new.title).to_be("End-of-turn question asks")
	expect(f.threads.gone.title).to_be("Hover cards")
	expect(f.threads.o.title).to_be("Zeron fixes")
end)

test("crew names: Machines names each crew thread, none a bare dot", function()
	local app = open("machines")
	expect(app:count(`{MAP} >> .orch-pill`)).to_be(#NAMES)
	for _, n in NAMES do
		expect(app:has(`{MAP} >> .orch-pill ~"{n}"`)).to_be(true)
	end
	-- A just-created one opens as its chat.
	app:click(`{MAP} >> .orch-pill ~"End-of-turn question asks"`)
	app:frames(10)
	expect(app:state("shell_route_path")).to_be("/session/th-asks")
end)

test("crew names: Topology and the Timeline name them alike", function()
	local app = open("topology")
	expect(app:count(`{MAP} >> .orch-pill`)).to_be(#NAMES)
	for _, n in NAMES do
		expect(app:has(`{MAP} >> .orch-pill ~"{n}"`)).to_be(true)
	end
	app:command("map_timeline")
	app:frames(30)
	for _, n in NAMES do
		expect(app:has(`{MAP} >> "{n}"`)).to_be(true)
	end
end)

test("crew names: the Graph names its nodes alike", function()
	local app = open("graph")
	for _, n in NAMES do
		expect(app:has(`{MAP} >> ~"{n}"`)).to_be(true)
	end
end)
