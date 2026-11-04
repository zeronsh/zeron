--!strict
-- The pieces of a session's panel home (screens/panel/home): the right
-- panel's page while no tab is in front. What the session is (its device,
-- project and branch), the project's run actions, what can open here (each
-- with the one key that opens it), and the session's lanes, folded.
-- Flat rows on the page, no cards: a row's wash shows under the pointer,
-- cornered as the row is, and what stands inside a row (a key's cap, an
-- edit button) is cornered concentrically with it.
--
--   PanelHome.Row { icon = name | Node, label, trail?, on_click?, id?,
--                   tooltip?, tone? }     a row: interactive with `on_click`
--   PanelHome.Choice { id, icon, label, key, on_click }   what opens here,
--                                         its key's cap at its end
--   PanelHome.Heading { label, trail? }   a part's title
--   PanelHome.Lanes { id, list, label, icon, open, on_toggle, items, name,
--                     on_open, expanded, on_expand, harness? }
--                                         a part of lanes (side chats,
--                                         subagents) drawn as the sidebar's
--                                         groups (ds/session_row): it folds,
--                                         only its rows in view built
--   PanelHome.inner(inset)                a corner `inset` in from a row's
--
-- Plugins' rows on the page (`hook:panel.empty`) take `Choice` for the
-- same look: a plugin is lent the design system.

local Icon = require("@self/icon")
local Kbd = require("@self/kbd")
local control = require("@self/control")
local status = require("@self/status")
local motion = require("@self/motion")
local locale = require("@self/locale")
local Tooltip = require("@self/tooltip")
-- Side chats and subagents listed as the sidebar lists sessions.
local SessionRow = require("@self/session_row")
local Tile = require("@self/tile")

local M = {}

-- A row: its height, its corner and its side padding. The corner is a
-- step rounder than a menu's so what stands inside it (a key's cap 8 in,
-- an edit button 6 in) keeps a curve of its own.
-- (The left sidebar's own row: its one-line height and its corner, so the
-- two read as one list.)
M.ROW_H = SessionRow.SLOT
M.ROW_RADIUS = "$radius.lg"
M.ICON = 18
local PAD = "$space.2"
local PAD_PT = 8
-- A lane's row: a list's, shorter, as the lanes card's.
M.LANE_H = 32
-- How many lanes show before "Show N more" (as the sidebar's groups: those
-- at work or waiting on you show past them). A list is always as tall as
-- its rows: the panel scrolls, never a part inside it.
M.LANE_FIRST = 5
-- How a list opens and closes: the sidebar's own move.
local SLIDE = { duration = 260, ease = "out(3)" }

-- The corner `inset` points in from a row's edge (control.nest).
function M.inner(inset: number, outer: any?): any
	return ui.raw(control.nest(outer or M.ROW_RADIUS, inset), "nested in the panel home's row")
end

export type RowProps = {
	icon: (Icon.Name | Node)?,
	label: string | Message | () -> string,
	trail: Node?,
	on_click: (() -> ())?,
	id: string?,
	-- Read out for it (else its label).
	name: (string | Message | () -> string)?,
	tooltip: (string | () -> string)?,
	-- `muted`: a quieter row (Add action, a note).
	tone: ("normal" | "muted")?,
	-- What its glyph's slot holds instead of a glyph (a project's tile).
	lead: Node?,
	class: string?,
	h: number?,
}

local function glyph(icon: (Icon.Name | Node)?, lead: Node?, muted: boolean): Node
	local inside: Node? = lead
	if not inside and icon ~= nil then
		inside = if type(icon) == "string" then Icon { name = icon :: Icon.Name, size = M.ICON } else icon :: Node
	end
	return ui.row {
		w = M.ICON,
		h = M.ICON,
		shrink = 0,
		items = "center",
		justify = "center",
		color = if muted then "$text.tertiary" else "$text.secondary",
		inside,
	}
end

