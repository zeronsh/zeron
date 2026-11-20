-- A Settings row renamed where it stands (Devices', Projects'): its name
-- turns into a field over the row, the field's words exactly where the
-- name's were, Save and Cancel where the row's control was. Enter saves,
-- Escape cancels; nothing moves, the row's height and its neighbours stay
-- put. One row of a page renames at a time (`renaming`, the page's).
--
--   rename.lead(icon)      a row's 28-point glyph box, as Settings.Row draws
--                          its `icon` (the field's copy of it stands on it)
--   rename.Row { row, lead, editing, value, name, can_save, on_save, on_cancel }
--     row        the Settings.Row as it is when not renaming
--     lead       builds the row's lead again (the field's own copy)
--     editing    whether this row is renaming now
--     value      the name being typed (a state)
--     name       the field's accessible name ("Name for devbox")
--     can_save   whether the typed name would do
--     on_save, on_cancel

local ds = require("@ds")

local M = {}

-- A row's glyph in its box (ds/settings `Row`'s for `icon`).
function M.lead(icon: any): Node
	return ui.row {
		w = 28,
		h = 28,
		shrink = 0,
		items = "center",
		justify = "center",
		rounded = "$radius.lg",
		bg = "$zeron.secondary.bg",
		color = "$text.secondary",
		ds.Icon { name = icon, size = 16 },
	}
end

export type Props = {
	row: Node,
	lead: () -> Node,
	editing: () -> boolean,
	value: State<string>,
	name: Message | string,
	can_save: () -> boolean,
	on_save: () -> (),
	on_cancel: () -> (),
}

function M.Row(props: Props): Node
	local function save()
		if props.can_save() then
			props.on_save()
		end
	end
	return ui.div {
		w_full = true,
		relative = true,
		-- The row as ever, kept for its size while the field stands over it.
		ui.div {
			w_full = true,
			opacity = function(): number
				return if props.editing() then 0 else 1
			end,
			pointer_events = function(): any
				return if props.editing() then "none" else "auto"
			end,
			-- (Nor Tab's way past the field: its ⋯ waits under it.)
			disabled = props.editing,
			props.row,
		},
		ui.show(
			props.editing,
			ui.row {
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				px = "$space.4",
				-- (The field's words on the name's: the row's 12 between its
				-- lead and its name, less the field's 8 of padding and 1 edge.)
				gap = ui.raw(3, "the field's text on the row's name"),
				items = "center",
				props.lead(),
				ds.TextField {
					value = props.value,
					name = props.name,
					size = "sm",
					grow = 1,
					autofocus = true,
					select_on_focus = true,
					on_commit = function()
						save()
					end,
					on_key = function(e: any): boolean?
						if e.key == "Escape" then
							props.on_cancel()
							return true
						end
						return nil
					end,
				},
				ui.row {
					shrink = 0,
					pl = "$space.3",
					gap = "$space.1",
					items = "center",
					ds.Button {
						ui.t("common.save"),
						size = "sm",
						disabled = function(): boolean
							return not props.can_save()
						end,
						on_click = save,
					},
					ds.Button { ui.t("common.cancel"), tone = "ghost", size = "sm", on_click = props.on_cancel },
				},
			}
		),
	}
end

return M
