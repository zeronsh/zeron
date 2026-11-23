--!strict
-- Orchestrators' app-side module (`@app/builtin/orchestrators` while the
-- plugin is on): its parts by name, each required as it's first read, so a
-- hook reaching one part loads that part alone. It runs as the app's code
-- (init.luau). With the plugin off, the app has its own stand-in
-- (ui/builtin/orchestrators.luau), which is `false`.
--
-- One part a line (`name = "module",`), each a module in app/.

local PARTS: { [string]: string } = {
	orchestration = "./app/account/orchestration",
	actions = "./app/screens/orchestrator/actions",
	dialogs = "./app/screens/orchestrator/dialogs",
	orchestrator_page = "./app/screens/orchestrator/page",
	control_page = "./app/screens/control/page",
	progress = "./app/screens/control/progress",
	settings_page = "./app/screens/settings/orchestration",
	data = "./app/ds/orch_data",
	decide = "./app/ds/orch_decide",
	name = "./app/ds/orch_name",
	hue = "./app/ds/orch_hue",
	map = "./app/ds/orch_map",
	ref = "./app/ds/orch_ref",
	injected = "./app/ds/orch_injected",
	reports = "./app/ds/orch_reports",
	hold = "./app/ds/hold",
	demos = "./app/demos/orchestration",
	demo_board = "./app/demos/orch_board",
	demo_crew = "./app/demos/orch_crew",
	demo_reports = "./app/demos/orch_reports",
	demo_live = "./app/demos/orch_live",
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
