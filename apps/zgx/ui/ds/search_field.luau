-- A search field: a text field with the magnifying glass before it, a
-- clear button once something is typed, and Escape clearing it. `shortcut`
-- shows the keys that reach it while it is empty (⌘F), and `count` how
-- many results the search has (a list's filter: "12").
--
-- `SearchField { value = query, placeholder = "Search branches", shortcut = "cmd+f" }`.
-- `shape = "pill"` rounds it fully (a toolbar's search).

local Kbd = require("@self/kbd")
local TextField = require("@self/text_field")
local control = require("@self/control")
local locale = require("@self/locale")

export type Props = {
	value: State<string>,
	placeholder: (string | Message)?,
	shortcut: string?,
	-- How many results there are, shown at its end while something is
	-- typed.
	results: (() -> number)?,
	size: control.Size?,
	shape: ("rounded" | "pill")?,
	-- No frame: a row of what holds it (a list's filter in its card).
	quiet: boolean?,
	-- Room before the glyph (TextField's `pl`).
	pl: number?,
	name: (string | Message)?,
	autofocus: boolean?,
	on_key: ((e: any) -> boolean?)?,
	on_commit: ((text: string) -> ())?,
	w: number?,
	w_full: boolean?,
	grow: number?,
}

return function(props: Props): Node
	local value = props.value
	local size = props.size or "md"
	local trailing: Node? = nil
	if props.shortcut or props.results then
		trailing = ui.row {
			gap = "$space.1",
			items = "center",
			pr = "$space.1",
			if props.results
				then ui.show(function()
					return value:get() ~= ""
				end, ui.text {
					function()
						local n = (props.results :: () -> number)()
						return locale.s("ds.search_field.results", { n = n, count = tostring(n) })
					end,
					tabular = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				})
				else nil,
			if props.shortcut
				then ui.show(function()
					return value:get() == ""
				end, Kbd { keys = props.shortcut, tone = "quiet" })
				else nil,
		}
	end
	return TextField {
		value = value,
		placeholder = props.placeholder or locale.t("common.search"),
		name = props.name or locale.t("common.search"),
		icon = "search",
		clearable = true,
		escape_clears = true,
		inputmode = "search",
		trailing = trailing,
		size = size,
		rounded = if props.shape == "pill" then "$radius.full" else nil,
		quiet = props.quiet,
		pl = props.pl,
		autofocus = props.autofocus,
		on_key = props.on_key,
		on_commit = props.on_commit,
		w = props.w,
		w_full = props.w_full,
		grow = props.grow,
	}
end
