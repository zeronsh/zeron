-- A row's inline link drawn eight ways, each numbered, to find on paint
-- which draw an underline and which turn accent under the pointer (zgx tests
-- see no paint). Each in a row with the app's hover ink, as a Settings row's
-- link sits (ds/settings InlineLink). 8 is a Markdown link, which draws its
-- underline in this build.

local ds = require("@ds")
ds.setup()
local Markdown = require("@ds/markdown")

local function case(n: number, what: string, link: Node): Node
	return ui.row {
		id = `case-{n}`,
		w_full = true,
		gap = "$space.4",
		items = "center",
		ui.text { `{n}`, w = 16, text_size = "$font.size.sm", color = "$text.tertiary", tabular = true },
		ui.div { w = 120, link },
		ui.text { what, text_size = "$font.size.xs", line_height = "$font.line.xs", color = "$text.tertiary", wrap = true, grow = 1, min_w = 0 },
	}
end

-- The row round each, as InlineLink's: the text's ink, the accent under
-- the pointer.
local function row(child: Node, hover: boolean?): Node
	return ui.row {
		role = "link",
		label = "Publish…",
		focusable = true,
		shrink = 0,
		cursor = "pointer",
		color = "$text.primary",
		hover = if hover == false then nil else { color = "$accent" },
		on_click = function() end,
		child,
	}
end

local function rich(span: { [any]: any }, color: string?): Node
	return ui.rich {
		text_size = "$font.size.sm",
		line_height = "$font.line.sm",
		weight = "$font.weight.medium",
		color = color,
		ui.span(span),
	}
end

return ui.column {
	id = "inline-link-cases",
	w_full = true,
	p = "$space.6",
	gap = "$space.5",
	bg = "$surface.base",
	case(1, "rich, its ink set primary, span underline offset 4, span hover accent (c77af2ddd)", row(rich({ "Publish…", underline = true, underline_offset = 4, hover = { underline = true, color = "$accent" } }, "$text.primary"), false)),
	case(2, "rich, ink from the row (hover accent), span underline offset 4", row(rich({ "Publish…", underline = true, underline_offset = 4 }))),
	case(3, "rich, ink from the row (hover accent), span underline offset 2", row(rich({ "Publish…", underline = true, underline_offset = 2 }))),
	case(4, "rich, ink from the row (hover accent), span underline, no offset", row(rich({ "Publish…", underline = true }))),
	case(5, "plain text, underline offset 2 (row hover accent)", row(ui.text { "Publish…", text_size = "$font.size.sm", line_height = "$font.line.sm", weight = "$font.weight.medium", underline = true, underline_offset = 2 })),
	case(6, "plain text, underline offset 4, a 24 line (row hover accent)", row(ui.text { "Publish…", text_size = "$font.size.sm", line_height = 24, weight = "$font.weight.medium", underline = true, underline_offset = 4 })),
	case(7, "rich, span its own link (on_click), underline offset 4, span hover accent", rich({ "Publish…", underline = true, underline_offset = 4, on_click = function() end, hover = { underline = true, color = "$accent" } }, "$text.primary")),
	case(8, "Markdown link, small (the reference)", ui.div { w = 120, Markdown { text = "[Publish…](https://example.com)", small = true, foreign = true } }),
}
