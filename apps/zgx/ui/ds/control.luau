-- What every control shares, so the primitives read as one set: their
-- heights and corners at each size (Button's: 28, 32, 40; an 8-point
-- corner, 6 small), their type, the one focus ring, and how they look when
-- they can't be used. A component takes these rather than its own numbers.

local M = {}

export type Size = "sm" | "md" | "lg"

-- A control's height, corner, type and inner padding at each size.
M.H = { sm = 28, md = 32, lg = 40 } :: { [Size]: number }
M.RADIUS = { sm = "$radius.md", md = "$radius.lg", lg = "$radius.lg" } :: { [Size]: string }
M.TEXT = { sm = "$font.size.sm", md = "$font.size.md", lg = "$font.size.lg" } :: { [Size]: string }
M.PAD = { sm = "$space.2", md = "$zeron.space.snug", lg = "$space.3" } :: { [Size]: string }
-- The same in points, where a token can't go (an absolute node's insets).
M.PAD_PT = { sm = 8, md = 10, lg = 12 } :: { [Size]: number }
-- An icon beside a control's text, at each size.
M.ICON = { sm = 14, md = 16, lg = 18 } :: { [Size]: number }

-- How anything inside a rounded box is cornered, and how a box holds
-- controls (the README's rules 5 and 6). A pill's radius, as a number.
local PILL = 9999

-- A radius or inset in points: a number, or a token ("$radius.lg").
local function points(v: any): number
	if type(v) == "number" then
		return v
	elseif v == "$radius.full" then
		return PILL
	end
	return (ui.theme.get(string.sub(v, 2)) :: number?) or 0
end

-- The corner inside `outer`, `...` in from its edge (a border, then padding,
-- as many as stand between): the two curves run parallel. A pill's contents
-- are pills; an inset at or past the radius is square.
function M.nest(outer: any, ...: any): number
	local r = points(outer)
	if r >= PILL then
		return PILL
	end
	for _, inset in { ... } do
		r -= points(inset)
	end
	return math.max(0, r)
end

-- A box that holds controls: its corner, the one inset on all four of its
-- sides (a `border` adds to it), and what follows from those: every radius
-- token inside takes the nested corner, so a Button or an IconButton in it,
-- whoever built them, comes out concentric in all its states without being
-- told. Adds them to `props`, and returns it.
function M.contain(props: { [any]: any }, box: { radius: any, inset: any, border: number? }): { [any]: any }
	local inner = M.nest(box.radius, box.border or 0, box.inset)
	props.rounded = box.radius
	props.p = box.inset
	props.theme = {
		radius = { sm = inner, md = inner, lg = inner, xl = inner },
		zeron = { icon_button = { radius = inner } },
	}
	return props
end

-- The focus ring, as a `focus` style: the accent, outside the edge, so it
-- never moves what is inside. Keyboard focus only (zgx's `focus` is
-- `:focus-visible`).
M.ring = {
	outline = "$zeron.focus.width",
	outline_color = "$zeron.focus.ring",
	outline_offset = "$zeron.focus.offset",
	border = 0,
} :: Styles

-- The same ring drawn by a box whose focus is inside it (a field's frame
-- around its input): while `on` says so, invalid in the danger's.
function M.ring_when(on: () -> boolean, invalid: (() -> boolean)?): { [string]: any }
	return {
		outline = function()
			return if on() then "$zeron.focus.width" else 0
		end,
		outline_offset = "$zeron.focus.offset",
		outline_color = function()
			return if invalid and invalid() then "$zeron.focus.error" else "$zeron.focus.ring"
		end,
	}
end

-- A state, as `read` and `follow` call it (`Readable`'s `get` takes no
-- `self` the checker can name).
type Getter<T> = { get: (self: unknown) -> T }

-- A prop that may be a value, a state or a function of one (zgx's
-- `Reactive` but a `Binding`, which a text doesn't take).
export type Live<T> = T | Readable<T> | (() -> T)

-- Whether `v` is a state (`ui.state`, `ui.computed`, a row's state): zgx
-- hands them over as userdata, a few as tables with a metatable.
local function is_state(v: unknown): boolean
	return type(v) == "userdata" or (type(v) == "table" and getmetatable(v :: any) ~= nil and (v :: any).get ~= nil)
end

-- A prop that may be a value, a state or a function, as it stands now:
-- read inside a binding or a computed, it follows the prop. Every ds
-- component reads a prop that can change through this (README, "Patterns").
-- (Untyped on the way out: a generic `T` here takes the whole union, so
-- each caller says what it reads, `control.value(p) :: string?`.)
function M.value(v: unknown): unknown
	if type(v) == "function" then
		return (v :: () -> unknown)()
	elseif is_state(v) then
		return (v :: Getter<unknown>):get()
	end
	return v
end

-- (A message, `ui.t`, reads as its text there too: a userdata with a
-- `get`, read as a state is, so it follows the locale where it's read.)

-- Whether `v` is a message (`ui.t`), not a string or a state.
function M.is_message(v: unknown): boolean
	return typeof(v) == "MessageUd"
end

-- A label as a string, for what takes only one (a tooltip, a sort, a
-- search): a string as it is, a message's text now, a state's or a
-- function's as it stands. Nil for nothing.
function M.text(v: unknown): string?
	if v == nil then
		return nil
	end
	local now = M.value(v)
	return if now == nil then nil else tostring(now)
end

-- The same, typed for a field's `error`.
function M.read(v: Props_error): string?
	return M.value(v) :: string?
end

-- The same, as a function (for a property that should follow it), or the
-- value itself when it can't change.
function M.follow<T>(v: T | Readable<T> | (() -> T)): T | (() -> T)
	if type(v) == "function" then
		return v :: () -> T
	elseif is_state(v) then
		local state = v :: Getter<T>
		return function()
			return state:get()
		end
	end
	return v :: T
end

return M
