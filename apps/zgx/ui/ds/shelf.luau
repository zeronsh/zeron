--!strict
-- A shelf: like things side by side across a column (a plugin's
-- screenshots, the plugin store's featured), sized by how many there are and
-- how wide the column is as it's laid out, and never wrapping to a second
-- row. One rule:
--   - no more than `across` of them, each at least `min_w` when they share
--     the column: they share it, `gap` apart, end to end;
--   - else they run sideways, each as wide as (column − k·gap) / (k + 0.4)
--     for the most whole ones, k (at most `across`), that keep `min_w`: the
--     next shows cut by the column's end, which says there are more.
-- (A picture strip: `across = 2`, no minimum: one fills the column, two
-- share it, three or more show two whole and the third begun.)
--
--   Shelf { id = "shots", across = 2, min_w = 0, count = function() … end,
--     build = function(w: () -> number): Node … end }
--
-- `build` makes the items (a list of nodes, or a `ui.each`), each `w()`
-- wide (`ui.div { w = w, shrink = 0, … }`), its height its own (a
-- picture's aspect, a card's rows); the shelf sets them `gap` apart.
-- `count` is how many there are (followed as it changes).

local M = {}

-- An item's width on a shelf `column` wide, holding `n`. (Pure, for tests.)
function M.width(column: number, n: number, across: number, min_w: number, gap: number): number
	if n <= 0 or column <= 0 then
		return 0
	end
	if n <= across then
		-- (Unrounded: sharing it, they meet the column's ends exactly.)
		local shared = (column - (n - 1) * gap) / n
		if shared >= min_w then
			return shared
		end
	end
	for k = math.min(across, n - 1), 1, -1 do
		local w = (column - k * gap) / (k + 0.4)
		if w >= min_w then
			return math.floor(w)
		end
	end
	-- (Narrower than one at its least and the next begun: one at its least.)
	return math.floor(math.max(min_w, (column - gap) / 1.4))
end

export type Props = {
	id: string?,
	class: string?,
	across: number,
	min_w: number?,
	-- The space between items: a theme spacing (`"$space.3"`, the default).
	gap: string?,
	count: () -> number,
	build: (w: () -> number) -> any,
}

M.Shelf = ui.component(function(props: Props): Node
	local gap_token = props.gap or "$space.3"
	local gap = (ui.theme.get(string.sub(gap_token, 2)) :: number?) or 12
	-- (The column as laid out; the settings measure until then.)
	local column = ui.use_state((ui.theme.get("zeron.settings.measure") :: number?) or 640)
	local function w(): number
		return M.width(column:get(), props.count(), props.across, props.min_w or 0, gap)
	end
	return ui.div {
		id = props.id,
		class = props.class,
		w_full = true,
		on_resize = function(size: { w: number, h: number })
			if math.abs(size.w - column:peek()) >= 0.5 then
				column:set(size.w)
			end
		end,
		ui.scroll_x {
			w_full = true,
			fade = { 12, 12 },
			fade_overflow = true,
			ui.row { gap = gap_token, items = "start", props.build(w) },
		},
	}
end)

return M
