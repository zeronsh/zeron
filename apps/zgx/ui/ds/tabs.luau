-- Tabs: the views of one thing, side by side, the chosen one marked by an
-- indicator that springs from tab to tab, each tab as wide as its label.
-- Two looks: `underline` (a page's or a panel's sections: a two-point bar
-- under the chosen label, a hairline under them all) and `pill` (a quiet
-- run of views in a header: the chosen one on the flat selected wash, the
-- segmented control's thumb without its track). A tab may carry an icon
-- and a count. One Tab stop; the arrows move between tabs (and choose).
--
-- `Tabs { value = state, options = { { value = "files", label = "Files", count = 12 }, … } }`.

local motion = require("@self/motion")
local Icon = require("@self/icon")
local TabStrip = require("@self/tab_strip")
local control = require("@self/control")

-- (A tab's label may be a message, `ui.t`, drawn as it is.)
export type Option = { value: any, label: string | Message, icon: Icon.Name?, count: (number | () -> number)?, disabled: boolean? }
export type Look = "underline" | "pill"
export type Props = {
	value: State<any>,
	options: { Option },
	look: Look?,
	size: ("sm" | "md")?,
	-- What assistive technology calls the list.
	label: Label?,
	-- The hairline under underline tabs, across the row's whole width.
	rule: boolean?,
	-- What sits at the far end of that rule (a page's badges), as a
	-- Section's `trail` does: the rule runs under the tabs and it alike.
	trail: Node?,
	-- Room either side of the tabs (and the trail) inside the rule, which
	-- still runs the whole width: labels on a card's words, the rule on its
	-- edges (Settings.Catalog).
	inset: any?,
	on_change: ((value: any) -> ())?,
}

local SPRING: SpringOptions = motion.TAB_SPRING

-- (A component: each tab keeps its own hover.)
return ui.component(function(props: Props): Node
	local value = props.value
	local look: Look = props.look or "underline"
	local size = props.size or "md"
	local pill = look == "pill"
	local H = if pill then (if size == "sm" then 26 else 30) else (if size == "sm" then 32 else 36)
	local refs: { any } = {}
	local tabs: { Node } = {}
	local index = ui.computed(function(): number
		local v = value:get()
		for i, o in props.options do
			if o.value == v then
				return i
			end
		end
		return 1
	end)
	local function choose(o: Option)
		if o.disabled or value:peek() == o.value then
			return
		end
		value:set(o.value)
		if props.on_change then
			props.on_change(o.value)
		end
	end
	for _, o in props.options do
		local ref = ui.ref()
		table.insert(refs, ref)
		local function chosen(): boolean
			return value:get() == o.value
		end
		local hovered = ui.use_state(false)
		local ink = function()
			if o.disabled then
				return "$text.disabled"
			end
			return if chosen() then "$text.primary" else "$text.secondary"
		end
		local count: Node? = nil
		if o.count ~= nil then
			local c = o.count
			count = ui.text {
				function()
					return tostring(if type(c) == "function" then c() else c)
				end,
				tabular = true,
				text_size = "$font.size.xs",
				weight = "$font.weight.medium",
				color = function()
					return if chosen() then "$text.secondary" else "$text.tertiary"
				end,
			}
		end
		table.insert(
			tabs,
			ui.row {
				ref = ref,
				role = "tab",
				selected = chosen,
				label = o.label,
				focusable = not o.disabled,
				disabled = o.disabled,
				cursor = if o.disabled then nil else "pointer",
				h = H,
				px = if pill then "$space.3" else "$space.1",
				gap = "$zeron.space.tight",
				items = "center",
				shrink = 0,
				rounded = if pill then "$radius.lg" else "$radius.sm",
				relative = true,
				color = ink,
				-- (Under the pointer, a pill's wash fades in, `TabStrip.Wash`, the
				-- ink brightening; pressed, a pill darkens, as the strips'. The
				-- underline look stays as it was.)
				hover = if o.disabled then nil else { color = "$text.primary" },
				active = if pill and not o.disabled then { bg = "$pressed" } else nil,
				on_hover = if pill and not o.disabled
					then function(inside: boolean)
						hovered:set(inside)
					end
					else nil,
				focus = control.ring,
				on_click = function()
					choose(o)
				end,
				on_focus = function()
					choose(o)
				end,
				if pill
					then TabStrip.Wash(function(): boolean
						return hovered:get() and not chosen()
					end)
					else nil,
				if o.icon then ui.div { y = -0.5, Icon { name = o.icon, size = if size == "sm" then 14 else 16 } } else nil,
				ui.text {
					o.label,
					text_size = if size == "sm" then "$font.size.sm" else "$font.size.md",
					weight = "$font.weight.medium",
				},
				count,
			}
		)
	end
	local function width(i: number): number
		return refs[i].width or 0
	end
	-- The gap between tabs, in points for the indicator's sums (the
	-- row's own gap, zeron.space.hair or space.4).
	local GAP = if pill then 2 else 16
	local function measured(): boolean
		for i in refs do
			if width(i) <= 0 then
				return false
			end
		end
		return true
	end
	local w = motion.spring(ui.computed(function(): number
		return width(index:get())
	end), SPRING)
	local x = motion.spring(ui.computed(function(): number
		local at = 0
		for i = 1, index:get() - 1 do
			at += width(i) + GAP
		end
		return at
	end), SPRING)
	local indicator = if pill
		then ui.div {
			absolute = true,
			left = 0,
			top = 0,
			h = H,
			rounded = "$radius.lg",
			bg = "$zeron.selected.flat.bg",
			w = w,
			x = x,
			pointer_events = "none",
		}
		else ui.div {
			absolute = true,
			left = 0,
			bottom = 0,
			h = 2,
			rounded = "$radius.full",
			bg = "$text.primary",
			w = w,
			x = x,
			pointer_events = "none",
		}
	local row = ui.row {
		role = "tablist",
		label = props.label,
		relative = true,
		gap = if pill then "$zeron.space.hair" else "$space.4",
		items = "center",
		shrink = 0,
		ui.show(measured, indicator),
		table.unpack(tabs),
	}
	if pill or props.rule == false then
		return row
	end
	-- Underlined: the hairline under every tab, the bar sitting on it, and
	-- the trail (if any) at the far end over the same line.
	local line = if props.trail or props.inset
		then ui.row { w_full = true, px = props.inset, items = "center", gap = "$space.3", row, ui.div { grow = 1 }, props.trail }
		else row
	return ui.column {
		relative = true,
		w_full = true,
		ui.div { absolute = true, left = 0, right = 0, bottom = 0, h = 1, bg = "$border.subtle", pointer_events = "none" },
		line,
	}
end)
