--!strict
-- Orchestrators' app-side module (`@app/builtin/orchestrators` while the
-- plugin is on): its parts by name, each required as it's first read, so a
-- hook reaching one part loads that part alone. It runs as the app's code
-- (init.luau). The same parts as the app's own stand-in
-- (ui/builtin/orchestrators.luau) while their modules are still in ui/.
--
-- One part a line (`name = "module",`): moving the modules into app/
-- rewrites these paths to `./app/…`.

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
	proof = "./app/proof",
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
