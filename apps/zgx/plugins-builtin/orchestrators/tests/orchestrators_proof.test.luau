-- Orchestrators' app-side module, reached the way the app's hooks reach it:
-- `@account/plugins` says whether it's on, and `module("orchestrators")`
-- gives the plugin's own `@app/builtin/orchestrators` (overrides.luau), which
-- runs as the app's code and requires the plugin's other files.

test("Orchestrators proof: off by default, the gated hook isn't there", function()
	local app = mount("orchestrators_proof_fixture.luau")
	app:frames(2)
	expect(app:error()).to_be_nil()
	expect(app:has("#orch-proof-shell")).to_be(true)
	expect(app:has("#orch-proof")).to_be(false)
end)

test("Orchestrators proof: on, the hook draws the plugin's module", function()
	local app = mount("orchestrators_proof_fixture.luau")
	app:set_state("orch_proof_on", true)
	app:frames(2)
	expect(app:error()).to_be_nil()
	expect(app:text("#orch-proof")).to_be("Orchestrators")
end)

test("Orchestrators proof: turned off again, the hook goes", function()
	local app = mount("orchestrators_proof_fixture.luau")
	app:set_state("orch_proof_on", true)
	app:frames(2)
	app:set_state("orch_proof_on", false)
	app:frames(2)
	expect(app:has("#orch-proof")).to_be(false)
	expect(app:error()).to_be_nil()
end)
