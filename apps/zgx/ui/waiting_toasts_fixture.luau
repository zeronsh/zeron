-- What's waiting for you to allow it, in the window's corner
-- (screens/plugins/waiting): the shell (ui/shell_fixture.luau, as it is) with
-- screens/plugins loaded, the host's lists made up here and the host's
-- windows stubbed. `ui.command`s change the lists as the engine would. What
-- tests/waiting_toasts.test.luau and src/waiting_toast_tests.rs drive.

local node = require("./shell_fixture")
local plugins = require("@app/screens/plugins/state")
local templates = require("@app/screens/templates/state")

-- What Review asked of the host, in order.
local calls = ui.state("waiting_calls", {} :: { string })
local function called(what: string)
	local list = table.clone(calls:peek())
	table.insert(list, what)
	calls:set(list)
end
local stub_plugins = plugins :: any
stub_plugins.review = function(id: string)
	called(`review:{id}`)
end
stub_plugins.check_updates = function(): any
	return nil
end
local stub_templates = templates :: any
stub_templates.review = function(name: string)
	called(`template:{name}`)
end

require("@app/screens/plugins")

local function plugin(id: string, name: string, status: string?, hash: string?): any
	local waiting = status == nil or status == "pending"
	return {
		id = id,
		name = name,
		version = "1.0.0",
		description = `What {name} does.`,
		author = { kind = "unknown" },
		status = status or "pending",
		permissions = { "ui.add" },
		allowed = if waiting then {} else { "ui.add" },
		ask = if waiting or status == "off" then { "ui.add" } else {},
		hash = hash or `{id}-1`,
	}
end

local function publish(list: { any })
	plugins.now:set({ state = "ready", list = list })
end
local function pending(list: { any })
	templates.now:set({ state = "ready", active = "zeron", list = {}, pending = list })
end

local function command(id: string, run: () -> ())
	ui.command { id = id, title = id, menu = "Fixture", run = run }
end

command("waiting.two", function()
	publish { plugin("cua", "Cua"), plugin("random-number", "Random Number") }
end)
command("waiting.five", function()
	publish {
		plugin("cua", "Cua"),
		plugin("random-number", "Random Number"),
		plugin("focus", "Focus"),
		plugin("ping", "Ping"),
		plugin("notes", "Notes"),
	}
end)
-- One of the five dealt with.
command("waiting.five_less", function()
	publish {
		plugin("cua", "Cua"),
		plugin("random-number", "Random Number", "on"),
		plugin("focus", "Focus"),
		plugin("ping", "Ping"),
		plugin("notes", "Notes"),
	}
end)
-- Two plugins with the one name: a notice each.
command("waiting.twins", function()
	publish { plugin("cua", "Cua"), plugin("cua-two", "Cua") }
end)
-- Dealt with, however it ended; both on, or one declined.
command("waiting.allow_cua", function()
	publish { plugin("cua", "Cua", "on"), plugin("random-number", "Random Number") }
end)
command("waiting.decline_random", function()
	publish { plugin("cua", "Cua"), plugin("random-number", "Random Number", "off") }
end)
-- Cua again, as a new version.
command("waiting.change_cua", function()
	publish { plugin("cua", "Cua", nil, "cua-2"), plugin("random-number", "Random Number") }
end)
-- Not there yet (the engine not answered).
command("waiting.none", function()
	publish {}
end)
command("waiting.loading", function()
	plugins.now:set({ state = "loading", list = {} })
end)

command("waiting.template", function()
	pending { { pending = "dusk-1", name = "Dusk" } }
end)
command("waiting.template_here", function()
	pending { { pending = "dusk-1", name = "Dusk", here = true } }
end)
command("waiting.template_done", function()
	pending {}
end)

-- Settings → Plugins, where each has its own Review.
command("waiting.plugins_page", function()
	require("@app/screens/shell/route").settings("plugins")
end)

-- A toast whose action is a function: this load's alone.
local toast = require("@ds/toast")
command("waiting.function_toast", function()
	toast.show { title = "Branch deleted", duration = 0, action = { label = "Undo", run = function()
		called("undo")
	end } }
end)
-- And one asking for an action no module declared.
command("waiting.unknown_action", function()
	toast.show { title = "Nowhere", duration = 0, action = { label = "Go", run = "nowhere" } }
end)

return node
