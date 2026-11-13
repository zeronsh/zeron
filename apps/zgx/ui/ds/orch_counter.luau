-- A live number: its digits roll as it changes (the changed ones rise out
-- as the new rise in, softened while they move; ds/roll_text, the roll
-- Zeron's chips use), in tabular figures. It keeps its width: the digits
-- roll inside a box as wide as its largest value (`max`: 100 for a
-- percent, else 99), clipped to it, so a roll from 4 to 33 to 100 moves
-- nothing beside it; its suffix ("%") stands after that box, still.
-- Reduced motion: the new number at once.
--
--   Counter { value = function() return cpu * 100 end, suffix = "%" }
--   Counter { value = fn, format = function(n) return string.format("%.1f", n) end,
--             max = 60, text_size = "$font.size.sm", weight = "$font.weight.medium",
--             color = "$text.primary", anchor = "end" }
--
-- `value` nil (or a function giving nil): `empty` ("–") shows. `anchor`:
-- which side of its box the digits hold to ("end", a figure at a row's
-- end, by default; "start" a count after a title).

local RollText = require("@self/roll_text")

export type Props = {
	value: number | () -> number?,
	format: ((n: number) -> string)?,
	suffix: string?,
	empty: string?,
	-- The largest value it is laid out for (wider ones widen it).
	max: number?,
	text_size: string?,
	weight: string?,
	color: string?,
	anchor: ("start" | "end")?,
	label: string?,
	shrink: number?,
	id: string?,
}

local function whole(n: number): string
	return tostring(math.floor(n + 0.5))
end

return ui.component(function(props: Props): Node
	local fmt = props.format or whole
	local suffix = props.suffix or ""
	local empty = props.empty or "–"
	local anchor = props.anchor or "end"
	local max = props.max or (if suffix == "%" then 100 else 99)
	local text_size = props.text_size or "$font.size.sm"
	local color = props.color or "$text.primary"
	local function digits(): string
		local given: any = props.value
		local v: number? = if type(given) == "function" then (given :: any)() else given
		if v == nil then
			return empty
		end
		return fmt(v)
	end
	-- What its box is as wide as: its largest value, or one past it.
	local function widest(): string
		local now = digits()
		local most = fmt(max)
		return if #now > #most then now else most
	end
	local function face(text: any, extra: { [string]: any }?): Node
		local p: { [any]: any } = {
			text,
			tabular = true,
			text_size = text_size,
			weight = props.weight,
			color = color,
		}
		for k, v in extra or {} do
			p[k] = v
		end
		return ui.text(p :: any)
	end
	return ui.row {
		id = props.id,
		class = "orch-counter",
		shrink = props.shrink or 0,
		items = "center",
		label = props.label,
		ui.overlay {
			class = "orch-counter-digits",
			shrink = 0,
			overflow = "hidden",
			-- (Unseen, it sizes the box.)
			face(widest, { opacity = 0, selectable = false }),
			ui.row {
				w_full = true,
				justify = if anchor == "start" then "start" else "end",
				RollText {
					digits,
					tabular = true,
					glide = false,
					anchor = anchor,
					shrink = 0,
					text_size = text_size,
					weight = props.weight,
					color = color,
				},
			},
		},
		if suffix ~= "" then face(suffix, { shrink = 0 }) else nil,
	}
end)
