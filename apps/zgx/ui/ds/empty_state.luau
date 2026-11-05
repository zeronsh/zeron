-- An empty state: what a list or page shows with nothing in it, saying
-- what would be there and how to get some. A glyph on a quiet tile, a
-- title, a line under it, and an action or two, centred in the room it
-- has. `size = "sm"` for a panel or a list's room (a smaller tile, no
-- second action's worth of space).
--
-- `EmptyState { icon = "inbox", title = "No pull requests", body = "…",
--   action = { label = "New pull request", on_click = … } }`. `title` and
-- `body` may be functions (words that follow a search as it is typed).

local Button = require("@self/button")
local Icon = require("@self/icon")

-- (Its words may be messages, `ui.t`, each drawn as it is.)
export type Action = { label: string | Message | () -> string, on_click: (() -> ())?, icon: Icon.Name? }
export type Props = {
	icon: Icon.Name?,
	-- Anything in the tile's place (a mark, an illustration).
	art: Node?,
	title: string | Message | () -> string,
	body: (string | Message | () -> string)?,
	action: Action?,
	secondary: Action?,
	size: ("sm" | "md")?,
	-- Grow into the room it is in (a page's).
	fill: boolean?,
}

return function(props: Props): Node
	local sm = props.size == "sm"
	local tile = props.art
	if not tile and props.icon then
		local box = if sm then 36 else 44
		tile = ui.row {
			w = box,
			h = box,
			shrink = 0,
			-- (Decoration: the title says what the page is.)
			aria_hidden = true,
			items = "center",
			justify = "center",
			rounded = "$radius.xl",
			bg = "$zeron.secondary.bg",
			border = 1,
			border_color = "$border.subtle",
			color = "$text.secondary",
			Icon { name = props.icon, size = if sm then 18 else 20 },
		}
	end
	local actions: { Node } = {}
	if props.action then
		local a = props.action
		table.insert(actions, Button {
			tone = "secondary",
			size = if sm then "sm" else "md",
			on_click = a.on_click,
			if a.icon then ui.div { y = -0.5, Icon { name = a.icon, size = if sm then 14 else 16 } } else nil,
			a.label,
		})
	end
	if props.secondary then
		local a = props.secondary
		table.insert(actions, Button { a.label, tone = "ghost", size = if sm then "sm" else "md", on_click = a.on_click })
	end
	return ui.column {
		items = "center",
		justify = "center",
		gap = if sm then "$space.3" else "$space.4",
		w_full = true,
		grow = if props.fill then 1 else nil,
		py = if sm then "$space.5" else "$space.7",
		tile,
		ui.column {
			items = "center",
			gap = "$space.1",
			max_w = 340,
			ui.text {
				props.title,
				align = "center",
				text_size = if sm then "$font.size.md" else "$font.size.lg",
				line_height = if sm then "$font.line.md" else "$font.line.lg",
				weight = "$font.weight.medium",
				color = "$text.primary",
				wrap = true,
			},
			if props.body
				then ui.text {
					props.body,
					align = "center",
					text_size = if sm then "$font.size.sm" else "$font.size.md",
					line_height = if sm then "$font.line.sm" else 20,
					color = "$text.secondary",
					wrap = true,
				}
				else nil,
		},
		if #actions > 0 then ui.row { gap = "$space.2", items = "center", pt = "$space.1", table.unpack(actions) } else nil,
	}
end
