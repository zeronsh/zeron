-- The orchestrator page's Flow view (ds/orch_flow_editor) in the sample
-- world (ui/orchestrator_fixture.luau): the Orchestrators orchestrator open
-- on its flow, Plan → Build → Review, its Worker selected. For
-- tests/orch_flow_page.test.luau and pictures of the view. Commands:
--   orch.dark / orch.light      the theme (the page fixture's)
--   flow.none                   no flow yet (its starters offered)
--   flow.default                Plan → Build → Review again

local shell = require("./orchestrator_fixture")
local flow = require("@ds/orch_flow")
local flows = require("@app/screens/orchestrator/flows")
local page = require("@app/screens/orchestrator/page")

local ID = "orc-orch"

flows.set(ID, flow.initial())
page.view:set("flow")
require("@ds/orch_flow_editor").selected:set({ node = "3" })

ui.command { id = "flow.none", title = "No flow", run = function()
	flows.forget(ID)
end }
ui.command { id = "flow.default", title = "Plan → Build → Review", run = function()
	flows.set(ID, flow.initial())
end }

return shell
