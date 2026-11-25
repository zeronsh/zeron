--!strict
-- A folding section of an orchestrator's side panel: its head (a chevron,
-- an icon if it has one, its title and count, something quiet at the end),
-- pressed to fold or open what's under it. Where it's open is its
-- caller's to keep (screens/orchestrator/panel_state).
--
--   Fold {
--     id = "orch-fold-needs",
--     title = ui.t("orch.panel.needs_you"),
--     count = function() return 9 end,       -- shown while above 0
--     icon = Node?,                          -- before the title
--     aside = Node?,                         -- at the head's end
--     open = function() return true end,
--     on_toggle = function(open) end,
--     body,
--   }

local Icon = require("@self/icon")

export type Props = {
	id: string?,
	title: any,
	count: (() -> number)?,
	icon: Node?,
	aside: Node?,
	open: () -> boolean,
	on_toggle: (open: boolean) -> (),
	[number]: Node?,
}

return function(props: Props): Node
	local head = ui.button {
		class = "orch-fold-head",
		w_full = true,
		h = 32,
		px = "$space.2",
		gap = "$space.2",
		items = "center",
		rounded = "$radius.md",
		bg = "#00000000",
		hover = { bg = "$hover" },
		active = { bg = "$pressed" },
		expanded = props.open,
		label = props.title,
		on_click = function()
			props.on_toggle(not props.open())
		end,
		ui.div {
			color = "$text.tertiary",
			shrink = 0,
			Icon {
				name = function(): Icon.Name
					return if props.open() then "alt-arrow-down" else "alt-arrow-right"
				end,
				size = 14,
			},
		},
		props.icon,
		ui.text {
			props.title,
			class = "orch-fold-title",
			shrink = 0,
			text_size = "$font.size.sm",
			weight = "$font.weight.semibold",
			color = "$text.primary",
		},
		if props.count
			then ui.show(function(): boolean
				return (props.count :: () -> number)() > 0
			end, ui.text {
				function(): string
					return tostring((props.count :: () -> number)())
				end,
				class = "orch-fold-count",
				shrink = 0,
				tabular = true,
				text_size = "$font.size.sm",
				color = "$text.tertiary",
			})
			else nil,
		ui.div { grow = 1, min_w = 0 },
		if props.aside then ui.div { shrink = 1, min_w = 0, props.aside } else nil,
	}
	return ui.column {
		id = props.id,
		class = "orch-fold",
		w_full = true,
		gap = "$space.1",
		head,
		ui.show(props.open, ui.column {
			class = "orch-fold-body",
			w_full = true,
			props[1],
		}),
	}
end
