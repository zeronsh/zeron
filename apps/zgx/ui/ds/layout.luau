-- Small pieces of a page's structure: a divider, a link, breadcrumbs and a
-- key–value list.
--
--   Divider {}                         a hairline across
--   Divider { label = "or" }           with a word in it
--   Divider { vertical = true }        between a bar's groups
--   Link { "Learn more", on_click = … } inline, the accent's ink
--   Breadcrumbs { items = { { label = "zeron" }, { label = "ds" }, { label = "badge.luau" } } }
--   KeyValue { items = { { key = "Branch", value = "main" }, … } }

local Icon = require("@self/icon")
local FadeText = require("@self/fade_text")
local control = require("@self/control")
local SelectionMenu = require("@self/selection_menu")
local locale = require("@self/locale")
-- (By the alias the shell mounts the menus' host by: one instance per path.)
local Links = require("@ds/links")

local M = {}

-- A hairline: across, or down (a bar's groups apart). `strong` for the
-- one rule that divides a whole page. With a label, the word sits in a
-- gap in it.
-- (Each piece's words may be messages, `ui.t`, drawn as they are; a
-- KeyValue's value stays a string or a node: it is copied as text.)
export type DividerProps = { vertical: boolean?, label: (string | Message)?, strong: boolean? }

function M.Divider(props: DividerProps?): Node
	local p: DividerProps = props or {}
	local ink = if p.strong then "$border" else "$border.subtle"
	if p.vertical then
		return ui.div { w = 1, h = 16, shrink = 0, self = "center", bg = ink, aria_hidden = true }
	end
	if p.label then
		return ui.row {
			w_full = true,
			gap = "$space.3",
			items = "center",
			ui.div { h = 1, grow = 1, bg = ink },
			ui.text { p.label, shrink = 0, text_size = "$font.size.xs", weight = "$font.weight.medium", color = "$text.tertiary" },
			ui.div { h = 1, grow = 1, bg = ink },
		}
	end
	return ui.div { h = 1, w_full = true, shrink = 0, bg = ink, aria_hidden = true }
end

-- A link: text that goes somewhere, in the accent's ink, underlined under
-- the pointer. `external` adds the arrow out (it opens in the browser);
-- `tone = "quiet"` for a link among quiet text (a hint's), in its ink
-- until touched. Where it goes is its `on_click`'s to say (the host
-- opens URLs).
export type LinkProps = {
	[number]: string | Message,
	on_click: (() -> ())?,
	external: boolean?,
	-- Where it goes, a web address its click opens outside: its context
	-- menu is a link's (ds/links: Open, Copy link).
	href: string?,
	tone: ("accent" | "quiet")?,
	size: ("sm" | "md")?,
}

function M.Link(props: LinkProps): Node
	local quiet = props.tone == "quiet"
	local href = props.href
	return ui.row {
		role = "link",
		label = props[1],
		focusable = true,
		cursor = "pointer",
		gap = "$zeron.space.hair",
		items = "center",
		shrink = 0,
		rounded = "$radius.sm",
		color = if quiet then "$text.secondary" else "$zeron.tone.accent.ink",
		hover = { underline = true, color = if quiet then "$text.primary" else "$zeron.tone.accent.ink" },
		underline_offset = 3,
		focus = control.ring,
		on_click = props.on_click,
		context_menu = if href
			then Links.outside_menu(function(): string?
				return href
			end, props.on_click)
			else nil,
		ui.text { props[1], text_size = if props.size == "sm" then "$font.size.sm" else "$font.size.md" },
		if props.external then ui.div { y = -0.5, Icon { name = "arrow-up-right", size = 12 } } else nil,
	}
end

-- Breadcrumbs: where something is, its parents before it, each a link
-- but the last (where you are), a quiet chevron between. A long middle
-- fades, never cut to "…".
export type Crumb = { label: string | Message, icon: Icon.Name?, on_click: (() -> ())? }
export type BreadcrumbsProps = { items: { Crumb }, size: ("sm" | "md")? }

function M.Breadcrumbs(props: BreadcrumbsProps): Node
	local size = if props.size == "sm" then "$font.size.sm" else "$font.size.md"
	local out: { Node } = {}
	for i, c in props.items do
		local last = i == #props.items
		if i > 1 then
			table.insert(out, ui.div { shrink = 0, color = "$text.tertiary", aria_hidden = true, Icon { name = "alt-arrow-right", size = 12 } })
		end
		local crumb = ui.row {
			role = if last then nil else "link",
			label = c.label,
			focusable = not last and c.on_click ~= nil,
			cursor = if last or not c.on_click then nil else "pointer",
			gap = "$space.1",
			items = "center",
			min_w = 0,
			shrink = if last then 0 else 1,
			px = "$space.1",
			h = 22,
			rounded = "$radius.sm",
			color = if last then "$text.primary" else "$text.secondary",
			hover = if last then nil else { bg = "$hover", color = "$text.primary" },
			focus = if last then nil else control.ring,
			on_click = c.on_click,
			if c.icon then ui.div { shrink = 0, y = -0.5, Icon { name = c.icon, size = 14 } } else nil,
			FadeText { c.label, text_size = size, weight = if last then "$font.weight.medium" else nil },
		}
		table.insert(out, crumb)
	end
	return ui.row { role = "group", label = locale.t("ds.layout.breadcrumbs"), gap = "$zeron.space.hair", items = "center", min_w = 0, table.unpack(out) }
end

-- A key–value list: what something is, a fact a line (a commit's author,
-- a branch's upstream, a device's address). Keys in a column of their own
-- in the tertiary ink, values beside them; a value may be a node (a badge,
-- a person). `copy` puts a copy button at a value's end, shown under the
-- pointer.
export type Pair = { key: string | Message, value: string | Node, mono: boolean?, copy: boolean? }
export type KeyValueProps = { items: { Pair }, key_width: number?, dense: boolean? }

local Pair = ui.component(function(props: { pair: Pair, key_width: number, dense: boolean? }): Node
	local pair = props.pair
	local over = ui.use_state(false)
	local copied = ui.use_state(false)
	local value: Node
	if type(pair.value) == "string" then
		value = SelectionMenu { grow = 1, min_w = 0, FadeText {
			pair.value,
			w_full = true,
			text_size = if pair.mono then "$font.size.sm" else "$font.size.md",
			font = if pair.mono then "$font.mono" else nil,
			color = "$text.primary",
		} }
	else
		value = ui.row { grow = 1, min_w = 0, items = "center", pair.value :: Node }
	end
	return ui.row {
		w_full = true,
		min_h = if props.dense then 28 else 32,
		gap = "$space.3",
		items = "center",
		on_hover = function(inside: boolean)
			over:set(inside)
		end,
		ui.text { pair.key, w = props.key_width, shrink = 0, text_size = "$font.size.md", color = "$text.tertiary" },
		value,
		if pair.copy and type(pair.value) == "string"
			then ui.div {
				shrink = 0,
				opacity = ui.tween(over, { duration = 120 }),
				ui.button {
					label = locale.t("common.copy"),
					w = 22,
					h = 22,
					px = 0,
					py = 0,
					justify = "center",
					rounded = "$radius.md",
					bg = "#00000000",
					color = "$text.tertiary",
					cursor = "pointer",
					hover = { bg = "$hover", color = "$text.primary" },
					focus = control.ring,
					on_click = function()
						ui.clipboard.write(pair.value :: string)
						copied:set(true)
						ui.after(1600, function()
							copied:set(false)
						end)
					end,
					Icon {
						name = function()
							return if copied:get() then "check" else "copy"
						end,
						size = 14,
					},
				},
			}
			else nil,
	}
end)

function M.KeyValue(props: KeyValueProps): Node
	local rows: { Node } = {}
	for _, pair in props.items do
		table.insert(rows, Pair { pair = pair, key_width = props.key_width or 96, dense = props.dense })
	end
	return ui.column { w_full = true, role = "list", table.unpack(rows) }
end

return M
