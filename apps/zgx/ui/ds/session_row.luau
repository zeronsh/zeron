--!strict
-- A session's row, wherever sessions are listed: the left sidebar's
-- (ds/sidebar) and the panel home's side chats and subagents
-- (ds/panel_home). One look, from one place: a theme, a template or a
-- plugin changing it changes both.
--
-- What a row is made of (ds/sidebar builds its own frame round them, for
-- its rail and its drag; any other list takes `Row`, `Header` and `Pager`
-- whole):
--
--   SessionRow.line(text, ...)            a compact row's line: its words and
--                                         what sits at its end
--   SessionRow.label(text, strong?)       a title, faded where it's cut
--   SessionRow.hint(text)                 a quiet note ("⌘N", a time, a count)
--   SessionRow.mark(cur, strong)          its harness's mark, faint at rest
--   SessionRow.state(kind)                its state's glyph, in its hue
--   SessionRow.standing(cur)              its corner: its state, else its time
--   SessionRow.Corner { cur, hovered, jump?, action? }
--                                         the corner as a row wears it: its
--                                         action under the pointer, ⌘N while
--                                         ⌘ is held, its width kept
--   SessionRow.pr_badge(pr)               its pull request
--   SessionRow.subagents(n)               subagents running under it
--   SessionRow.on_tree(body, tree, lead)  under a group: a branch off the
--                                         group's trunk
--   SessionRow.pager_lead(whole)          "Show N more"'s chevron
--   SessionRow.pager_words(says)          and its words
--   SessionRow.header_words(name, count, folded, trail?)
--                                         a group header's name, and folded
--                                         how many it holds
--   SessionRow.tile(inside, badge?)       a group's tile, a badge for the most
--                                         pressing while its rows are hidden
--
--   SessionRow.Row { … }                  a whole row, for lists outside the
--   SessionRow.Header { … }               sidebar (the sidebar's frame, at a
--   SessionRow.Pager { … }                list's own width)
--   SessionRow.Fold { open, h, body }     a group's rows, opening and closing
--                                         as the sidebar's do
--
-- A row's session (`cur`) is read through a function, so what it shows
-- follows the session in place and the row keeps its node.

local Icon = require("@self/icon")
local Mark = require("@self/mark")
local AgentMark = require("@self/agent_mark")
local status = require("@self/status")
local FadeText = require("@self/fade_text")
local Surface = require("@self/surface")
local Tile = require("@self/tile")
local tree_line = require("@self/tree_line")
local icon_button = require("@self/icon_button")
local motion = require("@self/motion")
local ContextMenu = require("@self/context_menu")
local locale = require("@self/locale")

local M = {}

-- A compact row's height (its leading slot's), a regular one's (two
-- lines), and how far an icon beside a name sits from its line's middle
-- (an icon is taller than a name's capitals: centred, it reads low).
M.SLOT, M.TALL, M.OPTICAL = 30, 46, -0.5

-- The session a row shows, as far as it reads it.
export type Session = {
	id: string,
	title: string,
	status: status.Kind?,
	time: (number | string)?,
	agent: Mark.Name?,
	-- An agent from the ACP registry: its harness (`acp.<id>`) and icon,
	-- drawn in its colours (ds.AgentMark) where `agent` is a built-in mark.
	harness: string?,
	agent_icon: string?,
	device: string?,
	branch: string?,
	subagents: number?,
	pr: { number: number, state: string?, title: string? }?,
}

function M.line(text: Node, ...: Node?): Node
	local parts: ContainerProps = {
		w_full = true,
		h = M.SLOT,
		pr = "$space.2",
		gap = "$space.2",
		items = "center",
		text,
	}
	-- (Each in order: a nil, one not shown, leaves no gap.)
	for i = 1, select("#", ...) do
		local node = select(i, ...)
		if node then
			table.insert(parts, node)
		end
	end
	return ui.row(parts)
end

-- (`ink`: its colour at rest, where a list's titles read in another, a
-- settings page's in the primary.)
function M.label(text: string | () -> string, strong: (() -> boolean)?, weight: string?, class: string?, ink: string?): Node
	return FadeText {
		text,
		class = class,
		grow = 1,
		text_size = "$font.size.md",
		weight = function()
			if weight then
				return weight
			end
			return if strong and strong() then "$font.weight.medium" else "$font.weight.regular"
		end,
		color = function()
			return if strong == nil or strong() then "$text.primary" else ink or "$text.secondary"
		end,
	}
end

function M.hint(text: string | () -> string): Node
	return ui.text { text, shrink = 0, text_size = "$font.size.xs", color = "$text.tertiary", tabular = true }
end

-- Its harness's mark, faint unless the session asks to be seen; an agent
-- from the ACP registry's, in its colours; none, a faint dot.
function M.mark(cur: () -> Session, strong: () -> boolean): Node
	-- (Keyed by the agent and whether its icon is known: built again only
	-- when either changes.)
	local function registry(): string
		local c = cur()
		local h = c.harness or ""
		if (c.agent or "") ~= "" or string.sub(h, 1, 4) ~= "acp." then
			return ""
		end
		return if c.agent_icon then `{h}\0icon` else h
	end
	return ui.div {
		opacity = function(): number
			return if strong() then 0.9 else 0.5
		end,
		ui.show(
			function(): boolean
				return registry() ~= ""
			end,
			ui.presence(registry, function(key: string): Node
				if key == "" then
					return ui.div {}
				end
				local harness = string.match(key, "^[^%z]+") or key
				return AgentMark { label = harness, seed = harness, icon = cur().agent_icon, size = 14, bare = true }
			end)
		),
		ui.show(
			function(): boolean
				return registry() == "" and (cur().agent or "") ~= ""
			end,
			Mark {
				name = function(): Mark.Name
					return (cur().agent or "claude") :: Mark.Name
				end :: any,
				size = 14,
				bare = true,
			},
			ui.show(function(): boolean
				return registry() == ""
			end, ui.div { w = 5, h = 5, rounded = "$radius.full", bg = "$text.disabled" })
		),
	}
end

-- A state, small: its glyph alone, in its hue (a state with no glyph, its
-- dot), its name a tooltip.
function M.state(kind: status.Kind): Node
	local k = status.KINDS[kind]
	return ui.div {
		shrink = 0,
		w = 16,
		h = 16,
		items = "center",
		justify = "center",
		color = `$zeron.status.{kind}`,
		tooltip = k.long,
		label = k.long,
		if k.glyph then status.StatusIcon { kind = kind, size = 14 } else status.StatusDot { kind = kind, size = 6 },
	}
end

-- When it last moved, as a corner reads it.
function M.when(s: Session): string
	local t = s.time
	if type(t) == "number" then
		return status.ago(math.max(0, os.time() - t))
	end
	return t or ""
end

-- Its corner at rest: its state's glyph, else how long ago it moved.
function M.standing(cur: () -> Session): Node
	return ui.presence(function(): string
		return cur().status or "time"
	end, function(kind: string): Node
		if kind == "time" then
			return M.hint(function(): string
				return M.when(cur())
			end)
		end
		return M.state(kind :: status.Kind)
	end)
end

-- A small quiet icon button in a row's corner (`key`: its name's message).
function M.corner_button(key: string, icon: Icon.Name, on_click: (() -> ())?, args: { [string]: any }?): Node
	local button: icon_button.Props = {
		icon = icon,
		label = locale.t(key, args),
		tooltip = locale.s(key, args),
		size = "sm",
		tone = "quiet",
		on_click = on_click,
	}
	return icon_button.IconButton(button)
end

export type CornerProps = {
	cur: () -> Session,
	hovered: State<boolean>,
	-- Its jump (⌘1–9) while ⌘ is held alone; nil, none.
	jump: (() -> number?)?,
	-- What the pointer finds there (archive, unarchive); nil, the corner
	-- stays as it is under the pointer.
	action: Node?,
}

-- The corner as a row wears it: its state (or time) at rest; under the
-- pointer, its action, its glyph on the state's own centre; ⌘ held alone,
-- its jump's keys over both (as Zeron's). What's at rest stays in place,
-- unseen, so its width holds and the title and badges before it never
-- shift.
function M.Corner(props: CornerProps): Node
	local hovered = props.hovered
	local jump = props.jump or function(): number?
		return nil
	end
	local action = props.action
	return ui.row {
		shrink = 0,
		relative = true,
		min_w = 16,
		h = 16,
		items = "center",
		justify = "end",
		ui.div {
			opacity = function(): number
				return if (action ~= nil and hovered:get()) or jump() ~= nil then 0 else 1
			end,
			pointer_events = "none",
			M.standing(props.cur),
		},
		ui.show(
			function(): boolean
				return jump() ~= nil
			end,
			ui.div {
				class = "jump-hint",
				absolute = true,
				right = 0,
				top = 0,
				h = 16,
				items = "center",
				pointer_events = "none",
				M.hint(function(): string
					return `⌘{jump() or ""}`
				end),
			}
		),
		if action
			then ui.show(
				function(): boolean
					return hovered:get() and jump() == nil
				end,
				-- (The button 3 points past the corner's 16 either side.)
				ui.div { absolute = true, right = -3, top = -3, action }
			)
			else nil,
	}
end

-- A pull request's badge, as Zeron's: its glyph and number in its state's
-- colour, on a wash of it; its state and title a tooltip.
function M.pr_badge(pr: { number: number, state: string?, title: string? }): Node
	local state = pr.state or "open"
	local hue = `$zeron.pr.{state}`
	local known = state == "open" or state == "merged" or state == "closed" or state == "draft"
	local named = locale.s(`ds.sidebar.pr.{if known then state else "open"}`)
	return ui.row {
		shrink = 0,
		relative = true,
		h = 16,
		px = "$space.1",
		gap = "$zeron.space.hair",
		items = "center",
		rounded = "$radius.sm",
		tooltip = if pr.title
			then locale.s("ds.sidebar.pr.tooltip", { state = named, title = pr.title })
			else locale.s("ds.sidebar.pr.tooltip_untitled", { state = named }),
		label = locale.t("ds.sidebar.pr.label", { n = tostring(pr.number), state = state }),
		ui.div { absolute = true, left = 0, right = 0, top = 0, bottom = 0, rounded = "$radius.sm", bg = hue, opacity = 0.1 },
		ui.div {
			color = hue,
			opacity = 0.85,
			Icon { name = if state == "draft" then "pull-request-draft" else "pull-request", size = 11 },
		},
		ui.text { tostring(pr.number), text_size = "$font.size.xs", weight = "$font.weight.medium", color = hue, tabular = true, opacity = 0.85 },
	}
end

-- Subagents running under it (Zeron's running_pill.rs): its working dot
-- and how many (99+ past that), on a wash of the working hue; none, nothing.
function M.subagents(n: () -> number): Node
	return ui.show(
		function(): boolean
			return n() > 0
		end,
		ui.row {
			shrink = 0,
			relative = true,
			h = 16,
			px = "$space.1",
			gap = "$space.1",
			items = "center",
			rounded = "$radius.full",
			label = function(): string
				return locale.s("ds.sidebar.subagents_running", { n = n() })
			end,
			ui.div { absolute = true, left = 0, right = 0, top = 0, bottom = 0, rounded = "$radius.full", bg = "$zeron.status.working", opacity = 0.1 },
			status.StatusDot { kind = "working", size = 5 },
			ui.text {
				function(): string
					local c = n()
					return if c > 99 then "99+" else tostring(c)
				end,
				text_size = "$font.size.xs",
				weight = "$font.weight.medium",
				color = "$zeron.status.working",
				tabular = true,
			},
		}
	)
end

-- A group's tree, as Zeron's transcript draws its tools: a hairline trunk
-- down from under the group's tile (`trunk`: how far in), and at each row
-- a rounded elbow off it toward the row. `last`: the trunk ends there.
-- `through`: outer groups' trunks going on past this row (with two levels
-- of groups, the outer one's, past an inner group's rows), by x.
export type Tree = { last: () -> boolean, trunk: () -> number, through: (() -> { number })? }
-- The elbow's radius, how far the gutter runs past it, and the break
-- between the branch's tip and what starts the row (Zeron's 6, 0, 4).
M.BEND, M.REACH, M.BREAK = 6, 0, 4
function M.gutter(t: Tree): number
	return t.trunk() + M.BEND + M.REACH
