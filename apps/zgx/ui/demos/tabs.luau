-- Tabs, underline and pill.

local Tabs = require("@ds/tabs")
local kit = require("@demos/kit")

local section = ui.state("tb_section", "conversation")
local view = ui.state("tb_view", "open")
local settings = ui.state("tb_settings", "general")
local small = ui.state("tb_small", "unified")

return {
	id = "tabs",
	title = "Tabs",
	about = "The views of one thing, the indicator springing from tab to tab. Underline for a page's sections; pill for a quiet run of views in a header. A tab may carry an icon and a count.",
	demo = function()
		return kit.page(
			kit.block(
				"UNDERLINE, A PULL REQUEST'S SECTIONS",
				ui.column {
					w = 560,
					Tabs {
						value = section,
						label = "Pull request",
						options = {
							{ value = "conversation", label = "Conversation", icon = "chat-round-line", count = 8 },
							{ value = "commits", label = "Commits", icon = "git-branch", count = 3 },
							{ value = "checks", label = "Checks", icon = "checklist", count = 12 },
							{ value = "files", label = "Files", icon = "document", count = 21 },
						},
					},
				}
			),
			kit.block(
				"UNDERLINE, PLAIN",
				ui.column {
					w = 420,
					Tabs {
						value = settings,
						options = {
							{ value = "general", label = "General" },
							{ value = "agents", label = "Agents" },
							{ value = "git", label = "Git" },
							{ value = "beta", label = "Beta", disabled = true },
						},
					},
				}
			),
			kit.block(
				"PILL",
				Tabs {
					value = view,
					look = "pill",
					label = "Pull requests",
					options = {
						{ value = "open", label = "Open", count = 14 },
						{ value = "review", label = "Needs review", count = 3 },
						{ value = "mine", label = "Mine" },
						{ value = "closed", label = "Closed" },
					},
				},
				Tabs {
					value = small,
					look = "pill",
					size = "sm",
					options = {
						{ value = "unified", label = "Unified" },
						{ value = "split", label = "Split" },
					},
				}
			)
		)
	end,
}
