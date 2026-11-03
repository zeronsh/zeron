-- Badges, tags and pills: small labels that say what something is or how
-- it stands. One look under them all, the sidebar's pull request badge
-- generalised: a colour's ink on a faint wash of the same colour, so any
-- colour makes one (a tone's, a pull request's state, a file's git status)
-- and it reads on glass and opaque, light and dark.
--
--   Badge { "Beta", tone = "accent" }            a label on a tone
--   Badge { "Merged", color = "$zeron.pr.merged", icon = "git-merge" }
--   Badge { "3", tone = "danger", look = "solid", shape = "pill" }
--   Tag { "frontend", on_remove = … }            a chip that can go
--   Count { n = 12 }                              a number in a pill
--   Dot { tone = "success" }                      a state's dot
--
-- And the git pages' badges, built on Badge: `PullRequest`, `Review`,
-- `Checks`, `FileStatus` and `DiffStat`.

local Icon = require("@self/icon")
local control = require("@self/control")
local locale = require("@self/locale")
local FadeText = require("@self/fade_text")

local M = {}

export type Tone = "neutral" | "accent" | "info" | "success" | "warning" | "danger"
-- `soft`: ink on its wash (the default); `solid`: the colour filled, white
-- ink (a count that must be seen); `outline`: ink in a ring of it, no wash
-- (beside other badges, a quieter kind).
export type Look = "soft" | "solid" | "outline"
export type Size = "sm" | "md"

export type Props = {
	[number]: (string | Message | () -> string)?,
	tone: Tone?,
	-- Any colour token in place of a tone's (a pull request's state).
	color: string?,
	look: Look?,
	size: Size?,
	shape: ("rounded" | "pill")?,
	icon: (Icon.Name | () -> Icon.Name)?,
	-- A dot before the label in its colour (a live state).
	dot: boolean?,
	-- Numbers that change hold still (a count, a PR's number).
	tabular: boolean?,
	mono: boolean?,
	tooltip: string?,
	-- What finds it (a test, another plugin).
	class: string?,
	-- How long the pointer rests before its tooltip shows (ms).
	tooltip_delay: number?,
	-- The most it may take (points): past it, its label fades at its end.
	max_w: number?,
	label: Label?,
	-- Lit (a badge that's a link, under the pointer): a colour's wash and
	-- ink stronger.
	lit: (() -> boolean)?,
}

-- How strong a badge's ink is on any colour (the sidebar's pull request
-- badge's): for what draws a state's colour as a badge would, outside one
-- (a hover card's pull request row).
M.INK = "$zeron.badge.ink"

-- Each size: its height, padding, gap, type and glyph.
local SIZE = {
	-- (Small: the sidebar's pull request badge, its 3-point gap kept.)
	sm = { h = 16, px = "$space.1", gap = ui.raw(3, "the sidebar badge's glyph gap"), text = "$font.size.xs", icon = 11, radius = "$radius.sm" },
	md = { h = 20, px = "$zeron.space.tight", gap = "$space.1", text = "$font.size.sm", icon = 12, radius = "$radius.md" },
}

-- A solid badge's fill, by tone.
local SOLID = {
	neutral = "$text.tertiary",
	accent = "$accent",
	info = "$palette.blue.600",
	success = "$palette.emerald.600",
	warning = "$palette.amber.600",
	danger = "$palette.red.600",
}

-- The ink of a badge: a tone's, or the colour given.
local function ink_of(props: Props): string
	if props.color then
		return props.color
	end
	return `$zeron.tone.{props.tone or "neutral"}.ink`
end

function M.Badge(props: Props): Node
	local size = SIZE[props.size or "sm"]
	local look: Look = props.look or "soft"
	local ink = ink_of(props)
	local radius = if props.shape == "pill" then "$radius.full" else size.radius
	local text = props[1]
	local children: { any } = {}
	-- Soft: the wash under it, its colour at the theme's strength (a
	-- tone's own wash where it has one).
	if look == "soft" then
		if props.color or props.tone == nil or props.tone == "neutral" then
			table.insert(children, ui.div {
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				rounded = radius,
				bg = if props.color then props.color else "$text.primary",
				opacity = if props.color and props.lit
					then function(): string
						return if (props.lit :: () -> boolean)() then "$zeron.badge.lit.wash" else "$zeron.badge.wash"
					end
					elseif props.color then "$zeron.badge.wash"
					else 0.07,
				pointer_events = "none",
			})
		else
			table.insert(children, ui.div {
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				rounded = radius,
				bg = `$zeron.tone.{props.tone}.wash`,
				pointer_events = "none",
			})
		end
	end
	local fg = if look == "solid" then "$zeron.check.mark" else ink
	if props.dot then
		table.insert(children, ui.div { w = 6, h = 6, shrink = 0, rounded = "$radius.full", bg = fg })
	end
	if props.icon then
		table.insert(children, ui.div { shrink = 0, y = -0.5, Icon { name = props.icon, size = size.icon } })
	end
	if text then
		if props.max_w then
			-- (Held to a width: its label fades at its end where it runs
			-- long, as a list row's words do, never "…".)
			table.insert(children, FadeText {
				text,
				shrink = 1,
				min_w = 0,
				text_size = size.text,
				weight = "$font.weight.medium",
				tabular = props.tabular,
				font = if props.mono then "$font.mono" else nil,
			})
		else
			table.insert(children, ui.text {
				text,
				text_size = size.text,
				weight = "$font.weight.medium",
				tabular = props.tabular,
				font = if props.mono then "$font.mono" else nil,
			})
		end
	end
	return ui.row {
		shrink = 0,
		relative = true,
		h = size.h,
		min_w = size.h,
		px = if text then size.px else 0,
		justify = "center",
		gap = size.gap,
		items = "center",
		rounded = radius,
		color = fg,
		opacity = if look == "soft" and props.color and props.lit
			then function(): string
				return if (props.lit :: () -> boolean)() then "$zeron.badge.lit.ink" else "$zeron.badge.ink"
			end
			elseif look == "soft" and props.color then "$zeron.badge.ink"
			else nil,
		-- Solid fills with the tone's own colour, not its ink (the pale
		-- inks of dark would wash out the white on them).
		bg = if look == "solid" then (props.color or SOLID[props.tone or "neutral"]) else nil,
		border = if look == "outline" then 1 else nil,
		border_color = if look == "outline" then ink else nil,
		class = props.class,
		max_w = props.max_w,
		tooltip = props.tooltip,
		tooltip_delay = props.tooltip_delay,
		label = props.label,
		table.unpack(children),
	}
end

-- A tag: a neutral chip (a label on a pull request, a filter applied),
-- with a × to remove it when it can go. `selected` lights it (a filter
-- that is on): the flat selected wash and a ring, never a shadow.
export type TagProps = {
	[number]: string,
	icon: Icon.Name?,
	-- A palette hue for a dot before it (a label's colour).
	hue: string?,
	selected: (boolean | () -> boolean)?,
	-- Something's wrong with it (a token that doesn't parse): the danger's.
	invalid: boolean?,
	on_click: (() -> ())?,
	on_remove: (() -> ())?,
	size: Size?,
}

function M.Tag(props: TagProps): Node
	local sm = props.size == "sm"
	local h = if sm then 20 else 24
	local selected = props.selected
	local function on(): boolean
		return if type(selected) == "function" then selected() else selected == true
	end
	local remove: Node? = nil
	if props.on_remove then
		remove = ui.button {
			label = locale.t("ds.badge.remove", { name = props[1] }),
			w = h - 6,
			h = h - 6,
			px = 0,
			py = 0,
			-- As far from the chip's end as from its top: past its own padding.
			mr = ui.raw(if sm then -4 else -6, "the × in the chip's corner"),
			justify = "center",
			-- (Concentric with the tag: a token tokens.luau derives.)
			rounded = "$zeron.tag.remove_radius",
			bg = "#00000000",
			color = "$text.tertiary",
			cursor = "pointer",
			hover = { bg = "$hover", color = "$text.primary" },
			active = { bg = "$pressed", color = "$text.primary" },
			focus = control.ring,
			on_click = props.on_remove,
			Icon { name = "close", size = if sm then 10 else 12 },
		}
	end
	return ui.row {
		shrink = 0,
		h = h,
		px = if sm then "$zeron.space.tight" else "$space.2",
		gap = "$space.1",
		items = "center",
		rounded = "$radius.md",
		border = 1,
		role = if props.on_click then "button" else nil,
		focusable = props.on_click ~= nil,
		cursor = if props.on_click then "pointer" else nil,
		on_click = props.on_click,
		focus = if props.on_click then control.ring else nil,
		bg = function()
			if props.invalid then
				return "$zeron.tone.danger.wash"
			end
			return if on() then "$zeron.selected.flat.bg" else "$zeron.secondary.bg"
		end,
		border_color = function()
			if props.invalid then
				return "$zeron.tone.danger.edge"
			end
			return if on() then "$zeron.selected.ring" else "#00000000"
		end,
		hover = if props.on_click then { bg = "$zeron.secondary.hover" } else nil,
		color = function()
			if props.invalid then
				return "$zeron.tone.danger.ink"
			end
			return if on() then "$text.primary" else "$text.secondary"
		end,
		if props.hue then ui.div { w = 8, h = 8, rounded = "$radius.full", shrink = 0, bg = `$palette.{props.hue}.500` } else nil,
		if props.icon then ui.div { shrink = 0, y = -0.5, Icon { name = props.icon, size = if sm then 12 else 14 } } else nil,
		ui.text { props[1], text_size = if sm then "$font.size.xs" else "$font.size.sm", weight = "$font.weight.medium" },
		remove,
	}
end

-- A count in a pill: quiet by default (a tab's, a section's), a tone's
-- solid fill to be seen (unread, failing). Past `max` it says `max+`.
export type CountProps = { n: number | () -> number, tone: Tone?, solid: boolean?, max: number? }

function M.Count(props: CountProps): Node
	local n = props.n
	local max = props.max or 99
	return M.Badge {
		function()
			local v = if type(n) == "function" then n() else n
			return if v > max then `{max}+` else tostring(v)
		end,
		tone = props.tone,
		look = if props.solid then "solid" else "soft",
		shape = "pill",
		tabular = true,
	}
end

-- A state's dot: a tone's ink (or a colour), and a soft halo while
-- `pulse` says it is live.
export type DotProps = { tone: Tone?, color: string?, size: number?, pulse: boolean?, label: string? }

function M.Dot(props: DotProps): Node
	local size = props.size or 8
	local ink = if props.color then props.color else `$zeron.tone.{props.tone or "neutral"}.ink`
	return ui.div {
		w = size,
		h = size,
		shrink = 0,
		relative = true,
		label = props.label,
		role = if props.label then "img" else nil,
		if props.pulse
			then ui.div {
				absolute = true,
				left = 0,
				top = 0,
				w = size,
				h = size,
				rounded = "$radius.full",
				bg = ink,
				scale = ui.loop { from = 1, to = 2.2, duration = 1600, yoyo = false, fps = 30 },
				opacity = ui.loop { from = 0.45, to = 0, duration = 1600, yoyo = false, fps = 30 },
			}
			else nil,
		ui.div { absolute = true, left = 0, top = 0, w = size, h = size, rounded = "$radius.full", bg = ink },
	}
end

-- A pull request's state, its glyph and number (or its state's name),
-- in the state's colour: open, merged, closed, draft. (Each name its
-- message's key.)
export type PrState = "open" | "merged" | "closed" | "draft"
local PR: { [PrState]: { icon: Icon.Name, label: string } } = {
	open = { icon = "pull-request", label = "ds.badge.pr.open" },
	merged = { icon = "git-merge", label = "ds.badge.pr.merged" },
	closed = { icon = "pull-request-closed", label = "ds.badge.pr.closed" },
	draft = { icon = "pull-request-draft", label = "ds.badge.pr.draft" },
}

function M.PullRequest(props: { state: PrState, number: number?, size: Size?, title: string?, lit: (() -> boolean)? }): Node
	local s = PR[props.state] or PR.open
	local state = if PR[props.state] then props.state else "open"
	return M.Badge {
		if props.number then tostring(props.number) else locale.t(s.label),
		color = `$zeron.pr.{props.state}`,
		icon = s.icon,
		size = props.size,
		tabular = props.number ~= nil,
		tooltip = if props.title then locale.s("ds.badge.pr.tooltip", { state = locale.s(s.label), title = props.title }) else nil,
		label = if props.number
			then locale.t("ds.badge.pr.label", { n = tostring(props.number), state = state })
			else locale.t(s.label),
		lit = props.lit,
	}
end

-- A review's verdict, as a chip.
export type ReviewState = "approved" | "changes" | "commented" | "pending"
local REVIEW: { [ReviewState]: { icon: Icon.Name, label: string } } = {
	approved = { icon = "check-circle", label = "ds.badge.review.approved" },
	changes = { icon = "danger-triangle", label = "ds.badge.review.changes" },
	commented = { icon = "chat-round-line", label = "ds.badge.review.commented" },
	pending = { icon = "clock-circle", label = "ds.badge.review.pending" },
}

function M.Review(props: { state: ReviewState, size: Size? }): Node
	local s = REVIEW[props.state] or REVIEW.pending
	return M.Badge { locale.t(s.label), color = `$zeron.review.{props.state}`, icon = s.icon, size = props.size or "md" }
end

-- A commit's checks: passing, failing or running, with how many passed.
export type ChecksState = "passing" | "failing" | "running"
local CHECKS: { [ChecksState]: { icon: Icon.Name, label: string } } = {
	passing = { icon = "check-circle", label = "ds.badge.checks.passing" },
	failing = { icon = "close-circle", label = "ds.badge.checks.failing" },
	running = { icon = "clock-circle", label = "ds.badge.checks.running" },
}

function M.Checks(props: { state: ChecksState, passed: number?, total: number?, size: Size? }): Node
	local s = CHECKS[props.state] or CHECKS.running
	local text: string | Message = if props.total then `{props.passed or 0}/{props.total}` else locale.t(s.label)
	return M.Badge {
		text,
		color = `$zeron.checks.{props.state}`,
		icon = s.icon,
		size = props.size or "md",
		tabular = props.total ~= nil,
		tooltip = if props.total then locale.s(s.label) else nil,
		label = locale.t(s.label),
	}
end

-- A file's git status: its letter in its colour (a list's column, as an
-- editor's tree shows it), or `look = "badge"` its name on a wash.
export type FileState = "added" | "modified" | "deleted" | "renamed" | "untracked" | "conflict"
local FILE: { [FileState]: { letter: string, label: string } } = {
	added = { letter = "A", label = "ds.badge.file.added" },
	modified = { letter = "M", label = "ds.badge.file.modified" },
	deleted = { letter = "D", label = "ds.badge.file.deleted" },
	renamed = { letter = "R", label = "ds.badge.file.renamed" },
	untracked = { letter = "U", label = "ds.badge.file.untracked" },
	conflict = { letter = "!", label = "ds.badge.file.conflict" },
}

function M.FileStatus(props: { status: FileState, look: ("letter" | "badge")? }): Node
	local s = FILE[props.status] or FILE.modified
	if props.look == "badge" then
		return M.Badge { locale.t(s.label), color = `$zeron.git.{props.status}` }
	end
	return ui.text {
		s.letter,
		shrink = 0,
		w = 14,
		align = "center",
		font = "$font.mono",
		text_size = "$font.size.sm",
		weight = "$font.weight.medium",
		color = `$zeron.git.{props.status}`,
		-- (zgx reads no `label` on a text: the tooltip names it.)
		tooltip = locale.s(s.label),
	}
end

-- A diff's size: what it adds and takes away (+12 −3, a real minus), and
-- with `bar`, five blocks shared between them in proportion, as a list's
-- row shows a change's weight. Big numbers shorten (1.2k).
local function short(n: number): string
	if n >= 10000 then
		return `{math.floor(n / 1000)}k`
	elseif n >= 1000 then
		return string.format("%.1fk", n / 1000)
	end
	return tostring(n)
end

function M.DiffStat(props: { added: number, removed: number, bar: boolean?, size: Size? }): Node
	local sm = (props.size or "sm") == "sm"
	local text = if sm then "$font.size.xs" else "$font.size.sm"
	local bar: Node? = nil
	if props.bar then
		local total = props.added + props.removed
		local adds = if total == 0 then 0 else math.floor(props.added / total * 5 + 0.5)
		local dels = if total == 0 then 0 else math.min(5 - adds, math.floor(props.removed / total * 5 + 0.5))
		local blocks: { Node } = {}
		for i = 1, 5 do
			table.insert(blocks, ui.div {
				w = 6,
				h = 6,
				rounded = ui.raw(1.5, "a 6-point block's corner"),
				bg = if i <= adds then "$zeron.diff.added" elseif i <= adds + dels then "$zeron.diff.removed" else "$border.strong",
			})
		end
		bar = ui.row { gap = ui.raw(1.5, "between a diff bar's blocks"), items = "center", aria_hidden = true, table.unpack(blocks) }
	end
	return ui.row {
		shrink = 0,
		gap = "$zeron.space.tight",
		items = "center",
		label = locale.t("ds.badge.diff", { added = tostring(props.added), removed = tostring(props.removed) }),
		ui.text { `+{short(props.added)}`, font = "$font.mono", text_size = text, tabular = true, color = "$zeron.diff.added" },
		ui.text { `−{short(props.removed)}`, font = "$font.mono", text_size = text, tabular = true, color = "$zeron.diff.removed" },
		bar,
	}
end

return M