end
-- A row under its group's header: the branch off the trunk in the gutter
-- at its start, drawn over its wash. `lead_at`: where the row's mark (or
-- its words) starts past the gutter; `gap`: the list's gap under it, the
-- trunk running through it to the next row unbroken.
function M.on_tree(body: Node, now_tree: () -> Tree?, lead_at: () -> number, gap: number?): Node
	-- (A tree with no trunk now draws nothing: a row out of its group.)
	local none: Tree = { trunk = function(): number
		return 0
	end, last = function(): boolean
		return true
	end }
	local function current(): Tree
		return now_tree() or none
	end
	local t = {
		trunk = function(): number
			return current().trunk()
		end,
		last = function(): boolean
			return current().last()
		end,
		through = function(): { number }
			local th = current().through
			return if th then th() else {}
		end,
	} :: Tree
	return ui.div {
		relative = true,
		w_full = true,
		shrink = 0,
		body,
		ui.canvas {
			absolute = true,
			left = 0,
			hidden = function(): boolean
				return now_tree() == nil
			end,
			top = 0,
			bottom = -(gap or 0),
			w = function(): number
				return M.gutter(t) + lead_at()
			end,
			pointer_events = "none",
			draw = function(c, _, h)
				for _, tx in (t.through :: () -> { number })() do
					c:fill(tree_line.trunk(tx, h), "$zeron.tree")
				end
				local x = t.trunk()
				-- The branch meets the title's line where it reads (a compact
				-- row's middle), the trunk going on below unless it's last.
				c:fill(tree_line.branch {
					x = x,
					mid = M.SLOT / 2 + M.OPTICAL,
					tip = M.gutter(t) + lead_at() - M.BREAK,
					h = h,
					bend = M.BEND,
					on = not t.last(),
				}, "$zeron.tree")
			end,
		},
	}
