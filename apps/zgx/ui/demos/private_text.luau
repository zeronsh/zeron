-- The gallery's Private text page: email addresses, hidden, blurred, for
-- screen sharing (Settings → General → Hide email addresses), each one shown
-- by a click. The switch here is the app's own setting.

local ds = require("@ds")

local PEOPLE = {
	{ seed = "mara", name = "Mara Okafor", email = "mara@halcyon.dev" },
	{ seed = "theo", name = "Theo Lindqvist", email = "theo.lindqvist@halcyon.dev" },
	{ seed = "ines", name = "Ines Duarte", email = "ines_duarte+zeron@northwind.io" },
}

return {
	id = "private_text",
	title = "Private text",
	about = "An email address that hides, blurred, while screens may be seen: a blurred stand-in of the same length, never the address, that a click shows. Settings → General turns it on everywhere.",
	demo = function(): Node
		local rows: { Node } = {}
		for _, p in PEOPLE do
			table.insert(rows, ds.Settings.Row {
				label = p.name,
				description = p.email,
				private_description = true,
				lead = ds.Avatar { seed = p.seed, size = 32, label = p.name },
			})
		end
		return ui.column {
			w = 420,
			gap = "$space.6",
			ds.Settings.Group {
				ds.Settings.Row {
					label = "Hide email addresses",
					description = "Blurred everywhere, for screen sharing. Click one to see it.",
					control = ds.Switch { value = ds.PrivateText.hidden, label = "Hide email addresses" },
				},
			},
			ds.Settings.Group(rows :: any),
			ui.column {
				gap = "$space.1",
				ds.Hint { "In a line of text, the same height and baseline:" },
				ui.row {
					gap = "$space.1",
					items = "baseline",
					ui.text { "Signed in as", text_size = "$font.size.md", color = "$text.secondary" },
					ds.PrivateText { "mara@halcyon.dev", text_size = "$font.size.md", color = "$text.primary" },
				},
			},
		}
	end,
}
