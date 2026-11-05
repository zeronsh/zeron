-- Settings: a page, sections, groups and rows, built from the other
-- primitives.

local Settings = require("@ds/settings")
local Switch = require("@ds/switch")
local Select = require("@ds/select")
local TextField = require("@ds/text_field")
local NumberStepper = require("@ds/stepper")
local Segmented = require("@ds/segmented")
local Button = require("@ds/button")
local SearchField = require("@ds/search_field")

local sign = ui.state("st_sign", true)
local push = ui.state("st_push", false)
local fetch = ui.state("st_fetch", true)
local editor = ui.state("st_editor", "zed" :: string?)
local author = ui.state("st_author", "Mara")
local email = ui.state("st_email", "ada@example.net")
local agents = ui.state("st_agents", 4)
local mode = ui.state("st_mode", "worktree")
local query = ui.state("st_query", "")

return {
	id = "settings",
	title = "Settings",
	about = "A page at a reading width; sections under a small title; groups of rows on a quiet card, a hairline between each; a row's name and line, its control at its end, or stacked under it, or leading on.",
	demo = function()
		return Settings.Page {
			title = "Git",
			description = "How Zeron commits, pushes and keeps your branches in step.",
			trail = SearchField { value = query, placeholder = "Search settings", size = "sm", w = 200 },
			Settings.Section {
				title = "Commits",
				description = "Applies to every project on this Mac.",
				Settings.Group {
					Settings.Row {
						label = "Sign commits",
						description = "With the SSH key Zeron made for this device.",
						control = Switch { value = sign, label = "Sign commits" },
					},
					Settings.Row {
						label = "Push after committing",
						description = "To the branch's upstream, setting one if it has none.",
						control = Switch { value = push, label = "Push after committing" },
					},
					Settings.Row {
						label = "Author",
						description = "The name and email on commits Zeron makes for you.",
						stacked = true,
						control = ui.row {
							gap = "$space.3",
							TextField { value = author, name = "Name", grow = 1 },
							TextField { value = email, name = "Email", grow = 1, icon = "global" },
						},
					},
				},
			},
			Settings.Section {
				title = "Agents",
				Settings.Group {
					Settings.Row {
						label = "Where agents work",
						control = Segmented {
							value = mode,
							label = "Where agents work",
							options = { { value = "worktree", label = "Worktree" }, { value = "checkout", label = "Checkout" } },
						},
					},
					Settings.Row {
						label = "Parallel agents",
						description = "How many run at once before the rest wait.",
						control = NumberStepper { value = agents, min = 1, max = 8, name = "Parallel agents" },
					},
					Settings.Row {
						label = "Open files in",
						control = Select {
							value = editor,
							name = "Open files in",
							w = 180,
							options = { { value = "zed", label = "Zed" }, { value = "cursor", label = "Cursor" }, { value = "code", label = "VS Code" } },
						},
					},
					Settings.Row { label = "Fetch in the background", description = "Every 5 minutes, while Zeron is open.", control = Switch { value = fetch, label = "Fetch" }, disabled = true },
				},
			},
			Settings.Section {
				title = "Accounts",
				Settings.Group {
					Settings.Row { label = "GitHub", description = "Signed in as maraokafor", icon = "global", on_click = function() end, value = "Connected" },
					Settings.Row { label = "SSH keys", icon = "lock-keyhole-unlocked", on_click = function() end, value = "2 keys" },
				},
			},
			Settings.Section {
				Settings.Group {
					Settings.Row {
						label = "Reset Git settings",
						description = "Back to Zeron's defaults. Your repositories aren't touched.",
						control = Button { "Reset", tone = "secondary", size = "sm" },
					},
					Settings.Row { label = "Sign out of GitHub", danger = true, on_click = function() end },
				},
			},
		}
	end,
}
