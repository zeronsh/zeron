-- The window that asks before you act as yourself in the store, mounted on
-- its own with acts made up here (no engine in a test host): what
-- tests/store_consent.test.luau drives. The host's `store_consent` is set
-- here; `ui.command`s show another act, or where one stands.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local consent = require("@screens/plugins/act_consent")

local shown = ui.shared("store_consent", { busy = true } :: any)

local ACTS: { [string]: any } = {
	publish = { act = "publish", kind = "plugin", id = "focus-timer", name = "Focus timer", version = "1.3.0" },
	unyank = { act = "unyank", kind = "plugin", id = "focus-timer", name = "focus-timer", version = "1.2.0" },
	token = { act = "token", name = "GitHub Actions", scope = { plugins = { "focus-timer", "tidy" } }, expires = 1795132800000 },
	many = {
		act = "token",
		name = "Release everything",
		scope = { plugins = (function(): { string }
			local ids = {}
			for i = 1, 50 do
				table.insert(ids, `a-plugin-with-a-long-id-number-{i}`)
			end
			return ids
		end)() },
	},
	token_both = { act = "token", name = "Both", scope = { all = true, all_kinds = { "plugin", "template" } } },
	token_all = { act = "token", name = "CI", scope = { all = true } },
	hostile = {
		act = "publish",
		kind = "plugin",
		id = string.rep("a", 40),
		name = "{count} [b](https://x) " .. string.rep("W", 60),
		version = string.rep("9", 32),
	},
}

local said: { string } = {}
local calls = ui.state("calls", "")
local function note(what: string)
	table.insert(said, what)
	calls:set(table.concat(said, ","))
end

ui.actions {
	["store.decide"] = function(arg: any)
		note(`decide:{tostring(arg.allow)}`)
	end,
	close = function()
		note("close")
	end,
}
ui.actions {
	["secret.copy"] = function(arg: any)
		note(`copy:{arg.text}`)
	end,
}

for id in ACTS do
	ui.command {
		id = id,
		title = id,
		run = function()
			shown:set({ busy = false, item = ACTS[id] } :: any)
		end,
	}
end
ui.command { id = "raised", title = "Raised", run = function()
	local now = table.clone(shown:peek())
	now.raised = (now.raised or 0) + 1
	shown:set(now)
end }
ui.command { id = "busy", title = "Busy", run = function()
	shown:set({ busy = true, item = ACTS.token } :: any)
end }
ui.command { id = "made", title = "Made", run = function()
	shown:set({ busy = false, item = ACTS.token, secret = "zdt_abcdefgh0123456789" } :: any)
end }

-- (Pictures: each scheme.)
local actions = require("@screens/onboarding/actions")
for _, scheme in { "light", "dark" } do
	ui.command { id = scheme, title = scheme, run = function()
		actions.apply_scheme(scheme :: any)
	end }
end
for _, name in { "en", "zh-Hans" } do
	ui.command { id = `locale_{name}`, title = name, run = function()
		ui.locale:set(name)
	end }
end

shown:set({ busy = false, item = ACTS.publish } :: any)

return ui.column {
	w = 460,
	h = 540,
	consent.View(),
}
