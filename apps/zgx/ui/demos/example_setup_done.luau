-- Setup's ending for this device (onboarding.rs render_done): what was set
-- up, a line each (the look, sync, GitHub when you're signed in, the
-- agents), and the way into Zeron. Not a numbered step, so no dots. With
-- no project yet, the way on is adding one (a project is a folder on one
-- of your devices) and I'll do this later stands beside it; with projects
-- already (synced from another device), it's simply Start using Zeron.
-- Anything skipped says where it waits ("Not yet · Settings → Cloud").
--
-- Each line keeps its place whatever it says; signed out, GitHub's line
-- isn't there at all, as GitHub isn't a step then.

local ds = require("@ds")
local kit = require("@demos/example_kit")
local setup = require("@demos/setup_parts")

local shown = ui.state("ex_sd_state", "first")

local function skipped(): boolean
	return shown:get() == "skipped"
end
local function projects(): boolean
	return shown:get() == "projects"
end

-- A line of the summary (ds.Onboarding.Fact, the app's own step's).
local function fact(label: string, value: () -> string): Node
	return ds.Onboarding.Fact { label = label, value = value }
end

return {
	id = "ex_setup_done",
	title = "You're all set",
	about = "The ending of this device's setup: the look, sync, GitHub (signed in only) and the agents, a line each, anything skipped saying where it waits; then Add a project (or I'll do this later), or Start using Zeron when projects came with the account.",
	demo = function()
		local function view(account: string): Node
			local inside = account == "in"
			local rows: { Node } = {
				fact("Appearance", function()
					return "Dark · Zeron"
				end),
				fact("Sync", function()
					return if inside then "On" else "Only on this Mac"
				end),
			}
			if inside then
				table.insert(rows, fact("GitHub", function()
					return if skipped() then "Not yet · Settings → Cloud" else "Connected"
				end))
			end
			table.insert(rows, fact("Agents", function()
				return if skipped() then "None yet" else "Claude Code, Codex and Cursor"
			end))
			return kit.step {
				back = kit.back {},
				aside = ui.show(function()
					return not projects()
				end, kit.quiet { label = "I'll do this later" }),
				continue = {
					label = function()
						return if projects() then "Start using Zeron" else "Add a project"
					end,
				},
				kit.form {
					wide = true,
					title = "You're all set",
					body = function()
						return if projects() then "Pick a project and start a chat." else "A project is a folder on one of your devices. Add one to start a chat."
					end,
					ds.Settings.Group(rows :: any),
				},
			}
		end
		return kit.stage {
			states = {
				{ value = "first", label = "No projects" },
				{ value = "projects", label = "Has projects" },
				{ value = "skipped", label = "Skipped some" },
			},
			state = shown,
			also = setup.account_switch(),
			w = 680,
			h = 500,
			ui.presence(function(): string
				return setup.account:get()
			end, view, { w_full = true, h_full = true }),
		}
	end,
}
