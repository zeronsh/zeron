-- A section with its head, a long run of text and a button at its foot, in
-- a scroll; the view scrolled to the button. `reveal` asks for the
-- section's head to come into view: today the only way is focus, which
-- shows the focused button, never the section round it.

local section = ui.ref()
local lines = {}
for i = 1, 40 do
	table.insert(lines, ui.text { `line {i}`, h = 20 })
end

ui.command {
	id = "reveal",
	title = "Bring the section into view",
	run = function()
		local el = section:peek()
		if el then
			el:blur()
			el:focus()
		end
	end,
}

return ui.scroll {
	id = "view",
	w_full = true,
	h = 300,
	ui.div { h = 600 },
	ui.column {
		ref = section,
		id = "section",
		ui.text { id = "head", "The section's head", h = 20 },
		ui.column(lines),
		ui.button { id = "foot", label = "Show less", ui.text { "Show less" } },
	},
	ui.div { h = 600 },
}
