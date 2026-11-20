-- Zeron's sidebar glyph (icons.zeron.sh "Sidebar", as the shipping app draws
-- it in crates/ui/src/icons.rs): a rounded frame holding a panel that
-- narrows as the sidebar closes. `SidebarIcon { open = state, size = 16 }`.
-- `side = "right"`: the same glyph mirrored, its panel at the right, for a
-- panel on the window's right (a session's panel), narrowing toward its edge.
-- `ink`: its colour, the secondary text's by default (a title bar's controls
-- give theirs, `$zeron.bar.ink`, solid so their wash never shows through).

export type Props = { open: State<boolean>, size: number?, side: ("left" | "right")?, ink: string? }

-- The source's 24-unit box: frame `x=3 y=4 w=18 h=16 rx=4`, stroke 1.75;
-- panel `x=6.5 y=7.5 h=9 rx=.875`, 5.5 wide open, 1.75 closed. The frame's
-- line, and the panel's width closed, are the theme's glyph stroke (heavier
-- in light, so it reads the same weight); the frame's grows inward from the
-- source's outer edge.
return function(props: Props): Node
	local size = props.size or 16
	local s = size / 24
	local stroke = 1.75
	local right = props.side == "right"
	local ink = props.ink or "$text.secondary"
	local function raw(v: number): any
		return ui.raw(v, "the glyph's own geometry, scaled from its 24-unit source")
	end
	return ui.div {
		w = size,
		h = size,
		shrink = 0,
		ui.div {
			absolute = true,
			left = (3 - stroke / 2) * s,
			top = (4 - stroke / 2) * s,
			w = (18 + stroke) * s,
			h = (16 + stroke) * s,
			rounded = raw((4 + stroke / 2) * s),
			border = "$zeron.glyph.stroke",
			border_color = ink,
		},
		-- The panel: as wide as the frame's line closed (so it weighs the
		-- same in either theme), opening out by the rest of its width.
		ui.row {
			absolute = true,
			-- (Mirrored: as far in from the right, held there as it narrows.)
			left = if right then nil else 6.5 * s,
			right = if right then 6.5 * s else nil,
			top = 7.5 * s,
			h = 9 * s,
			rounded = raw(0.875 * s),
			bg = ink,
			ui.div { w = "$zeron.glyph.stroke", shrink = 0 },
			ui.div { w = ui.tween(props.open, { from = 0, to = (5.5 - 1.75) * s, duration = 200 }), shrink = 0 },
		},
	}
end
