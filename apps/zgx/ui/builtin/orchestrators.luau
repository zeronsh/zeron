-- Orchestrators' app-side module as the app has it while the plugin's
-- modules are still in ui/: its parts by name, each required the first time
-- it's read, so a hook reaching one part loads that part alone. The
-- built-in plugin (plugins-builtin/orchestrators) puts its own index, the
-- same parts, in this one's place while it's on (`ui.override`, a bundled
-- built-in's trust: docs/plugins.md, "Zeron's own plugins"). Reach it
-- through `require("@account/plugins").module("orchestrators")`, never
-- directly: with the plugin off that's nil, and nothing here is required.
--
-- One part a line (`name = "module",`): moving the modules into the plugin
-- rewrites its index's paths and turns this back into `return false`; the
-- hooks stay as they are.

local PARTS: { [string]: string } = {
	orchestration = "@account/orchestration",
	actions = "@app/screens/orchestrator/actions",
	dialogs = "@app/screens/orchestrator/dialogs",
	orchestrator_page = "@app/screens/orchestrator/page",
	control_page = "@app/screens/control/page",
	progress = "@app/screens/control/progress",
	settings_page = "@app/screens/settings/orchestration",
	data = "@ds/orch_data",
	decide = "@ds/orch_decide",
	name = "@ds/orch_name",
	hue = "@ds/orch_hue",
	map = "@ds/orch_map",
	ref = "@ds/orch_ref",
	injected = "@ds/orch_injected",
	reports = "@ds/orch_reports",
	hold = "@ds/hold",
	demos = "@demos/orchestration",
	demo_board = "@demos/orch_board",
	demo_crew = "@demos/orch_crew",
	demo_reports = "@demos/orch_reports",
	demo_live = "@demos/orch_live",
}

return setmetatable({}, {
	__index = function(self: any, name: string): any
		local path = PARTS[name]
		if not path then
			return nil
		end
		local part = (require :: any)(path)
		rawset(self, name, part)
		return part
	end,
})
