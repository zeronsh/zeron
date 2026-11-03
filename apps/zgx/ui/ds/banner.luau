-- Notices in the page: a callout (saying something about what is around
-- it: a setting that needs a restart, a branch behind its base; its title
-- over its body, the actions under them) and a banner (one line: the title,
-- its body after it, the actions at the far end; about the whole of a page
-- or panel: offline, a sync failing). Each is a tone (info, success,
-- warning, danger, or neutral) whose glyph and ink say how much it matters,
-- a title and a line, an action or two, and a × to put it away.
--
-- The two are one notice in two layouts, and share one frame: a rounded
-- card, as the page's other cards are, with the tone's wash inside and its
-- edge round it. A banner that runs across a window or panel, edge to edge
-- under its title bar, says `edge = true`: there its sides are the panel's
-- own, so it squares its corners and keeps only a hairline under it. (A
-- square strip with a hairline, set among rounded cards, looked pasted in.)
--
-- `Callout { tone = "warning", title = "Behind main by 12 commits", body = "…",
--   action = { label = "Update branch", on_click = … } }`
-- `Banner { tone = "danger", title = "Tests failed", body = "2 of 318.", action = { label = "See what failed" } }`
-- `Banner { edge = true, tone = "danger", title = "Can't reach GitHub", action = { label = "Retry", on_click = … } }`

local Button = require("@self/button")
local IconButton = require("@self/icon_button").IconButton
local Icon = require("@self/icon")
local FadeText = require("@self/fade_text")
local control = require("@self/control")
local locale = require("@self/locale")

local M = {}

-- (Its words may be messages, `ui.t`, drawn as they are.)
export type Tone = "neutral" | "info" | "success" | "warning" | "danger"
export type Action = { label: string | Message, on_click: (() -> ())?, primary: boolean? }
export type Props = {
	tone: Tone?,
	title: string | Message | () -> string,
	body: (string | Message | () -> string)?,
	icon: Icon.Name?,
	action: Action?,
	actions: { Action }?,
	on_dismiss: (() -> ())?,
	-- A banner across a window or panel, edge to edge: square, a hairline
	-- under it. Off, it is a rounded card like everything round it.
	edge: boolean?,
}

-- Each tone's glyph.
local GLYPH: { [Tone]: Icon.Name } = {
	neutral = "info-circle",
	info = "info-circle",
	success = "check-circle",
	warning = "danger-triangle",
	danger = "danger-triangle",
}

local function actions(props: Props, size: "sm" | "md"): { Node }
	local list = props.actions or (if props.action then { props.action } else {})
	local out: { Node } = {}
	for i, a in list do
		-- The first, or one marked primary, is the way on; any other is
		-- quiet beside it.
		local lead = a.primary or (i == 1 and not list[2])
		table.insert(out, Button {
			a.label,
			size = "sm",
			tone = if lead then "secondary" else "ghost",
			on_click = a.on_click,
		})
	end
	if props.on_dismiss then
		table.insert(out, IconButton { icon = "close", label = locale.t("common.dismiss"), size = if size == "sm" then "sm" else "md", tone = "quiet", on_click = props.on_dismiss })
	end
	return out
end

local function glyph(props: Props, size: number): Node
	local tone: Tone = props.tone or "info"
	return ui.div {
		shrink = 0,
		color = if tone == "neutral" then "$text.secondary" else `$zeron.tone.{tone}.ink`,
		Icon { name = props.icon or GLYPH[tone], size = size },
	}
end

-- The notice's frame, laid onto its row: the tone's wash, and round it the
-- tone's edge as a card's border, or, edge to edge, as a hairline under it.
local function frame(row: { [any]: any }, tone: Tone, edge: boolean): Node
	row.role = if tone == "danger" or tone == "warning" then "alert" else "status"
	row.w_full = true
	row.bg = `$zeron.tone.{tone}.wash`
	row.border_color = `$zeron.tone.{tone}.edge`
	if edge then
		row.border_sides = { 0, 0, 1, 0 }
	else
		row.rounded = "$radius.xl"
		row.border = 1
	end
	return ui.row(row)
end

-- A callout: the title on the glyph's line, the body under it, the actions
-- at its end (under the text when there is a body, so a long line keeps its
-- room).
function M.Callout(props: Props): Node
	local tone: Tone = props.tone or "info"
	local acts = actions(props, "md")
	local has_body = props.body ~= nil
	local text = ui.column {
		grow = 1,
		min_w = 0,
		gap = "$zeron.space.hair",
		ui.text {
			props.title,
			text_size = "$font.size.md",
			line_height = "$font.line.md",
			weight = "$font.weight.medium",
			color = "$text.primary",
			wrap = true,
		},
		if props.body
			then ui.text {
				props.body,
				text_size = "$font.size.md",
				line_height = "$font.line.md",
				color = "$text.secondary",
				wrap = true,
			}
			else nil,
	}
	local dismiss = nil
	if props.on_dismiss then
		dismiss = table.remove(acts)
	end
	return frame({
		gap = "$space.3",
		-- One line: everything on its centre; with a body, the glyph on the
		-- title's line.
		items = if has_body then "start" else "center",
		pl = "$space.4",
		pr = if dismiss then "$space.2" else "$space.4",
		py = "$space.3",
		-- The glyph on the title's first line (18 high: centred on it).
		ui.div { pt = if has_body then ui.raw(1, "centres the 16 glyph on an 18 line") else 0, glyph(props, 16) },
		ui.column {
			grow = 1,
			min_w = 0,
			gap = "$space.3",
			if has_body
				then text
				else ui.row { gap = "$space.3", items = "center", text, table.unpack(acts) },
			if has_body and #acts > 0 then ui.row { gap = "$space.2", items = "center", ml = ui.raw(-6, "lines a button's label up with the text over it"), table.unpack(acts) } else nil,
		},
		if dismiss
			then ui.div {
				-- In the corner: as far from the top as from the end, concentric.
				mt = ui.raw(-4, "as far from the top as from the end"),
				theme = { zeron = { icon_button = { radius = control.nest("$radius.xl", 1, "$space.2") } } },
				dismiss,
			}
			else nil,
	}, tone, props.edge == true)
end

-- A banner: one line, the glyph, the title in the primary ink and the body
-- after it (fading where the line runs out), the actions at the far end.
function M.Banner(props: Props): Node
	local tone: Tone = props.tone or "info"
	return frame({
		min_h = 40,
		px = "$space.4",
		-- Its buttons as far from the end as from its top and foot.
		pr = ui.raw(6, "a 28 button's room in the 40 strip"),
		py = "$space.1",
		gap = "$space.2",
		items = "center",
		ui.div { y = -0.5, glyph(props, 16) },
		ui.row {
			grow = 1,
			min_w = 0,
			gap = "$space.2",
			items = "center",
			ui.text { props.title, shrink = 0, text_size = "$font.size.md", weight = "$font.weight.medium", color = "$text.primary" },
			if props.body
				then FadeText { props.body, grow = 1, text_size = "$font.size.md", color = "$text.secondary" }
				else nil,
		},
		table.unpack(actions(props, "sm")),
	}, tone, props.edge == true)
end

return M
