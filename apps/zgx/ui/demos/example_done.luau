-- Setup, the last step: the organization is made. What was set up, in
-- three lines (its name and address, who was invited, where its chats
-- live), and the way into it. Nothing to go back to.

local ds = require("@ds")
local kit = require("@demos/example_kit")

return {
	id = "ex_done",
	title = "All set",
	about = "The last step of setup: the organization made, what was set up in three lines, and the way in. After it the window closes and Zeron opens on the organization.",
	demo = function()
		return kit.stage {
			w = 680,
			h = 500,
			kit.step {
				step = 3,
				count = 3,
				continue = { label = "Open Halcyon Labs" },
				kit.form {
					wide = true,
					title = "Halcyon Labs is ready",
					body = "You're its owner. Here's what was set up.",
					ds.Settings.Group {
						ds.Settings.Row {
							label = "Halcyon Labs",
							description = "zeron.sh/halcyon-labs",
							lead = ds.Tile { label = "Halcyon Labs", hue = "violet", size = 32 },
							badge = ds.Badge { "Owner" },
						},
						ds.Settings.Row {
							label = "2 invites sent",
							description = "Theo Lindqvist and Priya Raman join as members once they accept.",
							lead = ds.AvatarStack { people = { { id = "theo", name = "Theo Lindqvist" }, { id = "priya", name = "Priya Raman" } }, size = 24 },
						},
						ds.Settings.Row {
							label = "Chats stay in step",
							description = "Your chats and agents sync across all your devices.",
							lead = ui.row {
								w = 32,
								h = 32,
								shrink = 0,
								items = "center",
								justify = "center",
								rounded = "$radius.full",
								bg = "$zeron.tone.success.wash",
								color = "$zeron.tone.success.ink",
								ds.Icon { name = "check-circle", size = 16 },
							},
						},
					},
				},
			},
		}
	end,
}
