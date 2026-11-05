-- The organizations someone is in, to choose one: rows of one card in the
-- provider cards' language (ds.Settings.Group, ds.ProviderCard's shell, on
-- its hairlines; a mark the agents' size), as an agent's card holds its
-- accounts; and an invitation waiting, a row with its answer at its end. A
-- click picks a row: its name comes to full ink and the accent's check
-- stands at its end (no fill: a picked row is chosen, not dimmed), and the
-- way on says which ("Continue in Halcyon Labs"); a double click, or Enter
-- on it, goes on at once. A row under the pointer washes, the wash nested
-- inside the card, its corners concentric with the card's. Setup's Choose
-- an organization is this; so is the gallery's.
--
--   OrgList.List { OrgList.Org { org = { name = "Halcyon Labs", detail = "Owner · 12 members" }, selected = …, on_select = …, on_open = … }, OrgList.Invitation { … } }
--   OrgList.Loading {}

local Settings = require("@self/settings")
local Icon = require("@self/icon")
local FadeText = require("@self/fade_text")
local Button = require("@self/button")
local skeleton = require("@self/skeleton")
local control = require("@self/control")
local locale = require("@self/locale")

local M = {}

-- An organization: its name, and its second line when there is one (what
-- you are there; never made up).
-- (Its name is the organization's own, a string: its initial is read from
-- it. Its second line may be a message, `ui.t`.)
export type Org = { name: string, detail: (string | Message | () -> string)? }

-- A mark's size: an agent's in Your agents (ds.Mark at 32).
local MARK = 32
-- How far the wash stands in from the row's edges.
local WASH_INSET = 4

-- An organization's mark: its initial on the quiet surface, the same for
-- every one, sized and cornered as the agents' marks are.
local function mark(name: string): Node
	return ui.row {
		w = MARK,
		h = MARK,
		shrink = 0,
		items = "center",
		justify = "center",
		rounded = ui.raw(MARK * 0.25, "a mark tile's radius scales with it"),
		bg = "$zeron.secondary.bg",
		ui.text { string.upper(string.sub(name, 1, 1)), text_size = "$font.size.md", weight = "$font.weight.medium", color = "$text.secondary" },
	}
end

-- What a row says: its name (quieter unless picked, when `picked` is
-- given), and its second line when there is one.
local function words(o: Org, picked: (() -> boolean)?): Node
	return ui.column {
		grow = 1,
		min_w = 0,
		gap = "$zeron.space.hair",
		FadeText {
			o.name,
			text_size = "$font.size.md",
			line_height = "$font.line.md",
			weight = "$font.weight.medium",
			color = if picked
				then function()
					return if picked() then "$text.primary" else "$text.secondary"
				end
				else "$text.primary",
		},
		if o.detail
			then FadeText { o.detail, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary" }
			else nil,
	}
end

-- A row's shape: a settings row's (its height, its insets, the mark's gap
-- to the words), with a wash laid in it, in from the row's edges, its
-- corners the card's less its edge and that inset (concentric).
local function shape(props: { [any]: any }, wash: (() -> string)?, kids: { Node }): Node
	props.w_full = true
	props.min_h = "$zeron.settings.row"
	props.px = "$space.4"
	props.py = "$space.3"
	props.gap = "$space.3"
	props.items = "center"
	props.relative = true
	if wash then
		table.insert(props, ui.div {
			absolute = true,
			left = WASH_INSET,
			right = WASH_INSET,
			top = WASH_INSET,
			bottom = WASH_INSET,
			rounded = ui.raw(control.nest("$radius.xl", 1, WASH_INSET), "the card's corner, its edge and the wash's inset in"),
			pointer_events = "none",
			bg = wash,
		})
	end
	for _, k in kids do
		table.insert(props, k)
	end
	return ui.row(props)
end

-- An organization you're in: a click picks it (the way on opens the one
-- picked); a second click in a row, or Enter, opens it at once.
type OrgProps = { org: Org, on_open: () -> (), selected: () -> boolean, on_select: () -> () }

-- The accent's check at a row's end, easing in as it's picked.
local function check(on: () -> boolean): Node
	return ui.div {
		shrink = 0,
		color = "$accent",
		opacity = ui.tween(ui.computed(function(): number
			return if on() then 1 else 0
		end), { duration = 140 }),
		Icon { name = "check", size = 16 },
	}
end

local OrgRow = ui.component(function(props: OrgProps): Node
	local hovered = ui.use_state(false)
	return shape({
		role = "radio",
		checked = props.selected,
		label = props.org.name,
		focusable = true,
		cursor = "pointer",
		focus = { outline = "$zeron.focus.width", outline_color = "$zeron.focus.ring", outline_offset = -2 },
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		on_press = function(e: any)
			props.on_select()
			if (e.clicks or 1) >= 2 then
				props.on_open()
			end
		end,
		on_key = function(e: any): boolean
			if e.key == "Enter" then
				props.on_select()
				props.on_open()
				return true
			elseif e.key == " " then
				props.on_select()
				return true
			end
			return false
		end,
	}, function(): string
		return if hovered:get() then "$hover" else "#00000000"
	end, { mark(props.org.name), words(props.org, props.selected), check(props.selected) })
end)

-- An organization: a row of the card.
function M.Org(props: OrgProps): Node
	return OrgRow(props)
end

-- An invitation waiting: who asked, and its answer at its end (as a
-- provider card's foot holds its actions).
function M.Invitation(props: { name: string, from: string, on_accept: () -> (), on_decline: () -> () }): Node
	return shape({}, nil, {
		mark(props.name),
		words({ name = props.name, detail = locale.t("ds.org_list.invited", { from = props.from }) }),
		ui.row {
			gap = "$space.1",
			items = "center",
			shrink = 0,
			Button { locale.t("ds.org_list.decline"), tone = "ghost", size = "sm", on_click = props.on_decline },
			Button { locale.t("ds.org_list.accept"), tone = "secondary", size = "sm", on_click = props.on_accept },
		},
	})
end

-- The rows, in the card, on its hairlines.
function M.List(items: { Node }): Node
	return Settings.Group(items :: any)
end

-- The rows of a list that changes (an invitation answered, its
-- organization joining), each its item's as the rest come and go:
-- `OrgList.Each(entries, row, { key = … })`, as Settings.Each.
M.Each = Settings.Each

-- While the list loads: its shape.
function M.Loading(): Node
	return Settings.Group {
		skeleton.Row { title = 120, line = 0, trail = 0 },
		skeleton.Row { title = 150, line = 0, trail = 0 },
	}
end

return M
