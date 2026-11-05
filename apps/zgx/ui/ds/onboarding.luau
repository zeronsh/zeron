-- Onboarding's building blocks: a hero (a screen's opening, centred),
-- a step header (where in the flow this is, and what it asks), and option
-- cards (a choice made by picking a card: one of them, or several).
--
--   Hero { title = "Welcome to Zeron", body = "…", art = Mark { … }, …actions }
--   StepHeader { step = 2, count = 4, title = "Which agents do you use?", body = "…" }
--   OptionCards { value = state, options = { { value = "claude", title = "Claude", mark = "claude" }, … } }
--   OptionCards { value = list_state, multiple = true, options = … }
--
-- An option card is a quiet card with a hairline edge; under the pointer
-- it washes; chosen, it wears the accent's ring inside its edge (a ring,
-- never a fill or a shadow), and its corner mark (a radio's dot, or a
-- check for several) fills.

local Icon = require("@self/icon")
local Mark = require("@self/mark")
local Steps = require("@self/steps")
local Text = require("@self/text")
local control = require("@self/control")

local M = {}

-- (Words here may be messages, `ui.t`, drawn as they are.)
export type HeroProps = {
	title: string | Message,
	body: (string | Message)?,
	-- Over the title: a mark, an illustration.
	art: Node?,
	-- A small line over the title ("Step 1", "Welcome back").
	eyebrow: (string | Message)?,
	-- Under the actions: a quiet line (terms, a sign-in link).
	foot: Node?,
	[number]: Child,
}

function M.Hero(props: HeroProps): Node
	local actions: { any } = {}
	for _, c in ipairs(props :: any) do
		table.insert(actions, c)
	end
	return ui.column {
		items = "center",
		justify = "center",
		w_full = true,
		grow = 1,
		gap = "$space.6",
		ui.column {
			items = "center",
			gap = "$space.5",
			max_w = ui.theme.get("zeron.hero.measure") :: number?,
			props.art,
			ui.column {
				items = "center",
				gap = "$space.3",
				if props.eyebrow then Text { props.eyebrow, style = "label", tone = "accent" } else nil,
				Text { props.title, style = "display", align = "center", wrap = true, heading = 1 },
				if props.body
					then ui.text { props.body, align = "center", wrap = true, text_size = "$font.size.lg", line_height = 24, color = "$text.secondary" }
					else nil,
			},
		},
		if #actions > 0 then ui.column { items = "center", gap = "$space.3", table.unpack(actions) } else nil,
		props.foot,
	}
end

export type StepHeaderProps = {
	step: number | () -> number,
	count: number,
	title: string | Message,
	body: (string | Message)?,
}

function M.StepHeader(props: StepHeaderProps): Node
	local step = props.step
	local function at(): number
		return if type(step) == "function" then step() else step
	end
	return ui.column {
		gap = "$space.4",
		w_full = true,
		ui.row {
			gap = "$space.3",
			items = "center",
			Steps { count = props.count, current = at },
			ui.text {
				function()
					return `{at()} of {props.count}`
				end,
				tabular = true,
				text_size = "$font.size.xs",
				weight = "$font.weight.medium",
				color = "$text.tertiary",
			},
		},
		ui.column {
			gap = "$space.2",
			Text { props.title, style = "title", wrap = true, heading = 1 },
			if props.body
				then ui.text { props.body, wrap = true, text_size = "$font.size.lg", line_height = 24, color = "$text.secondary" }
				else nil,
		},
	}
end

export type Option = {
	value: any,
	title: string | Message,
	description: (string | Message)?,
	icon: Icon.Name?,
	mark: Mark.Name?,
	-- Anything in the glyph's place.
	art: Node?,
	-- A small badge at the title's end ("Recommended").
	badge: Node?,
	disabled: boolean?,
}

