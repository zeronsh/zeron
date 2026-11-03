-- The gallery's Theme library page: every palette a window can wear, Zeron's
-- built in and your imported VS Code themes (screens/settings/theme_library,
-- src/themes.rs), each as a swatch of its surfaces, inks, accent, tones and
-- code colours; and the import itself, as Settings → Appearance has it.

local ds = require("@ds")
local palettes = require("@ds/palettes")
local library = require("@app/screens/settings/theme_library")

-- A colour of a palette's own (not a token: it's the palette's).
local function own(color: string): any
	return ui.raw(color, "the palette's own colour")
end

local function swatch(p: any): Node
	local chips: { Node } = {}
	for _, c in { p.accent, p.danger, p.warning, p.success } do
		table.insert(chips, ui.div { w = 10, h = 10, rounded = "$radius.full", bg = own(c) })
	end
	local code: { Node } = {}
	for _, c in p.syntax or {} do
		table.insert(code, ui.div { w = 6, h = 14, rounded = "$radius.sm", bg = own(c) })
	end
	return ui.column {
		w = 200,
		shrink = 0,
		gap = "$space.2",
		p = "$space.3",
		rounded = "$radius.xl",
		border = 1,
		border_color = "$border",
		bg = own(p.background),
		ui.text { p.name, truncate = true, weight = "$font.weight.medium", color = own(p.text) },
		ui.row {
			gap = "$space.2",
			items = "center",
			ui.text { "muted", text_size = "$font.size.xs", color = own(p.muted) },
			ui.text { "faint", text_size = "$font.size.xs", color = own(p.faint) },
			ui.div { grow = 1 },
			table.unpack(chips),
		},
		ui.row { gap = "$zeron.space.hair", p = "$space.1", rounded = "$radius.md", bg = own(p.raised), table.unpack(code) },
	}
end

-- One section's swatches, as the library changes (keyed by palette).
local function grid(list: () -> { any }): Node
	local items = ui.computed(list)
	return ui.row {
		w_full = true,
		flex_wrap = true,
		gap = "$space.3",
		ui.each(items, function(p: any): Node
			return swatch(p)
		end, {
			key = function(p: any): string
				return p.id
			end,
		}),
	}
end

return {
	id = "theme-library",
	title = "Theme library",
	about = "Every palette a window can wear: Zeron's built in, and VS Code themes you import (a theme's .json, comments allowed, or an extension's package.json), mapped by the shipping app's importer.",
	demo = function(): Node
		return ui.column {
			w_full = true,
			gap = "$space.6",
			ds.Text { "Built in", style = "caption", tone = "secondary" },
			grid(function(): { any }
				return palettes.LIST
			end),
			ds.Text { "Imported", style = "caption", tone = "secondary" },
			ui.show(function(): boolean
				return #palettes.imported() == 0
			end, ds.Text { "None yet: import one below.", style = "caption", tone = "tertiary" }),
			grid(function(): { any }
				return palettes.imported()
			end),
			library.Section(),
		}
	end,
}
