-- The Map's view: Graph (force) when nothing's saved (a fresh install, a
-- reset); a view the person chose stays theirs.

test("map default: nothing saved opens on Graph (force)", function()
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 2400)
	app:command("map_only")
	app:frames(30)
	expect(app:state("control_map") == nil or app:state("control_map").variant == "force").to_be(true)
	expect(app:has("#orch-force-field")).to_be(true)
end)

test("map default: a view the person chose is kept", function()
	local before = mount("orch_map_fixture.luau")
	before:frames(5)
	before:set_state("control_map", { variant = "timeline", window = 60 })
	before:frames(2)
	local app = mount("orch_map_fixture.luau")
	app:resize(1440, 2400)
	app:command("map_only")
	app:frames(30)
	expect(app:state("control_map").variant).to_be("timeline")
	expect(app:has("#orch-force-field")).to_be(false)
end)
