-- A skeleton: where something is still on its way, its shape held in a
-- quiet wash that breathes, so a page keeps its layout as it loads and
-- nothing jumps when the real thing arrives.
--
-- `Skeleton { w = 120 }` a line of text; `h`, `rounded` for anything else;
-- `Skeleton.Row {}` a list row's shape, its lead and two lines.

local M = {}

export type Props = { w: any?, h: number?, w_full: boolean?, grow: number?, rounded: string? }

function M.Bar(props: Props): Node
	return ui.div {
		w = props.w,
		w_full = props.w_full,
		grow = props.grow,
		h = props.h or 10,
		shrink = 0,
		rounded = props.rounded or "$radius.sm",
		bg = "$zeron.secondary.hover",
		aria_hidden = true,
		opacity = ui.loop { from = 0.55, to = 1, duration = 900, yoyo = true, ease = "inOut(2)", fps = 30 },
	}
end

-- A row of a settings group or a list: `lead` (points) for its tile, the
-- lines' widths (`line = 0` for a row with one line), and something at its
-- end. Each bar stands in its line's own height (a Settings.Row's name on
-- its line, its description on the next), so the row is as tall as the one
-- that replaces it and nothing moves when it does.
function M.Row(props: { lead: number?, title: number?, line: number?, trail: number? }): Node
	local lead = props.lead or 32
	return ui.row {
		w_full = true,
		min_h = "$zeron.settings.row",
		px = "$space.4",
		py = "$space.3",
		gap = "$space.3",
		items = "center",
		M.Bar { w = lead, h = lead, rounded = "$radius.md" },
		ui.column {
			grow = 1,
			min_w = 0,
			gap = "$zeron.space.hair",
			ui.div { h = "$font.line.md", justify = "center", M.Bar { w = props.title or 140 } },
			if props.line ~= 0 then ui.div { h = "$font.line.sm", justify = "center", M.Bar { w = props.line or 240, h = 8 } } else nil,
		},
		if props.trail then M.Bar { w = props.trail, h = 24, rounded = "$radius.lg" } else nil,
	}
end

return M
