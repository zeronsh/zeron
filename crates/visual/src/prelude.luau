-- The `ui` library every visual script sees. Constructors only tag their
-- props table; Zeron validates and renders the tree natively.
local ui = {}

local function constructor(kind)
	return function(props)
		if type(props) ~= "table" or rawget(props, "__kind") ~= nil then
			props = { props }
		end
		props.__kind = kind
		return props
	end
end

for _, kind in
	{
		"column",
		"row",
		"grid",
		"text",
		"span",
		"rule",
		"space",
		"stat",
		"table",
		"chart",
		"canvas",
		"scene",
		"shader",
		"button",
		"tabs",
		"toggle",
		"slider",
		"link",
	}
do
	ui[kind] = constructor(kind)
end

-- A theme token at an opacity.
function ui.color(token, alpha)
	return { __color = true, token = token, alpha = alpha }
end

-- A blend of two theme tokens, `t` of the way from `a` to `b`.
function ui.mix(a, b, t)
	return { __color = true, token = a, mix = b, t = t }
end

-- The `i`th categorical series color (1-based, cycling through six).
function ui.series(i)
	return "chart" .. ((i - 1) % 6 + 1)
end

-- The table your handlers change. Zeron renders the view again after every
-- handler, so plain assignment is enough.
function ui.state(initial)
	return initial or {}
end

-- A linear map from the domain [d0, d1] onto the range [r0, r1].
function ui.scale(d0, d1, r0, r1)
	local span = d1 - d0
	if span == 0 then
		span = 1
	end
	return function(v)
		return r0 + (v - d0) / span * (r1 - r0)
	end
end

function ui.clamp(v, lo, hi)
	return math.max(lo, math.min(hi, v))
end

function ui.lerp(a, b, t)
	return a + (b - a) * t
end

function ui.map(list, f)
	local out = {}
	for i, v in ipairs(list) do
		out[i] = f(v, i)
	end
	return out
end

function ui.range(a, b, step)
	if b == nil then
		a, b = 1, a
	end
	step = step or 1
	local out = {}
	for v = a, b, step do
		table.insert(out, v)
	end
	return out
end

return ui
