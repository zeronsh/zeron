-- zgx ask (ASK.md): an inline chip span can't be drawn as one pill.
-- Three lines; compare each with the line's comment.
local dot = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="#f97316"/></svg>'

local function line(span: { [any]: any }): Node
	return ui.rich { text_size = 18, color = "#e8e8e8", "Look at ", ui.span(span :: any), " please" }
end

return ui.column {
	id = "repro",
	p = 40,
	gap = 24,
	bg = "#2a2a2a",
	w_full = true,
	h_full = true,
	-- 1: underlined (under its padding too), though `underline = false`.
	line { "chat.rs", underline = false, bg = "#ffffff22", radius = 5, padding = { 4, 2 }, on_click = function() end },
	-- 2: three pieces of wash: the icon's box, the text's, a sliver for the padding.
	line { "chat.rs", bg = "#ffffff22", radius = 5, padding = { 4, 2 }, icon = { code = dot, size = 14, gap = 4 } },
	-- 3: both.
	line {
		"chat.rs",
		bg = "#ffffff22",
		radius = 5,
		padding = { 4, 2 },
		underline = false,
		on_click = function() end,
		icon = { code = dot, size = 14, gap = 4 },
	},
}
