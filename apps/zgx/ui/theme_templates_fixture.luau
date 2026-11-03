-- Zeron's built-in themes worn one at a time, as a theme template's
-- theme.luau has the window wear them (ds/palettes.wear), for
-- src/plugins/theme_templates.rs's tests. The host sets what to wear
-- (`theme_check.family`: a template's palettes; `theme_check.kind`: the
-- scheme showing), runs `wear` (or `zeron`, Zeron's own again), then `read`,
-- and reads `theme_check.report`: the theme worn, the names each scheme
-- wears, every token the mapping set as the window resolves it, what the
-- mapping wrote for each, the inks it moved to stay legible, and the
-- resolved value of each of `theme_check.keys`.

local ds = require("@ds")
ds.setup()
local palettes = require("@ds/palettes")

local family = ui.shared("theme_check.family", nil :: any)
local kind = ui.shared("theme_check.kind", "dark")
local keys = ui.shared("theme_check.keys", {} :: { string })
local report = ui.shared("theme_check.report", nil :: any)
local names = ui.state("theme_check_names", { dark = "zeron-dark", light = "zeron-light" })

local function show(worn: { dark: string, light: string })
	names:set({ dark = worn.dark, light = worn.light })
	ui.theme.follow_system(false)
	ui.theme.current:set(if kind:peek() == "light" then worn.light else worn.dark)
end

ui.command {
	id = "wear",
	title = "Wear the given palettes",
	run = function()
		show(palettes.wear(family:peek()))
	end,
}

ui.command {
	id = "zeron",
	title = "Wear Zeron's own",
	run = function()
		show(palettes.wear(nil))
	end,
}

ui.command {
	id = "read",
	title = "Read the tokens worn",
	run = function()
		local f = family:peek()
		local p = f and (f[kind:peek()] or f.dark or f.light)
		local resolved: { [string]: any } = {}
		local written: { [string]: any } = {}
		local fixes = {}
		if p and ui.theme.current:peek() ~= "zeron-dark" and ui.theme.current:peek() ~= "zeron-light" then
			local own = table.clone(p)
			own.kind = if f[kind:peek()] then kind:peek() else (if f.dark then "dark" else "light")
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
