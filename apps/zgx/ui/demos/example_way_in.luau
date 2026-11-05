-- The intro's way in (ds.Onboarding.WayIn), on stills of the scene it
-- stands on: the night it opens on and the sunrise the way in shows over,
-- with its words. Setup's forward pill in glass, a size for the window and
-- a step up for a whole display. Hover and press it; Tab for its focus
-- ring.

local ds = require("@ds")
local Onboarding = ds.Onboarding

-- The scene's plates (intro.luau's), shown whole: past the reveal, the
-- picture under its characters.
local PLATES = {
	{ src = "assets/art/plate-1-night.jpg", label = "Night" },
	{ src = "assets/art/plate-3-sunrise.jpg", label = "Sunrise" },
}

type Option = { label: string, size: "md" | "lg" }
local OPTIONS: { Option } = {
	{ label = "Window size", size = "md" },
	{ label = "Display size", size = "lg" },
}

-- A still of the scene, `w` by `h`, with what stands on it.
local function still(src: string, w: number, h: number, ...: Node): Node
	return ui.overlay {
		w = w,
		h = h,
		rounded = "$radius.lg",
		overflow = "hidden",
		ui.image { src = src, absolute = true, left = 0, right = 0, top = 0, bottom = 0, fit = "cover" },
		ui.column {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			items = "center",
			justify = "center",
			gap = "$space.6",
			...,
		},
	}
end

return {
	id = "ex_way_in",
	title = "Intro: the way in",
	about = "The intro's Get started: setup's forward pill (the one sign-in's Create an account is) in frosted glass, a white tint over a blur of the sky, a hairline and light words, so it stands on the night and on the sunrise, and is the same object the intro hands over to sign-in.",
	demo = function()
		local rows: { Node } = {}
		for _, plate in PLATES do
			local cells: { Node } = {}
			for _, o in OPTIONS do
				table.insert(
					cells,
					ui.column {
						gap = "$space.2",
						still(plate.src, 260, 150, Onboarding.WayIn { label = "Get started", size = o.size }),
						ds.Text { `{plate.label} · {o.label}`, style = "caption", tone = "tertiary" },
					}
				)
			end
			table.insert(rows, ui.row { gap = "$space.3", flex_wrap = true, table.unpack(cells) })
		end
		return ui.column {
			gap = "$space.6",
			ui.column { gap = "$space.4", table.unpack(rows) },
			-- In place: under the words, at the display's size.
			ui.column {
				gap = "$space.2",
				still(
					"assets/art/plate-3-sunrise.jpg",
					820,
					360,
					ui.text {
						"Your agents, anywhere.",
						-- (The intro's own words, art outside the ds, scaled to the still.)
						font = ui.raw("Instrument Serif", "the intro's own type, art outside the ds"),
						text_size = ui.raw(56, "the intro's words, at this still's scale"),
						color = "$zeron.scene.words",
					},
					Onboarding.WayIn { id = "way_in", label = "Get started", size = "lg" }
				),
				ds.Text { "In place, under the words", style = "caption", tone = "tertiary" },
			},
		}
	end,
}
