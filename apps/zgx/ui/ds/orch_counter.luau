-- A live number: its digits roll as it changes, each in a cell of its own
-- (a tabular figure's width, its line's height) that clips what it holds:
-- a changed digit's old figure rises out of its cell as the new one rises
-- in, a move of the figures alone (paint, never layout), so nothing past
-- the cell is ever drawn and nothing beside it moves. The cells are as many
-- as its largest value needs (`max`: 100 for a percent, else 99), so a roll
-- from 4 to 33 to 100 keeps its width; its suffix ("%") stands after them,
-- still. Reduced motion: the new number at once.
--
--   Counter { value = function() return cpu * 100 end, suffix = "%" }
--   Counter { value = fn, format = function(n) return string.format("%.1f", n) end,
--             max = 60, text_size = "$font.size.sm", weight = "$font.weight.medium",
--             color = "$text.primary", anchor = "end" }
--
-- `value` nil (or a function giving nil): `empty` ("–") shows. `anchor`:
-- which side the figures hold to ("end", a figure at a row's end, by
-- default; "start" a count after a title).

local motion = require("@self/motion")

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

-- The roll: Zeron's chips' length and ease (ds/roll_text), every changed
-- cell at once (a staggered one would show a number that never was).
local DURATION, EASE = 420, "outQuint"

local function whole(n: number): string
	return tostring(math.floor(n + 0.5))
end

local function chars(s: string): { string }
	local out: { string } = {}
	for _, code in utf8.codes(s) do
		table.insert(out, utf8.char(code))
	end
	return out
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
	local function face(text: any, extra: { [string]: any }?): Node
		local p: { [any]: any } = {
			text,
			tabular = true,
			text_size = text_size,
			weight = props.weight,
			color = color,
			selectable = false,
		}
		for k, v in extra or {} do
			p[k] = v
		end
		return ui.text(p :: any)
	end

	-- The figures, a cell each: as many as the largest value has (or the
	-- one showing, past it), padded with blanks on the side away from the
	-- anchor; each keyed by its place from the anchor, so a cell stays the
	-- same cell as the number grows.
	local slots = ui.computed(function(): { { key: string, at: number } }
		local n = math.max(#chars(fmt(max)), #chars(digits()))
		local out = {}
		for i = 1, n do
			table.insert(out, { key = tostring(i), at = i })
		end
		return out
	end)
	local function char_at(at: number): string
		local cs = chars(digits())
		local n = #slots:peek()
		local pad = n - #cs
		if anchor == "end" then
			-- (Places counted from the left over the padded string.)
			local i = at - pad
			return if i >= 1 then cs[i] else ""
		end
		return cs[at] or ""
	end

	-- A cell: clipped to a figure's box; inside it a column of the old
	-- figure over the new, moved up by one line as it rolls.
	local function cell(slot: { key: string, at: number }): Node
		local h = ui.use_state(0)
		local now = char_at(slot.at)
		local pair = ui.use_state({ from = now, to = now })
		local column = ui.ref()
		ui.effect(function()
			local c = char_at(slot.at)
			-- (Read so a change in the cells' count places it again.)
			slots:get()
			local was = pair:peek().to
			if c == was then
				return
			end
			local line = h:peek()
			if motion.reduced_now() or line <= 0 then
				pair:set({ from = c, to = c })
				return
			end
			pair:set({ from = was, to = c })
			local el = column:peek()
			if el then
				anime.animate(el, {
					y = { 0, -line },
					duration = DURATION,
					ease = EASE,
				})
			end
		end)
		return ui.overlay {
			class = "orch-counter-cell",
			shrink = 0,
			relative = true,
			overflow = "hidden",
			-- (Unseen, a figure's box: the cell's size whatever it holds.)
			face("0", {
				opacity = 0,
				on_resize = function(sz: { w: number, h: number })
					if math.abs(sz.h - h:peek()) >= 0.5 then
						h:set(sz.h)
					end
				end,
			}),
			ui.column {
				ref = column,
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				items = "center",
				-- At rest the new figure shows: the column one line up.
				y = function(): number
					return -h:get()
				end,
				face(function(): string
					return pair:get().from
				end),
				face(function(): string
					return pair:get().to
				end),
			},
		}
	end

	return ui.row {
		id = props.id,
		class = "orch-counter",
		shrink = props.shrink or 0,
		items = "center",
		label = props.label or function(): string
			return digits() .. suffix
		end,
		ui.each(slots, function(slot: any): Node
			return cell(slot)
		end, { key = function(s: any): string
			return s.key
		end, direction = "row" }),
		if suffix ~= "" then face(suffix, { shrink = 0 }) else nil,
	}
end)
