-- Multi-line text field: auto-grow with a cap, and a commit message's
-- subject count.

local TextArea = require("@ds/text_area")
local kit = require("@demos/kit")

local notes = ui.state("ta_notes", "")
local message = ui.state("ta_message", "Generalise the PR badge into Badge\n\nThe sidebar drew its own; the git pages need the same look for checks and reviews, so it moves into the design system.")
local review = ui.state("ta_review", "Looks good. One question about the wash in light:")
local bad = ui.state("ta_bad", "")

return {
	id = "text_area",
	title = "Multi-line field",
	about = "Grows with what is written, from its least rows to a cap, then scrolls. A commit message holds to git's rules: its first line wants 50 characters and stops by 72, every other line wraps by 72, and whatever runs past takes the warning's ink, on whichever line it is.",
	demo = function()
		return kit.page(
			kit.block(
				"GROWS TO A CAP",
				ui.row {
					gap = "$space.6",
					items = "start",
					flex_wrap = true,
					TextArea { value = notes, label = "Notes", placeholder = "Anything the agent should know…", hint = "Grows from 3 to 8 lines.", rows = 3, max_rows = 8 },
					TextArea {
						value = bad,
						label = "Description",
						rows = 2,
						error = function()
							return if bad:get() == "" then "A description is required." else nil
						end,
					},
				}
			),
			kit.block(
				"A COMMIT MESSAGE",
				TextArea {
					value = message,
					label = "Message",
					placeholder = "Summary of the change",
					subject = { soft = 50, max = 72 },
					rows = 3,
					max_rows = 10,
					w = 440,
					hint = "⌘↵ commits.",
				}
			),
			kit.block("SMALL, IN A REVIEW", TextArea { value = review, size = "sm", rows = 2, max_rows = 6, name = "Reply", w = 360 })
		)
	end,
}