end

-- How sessions are grouped (the sidebar's Organize): its levels in order,
-- outermost first ("project", "device"), as a list; none, one list. Kept
-- as a key where a string's wanted ("project>device"; "none"). An old
-- single choice ("project", "device", "none") reads as its one level.
M.LEVELS = { project = true, device = true }
function M.levels(v: any): { string }
	local out: { string } = {}
	local seen: { [string]: boolean } = {}
	local list: { any } = if type(v) == "table" then v elseif type(v) == "string" then string.split(v, ">") else {}
	for _, l in list do
		if type(l) == "string" and M.LEVELS[l] and not seen[l] then
			seen[l] = true
			table.insert(out, l)
		end
	end
	return out
end
function M.organize_key(v: any): string
	local levels = M.levels(v)
	return if #levels == 0 then "none" else table.concat(levels, ">")
end
-- Whether `by` groups by `level`.
function M.groups_by(by: any, level: string): boolean
	return table.find(M.levels(by), level) ~= nil
end

-- Where a session runs, as its second line says it: what its groups don't
-- (by project, its device; by device, its project; by both, nothing; one
-- list, both).
function M.where(project: string?, device: string?, by: any): string?
	local p = if M.groups_by(by, "project") then nil else project
	local d = if M.groups_by(by, "device") then nil else device
	if p and d then
		return `{p} · {d}`
	end
	return p or d
end
-- Regular's second line: where it runs (`location` shown), then its branch
-- (`branch_shown`).
function M.detail(project: string?, device: string?, branch: string?, by: any, location: boolean, branch_shown: boolean): string
	local parts: { string } = {}
	local where = M.where(project, device, by)
	if where and location then
		table.insert(parts, where)
	end
	if branch and branch_shown then
		table.insert(parts, branch)
	end
	return table.concat(parts, " · ")
end

-- Regular's second line: where it runs, and on what (its branch's glyph
-- leading while `branch` shows); there while `shown` (regular, with
-- something to say), faded in and out (`wrap`: the caller's fade, the
-- sidebar's with its rail). It sits `PULL` up under the title.
M.PULL = -8
function M.second_line(
	text: () -> string,
	branch: () -> boolean,
	shown: () -> boolean,
	wrap: ((ContainerProps, () -> boolean) -> ContainerProps)?
): Node
	local props: ContainerProps = {
		w_full = true,
		h = 14,
		pr = "$space.2",
		gap = "$space.1",
		items = "center",
		ui.show(branch, ui.div { color = "$text.tertiary", Icon { name = "git-branch", size = 11 } }),
		FadeText {
			text,
			grow = 1,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		},
	}
	if wrap then
		props = wrap(props, shown)
	else
		props.opacity = motion.tween(ui.computed(function(): number
			return if shown() then 1 else 0
		end), { duration = 160 })
	end
	return ui.row(props)
end

-- "Show N more" / "Show less": its chevron where a row's mark stands, its
-- words quiet.
function M.pager_lead(whole: () -> boolean): Node
	return ui.div {
		color = "$text.tertiary",
		ui.presence(function(): string
			return if whole() then "alt-arrow-up" else "alt-arrow-down"
		end, function(name: string): Node
			return Icon { name = name :: Icon.Name, size = 12 }
		end),
	}
end
function M.pager_words(says: () -> string): Node
	return M.line(FadeText {
		says,
		grow = 1,
		text_size = "$font.size.sm",
		color = "$text.tertiary",
	})
end
-- What a pager says: its group's rest to show, or showing them.
function M.pager_says(whole: () -> boolean, more: () -> number): () -> string
	return function(): string
		if whole() then
			return locale.s("ds.sidebar.show_less")
		end
		return locale.s("ds.sidebar.show_more", { n = more() })
	end
end

-- A group header's words: its name, and folded, how many it holds (in
-- `trail`'s place while that shows: a project's new chat under the
-- pointer).
function M.header_words(name: () -> string, count: () -> string, folded: () -> boolean, trail: Node?, trail_shown: (() -> boolean)?): Node
	return M.line(
		FadeText {
			name,
			grow = 1,
			text_size = "$font.size.md",
			weight = "$font.weight.medium",
			color = "$text.primary",
		},
		ui.row {
			shrink = 0,
			items = "center",
			ui.show(function(): boolean
				return folded() and not (trail_shown ~= nil and trail_shown())
			end, M.hint(count)),
			if trail and trail_shown then ui.show(trail_shown, trail) else trail,
		}
	)
