-- A live number: its digits roll as it changes (the changed ones rise out
-- as the new rise in, softened while they move; ds/roll_text, the roll
-- Zeron's chips use), in tabular figures so the rest stay put. Reduced
-- motion: the new number at once.
--
--   Counter { value = function() return cpu * 100 end, suffix = "%" }
--   Counter { value = fn, format = function(n) return string.format("%.1f", n) end,
--             text_size = "$font.size.sm", weight = "$font.weight.medium",
--             color = "$text.primary", anchor = "end" }
--
-- `value` nil (or a function giving nil): `empty` ("–") shows.

local RollText = require("@self/roll_text")

export type Props = {
	value: number | () -> number?,
	format: ((n: number) -> string)?,
	suffix: string?,
	empty: string?,
	text_size: string?,
	weight: string?,
	color: string?,
	-- Held by its right edge (a figure at a row's end).
	anchor: ("start" | "end")?,
	label: string?,
	shrink: number?,
}

local function whole(n: number): string
	return tostring(math.floor(n + 0.5))
end

return ui.component(function(props: Props): Node
	local fmt = props.format or whole
	local suffix = props.suffix or ""
	local empty = props.empty or "–"
	local function shown(): string
		local given: any = props.value
		local v: number? = if type(given) == "function" then (given :: any)() else given
		if v == nil then
			return empty
		end
		return fmt(v) .. suffix
	end
	-- Held still for now: the roll painted digits outside the counter and
	-- left ghosts of the rows around it (live, 2026-11-12). It rolls again
	-- once it is clipped and paint-only.
	local _ = RollText
	return ui.text {
		shown,
		class = "orch-counter",
		tabular = true,
		shrink = props.shrink or 0,
		text_size = props.text_size or "$font.size.sm",
		weight = props.weight,
		color = props.color or "$text.primary",
	}
end)
