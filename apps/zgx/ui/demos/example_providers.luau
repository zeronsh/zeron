-- Settings: providers. The coding agents on a device and the accounts
-- they sign in with. Someone comes here to see which agent is ready, to
-- sign in, and to switch accounts when one hits its limit, so that is the
-- whole page: each installed agent a card, its accounts under it with how
-- much of each plan is used, the way to add one at the card's foot; and
-- under those the agents still to install, Zeron's own a click each and
-- every other agent a page away. Each agent's settings open from its card.
-- The pieces are in providers_parts.luau.

local ds = require("@ds")
local kit = require("@demos/example_kit")
local parts = require("@demos/providers_parts")

local shown = ui.state("ex_pv_state", "ready")

return {
	id = "ex_providers",
	title = "Providers",
	about = "Settings → Providers: the agents on a device, each with the accounts it signs in with, how much of each plan is used, and the way to add or switch one; a sign-in under way and one that failed, in place; Codex's banked resets offered where a limit is hit; each agent's settings from its card; the agents still to install. And what shows while the device is checked, and when it can't be.",
	demo = function()
		local function body(key: string): Node
			if key == "loading" then
				return parts.loading()
			elseif key == "failed" then
				return parts.unavailable(parts.device_name, function()
					shown:set("ready")
				end)
			end
			return ui.column { w_full = true, gap = "$space.6", parts.cards(), parts.more() }
		end
		local function page(view: string): Node
			if view ~= "agents" then
				return parts.page_for(view)
			end
			return ds.Settings.Page {
				title = "Providers",
				description = "The coding agents on a device, and the accounts they sign in with.",
				-- Agents are per device: which one's.
				trail = ds.Select {
					value = parts.device,
					name = "Device",
					size = "sm",
					w = 200,
					options = parts.DEVICES :: any,
				},
				-- Loading, failed or ready: the cards follow their sign-ins and
				-- resets in place.
				ui.presence(function(): string
					return shown:get()
				end, body, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
			}
		end
		-- Opening the page, it shows the agents (the add pages keep their own
		-- example).
		parts.view:set("agents")
		return kit.stage {
			states = {
				{ value = "ready", label = "Ready" },
				{ value = "loading", label = "Loading" },
				{ value = "failed", label = "Failed" },
			},
			state = shown,
			w = 900,
			h = 640,
			overlays = parts.dialogs(),
			kit.settings {
				page = "providers",
				ui.presence(function(): string
					return parts.view:get()
				end, page, { w_full = true, enter = { opacity = { 0, 1 }, y = { 6, 0 }, duration = 160 }, exit = { opacity = 0, duration = 80 }, mode = "wait" }),
			},
		}
	end,
}