function M.Row(props: RowProps): Node
	local muted = props.tone == "muted"
	local h = props.h or M.ROW_H
	local box: { [any]: any } = {
		id = props.id,
		class = props.class,
		w_full = true,
		h = h,
		shrink = 0,
		px = PAD,
		gap = "$space.3",
		items = "center",
		rounded = M.ROW_RADIUS,
		color = if muted then "$text.secondary" else "$text.primary",
		-- (What stands inside takes the nested corner: a key's cap, an
		-- edit button.)
		theme = { radius = { sm = control.nest(M.ROW_RADIUS, (h - 20) / 2) } },
		glyph(props.icon, props.lead, muted),
		ui.text { props.label, grow = 1, min_w = 0, truncate = true, text_size = "$font.size.md" },
		props.trail,
	}
	if props.on_click then
		box.role = "button"
		box.label = props.name or props.label
		box.focusable = true
		box.cursor = "pointer"
		box.hover = { bg = "$hover", color = "$text.primary" }
		box.focus = control.ring
		local click = props.on_click
		box.on_click = click
		box.on_key = function(e: any): boolean?
			if (e.key == "Enter" or e.key == " ") and not (e.cmd or e.ctrl or e.alt) then
				click()
				return true
			end
			return nil
		end
	else
		box.label = props.name or props.label
	end
	if props.tooltip then
		box.tooltip = if type(props.tooltip) == "function" then Tooltip { props.tooltip :: any } else props.tooltip
		box.tooltip_placement = require("@self/icon_button").placement("left")
	end
	return ui.row(box)
end

export type ChoiceProps = { id: string, icon: Icon.Name, label: string | Message, key: string, on_click: () -> () }

function M.Choice(props: ChoiceProps): Node
	return M.Row {
		id = props.id,
		class = "panel-home-choice",
		icon = props.icon,
		label = props.label,
		on_click = props.on_click,
		trail = Kbd { text = props.key, size = "md", tone = "quiet" },
	}
end

function M.Heading(props: { label: string | Message | () -> string, trail: Node? }): Node
	return ui.row {
		w_full = true,
		h = 28,
		px = PAD,
		items = "center",
		gap = "$space.2",
		ui.text { props.label, grow = 1, min_w = 0, truncate = true, text_size = "$font.size.xs", weight = "$font.weight.medium", color = "$text.tertiary" },
		props.trail,
	}
end

