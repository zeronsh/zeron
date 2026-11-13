-- Themes worn one at a time, as the window wears them (ds/palettes.wear),
-- for src/themes.rs's tests. The host sets what to wear
-- (`theme_check.slots`: `{ light?, dark? }`, a palette for each scheme
-- chosen; `theme_check.kind`: the scheme showing), runs `wear`, then
-- `read`, and reads `theme_check.report`: the theme worn, the names each
-- scheme wears, every token the mapping set as the window resolves it, what
-- the mapping wrote for each, the inks it moved to stay legible, and the
-- resolved value of each of `theme_check.keys`.

local ds = require("@ds")
ds.setup()
local palettes = require("@ds/palettes")

local slots = ui.shared("theme_check.slots", nil :: any)
local kind = ui.shared("theme_check.kind", "dark")
local keys = ui.shared("theme_check.keys", {} :: { string })
local report = ui.shared("theme_check.report", nil :: any)
local names = ui.state("theme_check_names", { dark = palettes.scheme.dark, light = palettes.scheme.light })

ui.command {
	id = "wear",
	title = "Wear the given palettes",
	run = function()
		local worn = palettes.wear(slots:peek())
		names:set({ dark = worn.dark, light = worn.light })
		ui.theme.follow_system(false)
		ui.theme.current:set(if kind:peek() == "light" then worn.light else worn.dark)
	end,
}

ui.command {
	id = "read",
	title = "Read the tokens worn",
	run = function()
		local s = slots:peek()
		local p = s and s[kind:peek()]
		local resolved: { [string]: any } = {}
		local written: { [string]: any } = {}
		local fixes = {}
		if p then
			local own = table.clone(p)
			own.kind = kind:peek()
			local t, moved = palettes.tokens(own)
			for k, v in t or {} do
				written[k] = v
				resolved[k] = ui.theme.get(k)
			end
			fixes = moved
		end
		local asked: { [string]: any } = {}
		for _, k in keys:peek() do
			asked[k] = ui.theme.get(k)
		end
		report:set({
			current = ui.theme.current:peek(),
			names = names:peek(),
			resolved = resolved,
			written = written,
			fixes = fixes,
			asked = asked,
		})
	end,
}

return ui.div {
	w_full = true,
	h_full = true,
	bg = "$background",
	ui.text { "Themes", color = "$text.primary" },
}