-- One card. `on` says whether it is chosen; `round` draws a radio's dot,
-- else a check.
local Card = ui.component(function(props: { option: Option, on: () -> boolean, round: boolean, pick: () -> (), layout: string }): Node
	local o = props.option
	local hovered = ui.use_state(false)
	local on = props.on
	local wide = props.layout == "row"
	local glyph: Node? = o.art
	if not glyph and o.mark then
		glyph = Mark { name = o.mark, size = 32 }
	elseif not glyph and o.icon then
		glyph = ui.row {
			w = 32,
			h = 32,
			shrink = 0,
			items = "center",
			justify = "center",
			rounded = "$radius.lg",
			bg = "$zeron.secondary.bg",
			color = function()
				return if on() then "$text.primary" else "$text.secondary"
			end,
			Icon { name = o.icon, size = 18 },
		}
	end
	-- The corner mark: empty, then filled with the accent.
	local mark = ui.row {
		w = 16,
		h = 16,
		shrink = 0,
		items = "center",
		justify = "center",
		rounded = if props.round then "$radius.full" else "$radius.sm",
		border = 1,
		bg = function()
			return if on() then "$zeron.check.on" else "$zeron.check.bg"
		end,
		border_color = function()
			if on() then
				return "$zeron.check.on"
			end
			return if hovered:get() then "$text.tertiary" else "$zeron.check.edge"
		end,
		color = "$zeron.check.mark",
		if props.round
			then ui.div {
				w = 6,
				h = 6,
				rounded = "$radius.full",
				bg = "$zeron.check.mark",
				scale = ui.spring(ui.computed(function(): number
					return if on() then 1 else 0
				end), { stiffness = 520, damping = 26 }),
			}
			else ui.div {
				opacity = ui.tween(ui.computed(function(): number
					return if on() then 1 else 0
				end), { duration = 120 }),
				ui.svg { src = "assets/icons/ui/check.svg", w = 12, h = 12, tint = "currentColor" },
			},
	}
	local text = ui.column {
		grow = 1,
		min_w = 0,
		gap = "$zeron.space.hair",
		ui.row {
			gap = "$space.2",
			items = "center",
			ui.text { o.title, text_size = "$font.size.md", line_height = "$font.line.md", weight = "$font.weight.medium", color = "$text.primary" },
			o.badge,
		},
		if o.description
			then ui.text { o.description, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary", wrap = true }
			else nil,
	}
	return ui.row {
		role = if props.round then "radio" else "checkbox",
		checked = on,
		label = o.title,
		focusable = not o.disabled,
		disabled = o.disabled,
		opacity = if o.disabled then "$zeron.disabled" else nil,
		cursor = if o.disabled then nil else "pointer",
		relative = true,
		w_full = true,
		direction = if wide then "row" else "column",
		items = if wide then "center" else "start",
		gap = "$space.3",
		p = "$space.4",
		rounded = "$radius.xl",
		border = 1,
		bg = function()
			return if hovered:get() and not on() then "$hover" else "$zeron.option.bg"
		end,
		border_color = "$zeron.option.edge",
		focus = control.ring,
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		on_click = props.pick,
		on_key = function(e)
			if e.key == " " or e.key == "Enter" then
				props.pick()
				return true
			end
			return false
		end,
		-- Chosen: the accent's ring, two points, over the edge, easing in.
		ui.div {
			absolute = true,
			left = -1,
			top = -1,
			right = -1,
			bottom = -1,
			rounded = "$radius.xl",
			border = 2,
			border_color = "$zeron.option.ring",
			pointer_events = "none",
			opacity = ui.tween(ui.computed(function(): number
				return if on() then 1 else 0
			end), { duration = 140 }),
		},
		if wide then glyph else ui.row { w_full = true, items = "start", justify = "between", glyph or ui.div {}, mark },
		text,
		if wide then mark else nil,
	}
end)

export type CardsProps = {
	-- One chosen (a value), or several (`multiple`: a list of values).
	value: State<any>,
	options: { Option },
	multiple: boolean?,
	-- How many across (a grid of tiles), or 1 for a list of wide rows.
	columns: number?,
	label: Label?,
	on_change: ((value: any) -> ())?,
}

function M.OptionCards(props: CardsProps): Node
	local value = props.value
	local columns = props.columns or 2
	local layout = if columns == 1 then "row" else "column"
	local cards: { Node } = {}
	for _, o in props.options do
		local function on(): boolean
			local v = value:get()
			if props.multiple then
				for _, x in v or {} do
					if x == o.value then
						return true
					end
				end
				return false
			end
			return v == o.value
		end
		local function pick()
			if o.disabled then
				return
			end
			if props.multiple then
				local list = table.clone(value:peek() or {})
				local found = table.find(list, o.value)
				if found then
					table.remove(list, found)
				else
					table.insert(list, o.value)
				end
				value:set(list)
			else
				value:set(o.value)
			end
			if props.on_change then
				props.on_change(value:peek())
			end
		end
		table.insert(cards, Card { option = o, on = on, round = not props.multiple, pick = pick, layout = layout })
	end
	return ui.column {
		role = if props.multiple then "group" else "radiogroup",
		label = props.label,
		w_full = true,
		grid = true,
		cols = columns,
		gap = "$space.3",
		table.unpack(cards),
	}
end

return M