-- A lane's group, as the sidebar would group it (its view's Organize):
-- a project's (its tile: hue and initial, or its own picture), a device's
-- (this Mac's laptop, another's server); nil, one list with no groups.
export type LaneGroup = { key: string, name: string, kind: "project" | "device", hue: string?, src: string?, emoji: string?, here: boolean? }
-- A lane, as it comes: what it is, and where it runs. Its `status` as it
-- should show (done or failed only while unseen); `at` when it last moved,
-- `created` when it was made (the view's Sort); its project and device
-- (its group, its second line, its title's tile, as the sidebar's view
-- shows them; a lane with neither stands in one list).
export type Lane = {
	id: string,
	title: string,
	status: string?,
	agent: string?,
	at: number?,
	created: number?,
	-- What its corner says instead of when it moved (a port's `:3000`).
	corner: string?,
	project: { id: string, name: string, hue: string?, src: string?, emoji: string? }?,
	device: { id: string, name: string, here: boolean? }?,
	branch: string?,
	-- A second line of its own, whatever the view (a port's address).
	detail: string?,
	-- Drawn faded: gone, kept a while (a port closed).
	faded: boolean?,
	-- (Its group, as the view makes it: the list's own.)
	group: LaneGroup?,
}
-- The sidebar's view, whole, as the shell publishes it (`sidebar.view`):
-- its density, how it groups and sorts, and every one of its Show options
-- by name (`details`, as ds/sidebar reads them). An option it doesn't know
-- reads as on, as the sidebar's defaults are.
export type View = { compact: boolean?, organize: string?, sort: string?, details: { [string]: any }? }
export type LanesProps = {
	-- The part's id (its title `{id}-head`), and its list's (a group's
	-- pager `{list}-pager`).
	id: string,
	list: string,
	label: string | Message,
	open: () -> boolean,
	on_toggle: () -> (),
	items: () -> { Lane },
	-- What it's read out as ("Open side chat {title}").
	name: (lane: Lane) -> string,
	on_open: (lane: Lane) -> (),
	-- Each group's fold and whether it's shown whole ("Show less"), by the
	-- group's key ("all" with no groups).
	group_open: (key: string) -> boolean,
	on_group_toggle: (key: string) -> (),
	expanded: (key: string) -> boolean,
	on_expand: (key: string, whole: boolean) -> (),
	-- The sidebar's view, whole: the lanes drawn as it draws sessions.
	view: (() -> View)?,
	-- Never grouped (subagents, ports: always their session's), one list
	-- as the sidebar's "None" draws it.
	flat: boolean?,
	-- Archive under the pointer, in the corner, as the sidebar's rows have
	-- it (side chats); nil, none.
	on_archive: ((lane: Lane) -> ())?,
	-- A lane's menu on a secondary click (the sidebar's session menu, for
	-- side chats); nil, none.
	menu: ((lane: Lane) -> { any }?)?,
	-- A glyph leading each lane instead of its harness's mark (a port's).
	icon: ((lane: Lane) -> Icon.Name?)?,
	-- Each lane's tooltip (a port's command line); nil, none.
	tooltip: ((lane: Lane) -> string)?,
}

-- Whether a lane shows past the first few, and in a folded group: at
-- work, waiting on you, or done or failed and not yet seen.
local function active(l: Lane): boolean
	local k = l.status
	return k == "working" or k == "running" or k == "input" or k == "approval" or k == "done" or k == "failed"
end

-- A lane as a session's row reads it (ds/session_row): its state one the
-- sidebar draws ("running" is working; a word it doesn't know, none).
local function as_session(l: Lane): SessionRow.Session
	local k = if l.status == "running" then "working" else l.status
	return {
		id = l.id,
		title = l.title,
		status = if k and status.KINDS[k :: any] then k :: any else nil,
		agent = l.agent :: any,
		time = l.corner or l.at,
	}
end

-- One row of a part's list: a group's header, a lane, or a group's pager.
type Item = {
	kind: "group" | "lane" | "pager",
	key: string,
	gkey: string,
	group: LaneGroup?,
	lane: Lane?,
	-- A header's: how many it holds, and its most pressing; a pager's: how
	-- many it hides, and whether all show.
	n: number?,
	pressing: string?,
	whole: boolean?,
	-- The last on its group's trunk.
	last: boolean?,
	-- How many groups the part has (one: its pager is `{list}-pager`).
	groups: number?,
	-- Its groups from the outermost (a header's: those above it; a lane's
	-- and a pager's: those it's in), for its trunks.
	path: { LaneGroup }?,
	-- An outer group's trunk runs on past it (two levels of groups).
	through: boolean?,
}

-- A part of lanes (side chats, subagents) as the sidebar draws its own: a
-- section's title (`Section`: folded, how many), then its lanes grouped as
-- the sidebar's view groups sessions (each group's header, `Header`, its
-- tile a project's or a device's; or one list), each group's first few and
-- those in a state, then "Show N more" / "Show less", a folded group still
-- listing those in a state while the view keeps them. Its rows are the
-- sidebar's session rows (ds/session_row), off their group's trunk. One
-- list, rows by what they are (a lane by its id, moving between groups the
-- same node); as tall as all its rows, so the home scrolls and never a
-- list inside it.
function M.Lanes(props: LanesProps): Node
	-- The sidebar's view, read as ds/sidebar reads it.
	local function view(): View
		return if props.view then props.view() else {}
	end
	local function on(option: string): boolean
		local d = view().details
		return not (d and d[option] == false)
	end
	-- (A lane with a glyph of its own, a port, always leads with it.)
	local function harness(): boolean
		return props.icon ~= nil or on("harness")
	end
	local function keep(): boolean
		return on("keep_active")
	end
	local function compact(): boolean
		return view().compact ~= false
	end
	-- How it groups: the sidebar's levels in order (project, device; both,
	-- a tree; none, one list), as a key ("project>device"); a flat part's,
	-- one list.
	local function levels(): { string }
		return if props.flat then {} else SessionRow.levels(view().organize)
	end
	local function organize(): string
		return SessionRow.organize_key(levels())
	end
	-- A lane's group at one level, as the sidebar's would be: its project's
	-- (none, the sidebar's "Chats"), its device's; its key under `prefix`.
	-- None for a lane with neither.
	local function group_at(l: Lane, level: string, prefix: string): LaneGroup?
		if not (l.project or l.device) then
			return nil
		end
		if level == "project" then
			local p = l.project
			if p then
				return { key = `{prefix}project:{p.id}`, name = p.name, kind = "project", hue = p.hue or "gray", src = p.src, emoji = p.emoji }
			end
			return { key = `{prefix}project:none`, name = locale.s("shell.sidebar.chats"), kind = "project", hue = "gray" }
		end
		local d = l.device
		return {
			key = `{prefix}device:{if d then d.id else "none"}`,
			name = if d then d.name else locale.s("ds.sidebar.unknown_device"),
			kind = "device",
			here = d ~= nil and d.here == true,
		}
	end
	-- Its second line, by the sidebar's rule (ds/session_row `detail`).
	local function detail_of(l: Lane): string
		if l.detail then
			return l.detail
		end
		return SessionRow.detail(
			if l.project then l.project.name else nil,
			if l.device then l.device.name else nil,
			l.branch,
			organize(),
			on("location"),
			on("branch")
		)
	end
	-- When it moved, or was made, as the view sorts.
	local function sort_key(l: Lane): number
		if view().sort == "created" then
			return l.created or l.at or -math.huge
		end
		return l.at or -math.huge
	end
	local all = ui.computed(props.items)
	-- The one group's pager, when the part has one group (under the list).
	local solo: Item? = nil
	local solo_state = ui.use_state(nil :: Item?)
	local items = ui.computed(function(): { Item }
		solo = nil
		-- Its lanes in the view's order (the most recent first, by when they
		-- moved or were made; else as they came).
		local list = table.clone(all:get())
		local came: { [Lane]: number } = {}
		for i, l in list do
			came[l] = i
		end
		table.sort(list, function(a: Lane, b: Lane): boolean
			local x, y = sort_key(a), sort_key(b)
			if x ~= y then
				return x > y
			end
			return came[a] < came[b]
		end)
		-- Its groups as the view groups them, at each of its levels (the most
		-- recently active first at each, as the sidebar orders them).
		type Node_ = { group: LaneGroup?, lanes: { Lane }, children: { Node_ }? }
		local function grouped(lanes: { Lane }, level: string?, prefix: string): { Node_ }
			local order: { string } = {}
			local by: { [string]: Node_ } = {}
			for _, l in lanes do
				local group = if level then group_at(l, level, prefix) else nil
				local key = if group then group.key else `{prefix}all`
				local g = by[key]
				if not g then
					g = { group = group, lanes = {} }
					by[key] = g
					table.insert(order, key)
				end
				table.insert(g.lanes, l)
			end
			local newest: { [string]: number } = {}
			local rank: { [string]: number } = {}
			for i, k in order do
				rank[k] = i
				local best = -math.huge
				for _, l in by[k].lanes do
					best = math.max(best, sort_key(l))
				end
				newest[k] = best
			end
			table.sort(order, function(a: string, b: string): boolean
				if newest[a] ~= newest[b] then
					return newest[a] > newest[b]
				end
				return rank[a] < rank[b]
			end)
			local out: { Node_ } = {}
			for _, k in order do
				table.insert(out, by[k])
			end
			return out
		end
		local lv = levels()
		local tops = grouped(list, lv[1], "")
		if lv[2] then
			for _, g in tops do
				if g.group then
					g.children = grouped(g.lanes, lv[2], `{g.group.key}/`)
				end
			end
		end
		local out: { Item } = {}
		local function pressing_of(lanes: { Lane }): string?
			local ss = {}
			for _, l in lanes do
				table.insert(ss, as_session(l))
			end
			return SessionRow.pressing(ss)
		end
		local function any_active(lanes: { Lane }): boolean
			for _, l in lanes do
				if active(l) then
					return true
				end
			end
			return false
		end
		-- A group's header and what's under it (`path`: its groups from the
		-- outermost; `only_active`: folded away above it).
		local function emit(g: Node_, path: { LaneGroup }, only_active: boolean)
			local gkey = if g.group then g.group.key else "all"
			local here = table.clone(path)
			if g.group then
				table.insert(here, g.group)
				table.insert(out, { kind = "group", key = `g:{gkey}`, gkey = gkey, group = g.group, path = path, n = #g.lanes, pressing = pressing_of(g.lanes) })
			end
			local open = (g.group == nil or props.group_open(gkey)) and not only_active
			if g.children then
				-- Paged as the sidebar's: its first few inner groups, those
				-- with a lane at work or waiting on you past them, then "Show N
				-- more" on its trunk (shown whole by `{key}/groups`).
				local pkey = `{gkey}/groups`
				local whole = props.expanded(pkey)
				local spans: { { from: number, to: number } } = {}
				local hidden = 0
				for i, c in g.children do
					local active_here = keep() and any_active(c.lanes)
					if open then
						if whole or i <= M.LANE_FIRST or active_here then
							local from = #out + 1
							emit(c, here, false)
							table.insert(spans, { from = from, to = #out })
						else
							hidden += 1
						end
					elseif active_here then
						local from = #out + 1
						emit(c, here, true)
						table.insert(spans, { from = from, to = #out })
					end
				end
				local paged = open and ((whole and #g.children > M.LANE_FIRST) or hidden > 0)
				if paged then
					table.insert(out, { kind = "pager", key = `p:{pkey}`, gkey = pkey, group = g.group, path = here, n = hidden, whole = whole, last = true, groups = #tops })
				end
				for i, span in spans do
					local final = i == #spans and not paged
					out[span.from].last = final
					if not final then
						for j = span.from + 1, span.to do
							out[j].through = true
						end
					end
				end
				return
			end
			local lanes = g.lanes
			local shown: { Lane } = {}
			local hidden = 0
			local whole = props.expanded(gkey)
			if open then
				for i, l in lanes do
					if whole or #lanes <= M.LANE_FIRST or i <= M.LANE_FIRST or active(l) then
						table.insert(shown, l)
					else
						hidden += 1
					end
				end
			elseif keep() then
				for _, l in lanes do
					if active(l) then
						table.insert(shown, l)
					end
				end
			end
			local paged = open and ((whole and #lanes > M.LANE_FIRST) or hidden > 0)
			for i, l in shown do
				table.insert(out, { kind = "lane", key = `l:{l.id}`, gkey = gkey, group = g.group, path = here, lane = l, last = i == #shown and not paged })
			end
			if paged then
				local pager: Item = { kind = "pager", key = `p:{gkey}`, gkey = gkey, group = g.group, path = here, n = hidden, whole = whole, last = true, groups = #tops }
				-- (One group's pager stands under the list, reachable with
				-- its rows shown whole and scrolled; several groups', each
				-- after its own rows.)
				if #tops == 1 and not g.children and #path == 0 then
					solo = pager
				else
					table.insert(out, pager)
				end
			end
		end
		for _, g in tops do
			emit(g, {}, false)
		end
		return out
	end)
	ui.effect(function()
		items:get()
		local was = solo_state:peek()
		local now = solo
		if was == nil and now == nil then
			return
		end
		if was and now and was.gkey == now.gkey and was.n == now.n and was.whole == now.whole then
			return
		end
		solo_state:set(if now then table.clone(now) else nil)
	end)
	-- Each row's height in the list (with the list's gap under it): a lane's
	-- as the sidebar's density makes it, a header's and a pager's a line.
	local GAP = M.LANE_H - SessionRow.SLOT
	local function item_h(it: Item): number
		if it.kind == "lane" then
			return SessionRow.row_h(compact(), detail_of(it.lane :: Lane)) + GAP
		end
		return M.LANE_H
	end
	local heights = ui.computed(function(): { number }
		local out = {}
		for i, it in items:get() do
			out[i] = item_h(it)
		end
		return out
	end)
	-- As tall as all its rows ("Show N more" keeps a group short).
	local height = ui.computed(function(): number
		local h = 0
		for _, x in heights:get() do
			h += x
		end
		return h
	end)
	-- Under a group's tile: the trunk at the tile's middle, as the
	-- sidebar's groups draw it (a project's tile hidden by the view, a
	-- little further in, as the sidebar's); an inner group's past its
	-- outer group's gutter. With no groups, none.
	local function trunk_of(g: LaneGroup?): number
		if g and g.kind == "project" and not on("project_icon") then
			return 12.5
		end
		return SessionRow.SLOT / 2
	end
	local function trunk_at(path: { LaneGroup }, depth: number): number
		local x = 0
		for i = 1, depth do
			x = (if i > 1 then x + SessionRow.BEND + SessionRow.REACH else 0) + trunk_of(path[i])
		end
		return x
	end
	-- A row's place on its groups' trees: a lane's or a pager's, on its
	-- group's trunk; a header's, on its outer group's (an inner group's).
	local function tree_of(now: State<any>): () -> SessionRow.Tree?
		return function(): SessionRow.Tree?
			local it = now:get() :: Item?
			if it == nil then
				return nil
			end
			local path = it.path or {}
			local depth = if it.kind == "group" then #path else #path
			if depth == 0 then
				return nil
			end
			return {
				trunk = function(): number
					local cur = now:get() :: Item?
					return if cur then trunk_at(cur.path or {}, #(cur.path or {})) else 0
				end,
				last = function(): boolean
					local cur = now:get() :: Item?
					return cur == nil or cur.last == true
				end,
				through = function(): { number }
					local cur = now:get() :: Item?
					if cur and cur.through and cur.path and #cur.path >= 1 then
						return { trunk_at(cur.path, 1) }
					end
					return {}
				end,
			}
		end
	end
	-- A group's "Show N more" / "Show less", the last leaf on its trunk.
	local function pager(now: State<any>, id: string): Node
		local function whole(): boolean
			local it = now:get() :: Item?
			return it ~= nil and it.whole == true
		end
		return SessionRow.Pager {
			id = id,
			whole = whole,
			more = function(): number
				local it = now:get() :: Item?
				return if it then it.n or 0 else 0
			end,
			on_click = function()
				local it = now:peek() :: Item?
				if it then
					props.on_expand(it.gkey, not whole())
				end
			end,
			harness = harness,
			tree = tree_of(now),
		}
	end
	local function row(first: Item, _index: State<number>, now: State<any>): Node
		local gkey = first.gkey
		if first.kind == "group" then
			local g = first.group :: LaneGroup
			return ui.div {
				w_full = true,
				h = M.LANE_H,
				SessionRow.Header {
					-- (An inner group's hangs off its outer group's trunk.)
					tree = tree_of(now),
					gap = M.LANE_H - SessionRow.SLOT,
					-- (A project's tile as the view's "Project icon" shows it,
					-- as the sidebar's group headers; a device's always.)
					leads = if g.kind == "project" then function(): boolean
						return on("project_icon")
					end else nil,
					name = function(): string
						local it = now:get() :: Item
						return if it.group then it.group.name else ""
					end,
					lead = function(badge: () -> any): Node
						if g.kind == "project" then
							return SessionRow.tile(ui.text {
								string.upper(string.sub(g.name, 1, 1)),
								text_size = "$font.size.xs",
								weight = "$font.weight.semibold",
							}, badge, g.hue or "gray", g.src, g.emoji)
						end
						return SessionRow.tile(Icon { name = if g.here then "laptop" else "remote-server", size = 12 }, badge)
					end,
					count = function(): number
						return (now:get() :: Item).n or 0
					end,
					open = function(): boolean
						return props.group_open(gkey)
					end,
					on_toggle = function()
						props.on_group_toggle(gkey)
					end,
					pressing = function(): any
						return (now:get() :: Item).pressing
					end,
				},
			}
		elseif first.kind == "pager" then
			return ui.div { w_full = true, h = M.LANE_H, pager(now, `{props.list}-pager-{gkey}`) }
		end
		local function lane(): Lane
			return (now:get() :: Item).lane :: Lane
		end
		local tip = props.tooltip
		return ui.div {
			w_full = true,
			h = function(): number
				return item_h(now:get() :: Item)
			end,
			opacity = function(): number
				return if lane().faded then 0.5 else 1
			end,
			tooltip = if tip
				then Tooltip {
					function(): string
						return tip(lane())
					end,
				}
				else nil,
			tooltip_placement = if tip then require("@self/icon_button").placement("left") else nil,
			SessionRow.Row {
				compact = compact,
				detail = function(): string
					return detail_of(lane())
				end,
				branch = function(): boolean
					return on("branch") and lane().branch ~= nil
				end,
				-- Its project's tile beside its title, while the view shows it.
				title_lead = ui.presence(function(): string
					local p = lane().project
					return if p then `{p.name}\0{p.hue or ""}\0{p.src or ""}\0{p.emoji or ""}` else ""
				end, function(_key: string): Node
					local p = lane().project
					if not p then
						return ui.div {}
					end
					return Tile { label = p.name, hue = p.hue, src = p.src, emoji = p.emoji, size = 16 }
				end),
				-- (Where its group isn't its project, and the view shows project
				-- icons: as the sidebar's rows.)
				title_lead_shown = function(): boolean
					return on("project_icon") and lane().project ~= nil and not SessionRow.groups_by(organize(), "project")
				end,
				class = "panel-home-lane",
				lead = if props.icon
					then ui.div {
						color = "$text.tertiary",
						Icon {
							name = function(): Icon.Name
								return (props.icon :: (Lane) -> Icon.Name?)(lane()) or "global"
							end :: any,
							size = 14,
						},
					}
					else nil,
				cur = function(): SessionRow.Session
					return as_session(lane())
				end,
				name = function(): string
					return props.name(lane())
				end,
				on_click = function()
					props.on_open((now:peek() :: Item).lane :: Lane)
				end,
				harness = harness,
				tree = tree_of(now),
				gap = M.LANE_H - SessionRow.SLOT,
				menu = if props.menu
					then function(): { any }?
						return (props.menu :: (Lane) -> { any }?)(lane())
					end
					else nil,
				action = if props.on_archive
					then SessionRow.corner_button("ds.sidebar.archive", "archive-minimalistic", function()
						(props.on_archive :: (Lane) -> ())(lane())
					end)
					else nil,
			},
		}
	end
	return ui.column {
		id = props.id,
		class = "panel-home-fold",
		w_full = true,
		gap = "$zeron.list.gap",
		SessionRow.Section {
			title = props.label :: any,
			open = props.open,
			on_toggle = props.on_toggle,
			count = function(): number
				return #all:get()
			end,
			frame = {
				id = `{props.id}-head`,
				w_full = true,
				role = "button",
				label = props.label,
				expanded = props.open,
				focusable = true,
				focus = control.ring,
				rounded = M.inner(4),
				on_key = function(e: any): boolean?
					if (e.key == "Enter" or e.key == " ") and not (e.cmd or e.ctrl or e.alt) then
						props.on_toggle()
						return true
					end
					return nil
				end,
			},
		},
		SessionRow.Fold {
			open = props.open,
			h = function(): number
				return height:get() + (if solo_state:get() then M.LANE_H else 0)
			end,
			-- (No gap between the list and its pager: each row keeps its
			-- own under it, the trunk running through it.)
			body = ui.column {
				w_full = true,
				gap = 0,
				ui.list {
				id = props.list,
				data = items,
				key = function(it: Item): string
					return it.key
				end,
				row = row,
				height = M.LANE_H,
				heights = function(): { number }
					return heights:get()
				end,
				w_full = true,
				h = motion.tween(height, SLIDE),
				-- (Never a scroll of its own: the home is the one that
				-- scrolls, the wheel over these rows going on to it.)
				scrollbar = false,
				overscroll = "auto",
				},
				ui.show(function(): boolean
					return solo_state:get() ~= nil
				end, pager(solo_state, `{props.list}-pager`)),
			},
		},
	}
end

-- (The row's own side padding, for what lines up with its text.)
M.PAD_PT = PAD_PT

return M