end

export type SectionProps = {
	title: string | Message | () -> string,
	-- Folds (a click on it; folded, how many it holds at its end).
	open: (() -> boolean)?,
	count: (() -> number)?,
	on_toggle: (() -> ())?,
	id: string?,
	-- Where it stands, from its list (the sidebar's: laid over its slot).
	frame: { [string]: any }?,
	-- Its row's props, wrapped before it's made (the sidebar fades it
	-- with its rail).
	wrap: ((ContainerProps) -> ContainerProps)?,
}

-- A section's title, over its groups (Pinned, a section of yours, the
-- sidebar's "Projects"; the panel home's "Side chats"): quiet small words,
-- brighter under the pointer, no caret; folded, how many it holds.
M.TITLE = 24
function M.Section(props: SectionProps): Node
	local open = props.open
	local foldable = open ~= nil and props.on_toggle ~= nil
	local row: { [any]: any } = {
		id = props.id,
		h = M.TITLE,
		pl = "$space.2",
		pr = "$space.2",
		items = "center",
		color = "$text.tertiary",
		cursor = if foldable then "pointer" else nil,
		hover = if foldable then { color = "$text.secondary" } else nil,
		on_click = if foldable then props.on_toggle else nil,
		ui.text {
			props.title,
			grow = 1,
			text_size = "$font.size.xs",
			weight = "$font.weight.medium",
		},
		if foldable and props.count
			then ui.show(function(): boolean
				return not (open :: () -> boolean)()
			end, M.hint(function(): string
				return tostring((props.count :: () -> number)())
			end))
			else nil,
	}
	for k, v in props.frame or { w_full = true } do
		row[k] = v
	end
	if props.wrap then
		row = props.wrap(row :: any) :: any
	end
	return ui.row(row)
