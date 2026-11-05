-- Onboarding: a hero, a step header, and option cards (one, or several).

local onboarding = require("@ds/onboarding")
local Button = require("@ds/button")
local Mark = require("@ds/mark")
local Icon = require("@ds/icon")
local badge = require("@ds/badge")
local Surface = require("@ds/surface")
local kit = require("@demos/kit")

local step = ui.state("ob_step", 2)
local look = ui.state("ob_look", "system")
local agents = ui.state("ob_agents", { "claude", "codex" })
local where = ui.state("ob_where", "worktree")

return {
	id = "onboarding_blocks",
	title = "Onboarding",
	about = "A hero opens a screen, centred. A step header says where in the flow this is and what it asks. Option cards make a choice by picking a card, one or several; chosen, a card wears the accent's ring, never a fill.",
	demo = function()
		return kit.page(
			kit.block(
				"HERO",
				Surface {
					kind = "sunken",
					w_full = true,
					h = 380,
					p = "$space.6",
					onboarding.Hero {
						art = ui.svg { src = "assets/icons/ui/zeron-logo.svg", w = 40, h = 46, tint = "$text.primary" },
						title = "Welcome to Zeron",
						body = "Every agent you use, on every machine you have, in one place.",
						foot = ui.text { "Already set up on another Mac? Sign in", text_size = "$font.size.sm", color = "$text.tertiary" },
						Button { "Get started", size = "lg", w = 280, Icon { name = "arrow-right" } },
					},
				}
			),
			kit.block(
				"STEP HEADER",
				ui.column {
					w = 520,
					gap = "$space.4",
					onboarding.StepHeader {
						step = function()
							return step:get()
						end,
						count = 4,
						title = "Which agents do you use?",
						body = "Zeron finds the ones installed here. Pick any to set up now; the rest can wait.",
					},
					kit.row(
						Button {
							"Back",
							tone = "ghost",
							size = "sm",
							on_click = function()
								step:set(math.max(1, step:peek() - 1))
							end,
						},
						Button {
							"Next",
							tone = "secondary",
							size = "sm",
							on_click = function()
								step:set(math.min(4, step:peek() + 1))
							end,
						}
					),
				}
			),
			kit.block(
				"SEVERAL, AS TILES",
				ui.column {
					w = 560,
					onboarding.OptionCards {
						value = agents,
						multiple = true,
						columns = 3,
						label = "Agents",
						options = {
							{ value = "claude", title = "Claude Code", description = "Installed", mark = "claude" },
							{ value = "codex", title = "Codex", description = "Installed", mark = "openai" },
							{ value = "grok", title = "Grok", description = "Not found", mark = "grok" },
							{ value = "cursor", title = "Cursor", description = "Installed", mark = "cursor" },
							{ value = "opencode", title = "OpenCode", description = "Not found", mark = "opencode" },
							{ value = "devin", title = "Devin", description = "Needs a team plan", mark = "devin", disabled = true },
						},
					},
				}
			),
			kit.block(
				"ONE, AS TILES",
				ui.column {
					w = 560,
					onboarding.OptionCards {
						value = look,
						columns = 3,
						label = "Appearance",
						options = {
							{ value = "system", title = "Match system", icon = "monitor" },
							{ value = "light", title = "Light", icon = "global" },
							{ value = "dark", title = "Dark", icon = "moon" },
						},
					},
				}
			),
			kit.block(
				"ONE, AS ROWS",
				ui.column {
					w = 480,
					onboarding.OptionCards {
						value = where,
						columns = 1,
						label = "Where agents work",
						options = {
							{
								value = "worktree",
								title = "In a worktree",
								description = "Each chat on a branch of its own. Your checkout stays as you left it.",
								icon = "worktree",
								badge = badge.Badge { "Recommended", tone = "accent" },
							},
							{ value = "checkout", title = "In your checkout", description = "Changes land where you're working.", icon = "folder" },
						},
					},
				}
			)
		)
	end,
}
