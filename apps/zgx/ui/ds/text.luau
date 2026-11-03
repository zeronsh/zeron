-- Type: every piece of text in a screen is one of these styles, in one of
-- these tones. `Text { "Hello", style = "title", tone = "secondary" }`.

local recipe = ui.variants {
	base = { font = "$font.sans" },
	variants = {
		style = {
			display = {
				text_size = "$zeron.display.size",
				line_height = "$zeron.display.line",
				weight = "$font.weight.semibold",
				letter_spacing = -1,
			},
			title = { text_size = "$font.size.xxl", line_height = "$font.line.xxl", weight = "$font.weight.semibold" },
			heading = { text_size = "$font.size.lg", line_height = "$font.line.lg", weight = "$font.weight.medium" },
			body = { text_size = "$font.size.md", line_height = "$font.line.md" },
			caption = { text_size = "$font.size.sm", line_height = "$font.line.sm" },
			label = { text_size = "$font.size.xs", line_height = "$font.line.xs", weight = "$font.weight.medium" },
			mono = { font = "$font.mono", text_size = "$font.size.sm", line_height = "$font.line.sm" },
		},
		tone = {
			primary = { color = "$text.primary" },
			secondary = { color = "$text.secondary" },
			tertiary = { color = "$text.tertiary" },
			accent = { color = "$accent" },
			danger = { color = "$danger" },
		},
	},
	defaults = { style = "body", tone = "primary" },
}

export type Style = "display" | "title" | "heading" | "body" | "caption" | "label" | "mono"
export type Tone = "primary" | "secondary" | "tertiary" | "accent" | "danger"
export type Props = Merge<TextProps, { style: Style?, tone: Tone? }>

return function(props: Props): Node
	local p = table.clone(props) :: any
	p.variants = recipe { style = props.style, tone = props.tone }
	p.style, p.tone = nil, nil
	return ui.text(p)
end
