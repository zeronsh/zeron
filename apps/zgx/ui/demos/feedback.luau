-- Empty states and progress.

local EmptyState = require("@ds/empty_state")
local progress = require("@ds/progress")
local Surface = require("@ds/surface")
local Button = require("@ds/button")
local kit = require("@demos/kit")

local amount = ui.state("fb_amount", 0.42)

return {
	id = "feedback",
	title = "Empty state and progress",
	about = "What an empty list says, and how to fill it. Progress fills to a value as it moves, or, with no value, says work of unknown size is going on.",
	demo = function()
		return kit.page(
			kit.block(
				"EMPTY STATES",
				ui.row {
					gap = "$space.4",
					items = "stretch",
					flex_wrap = true,
					Surface {
						kind = "raised",
						w = 400,
						EmptyState {
							icon = "pull-request",
							title = "No pull requests",
							body = "Pull requests from your agents and your team show here, with their checks and reviews.",
							action = { label = "New pull request", icon = "plus" },
							secondary = { label = "Learn more" },
						},
					},
					Surface {
						kind = "raised",
						w = 280,
						EmptyState {
							icon = "check-circle",
							size = "sm",
							fill = true,
							title = "Nothing to commit",
							body = "The working tree is clean.",
						},
					},
					Surface {
						kind = "raised",
						w = 280,
						EmptyState {
							icon = "search",
							size = "sm",
							fill = true,
							title = "No matches for “zgx”",
							body = "Try another word, or clear the filter.",
							action = { label = "Clear filter" },
						},
					},
				}
			),
			kit.block(
				"BAR",
				ui.column {
					gap = "$space.4",
					w = 360,
					progress.ProgressBar { value = amount, w_full = true, label = "Upload" },
					progress.ProgressBar { value = amount, w_full = true, size = "thin", tone = "success" },
					progress.ProgressBar { w_full = true, label = "Indexing" },
					progress.ProgressBar { value = amount, w_full = true, size = "thick", tone = "warning" },
				},
				kit.row(
					Button {
						"Less",
						size = "sm",
						tone = "secondary",
						on_click = function()
							amount:set(math.max(0, amount:peek() - 0.2))
						end,
					},
					Button {
						"More",
						size = "sm",
						tone = "secondary",
						on_click = function()
							amount:set(math.min(1, amount:peek() + 0.2))
						end,
					}
				)
			),
			kit.block(
				"RING",
				kit.row(
					progress.ProgressRing { value = amount },
					progress.ProgressRing { value = amount, size = 24 },
					progress.ProgressRing { value = amount, size = 32, tone = "success" },
					progress.ProgressRing {},
					progress.ProgressRing { size = 24, tone = "neutral" },
					progress.ProgressRing { size = 32 }
				)
			)
		)
	end,
}