end

-- A group's tile (18, in a row's slot): `inside` on its hue (or the
-- chrome's quiet wash), and (`badge`) a dot for its most pressing session
-- while its rows can't be seen.
function M.tile(inside: Node, badge: (() -> status.Kind?)?, hue: string?, src: string?, emoji: string?): Node
	return ui.div {
		relative = true,
		w = 18,
		h = 18,
		Tile { hue = hue, inside = inside, size = 18, src = src, emoji = emoji },
		if badge
			then ui.presence(badge, function(kind: status.Kind): Node
				return ui.div { absolute = true, right = -3, top = -3, status.StatusDot { kind = kind, size = 8 } }
			end)
			else nil,
	}
end

-- Which state needs you most: a group's badge shows its most pressing.
M.URGENCY = {
	approval = 1,
	input = 2,
	failed = 3,
	limited = 4,
	working = 5,
	connecting = 6,
	plan = 7,
	done = 8,
	waiting = 9,
} :: { [string]: number }
function M.pressing(sessions: { { status: status.Kind? } }): status.Kind?
	local best: status.Kind? = nil
	for _, s in sessions do
		if s.status and (best == nil or M.URGENCY[s.status] < M.URGENCY[best :: string]) then
			best = s.status
		end
	end
	return best
end

-- ---------------------------------------------------------------------------
-- Whole rows, for lists outside the sidebar: the sidebar's frame (a flat
-- surface, its leading slot, its words), at the list's own width.

