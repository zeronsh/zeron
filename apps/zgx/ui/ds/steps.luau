-- Progress through a flow: a dot per step, the current one drawn out long.
-- `Steps { count = 4, current = state }` (current counts from 1). Moving on,
-- the new step springs out as the last one draws in, and each dot eases to
-- its colour as it is reached or left. `count` may be a function (a flow
-- that grows or shortens as it goes): the dots there stay, a dot coming
-- fades in at its place and one going fades out, the rest sliding to
-- theirs.

export type Props = { count: number | () -> number, current: State<number> | () -> number }

local DOT, LONG = 6, 20

return function(props: Props): Node
	local current = props.current
	local function at(): number
		return if type(current) == "function" then current() else current:get()
	end
	local function dot(i: number): Node
		local here = ui.computed(function(): number
			return if at() == i then 1 else 0
		end)
		local reached = ui.computed(function(): number
			return if at() >= i then 1 else 0
		end)
		return ui.div {
			h = DOT,
			rounded = "$radius.full",
			w = ui.spring(here, { from = DOT, to = LONG, stiffness = 320, damping = 22 }),
			bg = ui.tween(reached, { from = "$border.strong", to = "$text.primary", duration = 220 }),
		}
	end
	local count = props.count
	if type(count) == "number" then
		local dots: { Node } = {}
		for i = 1, count do
			table.insert(dots, dot(i))
		end
		return ui.row { gap = "$space.1", items = "center", table.unpack(dots) }
	end
	-- (Each dot is its place's: a step's dot stays its node as the count
	-- changes around it.)
	local places = ui.computed(function(): { number }
		local out: { number } = {}
		for i = 1, count() do
			table.insert(out, i)
		end
		return out
	end)
	return ui.each(places, dot, {
		key = function(i: number): number
			return i
		end,
		direction = "row",
		gap = "$space.1",
		items = "center",
		animate = { duration = 220, ease = "outCubic" },
		enter = { opacity = { 0, 1 }, duration = 160 },
		exit = { opacity = 0, duration = 100 },
	})
end
