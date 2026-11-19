-- A skeleton: where something is still on its way, its shape held in a
-- quiet wash that breathes, so a page keeps its layout as it loads and
-- nothing jumps when the real thing arrives.
--
-- `Skeleton { w = 120 }` a line of text; `h`, `rounded` for anything else;
-- `Skeleton.Row {}` a list row's shape, its lead and two lines.

local M = {}

-- `still`: held at rest, not breathing (one that has waited long enough:
-- nothing redraws for it).
-- `fill`: the whole of its box, square (what holds it gives it its shape
-- and its corner: a card's cover).
export type Props = { w: any?, h: number?, w_full: boolean?, grow: number?, rounded: string?, still: boolean?, fill: boolean? }

function M.Bar(props: Props): Node
	return ui.div {
		w = props.w,
		w_full = props.w_full or props.fill,
		h_full = props.fill,
		grow = props.grow,
		h = if props.fill then nil else (props.h or 10),
		shrink = 0,
		rounded = if props.fill then nil else (props.rounded or "$radius.sm"),
		bg = "$zeron.secondary.hover",
		aria_hidden = true,
		opacity = if props.still then 0.55 else ui.loop { from = 0.55, to = 1, duration = 900, yoyo = true, ease = "inOut(2)", fps = 30 },
	}
end

-- A row of a settings group or a list: `lead` (points) for its tile, the
-- lines' widths (`line = 0` for a row with one line), and something at its
-- end. Each bar stands in its line's own height (a Settings.Row's name on
-- its line, its description on the next), so the row is as tall as the one
-- that replaces it and nothing moves when it does.
-- `byline`: a line of its own between the name and the line under it (who
-- made it, a Row's `byline`), its width.
function M.Row(props: { lead: number?, title: number?, byline: number?, line: number?, trail: number?, still: boolean? }): Node
	local lead = props.lead or 32
	return ui.row {
		w_full = true,
		px = "$space.4",
		py = "$space.3",
		gap = "$space.3",
		items = "center",
		M.Bar { w = lead, h = lead, rounded = "$radius.md", still = props.still },
		ui.column {
			grow = 1,
			min_w = 0,
			gap = "$zeron.space.hair",
			ui.div { h = "$font.line.md", justify = "center", M.Bar { w = props.title or 140, still = props.still } },
			if props.byline then ui.div { h = "$font.line.sm", justify = "center", M.Bar { w = props.byline, h = 8, still = props.still } } else nil,
			if props.line ~= 0 then ui.div { h = "$font.line.sm", justify = "center", M.Bar { w = props.line or 240, h = 8, still = props.still } } else nil,
		},
		if props.trail then M.Bar { w = props.trail, h = 24, rounded = "$radius.lg", still = props.still } else nil,
	}
end

-- How long a skeleton breathes while what it stands for is on its way:
-- past it, it rests, so a load that hangs (slow, or the network gone)
-- leaves nothing redrawing on every frame.
M.WAIT_MS = 15000

-- A skeleton that waits: `build(still)` makes it, breathing at first, and
-- again at rest once it has waited `WAIT_MS` (a timer of its own, gone with
-- it, so what replaces it in time never sees it rest).
M.Waiting = ui.component(function(props: { build: (still: boolean) -> Node }): Node
	local still = ui.use_state(false)
	ui.on_mount(function()
		ui.after(M.WAIT_MS, function()
			still:set(true)
		end)
	end)
	return ui.presence(function(): string
		return if still:get() then "still" else "breathing"
	end, function(key: string): Node
		return props.build(key == "still")
	end)
end)

return M