export type FrameProps = {
	name: string | () -> string,
	lead: Node?,
	-- Whether its slot shows (with none, its words start at the row's
	-- inset: a session with its harness hidden).
	leads: (() -> boolean)?,
	words: Node,
	-- Its height: a number, or a function it eases to as it changes (a
	-- session row's density).
	h: (number | () -> number)?,
	hovered: State<boolean>?,
	selected: (() -> boolean)?,
	on_click: (() -> ())?,
	-- How far in it starts (under a group, past its tree's gutter).
	indent: (() -> number)?,
	class: string?,
	id: string?,
	provide: { [string]: any }?,
}

-- A row's words' inset from its edge, with no slot before them.
local function words_inset(): number
	return (ui.theme.get("space.2") :: number?) or 8
end

function M.Frame(r: FrameProps): Node
	local hovered = r.hovered or ui.use_state(false)
	local leads = r.leads
	local function slot_w(): number
		return if leads and not leads() then words_inset() else M.SLOT
	end
	local content: { any } = {
		ui.div {
			w = if leads then motion.tween(ui.computed(slot_w), { duration = 260, ease = "out(3)" }) else M.SLOT,
			h = M.SLOT,
			shrink = 0,
			items = "center",
			justify = "center",
			overflow = if leads then "hidden" else nil,
			color = "$text.secondary",
			ui.div {
				y = M.OPTICAL,
				opacity = if leads
					then function(): number
						return if leads() then 1 else 0
					end
					else nil,
				r.lead,
			},
		},
		ui.div { grow = 1, min_w = 0, h_full = true, r.words },
	}
	if r.indent then
		local indent = r.indent
		table.insert(content, 1, ui.div {
			w = function(): number
				return indent()
			end,
			hidden = function(): boolean
				return indent() <= 0
			end,
			shrink = 0,
		})
	end
	return Surface {
		id = r.id,
		class = r.class,
		provide = r.provide,
		depth = "flat",
		selected = function(): boolean
			return r.selected ~= nil and r.selected()
		end,
		on_click = r.on_click,
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		label = r.name,
		w_full = true,
		h = if type(r.h) == "function"
			then motion.tween(ui.computed(r.h :: () -> number), { duration = 260, ease = "out(3)" })
			else (r.h :: number?) or M.SLOT,
		shrink = 0,
		border = 0,
		overflow = "hidden",
		ui.row { w_full = true, h_full = true, items = "start", table.unpack(content) },
	}
end

export type RowProps = {
	-- The session as it stands (a function: the row follows it in place).
	cur: () -> Session,
	-- What it's read out as (else its title).
	name: (() -> string)?,
	on_click: (() -> ())?,
	selected: (() -> boolean)?,
	-- Its harness's mark leads it (the view's "Harness").
	harness: (() -> boolean)?,
	-- Under a group's trunk.
	tree: (() -> Tree?)?,
	jump: (() -> number?)?,
	class: string?,
	-- The list's gap under it (the trunk runs on through it).
	gap: number?,
	-- Its menu on a secondary click (ds/context_menu's entries; nil, none):
	-- the sidebar's session menu, where the caller has it.
	menu: (() -> { any }?)?,
	-- The sidebar's density and details: compact (a line), else regular (a
	-- second line under the title while `detail` says something: where it
	-- runs, `branch` its branch's glyph leading); its project's tile beside
	-- its title while `title_lead_shown`.
	compact: (() -> boolean)?,
	detail: (() -> string)?,
	branch: (() -> boolean)?,
	title_lead: Node?,
	title_lead_shown: (() -> boolean)?,
	-- What leads it instead of its harness's mark (a port's glyph).
	lead: Node?,
	-- What the pointer finds in its corner (archive), as the sidebar's rows.
	action: Node?,
	-- Its title in the primary ink at rest (a settings page's rows, read as
	-- its other rows are), not the sidebar's quieter one.
	primary: boolean?,
}

-- A session row's height as the sidebar's density makes it: compact, or
-- with nothing for a second line, one line; else two.
function M.row_h(compact: boolean, detail: string?): number
	if compact or detail == nil or detail == "" then
		return M.SLOT
	end
	return M.TALL
end

-- A session's row: its harness's mark, its title (strong while it's in a
-- state, or chosen), its corner.
function M.Row(props: RowProps): Node
	local cur = props.cur
	local hovered = ui.use_state(false)
	local function strong(): boolean
		local s = cur()
		return (s.status ~= nil and s.status ~= "waiting") or (props.selected ~= nil and props.selected())
	end
	local harness = props.harness or function(): boolean
		return true
	end
	local function lead_at(): number
		return if harness() then (M.SLOT - 14) / 2 else words_inset()
	end
	local tree = props.tree
	local compact = props.compact or function(): boolean
		return true
	end
	local detail = props.detail or function(): string
		return ""
	end
	local function second_shown(): boolean
		return not compact() and detail() ~= ""
	end
	local title: Node = M.label(function(): string
		return cur().title
	end, strong, nil, nil, if props.primary then "$text.primary" else nil)
	if props.title_lead then
		title = ui.row {
			grow = 1,
			min_w = 0,
			gap = "$zeron.space.tight",
			items = "center",
			ui.show(props.title_lead_shown or function(): boolean
				return true
			end, ui.div { shrink = 0, y = M.OPTICAL, props.title_lead }),
			title,
		}
	end
	local body = M.Frame {
		class = props.class,
		h = function(): number
			return M.row_h(compact(), detail())
		end,
		name = props.name or function(): string
			return cur().title
		end,
		hovered = hovered,
		selected = props.selected,
		on_click = props.on_click,
		leads = harness,
		-- (A row of something else, a port's: its own glyph there.)
		lead = props.lead or M.mark(cur, strong),
		indent = if tree
			then function(): number
				local t = tree()
				return if t then M.gutter(t) else 0
			end
			else nil,
		words = ui.column {
			w_full = true,
			gap = "$zeron.space.hair",
			M.line(
				ui.row {
					class = "session-title",
					grow = 1,
					min_w = 0,
					items = "center",
					title,
				},
				M.subagents(function(): number
					return cur().subagents or 0
				end),
				M.Corner { cur = cur, hovered = hovered, jump = props.jump, action = props.action }
			),
			-- Pulled up under the title: the line's own inset is the row's.
			ui.div {
				mt = ui.raw(M.PULL, "up a row's inset, under the title"),
				M.second_line(detail, props.branch or function(): boolean
					return false
				end, second_shown),
			},
		},
	}
	-- (As the sidebar's rows take theirs: the row inside its ring.)
	local menu = props.menu
	if menu then
		body = ContextMenu {
			label = locale.t("ds.sidebar.session_actions"),
			ring = "$radius.md",
			w_full = true,
			shrink = 0,
			items = function(): any
				return menu()
			end,
			body,
		}
	end
	if tree then
		return M.on_tree(body, tree, lead_at, props.gap)
	end
	return body
end

export type HeaderProps = {
	id: string?,
	name: () -> string,
	-- Its tile's glyph (the chrome's quiet tile), or its own tile (`lead`:
	-- a project's, made with `tile`).
	icon: Icon.Name?,
	lead: ((badge: () -> status.Kind?) -> Node)?,
	count: () -> number,
	open: () -> boolean,
	on_toggle: () -> (),
	-- Its rows' most pressing state, for the tile's badge while folded.
	pressing: (() -> status.Kind?)?,
	-- Its tile shows (as a project group's follows the view's "Project
	-- icon"); hidden, the name starts at the row's inset.
	leads: (() -> boolean)?,
	-- An inner group's: its place on its outer group's trunk (indented a
	-- step, a branch to its tile).
	tree: (() -> Tree?)?,
	gap: number?,
}

-- A group's header: its tile and name; a click folds it (folded, how many
-- it holds, and its tile's badge for the most pressing).
function M.Header(props: HeaderProps): Node
	local open = props.open
	local function folded(): boolean
		return not open()
	end
	local tree = props.tree
	local leads = props.leads
	local body = M.Frame {
		id = props.id,
		class = "section-header",
		indent = if tree
			then function(): number
				local t = tree()
				return if t then M.gutter(t) else 0
			end
			else nil,
		name = props.name,
		leads = props.leads,
		lead = (function(): Node
			local function badge(): status.Kind?
				return if folded() and props.pressing then props.pressing() else nil
			end
			if props.lead then
				return props.lead(badge)
			end
			return M.tile(Icon { name = props.icon or "folder", size = 12 }, badge)
		end)(),
		words = M.header_words(props.name, function(): string
			return tostring(props.count())
		end, folded),
		on_click = props.on_toggle,
	}
	if tree then
		return M.on_tree(body, tree, function(): number
			return if leads == nil or leads() then (M.SLOT - 18) / 2 else words_inset()
		end, props.gap)
	end
	return body
end

export type PagerProps = {
	id: string?,
	whole: () -> boolean,
	more: () -> number,
	on_click: () -> (),
	harness: (() -> boolean)?,
	tree: (() -> Tree?)?,
}

-- "Show N more" / "Show less", a group's last leaf.
function M.Pager(props: PagerProps): Node
	local says = M.pager_says(props.whole, props.more)
	local harness = props.harness or function(): boolean
		return true
	end
	local tree = props.tree
	local body = M.Frame {
		id = props.id,
		class = "group-pager",
		name = says,
		leads = harness,
		lead = M.pager_lead(props.whole),
		indent = if tree
			then function(): number
				local t = tree()
				return if t then M.gutter(t) else 0
			end
			else nil,
		words = M.pager_words(says),
		on_click = props.on_click,
	}
	if tree then
		return M.on_tree(body, tree, function(): number
			return if harness() then (M.SLOT - 14) / 2 else words_inset()
		end)
	end
	return body
end

-- How a group's rows open and close, as the sidebar's: the height eases
-- while the ink fades (going, the ink first; coming, the height first).
M.FOLD = { open = 260, close = 240, ease = motion.EASE }

export type FoldProps = {
	open: () -> boolean,
	-- How tall its rows stand, open.
	h: () -> number,
	body: Node,
}

-- A group's rows under its header: open, as tall as they are; folding,
-- easing shut (then gone from the list, nothing of them left to reach);
-- opening, easing open from nothing. (Reduce motion: at once.)
function M.Fold(props: FoldProps): Node
	local was = props.open()
	local mounted = ui.use_state(was)
	local amount = ui.use_state(if was then 1 else 0)
	local going: any = nil
	ui.effect(function()
		local on = props.open()
		if on == was then
			return
		end
		was = on
		if going then
			going:cancel()
			going = nil
		end
		if on then
			mounted:set(true)
		end
		local this: any = nil
		this = anime.animate(amount, motion.params({
			value = if on then 1 else 0,
			duration = if on then M.FOLD.open else M.FOLD.close,
			ease = M.FOLD.ease,
			onComplete = function()
				if going == this then
					going = nil
				end
				if not props.open() then
					mounted:set(false)
				end
			end,
		}))
		going = this
	end)
	return ui.show(
		function(): boolean
			return mounted:get()
		end,
		ui.div {
			w_full = true,
			overflow = "hidden",
			-- (Whole points: a list inside lays its rows out again at every
			-- fraction, and zgx's layout never settles on some.)
			h = function(): number
				return math.floor(props.h() * amount:get() + 0.5)
			end,
			opacity = function(): number
				return amount:get()
			end,
			-- (A list in it keeps its own height as the box eases round it:
			-- zgx 78dcac2a4, zgx-asks/list-in-easing-height.)
			props.body,
		}
	)
end

return M
