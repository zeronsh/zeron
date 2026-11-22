-- An in-tree hook gated on Orchestrators, as the app's will be, for
-- tests/orchestrators_proof.test.luau: shown while `plugins.on`, drawing
-- what the plugin's app-side module gives. `orch_proof_on` seeds the
-- window's word on the plugin (off by default, as on a fresh install).
local ds = require("@ds")
ds.setup()
local plugins = require("@account/plugins")
local on = ui.state("orch_proof_on", false)
ui.effect(function()
	plugins.set({ orchestrators = on:get() })
end)
return ui.column {
	id = "root",
	ui.text { id = "orch-proof-shell", "Shell" },
	ui.show(function(): boolean
		return plugins.on("orchestrators")
	end, ui.text {
		id = "orch-proof",
		function(): string
			local orch = plugins.module("orchestrators")
			return if orch then orch.proof.label() else "no module"
		end,
	}),
}
