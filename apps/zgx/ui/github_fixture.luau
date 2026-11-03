-- Connect GitHub, mounted on its own with GitHub made up here (no engine in
-- a test host): the step tests/providers_stability.test.luau drives. The
-- host's calls (account/github) are stubbed; `ui.command`s change what the
-- engine would report (the repositories GitHub lets Cloud see, the account).

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local github = require("@account/github")
-- (A component, as the app's step is built inside one.)
local step = ui.component(require("@screens/onboarding/github"))

local stub = github :: any
stub.load = function() end
stub.connect = function() end
stub.cancel = function() end
stub.open = function() end
stub.choose = function() end
stub.disconnect = function() end

local repos = { "example/one", "example/two" }
local function publish(account: string)
	github.now:set({ phase = "connected", account = account, repos = table.clone(repos) } :: any)
end
publish("octo-example")

ui.command {
	id = "more_repos",
	title = "GitHub lets Cloud see another repository",
	run = function()
		table.insert(repos, "example/three")
		publish("octo-example")
	end,
}
ui.command {
	id = "renamed",
	title = "The GitHub account is renamed",
	run = function()
		publish("octo-renamed")
	end,
}

return ds.Onboarding.Frame {
	step {
		step = 6,
		count = 8,
		on_back = function() end,
		on_continue = function() end,
	},
}
