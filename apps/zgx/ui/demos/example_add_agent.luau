-- Settings: adding an agent. Every agent that speaks the Agent Client
-- Protocol, a page of its own inside Providers rather than a panel over
-- it: the registry searched, one agent read before it is added (where it
-- is from, what adding it takes on the device, where its models and
-- sign-in come from), the add in the same place, done or failed; and a
-- form for an agent of your own. The pieces are in providers_parts.luau.

local kit = require("@demos/example_kit")
local parts = require("@demos/providers_parts")

local shown = ui.state("ex_add_state", "browse")

return {
	id = "ex_add_agent",
	title = "Add an agent",
	about = "Settings → Providers → Add an agent: the registry's agents searched, each read before it is added (its maker, version, license, source, what adding it takes on the device), the add in place, done or failed; and a form for an agent of your own.",
	demo = function()
		ui.effect(function()
			local s = shown:get()
			if s == "browse" then
				parts.view:set("add")
			elseif s == "own" then
				parts.view:set("own")
			else
				parts.view:set("agent:gemini")
				parts.adding:set(if s == "details" then "idle" else s)
			end
		end)
		return kit.stage {
			states = {
				{ value = "browse", label = "Browse" },
				{ value = "details", label = "Details" },
				{ value = "adding", label = "Adding" },
				{ value = "added", label = "Added" },
				{ value = "failed", label = "Failed" },
				{ value = "own", label = "Your own" },
			},
			state = shown,
			w = 900,
			h = 640,
			kit.settings {
				page = "providers",
				ui.presence(function(): string
					return parts.view:get()
				end, function(view: string): Node
					return parts.page_for(view)
				end, { w_full = true, enter = { opacity = { 0, 1 }, y = { 6, 0 }, duration = 160 }, exit = { opacity = 0, duration = 80 }, mode = "wait" }),
			},
		}
	end,
}
