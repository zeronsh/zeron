-- What a drop does, over what it lands on, while files are carried over it
-- (Zeron's shell.rs `attachment_drop_overlay`): the area dimmed by the
-- scrim, "Drop to attach" on a small pill of the menus' glass in the middle
-- so it reads over whatever is under it, in either theme. There at once and
-- gone at once, as Zeron's is; it never takes the pointer (the drop goes to
-- what's under it).
--
-- `DropOverlay { shown = function() return carrying() end, id = "chat-drop" }`
-- (placed over the area, absolute, filling it).

local glass = require("@self/glass")
local locale = require("@self/locale")

export type Props = { shown: () -> boolean, id: string?, label: string? }

return function(props: Props): Node
	-- The pill: glass lying flat on the dimmed area (no drop shadow and no
	-- hairline: a label over a page, not a menu over it. The menus' neutral
	-- edge reads as a gray ring round a tinted theme's fill; Zeron's own
	-- overlay draws none).
	local pill = glass.style({
		rounded = "$radius.full",
		px = "$space.3",
		py = "$space.1",
		ui.text { props.label or locale.t("ds.drop_overlay.drop"), text_size = "$font.size.md", color = "$text.primary" },
	}, "menu")
	pill.shadow = nil
	pill.border = 0
	pill.border_color = nil
	return ui.show(
		props.shown,
		ui.div {
			id = props.id,
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			items = "center",
			justify = "center",
			pointer_events = "none",
			bg = "$zeron.drop.scrim",
			ui.row(pill :: any),
		}
	)
end
