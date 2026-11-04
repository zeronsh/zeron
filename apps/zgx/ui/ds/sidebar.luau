-- Zeron's sidebar: what you're working on, and what needs you. New chat
-- and search at the top; pinned sessions; each project's sessions under it;
-- you at the foot.
--
-- Two densities, as Zeron's: compact (a line a session: its glyph, its
-- title, its time) and regular (a second line under the title: its agent,
-- device and branch). Collapsed, it slides away whole; its rail, if the app
-- shows one (`island = true`), floats at the window's bottom left.
--
-- One frame for all of it: every row has a leading slot (30 points, at the
-- same place at any width) for its glyph, and its words beside it.
-- Collapsing narrows the sidebar and each row to that slot, so nothing that
-- stays moves: the words slide under its edge (they keep their width, so
-- nothing re-truncates on the way), sessions fold into their project, and
-- each project's tile takes a badge for its most pressing session; a tile
-- opens its sessions beside the rail. Changing density grows each row and
-- fades its second line in under the first, on the same ease.
--
-- A session's slot says which agent it is; its corner, its state (the
-- working ring, a question waiting on you, an approval, a failure, work done
-- you haven't seen: its glyph, in its hue, its name a tooltip) or, with none, how long
-- ago it moved. Sessions in a state stand out; idle ones recede. Under the
-- pointer the corner becomes its archive button. Collapsed (no corner to
-- see), the slot says the state instead. Text that doesn't fit fades out.
--
-- `Sidebar { compact = state, collapsed = state, selected = state, pinned = { … }, projects = { … }, user = { … } }`
--
-- Its data may change: `projects` (and `pinned`) may be a state or a
-- function, read as it changes. Every row is its session's (or group's)
-- for as long as it is there: a session's state, title or time moving
-- reaches its row in place, and one coming or going moves the rest aside,
-- each row keeping its node (its hover, a menu open on it).

local tree_line = require("@self/tree_line")
local Icon = require("@self/icon")
local Mark = require("@self/mark")
local status = require("@self/status")
local FadeText = require("@self/fade_text")
local island = require("@self/island")
local Tile = require("@self/tile")
local Surface = require("@self/surface")
local Avatar = require("@self/avatar")
local Switch = require("@self/switch")
local Glass = require("@self/glass")
local Menu = require("@self/menu")
local icon_button = require("@self/icon_button")
local IconButton = icon_button.IconButton
local ToggleButton = require("@self/toggle_button")
local ContextMenu = require("@self/context_menu")
local motion = require("@self/motion")
local locale = require("@self/locale")
-- A session's row, its corner, its group's header and pager: shared with
-- every other list of sessions (the panel home's).
local SessionRow = require("@self/session_row")
local People = require("@self/people")
local EditorMenu = require("@self/editor_menu")

-- The page's split chats (screens/shell/split, which keeps these):
-- the chats side by side now, each row of theirs selected; and whether a
-- chat is being dragged, so the split shows where it can land.
local split_grouped = ui.shared("split.grouped", {} :: { string })
local split_dragging = ui.shared("split.dragging", false)
local function in_split(id: string): boolean
	return table.find(split_grouped:get(), id) ~= nil
end

export type Session = {
	id: string,
	title: string,
	status: status.Kind?,
	-- When it last moved: a time (`os.time()` seconds, shown compactly: 5m,
	-- 3h, 1d, 2w), or text as it should read.
	time: (number | string)?,
	agent: Mark.Name?,
	-- Regular's second line: where it runs and on what.
	device: string?,
	branch: string?,
	-- Subagents and side chats working under it (Zeron's "● N" pill beside
	-- its corner).
	subagents: number?,
	-- Its pull request: a badge beside its corner.
	pr: PullRequest?,
	-- When it was made (as `time`), for sorting by it.
	created: (number | string)?,
}
export type PullRequest = { number: number, state: ("open" | "merged" | "closed" | "draft")?, title: string? }
-- (`src`: its own artwork, a picture chosen for it or its favicon, in place
-- of its initial and hue; `emoji`: a chosen emoji, on a wash of its hue.)
-- (`icon`: a group that is no folder, the chats in none, drawn as that
-- glyph on a quiet wash in place of an initial on a hue.)
-- (`empty`: the caller's word that it has no sessions but archived ones.
-- Grouped by project first, those are listed after the rest, folded into
-- one row of their own ("21 empty projects"), closed until it's opened;
-- `details.empty_projects` off, not listed at all. A project with none at
-- all, just added, isn't: a new chat starts in it.)
export type Project = { id: string, name: string, hue: string?, src: string?, emoji: string?, icon: string?, empty: boolean?, sessions: { Session } }
-- You: your name, and an id your avatar is drawn from.
export type User = { id: string?, name: string, avatar: string? }
-- How its sessions are grouped (Zeron's Organize), as a key: its levels
-- in order, outermost first ("project", "device", "project>device",
-- "device>project"), or "none", one list (ds/session_row `levels`). The
-- view's own state holds the levels as a list (`{ "project", "device" }`;
-- an old single choice, "project", reads as its one level).
export type Organize = string
-- Which of a session's details show (Zeron's Show): its branch and where it
-- runs (regular's second line), its pull request, its harness (the agent's
-- mark leading the row), its project's icon (beside its title, where the
-- group isn't its project already).
export type Details = {
	branch: State<boolean>,
	pr: State<boolean>,
	harness: State<boolean>,
	project_icon: State<boolean>,
	location: State<boolean>,
	-- The Archived shelf at the list's end (Zeron's "Show archived"), where
	-- the caller has one.
	archived: State<boolean>?,
	-- A project filter over the list (Zeron's), where the caller has one.
	project_filter: State<boolean>?,
	-- The empty projects' fold at the end of the projects (those it says
	-- are `empty`): listed there, or (off) not at all. On unless the caller
	-- says.
	empty_projects: State<boolean>?,
	-- A folded group (or Pinned, or a section of yours) still lists its
	-- sessions in a state (working, asking, done unseen: those whose corner
	-- shows a glyph), each leaving as it settles. On unless the caller says.
	keep_active: State<boolean>?,
}
-- How grouped sessions are ordered (Zeron's Sort): the most recently
-- updated first, or the most recently made. (Pinned and sections keep the
-- order they were put in.)
export type Sort = "updated" | "created"
-- A section of your own: its name, and its sessions (by id) in the order
-- they were put there.
export type Section = { id: string, name: string, sessions: { string } }
-- Where sessions are put by hand, in order: pinned, and each section. A
-- session is in one place at most; put in one, it leaves its group.
export type Places = { pinned: { string }, sections: { Section } }
-- What may change: a list as it is, or read through a state or a function.
export type Live<T> = T | State<T> | () -> T
export type Props = {
	-- Compact (a line a session) or regular (two).
	compact: State<boolean>,
	-- Its view options (the "…" at its top opens them); each its own by
	-- default. Its rail and it share them when given the same.
	organize: State<any>?,
	-- Both levels in the view menu's order (its rows' handles reorder them;
	-- Organize is those on, in it): `{ "project", "device" }` by default.
	organize_order: State<{ string }>?,
	sort: State<Sort>?,
	details: Details?,
	-- Where sessions are put by hand (dragged, or a section made): pinned,
	-- and custom sections. By default, `pinned`'s and none.
	places: State<Places>?,
	-- This device's name: first among the devices, a laptop (the rest are
	-- remote).
	here: Live<string?>?,
	-- Collapsed, it goes (sliding away whole); `island = true` is its rail
	-- instead: a floating island of its icons, shown while it's collapsed
	-- (placed by the app over the page, at the window's bottom left).
	collapsed: State<boolean>,
	-- The most it may take (the window's fit, short of room): it narrows to
	-- this before it's hidden, laying its rows out within it.
	limit: (() -> number)?,
	-- Whether a change of width now should follow at once, not ease (the
	-- window being resized).
	direct: (() -> boolean)?,
	island: boolean?,
	-- Its width open (dragging its edge resizes it, 242 to 400).
	width: State<number>?,
	selected: State<string?>,
	pinned: Live<{ Session }>?,
	projects: Live<{ Project }>,
	-- (A state or a function for an account that arrives or changes.)
	user: Live<User>?,
	-- Room left at its top for a title bar laid over it (the window's
	-- buttons): it runs to the window's top edge, its hairline with it.
	top: number?,
	-- A new chat: from the top (no project), or a project's header (its id);
	-- from a device's header inside a project's (or a project's inside a
	-- device's), on that device too (its name, as the sessions have it).
	on_new: ((project: string?, device: string?) -> ())?,
	on_search: (() -> ())?,
	-- Plugins' pages ("pages.add"), each a row under Search: `{ plugin, id,
	-- title, icon? }` as a state of a list, and what choosing one does.
	pages: State<{ any }>?,
	on_page: ((page: any) -> ())?,
	on_archive: ((id: string) -> ())?,
	-- You, at its foot, chosen (your profile), and whether your profile is
	-- open (the row shows chosen).
	on_user: (() -> ())?,
	user_open: (() -> boolean)?,
	-- Settings, its button beside you: a toggle (open, it closes them), on
	-- while they're open.
	on_settings: (() -> ())?,
	settings_open: (() -> boolean)?,
	-- How its view stands, kept by the caller so a sidebar made again (the
	-- scripts reloaded, its module replaced) comes back as it was: where its
	-- list is scrolled, and what's folded (sections, groups, Pinned).
	offset: State<number>?,
	folded: State<{ [string]: boolean }>?,
	-- The groups showing all their sessions (by group key), past the first
	-- few a group shows ("Show N more"). Kept by the caller for as long as
	-- it likes (the app: until it quits).
	expanded: State<{ [string]: boolean }>?,
	-- Written: the sessions its list shows, in its order (those folded away
	-- left out), what ⌘1–9 and the next and previous session go by.
	order: State<{ string }>?,
	-- Whether the first nine of those wear their jump's keys (⌘1–⌘9) in
	-- place of their corner: while ⌘ is held alone (the caller's to say).
	jump_hints: (() -> boolean)?,
	-- Archived sessions (the most recently moved first), for the Archived
	-- shelf when `details.archived` is on; a row's Unarchive.
	archived: Live<{ Session }>?,
	on_unarchive: ((id: string) -> ())?,
	-- A session renamed in its row (Zeron's inline rename): which one is
	-- being renamed (a double click on its row starts it; the caller may
	-- too), and the name it's given (trimmed, changed, not empty).
	renaming_session: State<string?>?,
	on_rename: ((id: string, title: string) -> ())?,
	-- A session's card, its row rested on (its id and the row's box in the
	-- window, to stand beside); nil, put away. (Zeron's hover card: the
	-- caller draws it, over the window, ds.SessionCard.)
	on_card: ((card: { id: string, x: number, y: number, w: number, h: number }?) -> ())?,
	-- The sessions shown are a narrowed few (a filter is on): your sections
	-- left with none of them fold away, as empty groups do.
	narrowed: (() -> boolean)?,
	-- A line between the top and the list (what the filter shows, and the
	-- way back to all of it).
	notice: Node?,
	-- Over you at the foot, under the list (a banner of the app's).
	tip: Node?,
	-- A session's context menu (a secondary click on its row): its entries
	-- (ds/context_menu's), nil for none.
	menu: ((id: string) -> { ContextMenu.Entry }?)?,
	-- A project group's menu, on its header (Rename…, Remove…), by its id.
	project_menu: ((id: string) -> { ContextMenu.Entry }?)?,
}

-- Widths: Zeron's default, the inset either side, and the leading slot (a
-- compact row's height); collapsed, the slot and its insets.
local WIDE, PAD, SLOT = 256, 8, 30
-- How far it resizes (Zeron's: 242 to 400, 256 by default).
local MIN_W, MAX_W = 242, 400
-- A regular session row: two lines (the title's 17, a gap, the detail's 14)
-- inside a compact row's top and bottom inset.
local TALL = 46
-- A section's title (Pinned, yours, the groups'): a short row over its
-- sessions.
local TITLE = 24
-- How far an icon beside a name sits from its line's middle (down;
-- negative, up). An icon is taller than a name's capitals, so centred it
-- hangs below the baseline and reads low: it sits half a point up.
local OPTICAL = -0.5
-- A flyout's rows: the default width's.
local FLY_ROW = WIDE - PAD * 2
-- Where sessions with no device are listed, by device: its name in the data
-- (and its group's key), drawn in the locale's words (`named`).
local UNKNOWN = "Unknown device"
-- How tall a flyout's list stands before it scrolls: eleven compact rows and
-- a little of the next, so the cut says there is more.
local FLY_ROOM = 400
-- How everything moves: one ease.
local SLIDE = { duration = 260, ease = "out(3)" }
local FADE = { duration = 160 }
-- Rows moving aside for a dragged one, and closing up after it (Zeron's
-- TAB_SLIDE: 150 ms, easing out).
local DRAG_SLIDE = { duration = 150, ease = "out(3)" }
-- How many of a group's sessions show before "Show N more" (the session
-- open, and with `keep_active` those in a state, showing past them).
local GROUP_FIRST = 5

-- Which state needs you most: a project's badge shows its most pressing.
local URGENCY: { [string]: number } = {
	approval = 1,
	input = 2,
	failed = 3,
	limited = 4,
	working = 5,
	connecting = 6,
	plan = 7,
	done = 8,
	waiting = 9,
}

-- Sessions as a place holds them: a list read once, or a function a part
-- reads through, so it follows them in place as they move (the rail's
-- tiles, as sessions are pinned or put in a section).
type Sessions = { Session } | () -> { Session }
local function listed(sessions: Sessions): { Session }
	return if type(sessions) == "function" then sessions() else sessions
end

local function pressing(sessions: { Session }): status.Kind?
	local best: status.Kind? = nil
	for _, s in sessions do
		if s.status and (best == nil or URGENCY[s.status] < URGENCY[best :: string]) then
			best = s.status
		end
	end
	return best
end

local function list_gap(): number
	return (ui.theme.get("zeron.list.gap") :: number?) or 2
end
-- A row's words' inset from its edge, with no slot before them.
local function words_inset(): number
	return (ui.theme.get("space.2") :: number?) or 8
end

-- A small icon button in a row's corner (shown under the pointer).
local corner = SessionRow.corner_button

return ui.component(function(props: Props): Node
	local compact = props.compact
	local collapsed = props.collapsed
	-- The rail (the island instance): every row its icon alone, every
	-- group folded into its tile.
	local is_island = props.island == true
	-- The rail's measure: the theme's island (the title bar's, turned): its
	-- controls' size, its padding, the gap between its groups.
	local im = island.metrics()
	local RAIL_SLOT, RAIL_GROUP = im.control, im.group
	local function railed(): boolean
		return is_island
	end
	-- Its width open, and the width it shows: open, collapsed (the rail's,
	-- or none), or between (easing as it collapses and opens; at once while
	-- its edge is dragged).
	local wide = props.width or ui.use_state(WIDE)
	-- The width it lays out at: its own, or, short of room, its limit.
	local limit = props.limit
	local function room(): number
		return if limit then math.min(wide:get(), limit()) else wide:get()
	end
	local function room_now(): number
		return if limit then math.min(wide:peek(), limit()) else wide:peek()
	end
	local function target(): number
		if collapsed:get() then
			return 0
		end
		return room()
	end
	local shown_w = ui.use_state(target())
	local resizing = false
	local dragging = ui.use_state(false)
	-- (Its edge dragged, told to the page beside it, which holds what it
	-- shows at one width until the drag ends: screens/shell/page. Made
	-- here, in the app's sidebar, never at the module's top, which plugins
	-- are lent.)
	local edge_dragging = ui.shared("shell.edge_dragging", false)
	local edge_over = ui.use_state(false)
	local from_x, from_w = 0, 0
	local easing: any = nil
	local was_folded = collapsed:peek()
	-- Its width is where it's going (`base`, which its edge dragged or the
	-- window resized moves at once) and, after a step (folding, coming
	-- back, a neighbour narrowing or freeing it), what's left of the step
	-- (`step`), easing to nothing on its own clock (Zeron's column motion,
	-- ds/motion): it glides whatever the window does meanwhile, and is over
	-- in its time.
	local base = shown_w:peek()
	local step = ui.use_state(0)
	ui.effect(function()
		local to = target()
		local folded = collapsed:get()
		local turned = folded ~= was_folded
		was_folded = folded
		-- (Asked every time, so the caller's answer follows what moved.)
		local direct = props.direct ~= nil and props.direct()
		if to == base then
			return
		end
		if resizing then
			if easing then
				easing:cancel()
				easing = nil
			end
			step:set(0)
			base = to
		elseif turned or not direct then
			local showing = base + step:peek()
			base = to
			if easing then
				easing:cancel()
				easing = nil
			end
			step:set(showing - to)
			local params = motion.column(step:peek(), 0)
			local this: any = nil
			params.onComplete = function()
				if easing == this then
					easing = nil
				end
			end
			this = anime.animate(step, params)
			easing = this
		else
			base = to
		end
		shown_w:set(math.max(base + step:peek(), 0))
	end)
	-- (Following the step as it eases.)
	ui.effect(function()
		local w = math.max(base + step:get(), 0)
		if shown_w:peek() ~= w then
			shown_w:set(w)
		end
	end)
	local selected = props.selected
	-- (Reduce motion: there at once; ds/motion.)
	local function eased(f: () -> number, how: any): any
		return motion.tween(ui.computed(f), how)
	end
	-- What shows only at full width fades with the collapse.
	local shown_wide = eased(function(): number
		return if collapsed:get() then 0 else 1
	end, FADE)
	-- Its view options: how it groups, and what a session shows.
	local organize: State<any> = props.organize or ui.use_state({ "project" } :: any)
	-- Both levels in the order the view menu's rows stand (one off, still
	-- in its place): Organize is those on, in this order.
	local organize_order: State<{ string }> = props.organize_order or ui.use_state({ "project", "device" })
	local function full_order(v: any): { string }
		local out = SessionRow.levels(v)
		for _, l in { "project", "device" } do
			if not table.find(out, l) then
				table.insert(out, l)
			end
		end
		return out
	end
	-- (Organize set elsewhere, both on: the rows follow its order.)
	ui.effect(function()
		local on = SessionRow.levels(organize:get())
		if #on == 2 then
			local order = full_order(organize_order:peek())
			if order[1] ~= on[1] then
				organize_order:set(on)
			end
		end
	end)
	-- Its grouping as a key ("project>device"; "none"), followed or read.
	local function by_now(): Organize
		return SessionRow.organize_key(organize:get())
	end
	local function by_peek(): Organize
		return SessionRow.organize_key(organize:peek())
	end
	local details: Details = props.details
		or {
			branch = ui.use_state(true),
			pr = ui.use_state(true),
			harness = ui.use_state(true),
			project_icon = ui.use_state(true),
			location = ui.use_state(true),
		}
	local sort: State<Sort> = props.sort or ui.use_state("updated" :: Sort)
	-- Its data as it stands (followed where it's read).
	local function follow<T>(v: Live<T>?, empty: T): T
		if v == nil then
			return empty
		elseif type(v) == "function" then
			return (v :: any)()
		elseif type(v) == "userdata" or (type(v) == "table" and getmetatable(v :: any) ~= nil) then
			-- (A state, or a computed.)
			return (v :: any):get()
		end
		-- (A plain value or list, as it is.)
		return v :: any
	end
	-- What the data says, worked out once a change: each session's project
	-- (for its icon and its place, grouped otherwise), every session by id
	-- (under projects, and pinned ones outside any), every session under a
	-- project in the projects' order, and the devices they run on (this one
	-- first, then as they come).
	type Index = {
		projects: { Project },
		project_of: { [string]: Project },
		-- (Each project by its id, sessions or not: its tile as it is now.)
		by_project: { [string]: Project },
		by_id: { [string]: Session },
		pinned_ids: { string },
		everything: { Session },
		devices: { string },
		here: string?,
	}
	local index = ui.computed(function(): Index
		local projects = follow(props.projects, {} :: { Project })
		local here: string? = follow(props.here, nil :: string?)
		local out: Index = { projects = projects, project_of = {}, by_project = {}, by_id = {}, pinned_ids = {}, everything = {}, devices = {}, here = here }
		for _, p in projects do
			out.by_project[p.id] = p
			for _, s in p.sessions do
				out.project_of[s.id] = p
				out.by_id[s.id] = s
				table.insert(out.everything, s)
			end
		end
		for _, s in follow(props.pinned, {} :: { Session }) do
			out.by_id[s.id] = s
			table.insert(out.pinned_ids, s.id)
		end
		local seen: { [string]: boolean } = {}
		if here then
			for _, s in out.everything do
				if s.device == here then
					table.insert(out.devices, here)
					seen[here] = true
					break
				end
			end
		end
		for _, s in out.everything do
			local d = s.device or UNKNOWN
			if not seen[d] then
				seen[d] = true
				table.insert(out.devices, d)
			end
		end
		return out
	end)
	local function project_of(id: string): Project?
		return index:get().project_of[id]
	end
	-- A project's tile as it is now (its picture chosen again): its hue,
	-- artwork and emoji, each followed in place by the tile that shows it.
	local function look(p: Project): (() -> string?, () -> string?, () -> string?)
		local function now(): Project
			return index:get().by_project[p.id] or p
		end
		return function(): string?
			return now().hue
		end, function(): string?
			return now().src
		end, function(): string?
			return now().emoji
		end
	end
	local pinned_ids = index:peek().pinned_ids
local places: State<Places> = props.places or ui.use_state({ pinned = pinned_ids, sections = {} } :: Places)
	-- Where each session is put by hand: "pinned", or its section's id.
	local function placed_in(p: Places): { [string]: string }
		local at: { [string]: string } = {}
		for _, id in p.pinned do
			at[id] = "pinned"
		end
		for _, sec in p.sections do
			for _, id in sec.sessions do
				at[id] = sec.id
			end
		end
		return at
	end
	-- Puts a session in `list` ("pinned", a section's id; nil, back in its
	-- group) at `index` (among the rest there), out of wherever it was.
	local function place(id: string, list: string?, index: number?)
		local p = places:peek()
		local function without(ids: { string }): { string }
			local out: { string } = {}
			for _, other in ids do
				if other ~= id then
					table.insert(out, other)
				end
			end
			return out
		end
		local function into(ids: { string }): { string }
			table.insert(ids, math.clamp(index or #ids + 1, 1, #ids + 1), id)
			return ids
		end
		local pinned = without(p.pinned)
		if list == "pinned" then
			into(pinned)
		end
		local sections: { Section } = {}
		for _, sec in p.sections do
			local ids = without(sec.sessions)
			if list == sec.id then
				into(ids)
			end
			table.insert(sections, { id = sec.id, name = sec.name, sessions = ids })
		end
		places:set({ pinned = pinned, sections = sections })
	end
	-- The sections changed: `edit` takes a copy of them and returns them.
	local function sections_edit(edit: ({ Section }) -> { Section })
		local p = places:peek()
		local copy: { Section } = {}
		for _, sec in p.sections do
			table.insert(copy, { id = sec.id, name = sec.name, sessions = table.clone(sec.sessions) })
		end
		places:set({ pinned = p.pinned, sections = edit(copy) })
	end
	-- A session being dragged: which, from which list and where in it, and
	-- the room it takes (its row and the gap after it). Where it would land:
	-- a list, and where among the rest there.
	type Drag = { id: string, list: string, index: number, h: number }
	type Over = { list: string, index: number }
	local drag = ui.use_state(nil :: Drag?)
	local over = ui.use_state(nil :: Over?)
	-- How far down its row a dragged session was grabbed: the preview keeps
	-- that point under the pointer, and a drop lands the row where the
	-- preview was.
	local grab_dy = 0
	-- A section being renamed (just made, or "Rename" chosen).
	local renaming = ui.use_state(nil :: string?)
	-- (The session being renamed in its row: the caller's, else its own.)
	local session_renaming: State<string?> = props.renaming_session or ui.use_state(nil :: string?)
	-- Each section's fold, by its id.
	local folded = props.folded or ui.use_state({} :: { [string]: boolean })
	-- The groups showing all their sessions, by key.
	local expanded = props.expanded or ui.use_state({} :: { [string]: boolean })
	-- Whether a folded place still lists its sessions in a state.
	local keep_active: State<boolean> = details.keep_active or ui.use_state(true)
	-- The first nine sessions the list shows, by id: their jump (⌘1–⌘9).
	local slots = ui.use_state({} :: { [string]: number })
	local function hinting(): boolean
		return props.jump_hints ~= nil and props.jump_hints() and not railed()
	end
	-- Regular's second line: where it runs (what its group doesn't say),
	-- then its branch, as the view shows them (ds/session_row's rule, the
	-- panel home's lists' too).
	local function detail(s: Session, by: Organize): string
		local p = project_of(s.id)
		return SessionRow.detail(if p then p.name else nil, s.device, s.branch, by, details.location:get(), details.branch:get())
	end
	-- A session's height: regular, two lines while it has a second to show.
	local function session_h(s: Session, by: Organize): number
		if compact:get() or detail(s, by) == "" then
			return SLOT
		end
		return TALL
	end
	-- Where the pointer is (window height) as a drag moves over the list,
	-- for scrolling it near its ends; and whether a section is being
	-- dragged (a session's drag is `drag`).
	local drag_y: number? = nil
	local held = ui.use_state(false)
	-- A session's drag starting and ending (the list's, below).
	local start_drag: (Drag) -> ()
	local end_drag: () -> ()
	-- The list's scroll position, and its element.
	local list_offset = props.offset or ui.use_state(0)
	local list_view = ui.ref()

	-- A scroll's edge fades: each as deep as there is more past it that way
	-- (up to `depth`, 12 by default), easing out as an end comes. (`over`:
	-- how far its content runs past its view.)
	local function edge_fades(offset: State<number>, over: () -> number, depth: number?): () -> { number }
		local d = depth or 12
		return function(): { number }
			local o = offset:get()
			return { math.min(o, d), math.min(math.max(0, over() - o), d) }
		end
	end

	-- How things come and go, so nothing is ever seen half cut: going, what
	-- a place holds fades out quickly, then its place closes; coming, the
	-- place opens, then what it holds fades in once there is room. (Each way
	-- its own timing, so an effect plays it rather than a tween.)
	local GO = 90
	local COME, COME_AFTER = 180, 140
	-- (Closing waits out the fade, so nothing is cut while it can be seen.)
	local CLOSE, CLOSE_AFTER = 240, GO
	local OPEN = 260
	-- (The empty projects' fold: its tiles fly as it opens and closes, so it
	-- moves in one stroke, quicker, its height and ink together.)
	local FLY = 200
	-- What fades with `shown`: away at once, back in once there is room.
	-- (Its props, `props`, changed in place and returned.)
	local function fades(props: ContainerProps, shown: () -> boolean): ContainerProps
		local el: Element? = nil
		local was = shown()
		ui.effect(function()
			local on = shown()
			if on == was then
				return
			end
			was = on
			if el then
				local opacity: Tween = if on then { to = 1, duration = COME, delay = COME_AFTER } else { to = 0, duration = GO }
				anime.animate(el, { opacity = opacity })
			end
		end)
		props.opacity = if was then 1 else 0
		props.on_mount = function(e: Element)
			el = e
		end
		return props
	end
	-- A place that folds: as tall as `tall` says while `shown`, else `low`
	-- (none, by default); what it holds fading first as it goes, last as it
	-- comes. (`fade = false`: what it holds fades on its own.) `together`:
	-- its fade runs with its fold (the last thing in a list, with nothing
	-- after it to slide in its place: a quick fade first and the fold after
	-- would read as a blink).
	local function folds(
		props: ContainerProps,
		shown: () -> boolean,
		tall: () -> number,
		low: number?,
		fade: boolean?,
		together: boolean?
	): ContainerProps
		local el: Element? = nil
		local rest = low or 0
		local fading = fade ~= false
		-- What it was built as: it starts there, and moves only when that
		-- changes. (Built again unseen, its rows made afresh as the list comes
		-- back into view, it would otherwise grow from nothing as it shows.)
		local was_on = shown()
		local was_h = if was_on then tall() else rest
		ui.effect(function()
			local on = shown()
			local h = if on then tall() else rest
			if on == was_on and h == was_h then
				return
			end
			was_on, was_h = on, h
			local target = el
			if not target then
				return
			end
			if together then
				local opacity: Tween? = if fading
					then { to = if on then 1 else 0, duration = if on then OPEN else CLOSE, ease = SLIDE.ease }
					else nil
				anime.animate(target, motion.params({
					h = { to = h, duration = if on then OPEN else CLOSE, ease = SLIDE.ease },
					opacity = opacity,
				}))
			elseif on then
				local opacity: Tween? = if fading then { to = 1, duration = COME, delay = COME_AFTER } else nil
				anime.animate(target, motion.params({
					h = { to = h, duration = OPEN, ease = SLIDE.ease },
					opacity = opacity,
				}))
			else
				local opacity: Tween? = if fading then { to = 0, duration = GO } else nil
				anime.animate(target, motion.params({
					opacity = opacity,
					h = { to = h, duration = CLOSE, delay = CLOSE_AFTER, ease = SLIDE.ease },
				}))
			end
		end)
		local on = shown()
		props.h = if on then tall() else rest
		props.opacity = if on or not fading then 1 else 0
		props.overflow = "hidden"
		props.shrink = 0
		props.on_mount = function(e: Element)
			el = e
		end
		return props
	end
	-- The rail's flyouts: one open at a time (opening one closes the rest).
	local flyouts: { State<boolean> } = {}
	local function show(open: State<boolean>)
		for _, other in flyouts do
			if other ~= open then
				other:set(false)
			end
		end
		open:set(true)
	end
	-- A flyout's state. `hovered`: its icon's hover, put out as it closes
	-- (the flyout took the pointer as it opened, so the icon never heard it
	-- leave).
	local function flyout_state(hovered: State<boolean>): State<boolean>
		local open = ui.use_state(false)
		table.insert(flyouts, open)
		ui.effect(function()
			if not open:get() then
				hovered:set(false)
			end
		end)
		return open
	end
	-- A click opens it (closing any other), or closes it.
	local function toggle(open: State<boolean>)
		if open:peek() then
			open:set(false)
		else
			show(open)
		end
	end

	-- A row: its slot, then its words (clipped as the row narrows to its
	-- slot, fading). `hovered` is the row's own; collapsed, the slot carries
	-- `name` as a tooltip. `h`: a session row's height (as its density and
	-- details make it), eased as it changes. `leads`: whether its slot shows
	-- (with none, its words start at the row's edge: a session with its
	-- harness hidden). `wide`: full width whatever the sidebar's (a flyout's
	-- rows).
	type Row = {
		lead: Node?,
		words: Node,
		-- (A function for a name that changes: a section's, renamed.)
		name: string | () -> string,
		h: (() -> number)?,
		leads: (() -> boolean)?,
		-- How far in its slot and words start (a session under its group,
		-- past the tree's gutter); its wash still runs its full width.
		indent: (() -> number)?,
		wide: boolean?,
		-- Room kept beside it at its end, in points (the foot's settings
		-- button): its wash stops short of it.
		less: number?,
		selected: (() -> boolean)?,
		hovered: State<boolean>?,
		on_click: (() -> ())?,
		-- (What plugins place by, and what's under them: ids, never records.)
		class: string?,
		provide: { [string]: any }?,
		-- Its box, read for where a session's card stands beside it.
		ref: Ref?,
		-- A press on it, with its count (a double click's second is 2).
		on_press: ((e: PointerEvent) -> ())?,
	}
	local function row(r: Row): Node
		local hovered = r.hovered or ui.use_state(false)
		local is_selected = r.selected
		local function narrow(): boolean
			return not r.wide and railed()
		end
		-- A flyout's row: its own width. The rail's: its icon. A sidebar's:
		-- the open width, less its insets (it keeps it as the sidebar slides
		-- away from over it).
		local function indent(): number
			return if r.indent then r.indent() else 0
		end
		local function width(): number
			if r.wide then
				return FLY_ROW
			elseif railed() then
				return RAIL_SLOT
			end
			return room() - PAD * 2 - (r.less or 0)
		end
		local height = if railed() and not r.wide then RAIL_SLOT elseif r.h then eased(r.h, SLIDE) else SLOT
		-- Its slot: the rail's own size there; closed, the words' inset.
		local slot = if railed() and not r.wide then RAIL_SLOT else SLOT
		local leads = r.leads
		local function slot_w(): number
			if leads and not leads() then
				return words_inset()
			end
			return slot
		end
		-- Its words. (The rail's: its icon alone; no words to keep,
		-- invisible, over the page.)
		local function words(): Node?
			if railed() and not r.wide then
				return nil
			end
			local box: ContainerProps = {
				-- Its words at the open width (on the rail, clipped as it
				-- narrows, not re-laid out).
				w = function(): number
					return (if r.wide then FLY_ROW else room() - PAD * 2 - indent() - (r.less or 0)) - slot_w()
				end,
				h_full = true,
				shrink = 0,
				r.words,
			}
			if not r.wide then
				fades(box, function(): boolean
					return not narrow()
				end)
			end
			return ui.div(box)
		end
		local content = {
			ui.div {
				w = if leads then eased(slot_w, SLIDE) else slot,
				h = slot,
				shrink = 0,
				items = "center",
				justify = "center",
				overflow = if leads then "hidden" else nil,
				color = "$text.secondary",
				-- (Beside its words, centred where they read; alone on the
				-- rail, on its box.)
				ui.div {
					y = if railed() and not r.wide then 0 else OPTICAL,
					if leads
						then ui.div(fades({ r.lead }, leads))
						else r.lead,
				},
			},
			words(),
		}
		-- (Indented, a gutter first, inside its wash.)
		if r.indent then
			table.insert(content, 1, ui.div {
				w = function(): number
					return indent()
				end,
				-- (None now, none at all: no gap for it either.)
				hidden = function(): boolean
					return indent() <= 0
				end,
				shrink = 0,
			})
		end
		local body: Node
		if r.wide then
			-- In a flyout, a menu's row, as the dropdown's: concentric with its
			-- panel (the panel's radius less its inset), the menu's selected
			-- fill, a wash under the pointer.
			body = ui.row {
				label = r.name,
				w = width,
				h = height,
				shrink = 0,
				items = "start",
				overflow = "hidden",
				rounded = "$radius.sm",
				cursor = "pointer",
				bg = function()
					if is_selected and is_selected() then
						return "$zeron.menu.selected"
					end
					return if hovered:get() then "$hover" else "#00000000"
				end,
				active = { bg = "$pressed" },
				on_hover = function(inside: boolean)
					hovered:set(inside)
				end,
				on_click = r.on_click,
				table.unpack(content),
			}
		else
			-- A sidebar's row: the design system's flat surface (its wash under
			-- the pointer, its selected fill winning over hover and press).
			body = Surface {
				depth = "flat",
				selected = function(): boolean
					return is_selected ~= nil and is_selected()
				end,

				on_click = r.on_click,
				on_hover = function(inside: boolean)
					hovered:set(inside)
				end,
				label = r.name,
				w = width,
				h = height,
				shrink = 0,
				border = 0,
				-- On the rail, an island's control: the title bar's corner.
				rounded = if railed() then "$radius.md" else nil,
				-- (The rail's doesn't clip: a tile's badge hangs past its corner.)
				overflow = if railed() then "visible" else "hidden",
				ui.row {
					w = function(): number
						return if railed() then RAIL_SLOT else room() - PAD * 2 - (r.less or 0)
					end,
					h_full = true,
					shrink = 0,
					items = "start",
					table.unpack(content),
				},
			}
		end
		-- Collapsed, the slot alone shows: its name as a tooltip. (A tooltip
		-- is a string, read as it is built: a name that changes makes the
		-- tooltip's own empty box again, never the row.)
		local function tip(name: string): Node
			return ui.div {
				absolute = true,
				left = 0,
				top = 0,
				w = SLOT,
				h = SLOT,
				tooltip = name,
				tooltip_delay = 150,
				pointer_events = "none",
			}
		end
		local name = r.name
		return ui.div {
			class = r.class,
			provide = r.provide,
			relative = true,
			shrink = 0,
			ref = r.ref,
			on_press = r.on_press,
			body,
			ui.show(narrow, if type(name) == "function" then ui.presence(name, tip) else tip(name)),
		}
	end

	-- A row's line, its title and a quiet note: the shared row's
	-- (ds/session_row), so every list of sessions reads alike.
	local line = SessionRow.line
	local label = SessionRow.label
	local hint = SessionRow.hint

	-- A session as it stands now: `s` while there's no newer.
	-- (Through a computed of its own: the row's bindings wake when its
	-- session's table changes, not whenever any session's does. A caller
	-- that keeps a session's table while it's the same keeps them asleep.)
	local function now_of(s: Session): () -> Session
		local last = s
		local row = ui.computed(function(): Session
			local cur = index:get().by_id[s.id]
			if cur then
				last = cur
			end
			return last
		end)
		return function(): Session
			return row:get()
		end
	end
	-- A session's agent: its mark, faint unless the session asks to be seen.
	local function agent(s: Session, strong: () -> boolean): Node
		return SessionRow.mark(now_of(s), strong)
	end
	-- A session's slot: its agent; collapsed, its state (if it has one, its
	-- corner unseen).
	local function session_glyph(s: Session, strong: () -> boolean): Node
		local cur = now_of(s)
		-- (On the rail, its state's glyph, one of its kind.)
		return ui.div {
			-- (Its glyph follows its state in place: ds/status.)
			ui.show(
				function(): boolean
					return railed() and cur().status ~= nil
				end,
				status.StatusIcon {
					kind = function(): status.Kind
						return (cur().status or "working") :: status.Kind
					end,
					size = 16,
				}
			),
			ui.show(function(): boolean
				return not railed() or cur().status == nil
			end, agent(s, strong)),
		}
	end
	-- A session's corner: its state (a glyph of its kind), else its time.
	local function standing(s: Session): Node
		return SessionRow.standing(now_of(s))
	end
	-- Whether a session asks to be seen: in a state other than waiting, or
	-- the one open.
	local function active(s: Session): boolean
		local cur = index:get().by_id[s.id] or s
		return cur.status ~= nil and cur.status ~= "waiting"
	end

	local pr_badge = SessionRow.pr_badge
	local subagents_pill = SessionRow.subagents

	-- A session's row, as its group (`by`) shows it. In a flyout (`wide`),
	-- full width and its agent in its slot whatever the sidebar's width, and
	-- a pick closes it (`on_pick`).
	-- (`list`: the sidebar's list it stands in, "pinned", a section's id or a
	-- group's key, and `index`, where in it: draggable there.)
	-- A group's tree, as Zeron's transcript draws its tools (transcript.rs,
	-- `activity_rail`): a hairline trunk down from under the group's tile
	-- (`trunk`: how far in), and at each session a rounded elbow off it
	-- toward the row, which starts a little past the elbow's end. `last`:
	-- the trunk ends at that one.
	type Tree = SessionRow.Tree
	local gutter = SessionRow.gutter
	-- Where a session's row starts, from its gutter: its harness's mark
	-- (14, centred in its slot); hidden, its words (its project's icon, or
	-- its title), at their inset.
	local function lead_at(): number
		return if details.harness:get() then (SLOT - 14) / 2 else words_inset()
	end
	-- (`tree`: under a group's header, a branch off its trunk; `last`, the
	-- trunk ending at it.)
	-- (In the sidebar's list, `list_now` and `tree_of` follow the row as it
	-- moves between lists, the row staying itself.)
	type Place = { wide: boolean?, on_pick: (() -> ())?, list: string?, list_now: (() -> string)?, at: (() -> number)?, tree: Tree?, tree_of: (() -> Tree?)? }
	-- A row under its group's header (`tree_of`: its tree as it stands;
	-- nil, out of any): a branch off the group's trunk (ds/session_row).
	local function on_tree(body: Node, now_tree: () -> Tree?): Node
		return SessionRow.on_tree(body, now_tree, lead_at, list_gap())
	end
	-- A session's card (Zeron's hover card, shell/session_hover_card.rs):
	-- the pointer resting on a row 550 ms, the caller shows its card beside
	-- it (`on_card`, told the row's box; nil, none); within 400 ms of one
	-- going, the next shows at once. A press, a scroll or a rename puts it
	-- away (and the next waits its full rest); on the rail, in a flyout,
	-- none.
	local DWELL, WARM = 550, 400
	local card_shown: string? = nil
	local card_waiting: string? = nil
	local card_cancel: (() -> ())? = nil
	local card_gone_at = -math.huge
	local function card_hide(cold: boolean?)
		if card_cancel then
			card_cancel()
			card_cancel = nil
		end
		card_waiting = nil
		if card_shown then
			card_shown = nil
			card_gone_at = if cold then -math.huge else ui.now()
			if props.on_card then
				props.on_card(nil)
			end
		elseif cold then
			card_gone_at = -math.huge
		end
	end
	local function card_rest(id: string, el: Ref)
		if not props.on_card or card_shown == id or card_waiting == id then
			return
		end
		if card_cancel then
			card_cancel()
			card_cancel = nil
		end
		local function show()
			card_cancel, card_waiting = nil, nil
			local box = el:peek()
			if session_renaming:peek() ~= nil or not box or box.x == nil then
				return
			end
			card_shown = id
			if props.on_card then
				props.on_card({ id = id, x = box.x or 0, y = box.y or 0, w = box.width or 0, h = box.height or 0 })
			end
		end
		if card_shown ~= nil or ui.now() - card_gone_at < WARM then
			show()
		else
			card_waiting = id
			card_cancel = ui.after(DWELL, show)
		end
	end
	local function card_leave(id: string)
		if card_waiting == id then
			card_hide()
		elseif card_shown == id then
			card_hide()
		end
	end
	-- (Scrolled, or a row being renamed: away.)
	ui.effect(function()
		list_offset:get()
		session_renaming:get()
		card_hide(true)
	end)

	local function session_row(s: Session, by: Organize, place: Place?): Node
		local in_flyout = place ~= nil and place.wide == true
		-- (On the Archived shelf: Unarchive under the pointer, no drag.)
		local shelved = place ~= nil and place.list == "archived"
		local on_pick = if place then place.on_pick else nil
		local hovered = ui.use_state(false)
		-- Its jump (1–9) while ⌘ is held alone and it's among the first nine
		-- shown; nil otherwise (a flyout's and the shelf's rows have none).
		local jumpable = not in_flyout and not shelved
		local function jump(): number?
			if not jumpable or not hinting() then
				return nil
			end
			return slots:get()[s.id]
		end
		-- Its card, as the pointer rests on it or leaves (not on the rail,
		-- nor in a flyout).
		local card_ref = ui.ref()
		if not in_flyout then
			local was = false
			ui.effect(function()
				local on = hovered:get() and not railed()
				if on and not was then
					card_rest(s.id, card_ref)
				elseif was and not on then
					card_leave(s.id)
				end
				was = on
			end)
			ui.on_cleanup(function()
				card_leave(s.id)
			end)
		end
		-- The session as it stands: what the row shows of it follows it.
		local cur = now_of(s)
		local function second_line(): boolean
			return not compact:get() and detail(cur(), by) ~= ""
		end
		-- Regular's second line: where it runs, and on what (its branch's
		-- glyph leading while the branch shows).
		local second = SessionRow.second_line(function(): string
			return detail(cur(), by)
		end, function(): boolean
			return details.branch:get() and cur().branch ~= nil
		end, second_line, fades)
		local function strong(): boolean
			return active(s) or selected:get() == s.id or in_split(s.id)
		end
		-- Its project's icon beside its title, where its group isn't its
		-- project.
		local p = project_of(s.id)
		-- Its title, or (renamed in its row) a field over it: the title
		-- selected, ↩ or leaving it keeps a changed name, Esc keeps the old.
		local function editing(): boolean
			return not in_flyout and session_renaming:get() == s.id
		end
		local draft = ui.use_state(s.title)
		local function rename_done(keep: boolean)
			if session_renaming:peek() ~= s.id then
				return
			end
			session_renaming:set(nil)
			local v = string.match(draft:peek(), "^%s*(.-)%s*$") or ""
			if keep and v ~= "" and v ~= cur().title and props.on_rename then
				props.on_rename(s.id, v)
			end
		end
		local title: Node = ui.show(
			editing,
			ui.input(EditorMenu.on({
				value = draft,
				label = locale.t("ds.sidebar.session_name"),
				placeholder = locale.t("ds.sidebar.session_name"),
				grow = 1,
				min_w = 0,
				h = 20,
				px = 0,
				border = 0,
				bg = "#00000000",
				focus = { border = 0, outline = 0 },
				text_size = "$font.size.md",
				weight = "$font.weight.medium",
				color = "$text.primary",
				select_on_focus = true,
				on_key = function(e): boolean
					if e.key == "Enter" then
						rename_done(true)
						return true
					elseif e.key == "Escape" then
						rename_done(false)
						return true
					end
					return false
				end,
				on_blur = function()
					rename_done(true)
				end,
				on_mount = function(el)
					draft:set(cur().title)
					-- (At once: the press that started it is over. Not yet laid
					-- out, a frame or two later.)
					if el:focus() then
						return
					end
					local tries = 0
					local function take()
						if session_renaming:peek() ~= s.id then
							return
						end
						tries += 1
						if not el:focus() and tries < 10 then
							ui.after(16, take)
						end
					end
					ui.after(16, take)
				end,
			})),
			label(function(): string
				return cur().title
			end, strong, nil)
		)
		if p and not SessionRow.groups_by(by, "project") then
			title = ui.row {
				grow = 1,
				min_w = 0,
				gap = "$zeron.space.tight",
				items = "center",
				ui.show(details.project_icon, ui.div { shrink = 0, y = OPTICAL, if p.icon then Tile { inside = Icon { name = p.icon, size = 10 }, size = 16 } else (function(): Node
					local hue, src, emoji = look(p)
					return Tile { label = p.name, hue = hue, src = src, emoji = emoji, size = 16 }
				end)() }),
				title,
			}
		end
		-- `.session-title`: the row's title, what plugins place beside (a
		-- badge after it stays in the row, outside the title's fade).
		title = ui.row {
			class = "session-title",
			grow = 1,
			min_w = 0,
			items = "center",
			title,
		}
		local body = row {
			class = "session-row",
			provide = { session = s.id },
			ref = card_ref,
			on_press = if in_flyout
				then nil
				else function(e: PointerEvent)
					-- (A press puts its card away, the next waiting its rest.)
					card_hide(true)
					-- (Once the click is over, as Zeron's starts on the second
					-- click: nothing of the press left to take focus back.)
					if e.clicks == 2 and props.on_rename then
						ui.after(0, function()
							session_renaming:set(s.id)
						end)
					end
				end,
			name = function(): string
				return cur().title
			end,
			h = function(): number
				return session_h(cur(), by)
			end,
			indent = if place and place.tree_of
				then function(): number
					local t = (place.tree_of :: () -> Tree?)()
					return if t then gutter(t) else 0
				end
				elseif place and place.tree then function(): number
					return gutter(place.tree :: Tree)
				end
				else nil,
			-- (Its slot holds its harness; hidden, the slot closes.)
			leads = function(): boolean
				return details.harness:get()
			end,
			wide = in_flyout,
			hovered = hovered,
			lead = if in_flyout then agent(s, strong) else session_glyph(s, strong),
			words = ui.column {
				w_full = true,
				gap = "$zeron.space.hair",
				line(
					title,
					if s.pr then ui.show(details.pr, pr_badge(s.pr)) else nil,
					subagents_pill(function(): number
						return cur().subagents or 0
					end),
					-- Its state (or time) at rest; under the pointer, archive (on
					-- the shelf, unarchive); ⌘ held alone, its jump's keys.
					SessionRow.Corner {
						cur = cur,
						hovered = hovered,
						jump = jump,
						action = if shelved
							then corner("ds.sidebar.unarchive", "inbox", function()
								if props.on_unarchive then
									props.on_unarchive(s.id)
								end
							end)
							else corner("ds.sidebar.archive", "archive-minimalistic", function()
								if props.on_archive then
									props.on_archive(s.id)
								end
							end),
					}
				),
				-- Pulled up under the title: the line's own inset is the row's.
				ui.div { mt = ui.raw(-8, "up a row's inset, under the title"), second },
			},
			-- (Every chat of the split showing is selected too.)
			selected = function(): boolean
				return selected:get() == s.id or in_split(s.id)
			end,
			on_click = function()
				selected:set(s.id)
				if on_pick then
					on_pick()
				end
			end,
		}
		-- Its menu, on a secondary click (the app's entries).
		if props.menu then
			local menu = props.menu
			body = ContextMenu {
				label = locale.t("ds.sidebar.session_actions"),
				ring = "$radius.md",
				w_full = true,
				shrink = 0,
				items = function()
					return menu(s.id)
				end,
				body,
			}
		end
		-- Under its group's header: a branch off the group's trunk.
		local tree = if place then place.tree else nil
		local tree_of: (() -> Tree?)? = if place then place.tree_of else nil
		if tree_of == nil and tree then
			local fixed = tree :: Tree
			tree_of = function(): Tree?
				return fixed
			end
		end
		if tree_of then
			body = on_tree(body, tree_of :: () -> Tree?)
		end
		local list = if place then place.list else nil
		if not list then
			return body
		end
		-- In the sidebar's list: dragged by its row (lifted as a card of the
		-- menus' glass; its own place left empty as it closes up behind it).
		local at = (place :: Place).at
		local function dragged(): boolean
			local d = drag:get()
			return d ~= nil and d.id == s.id
		end
		local list_now = (place :: Place).list_now
		return ui.div {
			w_full = true,
			shrink = 0,
			opacity = function(): number
				return if dragged() then 0 else 1
			end,
			on_press = function(e)
				grab_dy = e.local_y
			end,
			on_drag = function(): any
				-- (Archived: it stays on its shelf.)
				if shelved then
					return nil
				end
				local from = if list_now then list_now() else list :: string
				start_drag({ id = s.id, list = from, index = if at then at() else 1, h = session_h(cur(), by) + list_gap() })
				-- (The page's split shows where it can land.)
				split_dragging:set(true)
				return { session = s.id }
			end,
			drag_preview = function()
				local card: ContainerProps = {
					w = room_now() - PAD * 2,
					h = session_h(cur(), by),
					items = "start",
					rounded = "$radius.md",
					ui.div {
						w = SLOT,
						h = SLOT,
						shrink = 0,
						items = "center",
						justify = "center",
						color = "$text.secondary",
						agent(s, function(): boolean
							return true
						end),
					},
					line(label(cur().title)),
				}
				Glass.style(card, "menu")
				return ui.row(card)
			end,
			on_drag_end = function()
				split_dragging:set(false)
				end_drag()
			end,
			body,
		}
	end

	-- Collapsed: a group's sessions beside the rail, from its icon (`anchor`),
	-- as the sidebar shows them (compact or regular, their states, their
	-- badges); a pick closes it.
	local function fly(title: string | () -> string, sessions: Sessions, by: Organize, open: State<boolean>, anchor: Node): Node
		-- Where its list is scrolled: its own (each flyout keeps its place).
		local offset = ui.use_state(0)
		return ui.popover(Glass.style({
			open = open,
			anchor = anchor,
			label = title,
			placement = { side = "right", align = "start", offset = PAD + 4 },
			-- Its rows' width, inside its inset and its border either side.
			w = WIDE - PAD * 2 + 8 + 2,
			p = "$space.1",
			gap = "$zeron.list.gap",
			rounded = "$radius.lg",
			build = function()
				-- Its title on a short row, so the gap under it is the rows'.
				local function close()
					open:set(false)
				end
				-- Its rows, each with the gap under it but the last: a list, as
				-- a group can hold thousands, as tall as they are up to FLY_ROOM,
				-- and past that it scrolls, a gesture over it staying with it.
				-- (Its sessions as they stand, followed: one pinned or moved while
				-- it is open comes or goes in place.)
				type Laid = { ids: { string }, heights: { number }, total: number }
				local laid = ui.computed(function(): Laid
					local list = listed(sessions)
					local ids: { string } = {}
					local heights: { number } = {}
					local out: Laid = { ids = ids, heights = heights, total = 0 }
					for i, s in list do
						local h = session_h(s, by) + (if i < #list then list_gap() else 0)
						out.ids[i], out.heights[i] = s.id, h
						out.total += h
					end
					return out
				end)
				local function over(): boolean
					return laid:get().total > FLY_ROOM
				end
				return ui.column {
					w_full = true,
					gap = "$zeron.list.gap",
					Menu.Heading { if type(title) == "function" then title() else title },
					ui.list {
						w_full = true,
						h = function(): number
							return math.min(laid:get().total, FLY_ROOM)
						end,
						offset = offset,
						heights = function(): { number }
							return laid:get().heights
						end,
						overscroll = "contain",
						key = function(i: number): string
							return laid:peek().ids[i]
						end,
						row = function(id: string): Node
							local s = index:peek().by_id[id]
							if not s then
								return ui.div {}
							end
							return session_row(s, by, { wide = true, on_pick = close })
						end,
						fade = function(): { number }
							return if over() then { 12, 12 } else { 0, 0 }
						end,
						fade_overflow = over,
					},
				}
			end,
		}, "menu"))
	end

	-- Whether the session open is one of these.
	local function holds(sessions: Sessions): boolean
		local id = selected:get()
		for _, s in listed(sessions) do
			if s.id == id then
				return true
			end
		end
		return false
	end

	-- A section's title (wide).
	-- `open`: the section folds (a click on its title; folded, how many it
	-- holds at its end). Its title brightens under the pointer, no caret.
	local function section(title: Message, open: State<boolean>?, count: (() -> number)?): Node
		local foldable = open ~= nil
		-- Collapsed, no title: a little space between its group and the last.
		return ui.div(folds({
			class = "section-header",
			w_full = true,
			relative = true,
			-- (The shared section title, ds/session_row: on the sidebar's own
			-- inset, fading with its rail.)
			SessionRow.Section {
				title = title,
				open = if foldable then function(): boolean
					return (open :: State<boolean>):get()
				end else nil,
				count = count,
				on_toggle = if foldable
					then function()
						(open :: State<boolean>):set(not (open :: State<boolean>):peek())
					end
					else nil,
				frame = {
					absolute = true,
					left = 0,
					top = 0,
					w = function(): number
						return room() - PAD * 2
					end,
					pointer_events = function()
						return if collapsed:get() then "none" else "auto"
					end,
				},
				wrap = function(row: ContainerProps): ContainerProps
					return fades(row, function(): boolean
						return not railed()
					end)
				end,
			},
		}, function(): boolean
			return not railed()
		end, function(): number
			return TITLE
		end, 8, false))
	end

	-- A group's tile (a project's, a device's, pinned's): `inside` on its hue
	-- (a palette hue's 500, inked in its 50; or `{ bg, ink }`, tokens of its
	-- own), or the project's own artwork; and, when its sessions can't be
	-- seen, a badge for the most pressing of them.
	local function group_tile(hue: string | { bg: string, ink: string } | () -> string?, inside: Node, sessions: Sessions, hidden: () -> boolean, src: (string | () -> string?)?, emoji: (() -> string?)?): Node
		local function badge(kind: status.Kind): Node
			return ui.div {
				absolute = true,
				right = -3,
				top = -3,
				status.StatusDot { kind = kind, size = 8 },
			}
		end
		if type(sessions) == "table" then
			local most = pressing(sessions)
			return ui.div {
				relative = true,
				w = 18,
				h = 18,
				Tile { hue = if type(hue) == "string" or type(hue) == "function" then hue :: any else nil, inside = inside, size = 18, src = src, emoji = emoji },
				if most then ui.show(hidden, badge(most)) else nil,
			}
		end
		-- Sessions that move: the badge follows the most pressing, a dot of
		-- each kind its own.
		return ui.div {
			relative = true,
			w = 18,
			h = 18,
			Tile { hue = if type(hue) == "string" or type(hue) == "function" then hue :: any else nil, inside = inside, size = 18, src = src, emoji = emoji },
			ui.presence(function(): status.Kind?
				return if hidden() then pressing(listed(sessions)) else nil
			end, badge),
		}
	end
	-- The chrome's own tile (pinned's, a device's, the list's): quiet, a
	-- wash, not a hue.
	local QUIET = { bg = "$zeron.secondary.bg", ink = "$text.secondary" }

	-- A group: its name, its tile, its sessions, and (a project's) where a
	-- new chat in it starts.
	-- (`shows_tile`: whether its header shows its tile; a project's, as
	-- the view shows project icons. The rail always does: its tile is all
	-- it shows.)
	-- (On the rail, its name and sessions are read through functions, so its
	-- tile follows them in place; `tile` takes them so its badge does.)
	type Group = {
		key: string,
		name: string | () -> string,
		tile: (hidden: () -> boolean, sessions: Sessions?) -> Node,
		sessions: Sessions,
		new_in: boolean?,
		-- A project's group: the project (a new chat in it); a device's
		-- inside a project's, its project's too.
		project: string?,
		-- A device's group: the device's name; a project's inside a
		-- device's, that device's too (a new chat in it starts there).
		device: string?,
		-- The project's name, for a device's group inside it (its new chat's
		-- tooltip).
		project_name: string?,
		shows_tile: (() -> boolean)?,
		-- Its second level's groups, with both levels on (its sessions all
		-- of theirs).
		children: { Group }?,
		-- A project's with none but archived sessions (its `empty`), at the
		-- top level: listed in the empty projects' fold.
		empty: boolean?,
	}
	-- How long ago a session moved, in seconds (its `time`: a time, or as
	-- it reads: "now", "5m", "3h", "1d", "2w"), for the most recent first.
	local UNIT: { [string]: number } = { m = 60, h = 3600, d = 86400, w = 604800 }
	local function age(s: Session, made: boolean?): number
		local t = if made then s.created or s.time else s.time
		if type(t) == "number" then
			return os.time() - t
		elseif type(t) == "string" then
			local n, unit = string.match(t, "^(%d+)(%a)$")
			local count = if n then tonumber(n) else nil
			local scale = if unit then UNIT[unit] else nil
			if count and scale then
				return count * scale
			end
			return if t == "now" then 0 else math.huge
		end
		return math.huge
	end
	-- In the view's order (`sort`), leaving out those put elsewhere by hand.
	local function ordered(sessions: { Session }, placed: { [string]: string }): { Session }
		local order: { [string]: number } = {}
		local sorted: { Session } = {}
		for i, s in sessions do
			order[s.id] = i
			if not placed[s.id] then
				table.insert(sorted, s)
			end
		end
		local made = sort:peek() == "created"
		table.sort(sorted, function(a: Session, b: Session): boolean
			local x, y = age(a, made), age(b, made)
			if x ~= y then
				return x < y
			end
			return order[a.id] < order[b.id]
		end)
		return sorted
	end
-- The groups as `by` makes them (of the sessions not `placed` by hand),
	-- and the section they stand in (its title's message's key).
	-- Groups the most recently active first (their newest session, as the
	-- view sorts them: updated or made), those with none after in the order
	-- given (a project's own, a device's: this one first).
	local function by_recency(groups: { Group }): { Group }
		local made = sort:peek() == "created"
		local rank: { [Group]: number } = {}
		local newest: { [Group]: number } = {}
		for i, g in groups do
			rank[g] = i
			local first = listed(g.sessions)[1]
			newest[g] = if first then age(first, made) else math.huge
		end
		table.sort(groups, function(a: Group, b: Group): boolean
			if newest[a] ~= newest[b] then
				return newest[a] < newest[b]
			end
			return rank[a] < rank[b]
		end)
		return groups
	end
	-- The groups as `by` makes them (of the sessions not `placed` by hand),
	-- and the section they stand in (its title's message's key). `by` is
	-- its levels in order ("project", "device", "project>device",
	-- "device>project"; "none", one list): its first level's groups, each
	-- with its second level's inside (`children`), its sessions all of
	-- theirs (the rail's flyouts, its header's count and badge).
	local function groups_of(by: Organize, placed: { [string]: string }): ({ Group }, string)
		local ix = index:get()
		local everything, devices = ix.everything, ix.devices
		local levels = SessionRow.levels(by)
		-- One level's groups of `subset` (nil: all; at the top, projects with
		-- none show too, where a new chat starts), their keys under `prefix`;
		-- inside `outer`, its project or device carried (a new chat in an
		-- inner group starts in both).
		local function level_groups(level: string, subset: { [string]: boolean }?, prefix: string, outer: Group?): { Group }
			-- (A device the sessions don't name is nowhere a chat can start.)
			local outer_device = if outer and outer.device and outer.device ~= UNKNOWN then outer.device else nil
			local groups: { Group } = {}
			if level == "project" then
				for _, p in ix.projects do
					local mine: { Session } = {}
					for _, s in p.sessions do
						if subset == nil or subset[s.id] then
							table.insert(mine, s)
						end
					end
					if subset ~= nil and #mine == 0 then
						continue
					end
					local own = ordered(mine, placed)
					if subset ~= nil and #own == 0 then
						continue
					end
					table.insert(groups, {
						key = `{prefix}project:{p.id}`,
						name = p.name,
						sessions = own,
						new_in = true,
						project = p.id,
						device = outer_device,
						-- (Only at the top, and only while it holds none.)
						empty = if subset == nil and p.empty == true and #p.sessions == 0 then true else nil,
						shows_tile = function(): boolean
							return details.project_icon:get()
						end,
						tile = function(hidden: () -> boolean, live: Sessions?): Node
							-- (A group that isn't a folder, the chats in none, wears
							-- its glyph on the chrome's quiet wash, not a letter.)
							if p.icon then
								return group_tile(QUIET, Icon { name = p.icon, size = 12 }, live or own, hidden)
							end
							local hue, src, emoji = look(p)
							return group_tile(
								function(): string?
									return hue() or "gray"
								end,
								ui.text {
									string.upper(string.sub(p.name, 1, 1)),
									text_size = "$font.size.xs",
									weight = "$font.weight.semibold",
								},
								live or own,
								hidden,
								src,
								emoji
							)
						end,
					})
				end
			else
				for _, d in devices do
					local on: { Session } = {}
					for _, s in everything do
						if (s.device or UNKNOWN) == d and (subset == nil or subset[s.id]) then
							table.insert(on, s)
						end
					end
					on = ordered(on, placed)
					if subset ~= nil and #on == 0 then
						continue
					end
					-- (Inside a project's: a new chat in it, on this device.)
					local in_project = outer ~= nil and outer.project ~= nil and d ~= UNKNOWN
					table.insert(groups, {
						key = `{prefix}device:{d}`,
						name = d,
						device = d,
						new_in = if in_project then true else nil,
						project = if in_project then (outer :: Group).project else nil,
						project_name = if in_project then (outer :: Group).name :: string else nil,
						sessions = on,
						tile = function(hidden: () -> boolean, live: Sessions?): Node
							return group_tile(QUIET, Icon { name = if d == ix.here then "laptop" else "remote-server", size = 12 }, live or on, hidden)
						end,
					})
				end
			end
			return by_recency(groups)
		end
		if #levels > 0 then
			local groups = level_groups(levels[1], nil, "")
			if levels[2] then
				for _, g in groups do
					local subset: { [string]: boolean } = {}
					for _, s in listed(g.sessions) do
						subset[s.id] = true
					end
					g.children = level_groups(levels[2], subset, `{g.key}/`, g)
				end
			end
			return groups, if levels[1] == "project" then "ds.sidebar.projects" else "ds.sidebar.devices"
		end
		local groups: { Group } = {}
		local all = ordered(everything, placed)
		table.insert(groups, {
			key = "all",
			name = "Sessions",
			sessions = all,
			tile = function(hidden: () -> boolean, live: Sessions?): Node
				return group_tile(QUIET, Icon { name = "list", size = 12 }, live or all, hidden)
			end,
		})
		return groups, "ds.sidebar.sessions"
	end
	-- Each group's fold, kept across a change of grouping (a group folded
	-- by device stays folded, back again).
	-- (Groups come and go with the data: each fold is one key of one
	-- state, its own view of it made as the group is first met.)
	-- (Beside the sections' folds, by key: `group:…`.)
	local folded_groups = folded
	local opened: { [string]: State<boolean> } = setmetatable({}, {
		__index = function(t: any, key: string): any
			local st = {
				get = function(_): boolean
					return not folded_groups:get()[`group:{key}`]
				end,
				peek = function(_): boolean
					return not folded_groups:peek()[`group:{key}`]
				end,
				set = function(_, on: boolean)
					local f = table.clone(folded_groups:peek())
					f[`group:{key}`] = if on then nil else true
					folded_groups:set(f)
				end,
			}
			rawset(t, key, st)
			return st
		end,
	}) :: any

	-- A group's name as drawn: the two the sidebar names itself (one list's,
	-- and by device, the sessions with none's) in the locale's words; the
	-- rest their project's or device's own.
	local function named(key: string, name: string): string
		if key == "all" then
			return locale.s("ds.sidebar.sessions")
		elseif key == `device:{UNKNOWN}` or string.sub(key, -#`/device:{UNKNOWN}`) == `/device:{UNKNOWN}` then
			return locale.s("ds.sidebar.unknown_device")
		end
		return name
	end

	-- A group's new chat, under the pointer in its header's corner: in its
	-- project, and (a group with both levels' say) on its device. (`now`:
	-- the group as it stands, its project's id followed.)
	local function new_corner(g: Group, now: () -> Group): Node
		local project_name = g.project_name or g.name :: string
		local key, args = "ds.sidebar.new_chat_in", { name = project_name } :: { [string]: any }
		if g.device then
			key, args = "ds.sidebar.new_chat_in_on", { name = project_name, device = g.device }
		end
		return corner(key, "plus", function()
			if props.on_new then
				local cur = now()
				props.on_new(cur.project, cur.device)
			end
		end, args)
	end

	-- A group's tile on the rail, its flyout of its sessions hung off it.
	local function group(g: Group, by: Organize): (Node, () -> number)
		local given = g.name
		local function name(): string
			return named(g.key, if type(given) == "function" then given() else given)
		end
		-- (A section's tile on the rail has no fold of its own.)
		local open = opened[g.key]
		local function hidden(): boolean
			return railed() or not open:get()
		end
		local hovered = ui.use_state(false)
		local flyout = flyout_state(hovered)
		local function n(): string
			return tostring(#listed(g.sessions))
		end
		local header = row {
			class = "section-header",
			provide = if g.project then { project = g.project } else nil,
			name = name,
			hovered = hovered,
			lead = g.tile(hidden, g.sessions),
			leads = if railed() then nil else g.shows_tile,
			words = line(
				-- (Its name folds it, or opens it; folded, how many it holds.)
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
						return not open:get() and (not hovered:get() or not g.new_in)
					end, hint(n)),
					if g.new_in
						then ui.show(
							hovered,
							-- (A project's: its name is its own.)
							new_corner(g, function(): Group
								return g
							end)
						)
						else nil,
				}
			),
			-- Collapsed, its tile stands for the session chosen in it.
			selected = function(): boolean
				return railed() and holds(g.sessions)
			end,
			on_click = function()
				if railed() then
					toggle(flyout)
				else
					open:set(not open:peek())
				end
			end,
		}
		-- Collapsed: its sessions beside the rail, the flyout hung off a mark
		-- of its own over the header's slot (not the header: a row inside a
		-- popover wouldn't fold and fade with the rest). Wide and open, under it.
		local anchor = ui.div {
			w = if railed() then RAIL_SLOT else SLOT,
			h = if railed() then RAIL_SLOT else SLOT,
			pointer_events = "none",
		}
		-- (Its sessions show in its flyout, built as it opens: none here.)
		local function tall(): number
			return SLOT
		end
		return ui.column {
			w_full = true,
			gap = 0,
			shrink = 0,
			relative = true,
			header,
			ui.div {
				absolute = true,
				left = 0,
				top = 0,
				pointer_events = "none",
				fly(name, g.sessions, by, flyout, anchor),
			},
		}, tall
	end

	-- The top: a new chat and search, each a row (collapsed, its icon).
	local top = ui.column {
		-- Where plugins add beside New chat and Search.
		hook = if is_island then "rail.actions" else "sidebar.actions",
		id = if is_island then nil else "sidebar-actions",
		w_full = true,
		gap = "$zeron.list.gap",
		shrink = 0,
		row {
			name = function(): string
				return locale.s("ds.sidebar.new_chat")
			end,
			lead = Icon { name = "plus", size = 16 },
			words = line(label(function(): string
				return locale.s("ds.sidebar.new_chat")
			end, nil, "$font.weight.medium"), hint(ui.shortcut_text("cmd+n"))),
			on_click = function()
				if props.on_new then
					props.on_new(nil)
				end
			end,
		},
		row {
			name = function(): string
				return locale.s("ds.sidebar.search")
			end,
			lead = Icon { name = "search", size = 16 },
			words = line(label(function(): string
				return locale.s("ds.sidebar.search")
			end), hint(ui.shortcut_text("cmd+k"))),
			on_click = props.on_search,
		},
		if props.pages
			-- (Each page's row follows it: a plugin renaming its page or
			-- changing its icon shows here in place.)
			then ui.each(props.pages, function(_: any, _i: number, now: State<any>): Node
				local function title(): string
					return now:get().title or ""
				end
				return row {
					name = title,
					class = "plugin-page-row",
					lead = ui.presence(function(): string
						return now:get().icon or "widget"
					end, function(icon: string): Node
						return Icon { name = icon :: any, size = 16 }
					end),
					words = line(label(title)),
					on_click = function()
						if props.on_page then
							props.on_page(now:peek())
						end
					end,
				}
			end, {
				key = function(p: any): string
					return `{p.plugin}/{p.id}`
				end,
			})
			else nil,
	}

	-- The middle: pinned, your sections, then the groups, as one virtualized
	-- list (only the rows in view are built; a fold or a drag re-lays it
	-- out from heights it knows, building nothing offscreen). Built again,
	-- the old fading over the new, as the view's grouping or sort changes.
	local pins_open = opened["::pinned"]
	local archived_open = opened["::archived"]
	-- How many of the Archived shelf show (Zeron's: 10, then 25 more a time).
	local SHELF_FIRST, SHELF_MORE = 10, 25
	local shelf_shown = ui.use_state(SHELF_FIRST)
	local groups_open = opened["::groups"]
	-- The empty projects' fold: closed until it's opened (kept with the
	-- folds, `folded`, as open: the one fold that starts closed).
	local EMPTIES = "::empty_projects"
	local empties_open: State<boolean> = {
		get = function(_): boolean
			return folded:get()[EMPTIES] == true
		end,
		peek = function(_): boolean
			return folded:peek()[EMPTIES] == true
		end,
		set = function(_, on: boolean)
			local f = table.clone(folded:peek())
			f[EMPTIES] = if on then true else nil
			folded:set(f)
		end,
	} :: any
	-- Whether the empty projects are listed (in their fold) at all.
	local function empties_shown(): boolean
		local st = (details :: any).empty_projects :: State<boolean>?
		return st == nil or st:get()
	end
	-- The sessions put in a place, in its order (any no longer known left
	-- out).
	local function sessions_of(ids: { string }): { Session }
		local out: { Session } = {}
		local by_id = index:get().by_id
		for _, id in ids do
			local s = by_id[id]
			if s then
				table.insert(out, s)
			end
		end
		return out
	end
	-- What the list animates (its folds opening and closing, a dragged row's
	-- place closing, the gap opening where it would land), on one clock:
	-- milliseconds, running only while something moves, at the display's
	-- rate. An animation is where it goes from and to, when, and how long,
	-- easing out.
	local clock = ui.use_state(0)
	-- (`css`: CSS `ease`, ds/motion's EASE, the column motion's curve, for
	-- rows taking new places; else easing out.)
	type Anim = { from: number, to: number, at: number, dur: number, delay: number, on_done: (() -> ())?, css: boolean? }
	-- cubic-bezier(.25, .1, .25, 1) at progress `x`: its time found by
	-- Newton's method (bisecting where the slope is flat), then its value.
	local function css_ease(x: number): number
		local function bez(t: number, a: number, b: number): number
			local u = 1 - t
			return 3 * u * u * t * a + 3 * u * t * t * b + t * t * t
		end
		local t = x
		for _ = 1, 8 do
			local err = bez(t, 0.25, 0.25) - x
			if math.abs(err) < 1e-4 then
				break
			end
			local u = 1 - t
			local slope = 3 * u * u * 0.25 + 6 * u * t * (0.25 - 0.25) + 3 * t * t * (1 - 0.25)
			if math.abs(slope) < 1e-6 then
				break
			end
			t = math.clamp(t - err / slope, 0, 1)
		end
		return bez(t, 0.1, 1)
	end
	local function value(a: Anim, t: number): number
		if a.dur <= 0 then
			return a.to
		end
		local p = math.clamp((t - a.at - a.delay) / a.dur, 0, 1)
		p = if a.css then css_ease(p) else 1 - (1 - p) ^ 3
		return a.from + (a.to - a.from) * p
	end
	local function still(v: number): Anim
		return { from = v, to = v, at = 0, dur = 0, delay = 0 }
	end
	local moving: { Anim } = {}
	local ticking: any = nil
	-- Each frame: what has arrived is done (its `on_done` run); with nothing
	-- left moving, the clock stops.
	local function settle()
		local t = clock:peek()
		local left: { Anim } = {}
		local arrived: { Anim } = {}
		for _, a in moving do
			if t >= a.at + a.delay + a.dur then
				table.insert(arrived, a)
			else
				table.insert(left, a)
			end
		end
		moving = left
		for _, a in arrived do
			if a.on_done then
				a.on_done()
			end
		end
		if #moving == 0 and ticking then
			ticking:cancel()
			ticking = nil
		end
	end
	-- From where `a` is now to `to`.
	local function anim(a: Anim?, to: number, dur: number, delay: number?, on_done: (() -> ())?): Anim
		-- (Reduce motion: where it goes, at once.)
		if motion.reduced_now() then
			dur, delay = 0, 0
		end
		local t = clock:peek()
		local n: Anim = { from = if a then value(a, t) else to, to = to, at = t, dur = dur, delay = delay or 0, on_done = on_done }
		table.insert(moving, n)
		if not ticking then
			ticking = anime.animate(clock, {
				value = clock:peek() + 1e7,
				duration = 1e7,
				ease = "linear",
				onUpdate = settle,
			})
		end
		return n
	end

	-- A place's fold (pinned's, a section's, the groups', a group's): open
	-- or not, its rows' height and their ink each easing (going, the ink
	-- fades first, then the place closes; coming, it opens, then the ink
	-- fades in; `together`, both at once, for the last place, with nothing
	-- after it to slide up), and whether its rows are in the list at all
	-- (still there while it closes).
	-- (`dur`: one duration both ways, for a fold played together; else
	-- OPEN and CLOSE.)
	type Fold = { key: string, is_open: () -> boolean, write: (boolean) -> (), h: Anim, o: Anim, present: boolean, together: boolean, dur: number? }
	local shape = ui.use_state(0)
	local folds_by: { [string]: Fold } = {}
	local function fold_of(key: string, is_open: () -> boolean, write: (boolean) -> (), together: boolean?, dur: number?): Fold
		local f = folds_by[key]
		if not f then
			local on = is_open()
			f = { key = key, is_open = is_open, write = write, h = still(if on then 1 else 0), o = still(if on then 1 else 0), present = on, together = together == true, dur = dur }
			folds_by[key] = f
		end
		return f
	end
	local function fold_set(f: Fold, on: boolean)
		f.write(on)
		-- (Its rows take their places again as it starts and as it settles:
		-- while it moves, those past what shows stand in as one; see
		-- `build_shape`.)
		shape:set(shape:peek() + 1)
		local open, close = f.dur or OPEN, f.dur or CLOSE
		if on then
			f.present = true
			f.h = anim(f.h, 1, open, 0, function()
				shape:set(shape:peek() + 1)
			end)
			f.o = anim(f.o, 1, if f.together then open else COME, if f.together then 0 else COME_AFTER)
			-- (Its height on the column motion's CSS ease, ds/motion: into
			-- place with no overshoot, as it opens so it closes.)
			f.h.css = true
		else
			f.o = anim(f.o, 0, if f.together then close else GO)
			f.h = anim(f.h, 0, close, if f.together then 0 else CLOSE_AFTER, function()
				if not f.is_open() and f.present then
					f.present = false
				end
				shape:set(shape:peek() + 1)
			end)
			f.h.css = true
		end
	end
	-- The folds, by key: pinned, the groups, each group, each section.
	local function pinned_fold(): Fold
		return fold_of("pinned", function(): boolean
			return pins_open:peek()
		end, function(on: boolean)
			pins_open:set(on)
		end)
	end
	local function archived_fold(): Fold
		return fold_of("archived", function(): boolean
			return archived_open:peek()
		end, function(on: boolean)
			archived_open:set(on)
		end)
	end
	local function groups_fold(): Fold
		return fold_of("groups", function(): boolean
			return groups_open:peek()
		end, function(on: boolean)
			groups_open:set(on)
		end, true)
	end
	local function empties_fold(): Fold
		return fold_of("empties", function(): boolean
			return empties_open:peek()
		end, function(on: boolean)
			empties_open:set(on)
		end, true, FLY)
	end
	local function group_fold(key: string): Fold
		local st = opened[key]
		return fold_of(key, function(): boolean
			return st:peek()
		end, function(on: boolean)
			st:set(on)
		end)
	end
	local function section_fold(id: string): Fold
		return fold_of(`sec:{id}`, function(): boolean
			return not folded:peek()[id]
		end, function(on: boolean)
			local f = table.clone(folded:peek())
			f[id] = if on then nil else true
			folded:set(f)
		end)
	end
	-- A group's sessions past its first few: shown ("Show N more") or not.
	local function more_fold(key: string): Fold
		return fold_of(`more:{key}`, function(): boolean
			return expanded:peek()[key] == true
		end, function(on: boolean)
			local e = table.clone(expanded:peek())
			e[key] = if on then true else nil
			expanded:set(e)
		end)
	end
	-- A section folded or opened from outside (the caller's `folded`: on
	-- another device, say), or the empty projects': its fold plays as a
	-- click's would.
	ui.effect(function()
		local f = folded:get()
		for key, fold in folds_by do
			local id = string.match(key, "^sec:(.*)$")
			if id and (fold.h.to == 1) == (f[id] == true) then
				fold_set(fold, not f[id])
			elseif key == "empties" and (fold.h.to == 1) ~= (f[EMPTIES] == true) then
				fold_set(fold, f[EMPTIES] == true)
			end
		end
	end)
	-- A fold as `section` takes one (its title reads it, and a click on it
	-- sets it).
	local function fold_switch(f: () -> Fold, st: State<boolean>): any
		return {
			get = function(): boolean
				return st:get()
			end,
			peek = function(): boolean
				return st:peek()
			end,
			set = function(_, on: boolean)
				fold_set(f(), on)
			end,
		}
	end

	-- A drag's gaps (where it would land, opening; where it would have,
	-- closing), and its own place closing behind it while it's over
	-- somewhere that would take it.
	type Gap = { list: string, index: number, h: number, a: Anim, closing: boolean }
	local drag_zone: Fold? = nil
	local gaps: { Gap } = {}
	local leaving: Anim = still(0)
	local gaps_shape = ui.use_state(0)
	local function gaps_changed()
		gaps_shape:set(gaps_shape:peek() + 1)
	end
	-- Rows sliding to where a drop put them, from where they stood (the
	-- dropped one from where its preview was), by key: each eases its offset
	-- to nothing as the list takes its new shape at once. (`slid` tells the
	-- rows a slide began.)
	local slides: { [string]: Anim } = {}
	local slid = ui.use_state(0)
	local function set_over(o: Over?)
		local cur = over:peek()
		if (cur == nil and o == nil) or (cur and o and cur.list == o.list and cur.index == o.index) then
			return
		end
		over:set(o)
		local d = drag:peek()
		if not d then
			return
		end
		local keep: Gap? = nil
		for _, g in gaps do
			if o and g.list == o.list and g.index == o.index then
				keep = g
			elseif not g.closing then
				g.closing = true
				local gone = g
				g.a = anim(g.a, 0, DRAG_SLIDE.duration, 0, function()
					if gone.closing then
						local kept: { Gap } = {}
						for _, other in gaps do
							if other ~= gone then
								table.insert(kept, other)
							end
						end
						gaps = kept
						gaps_changed()
					end
				end)
			end
		end
		if o and o.list ~= "regular" then
			if keep then
				keep.closing = false
				keep.a = anim(keep.a, 1, DRAG_SLIDE.duration)
			else
				table.insert(gaps, { list = o.list, index = o.index, h = d.h, a = anim(still(0), 1, DRAG_SLIDE.duration), closing = false })
			end
		end
		leaving = anim(leaving, if o then 1 else 0, DRAG_SLIDE.duration)
		gaps_changed()
	end
	-- What a drag brings into the list (Pinned's drop zone while nothing is
	-- pinned, your sections a filter hid) opens from nothing, as a fold
	-- does, and only once the drag has started: zgx measures where the row
	-- was grabbed as it starts, and the preview keeps that point under the
	-- pointer, so the list must be as it was then. (Rows that sprang down by
	-- the zone's height as it started left the preview that far from the
	-- pointer.) A drag that ends before then leaves nothing behind.
	local starting: Drag? = nil
	start_drag = function(d: Drag)
		starting = d
		ui.after(0, function()
			if starting ~= d then
				return
			end
			starting = nil
			gaps = {}
			leaving = still(0)
			drag_zone = {
				key = "drag-zone",
				is_open = function(): boolean
					return true
				end,
				write = function() end,
				h = anim(still(0), 1, DRAG_SLIDE.duration),
				o = anim(still(0), 1, DRAG_SLIDE.duration),
				present = true,
				together = true,
			}
			drag:set(d)
			gaps_changed()
		end)
	end
	end_drag = function()
		starting = nil
		gaps = {}
		drag_zone = nil
		leaving = still(0)
		over:set(nil)
		drag:set(nil)
		drag_y = nil
		gaps_changed()
	end

	-- The list's rows, in order: what each is, its key (rows follow their
	-- keys; a key says what a row shows that can't change in place, so a
	-- title whose count changes is built again), the place a drop on it
	-- goes to ("pinned", a section's id, or "regular": back to its group),
	-- the folds it sits in, and its height before them (with the gap after
	-- it). A session's `index` is its place among its list's rows (the one
	-- dragged left out).
	type Item = {
		kind: string,
		key: string,
		place: string,
		chain: { Fold },
		base: () -> number,
		s: Session?,
		by: Organize?,
		list: string?,
		index: number?,
		last: boolean?,
		trunk: (() -> number)?,
		g: Group?,
		sec: Section?,
		n: number?,
		title: string?,
		text: string?,
		fold: (() -> Fold)?,
		open: State<boolean>?,
		gap: Gap?,
		-- A row arriving (0 to 1) or leaving (1 to 0, kept in the list as it
		-- was, `leaving`, until it's gone): its height and ink.
		grow: Anim?,
		leaving: boolean?,
		-- Outer groups' trunks running on past it (two levels of groups).
		through: (() -> { number })?,
		-- A pager's own key, where it isn't its group's sessions' (an outer
		-- group's inner groups: `{key}/groups`).
		pkey: string?,
		-- The empty projects' row: the projects (their tiles stack on it).
		gs: { Group }?,
	}
	type Shape = { items: { Item }, at: { [string]: number }, counts: { [string]: number } }
	local function build_shape(by: Organize): Shape
		shape:get()
		gaps_shape:get()
		local p = places:get()
		local d = drag:get()
		local placed = placed_in(p)
		local items: { Item } = {}
		local counts: { [string]: number } = {}
		local gap_after = list_gap()
		local function add(it: Item)
			table.insert(items, it)
		end
		local function gap_rows(list: string, index: number, chain: { Fold }, place: string)
			for _, g in gaps do
				if g.list == list and g.index == index then
					add({ kind = "gap", key = `gap:{list}:{index}`, place = place, chain = chain, gap = g, base = function(): number
						return 0
					end })
				end
			end
		end
		-- Whether a session stays in its list with its place folded: it's in a
		-- state (its corner a glyph, "waiting" none), and folded places keep
		-- those (`keep_active`).
		local keep = keep_active:get()
		-- (The session open shows in its group however far down it is.)
		local open_id = selected:get()
		local function peeks(s: Session): boolean
			return keep and s.status ~= nil and s.status ~= "waiting"
		end
		-- Each session's folds in a place folding at `f` (inside `outer`): its
		-- own, kept in the list as `f` folds when it peeks; nil, not in the
		-- list now (folded away).
		local function folding(outer: { Fold }, f: Fold): (s: Session) -> { Fold }?
			local inside = table.clone(outer)
			table.insert(inside, f)
			return function(s: Session): { Fold }?
				if peeks(s) then
					return outer
				end
				return if f.present then inside else nil
			end
		end
		-- A list's sessions (`list` its key, `place` where a drop on them
		-- goes), with the gaps among them. `chain_of`: a session's folds (nil,
		-- not in the list now), else `chain`'s. `tail`: a row follows them on
		-- their trunk (a group's "Show N more"), so none ends it.
		local function sessions_rows(
			list: string,
			all: { Session },
			row_by: Organize,
			chain: { Fold },
			place: string,
			trunk: (() -> number)?,
			chain_of: ((s: Session) -> { Fold }?)?,
			tail: boolean?
		)
			local sessions: { Session } = {}
			local chains: { [string]: { Fold } } = {}
			for _, s in all do
				local c = if chain_of then chain_of(s) else chain
				if c then
					table.insert(sessions, s)
					chains[s.id] = c
				end
			end
			local j = 0
			-- (Its own local: the captured one isn't narrowed in here.)
			local moving = d
			for i, s in sessions do
				local dragged = moving ~= nil and moving.id == s.id
				if not dragged then
					j += 1
					gap_rows(list, j, chain, place)
				end
				add({
					kind = "session",
					-- (By the session alone: moved to another list, a row is the
					-- same node, as it slides from where it was let go.)
					key = `s:{s.id}`,
					place = place,
					chain = chains[s.id],
					s = s,
					by = row_by,
					list = list,
					index = if dragged then j + 1 else j,
					last = i == #sessions and not tail,
					trunk = trunk,
					base = function(): number
						return session_h(s, row_by) + gap_after
					end,
				})
			end
			gap_rows(list, j + 1, chain, place)
			counts[list] = j
		end
		local function title_h(): number
			return TITLE + gap_after
		end
		local function room(chain: { Fold }, place: string, key: string)
			add({ kind = "room", key = `room:{key}`, place = place, chain = chain, base = function(): number
				return (ui.theme.get("zeron.space.hair") :: number?) or 2
			end })
		end
		local function drop_row(chain: { Fold }, place: string, text: string)
			add({ kind = "drop", key = `drop:{place}`, place = place, chain = chain, text = text, base = function(): number
				return 40 + gap_after
			end })
		end

		-- Pinned: while it has any, or while a session is dragged (to be
		-- pinned there).
		local pins = sessions_of(p.pinned)
		-- (A place there for the drag alone opens with its zone.)
		local function zoned(chain: { Fold }, for_drag: boolean): { Fold }
			if for_drag and drag_zone then
				local out = { drag_zone :: Fold }
				for _, f in chain do
					table.insert(out, f)
				end
				return out
			end
			return chain
		end
		if #pins > 0 or d ~= nil then
			local f = pinned_fold()
			local for_drag = #pins == 0
			add({ kind = "title", key = "title:pinned", place = "pinned", chain = zoned({}, for_drag), title = "ds.sidebar.pinned", n = #pins, fold = pinned_fold, open = pins_open, base = title_h })
			sessions_rows("pinned", pins, "none", { f }, "pinned", nil, folding({}, f))
			if f.present then
				if #pins == 0 then
					drop_row(zoned({ f }, for_drag), "pinned", "ds.sidebar.drop_to_pin")
				end
				room(zoned({ f }, for_drag), "pinned", "pinned")
			end
		end
		-- Your sections.
		local narrow = props.narrowed ~= nil and props.narrowed()
		for _, sec in p.sections do
			local own = sessions_of(sec.sessions)
			-- (Narrowed to none of its sessions: no place for it, but while a
			-- session is dragged, which may go there.)
			if narrow and #own == 0 and d == nil then
				continue
			end
			local for_drag = narrow and #own == 0
			local f = section_fold(sec.id)
			add({ kind = "own", key = `own:{sec.id}`, place = sec.id, chain = zoned({}, for_drag), sec = sec, n = #own, base = title_h })
			sessions_rows(sec.id, own, "none", { f }, sec.id, nil, folding({}, f))
			if f.present then
				if #own == 0 then
					drop_row(zoned({ f }, for_drag), sec.id, "ds.sidebar.drop_sessions")
				else
					room({ f }, sec.id, sec.id)
				end
			end
		end
		-- The groups.
		local all_groups, title = groups_of(by, placed)
		-- (Projects with none but archived sessions: after the rest, in a
		-- fold of their own; or, not shown, nowhere.)
		local gs: { Group } = {}
		local empties: { Group } = {}
		local show_empties = empties_shown()
		for _, g in all_groups do
			if not g.empty then
				table.insert(gs, g)
			elseif show_empties then
				table.insert(empties, g)
			end
		end
		local gf = groups_fold()
		local count = if by == "none" then #listed(gs[1].sessions) else #gs + #empties
		add({ kind = "title", key = "title:groups", place = "regular", chain = {}, title = title, n = count, fold = groups_fold, open = groups_open, base = title_h })
		if gf.present then
			-- A group's header, then what's under it: its sessions (its first
			-- few, then "Show N more"), or with two levels on, its inner
			-- groups', each indented a step, on its trunk. `chain`: the
			-- header's folds; `only_peeks`: its group folded away above it,
			-- only those in a state (and their groups) still listed;
			-- `outer`: the trunk it hangs off (an inner group's).
			local base = { gf }
			local function emit_group(g: Group, chain: { Fold }, only_peeks: boolean, outer: (() -> number)?)
				local f = group_fold(g.key)
				add({ kind = "group", key = `group:{g.key}`, place = "regular", chain = chain, g = g, trunk = outer, base = function(): number
					return SLOT + gap_after
				end })
				-- Its own trunk: down from its tile's middle, past its indent.
				local function trunk(): number
					local from = if outer then outer() + SessionRow.BEND + SessionRow.REACH else 0
					return from + (if g.shows_tile == nil or g.shows_tile() then SLOT / 2 else 12.5)
				end
				local inside = table.clone(chain)
				table.insert(inside, f)
				local open_now = f.present and not only_peeks
				if g.children then
					-- Its inner groups: listed while it's open; folded, those
					-- holding a session in a state (while folds keep them).
					-- Paged as sessions are: its first few inner groups (the most
					-- recently active), then "Show N more" (the rest folded under
					-- it, `more_fold` of `{key}/groups`); those holding a session
					-- in a state, and the one holding the session open, showing
					-- past them.
					local spans: { { from: number, to: number } } = {}
					local within: { [Group]: boolean } = {}
					local past = 0
					local peeking: { [Group]: boolean } = {}
					for i, c in g.children do
						local holds_open = false
						for _, sess in listed(c.sessions) do
							if peeks(sess) then
								peeking[c] = true
							end
							if sess.id == open_id then
								holds_open = true
							end
						end
						if i <= GROUP_FIRST or holds_open or peeking[c] then
							within[c] = true
						else
							past += 1
						end
					end
					local pkey = `{g.key}/groups`
					local xf = if past > 0 then more_fold(pkey) else nil
					local paged = past > 0 and open_now
					local more_chain = table.clone(inside)
					if xf then
						table.insert(more_chain, xf)
					end
					for _, c in g.children do
						if open_now and within[c] then
							local from = #items + 1
							emit_group(c, inside, false, trunk)
							table.insert(spans, { from = from, to = #items })
						elseif open_now and xf and xf.present then
							local from = #items + 1
							emit_group(c, more_chain, false, trunk)
							table.insert(spans, { from = from, to = #items })
						elseif not open_now and peeking[c] then
							local from = #items + 1
							emit_group(c, base, true, trunk)
							table.insert(spans, { from = from, to = #items })
						end
					end
					if paged then
						add({ kind = "pager", key = `more:{pkey}`, pkey = pkey, place = "regular", chain = inside, g = g, n = past, trunk = trunk, last = true, base = function(): number
							return SLOT + gap_after
						end })
					end
					-- Its trunk ends at its last inner group's header (or its
					-- pager, after them all); it runs on past every other inner
					-- group's rows.
					for i, span in spans do
						local head = items[span.from]
						local final = i == #spans and not paged
						head.last = final
						if not final then
							for j = span.from + 1, span.to do
								local it = items[j]
								local was = it.through
								it.through = function(): { number }
									local out = if was then table.clone(was()) else {}
									table.insert(out, trunk())
									return out
								end
							end
						end
					end
					return
				end
				-- Its first few, then "Show N more" (the rest folded under it,
				-- `more_fold`); the session open, and those that peek, showing
				-- past them wherever they stand.
				local all = listed(g.sessions)
				local within: { [string]: boolean } = {}
				local past = 0
				for i, sess in all do
					if i <= GROUP_FIRST or sess.id == open_id or peeks(sess) then
						within[sess.id] = true
					else
						past += 1
					end
				end
				local xf = if past > 0 then more_fold(g.key) else nil
				local paged = past > 0 and open_now
				local shown = folding(base, f)
				sessions_rows(g.key, all, by, inside, "regular", trunk, function(sess: Session): { Fold }?
					local c = shown(sess)
					-- (Folded away above it: only those in a state.)
					if only_peeks and not peeks(sess) then
						return nil
					end
					if c ~= nil and c ~= base then
						-- (Its own fold, under its outer groups' folds.)
						c = inside
					end
					if c == nil or within[sess.id] then
						return c
					end
					if xf and xf.present then
						local more = table.clone(c)
						table.insert(more, xf)
						return more
					end
					return nil
				end, paged)
				if paged then
					add({ kind = "pager", key = `more:{g.key}`, place = "regular", chain = inside, g = g, n = past, trunk = trunk, last = true, base = function(): number
						return SLOT + gap_after
					end })
				end
			end
			for _, g in gs do
				if by == "none" then
					sessions_rows(g.key, listed(g.sessions), by, base, "regular")
				else
					emit_group(g, base, false, nil)
				end
			end
			-- The empty projects: one row saying how many, its chevron
			-- opening them under it, each a project's header as any other
			-- (its new chat, its menu); closed, none of them in the list.
			if #empties > 0 then
				local ef = empties_fold()
				add({ kind = "empties", key = "empties", place = "regular", chain = base, n = #empties, gs = empties, base = function(): number
					return TITLE + gap_after
				end })
				if ef.present then
					local inner = { gf, ef }
					for _, g in empties do
						emit_group(g, inner, false, nil)
					end
				end
			end
		end
		-- The Archived shelf, last, while it's shown and has any: its first
		-- few, and a row for more.
		local shelf: { Session } = if props.archived and details.archived and details.archived:get()
			then follow(props.archived, {} :: { Session })
			else {}
		if #shelf > 0 then
			local af = archived_fold()
			add({ kind = "title", key = "title:archived", place = "archived", chain = {}, title = "ds.sidebar.archived", n = #shelf, fold = archived_fold, open = archived_open, base = title_h })
			if af.present then
				local n = math.min(#shelf, shelf_shown:get())
				sessions_rows("archived", { table.unpack(shelf, 1, n) }, "none", { af }, "archived")
				if #shelf > n then
					add({ kind = "more", key = "more:archived", place = "archived", chain = { af }, n = #shelf - n, base = function(): number
						return SLOT + gap_after
					end })
				end
			end
		end
		-- A place folding or unfolding moves only the rows that can show:
		-- those past a list's height into it stand in as one row, so the
		-- list (whose rows all shrink together) never finds a thousand tiny
		-- ones in view and builds them all.
		local t = clock:peek()
		local view = (list_view.height or 0) + 2 * SLOT
		if view < 600 then
			view = 600
		end
		local seen: { [Fold]: boolean } = {}
		for _, it in items do
			for _, f in it.chain do
				seen[f] = true
			end
		end
		for f in seen do
			if t < f.h.at + f.h.delay + f.h.dur then
				local kept: { Item } = {}
				local run, inside, rest, rest_chain = 0, false, 0, nil :: { Fold }?
				for _, it in items do
					local holds = table.find(it.chain, f) ~= nil
					if holds and run >= view then
						rest += it.base()
						if not rest_chain then
							local upto: { Fold } = {}
							for _, c in it.chain do
								table.insert(upto, c)
								if c == f then
									break
								end
							end
							rest_chain = upto
						end
						inside = true
					else
						if inside and not holds then
							table.insert(kept, { kind = "rest", key = `rest:{f.key}`, place = it.place, chain = rest_chain :: { Fold }, base = function(): number
								return rest
							end })
							inside = false
						end
						if holds then
							run += it.base()
						end
						table.insert(kept, it)
					end
				end
				if inside then
					table.insert(kept, { kind = "rest", key = `rest:{f.key}`, place = "regular", chain = rest_chain :: { Fold }, base = function(): number
						return rest
					end })
				end
				items = kept
			end
		end
		local at: { [string]: number } = {}
		for i, it in items do
			at[it.key] = i
		end
		return { items = items, at = at, counts = counts }
	end

	-- Each row's height now: its own, as its folds have it (a dragged one's
	-- closing as it's held elsewhere; a gap's opening), and their sum.
	local NONE = 0.001
	local function row_height(it: Item, t: number, d: Drag?): number
		local h: number
		if it.kind == "gap" then
			local g = it.gap :: Gap
			h = g.h * value(g.a, t)
		else
			h = it.base()
			if d and it.s and it.s.id == d.id then
				h *= 1 - value(leaving, t)
			end
		end
		for _, f in it.chain do
			h *= value(f.h, t)
		end
		if it.grow then
			h *= value(it.grow, t)
		end
		-- (Never quite none: zgx's list lays a row of 0 out a point tall,
		-- so rows under a fold just closed would sit a point low a row,
		-- then move up as it went. A hair is laid out as it is.)
		return math.max(h, NONE)
	end
	local function row_ink(it: Item, t: number): number
		local o = if it.grow then value(it.grow, t) else 1
		for _, f in it.chain do
			o *= value(f.o, t)
		end
		return o
	end

	-- What a drag held at window height `y` would land in: walking the rows
	-- as they stand, the one under it says. A session's row: before it or
	-- after it, by which half; a gap: where it is; a place's title: its
	-- first place; its foot (room, or where to drop): its last. Over the
	-- groups, back to its group.
	local function hover_at(sh: Shape, heights: { number }, top_y: number, y: number)
		local cy = y - top_y + list_offset:peek()
		local top = 0
		local hit: Item? = nil
		local dy, hh = 0, 0
		for i, it in sh.items do
			local h = heights[i] or 0
			if cy < top + h or i == #sh.items then
				hit, dy, hh = it, cy - top, h
				break
			end
			top += h
		end
		if not hit then
			set_over(nil)
			return
		end
		local it = hit :: Item
		if it.place == "regular" then
			set_over({ list = "regular", index = 0 })
			return
		end
		local list = it.place
		local index: number
		if it.kind == "session" then
			index = if dy < hh / 2 then it.index :: number else (it.index :: number) + 1
			local d = drag:peek()
			if d and it.s and it.s.id == d.id then
				index = it.index :: number
			end
		elseif it.kind == "gap" then
			index = (it.gap :: Gap).index
		elseif it.kind == "title" or it.kind == "own" then
			index = 1
		else
			index = (sh.counts[list] or 0) + 1
		end
		set_over({ list = list, index = index })
	end
	-- Where a dragged section lands among yours: before or after the one
	-- whose block (its title to its foot) it's let go over, by which half.
	local function section_drop(sh: Shape, heights: { number }, top_y: number, y: number, moving_id: string)
		local cy = y - top_y + list_offset:peek()
		local top = 0
		local blocks: { { id: string, from: number, to: number } } = {}
		for i, it in sh.items do
			local h = heights[i] or 0
			if it.kind == "own" then
				table.insert(blocks, { id = (it.sec :: Section).id, from = top, to = top + h })
			elseif #blocks > 0 and it.place == blocks[#blocks].id then
				blocks[#blocks].to = top + h
			end
			top += h
		end
		local target, after = nil :: string?, false
		for _, b in blocks do
			if cy < b.to or b == blocks[#blocks] then
				target, after = b.id, cy > (b.from + b.to) / 2
				break
			end
		end
		if not target or target == moving_id then
			return
		end
		sections_edit(function(all: { Section }): { Section }
			local moved: Section? = nil
			local rest: { Section } = {}
			for _, other in all do
				if other.id == moving_id then
					moved = other
				else
					table.insert(rest, other)
				end
			end
			if not moved then
				return all
			end
			for i, other in rest do
				if other.id == target then
					table.insert(rest, if after then i + 1 else i, moved)
					break
				end
			end
			return rest
		end)
	end

	-- Pinned on the rail: one icon (as a project's tile: a badge for the
	-- most pressing, chosen while it holds the session open) opening them
	-- beside it.
	local function pin_icon(pins: Sessions): Node
		local pin_hovered = ui.use_state(false)
		local flyout = flyout_state(pin_hovered)
		local function pinned(): string
			return locale.s("ds.sidebar.pinned")
		end
		local pin = row {
			name = pinned,
			hovered = pin_hovered,
			-- A tile as a project's (its pin where a project's initial is), but
			-- quiet: pinned is the chrome's, not a project.
			lead = group_tile(QUIET, Icon { name = "pin", size = 12 }, pins, function(): boolean
				return railed()
			end),
			-- (No words: it's only on the rail.)
			words = ui.div {},
			selected = function(): boolean
				return railed() and holds(pins)
			end,
			on_click = function()
				toggle(flyout)
			end,
		}
		return fly(pinned, pins, "none", flyout, pin)
	end

	-- What an empty place says, where a drop would go (its message's key).
	local function drop_here(key: string): Node
		return ui.row {
			w_full = true,
			h = 40,
			shrink = 0,
			px = "$space.2",
			items = "center",
			ui.text { locale.t(key), text_size = "$font.size.xs", color = "$text.tertiary" },
		}
	end
	-- A menu's item: its glyph and its name (`danger`, in the danger ink).
	local function menu_item(icon: Icon.Name, name: string, pick: () -> (), danger: boolean?): Node
		return Menu.Row { icon = icon, label = name, danger = danger, role = "menuitem", on_click = pick }
	end
	-- A dragged thing's card: the menus' glass, a row's height.
	local function lifted(h: number, ...: Node): Node
		local card: ContainerProps = {
			w = room_now() - PAD * 2,
			h = h,
			px = "$space.2",
			gap = "$space.2",
			items = "center",
			rounded = "$radius.md",
			...,
		}
		Glass.style(card, "menu")
		return ui.row(card)
	end

	-- A section of your own's title. A click folds it; it's renamed in place
	-- (Enter keeps the name, Escape the old one) and dragged among the
	-- others; under the pointer, its "…": rename, archive all, delete (its
	-- sessions go back to their groups). Folded, how many it holds.
	-- (`now`: the section as it stands, and how many it holds; the row is
	-- the section's for as long as it is there, renamed or refilled.)
	local function own_title(id: string, now: () -> (Section, number)): Node
		local function sec_now(): Section
			return (now())
		end
		local f = function(): Fold
			return section_fold(id)
		end
		local function open(): boolean
			return not folded:get()[id]
		end
		local hovered = ui.use_state(false)
		local menu_open = ui.use_state(false)
		local name = ui.use_state(sec_now().name)
		local function editing(): boolean
			return renaming:get() == id
		end
		local function rename_done(keep: boolean)
			if renaming:peek() ~= id then
				return
			end
			renaming:set(nil)
			local v = string.match(name:peek(), "^%s*(.-)%s*$") or ""
			if keep and v ~= "" and v ~= sec_now().name then
				sections_edit(function(all: { Section }): { Section }
					for _, other in all do
						if other.id == id then
							other.name = string.sub(v, 1, 120)
						end
					end
					return all
				end)
			else
				name:set(sec_now().name)
			end
		end
		local dots = IconButton {
			icon = "more-horizontal",
			label = locale.t("ds.sidebar.section_options"),
			tooltip = locale.s("ds.sidebar.section_options"),
			size = "sm",
			tone = "quiet",
			lit = menu_open,
			on_click = function()
				menu_open:set(true)
			end,
		}
		local menu = ui.popover(Glass.style({
			open = menu_open,
			anchor = dots,
			placement = { side = "bottom", align = "end", offset = 4 },
			w = 180,
			p = 0,
			gap = 0,
			rounded = "$radius.lg",
			build = function()
				return Menu.Group {
					menu_item("pen", locale.s("ds.sidebar.rename"), function()
						menu_open:set(false)
						name:set(sec_now().name)
						renaming:set(id)
					end),
					menu_item("archive-minimalistic", locale.s("ds.sidebar.archive_all"), function()
						menu_open:set(false)
						if props.on_archive then
							for _, sid in sec_now().sessions do
								props.on_archive(sid)
							end
						end
					end),
					menu_item("trash-bin-minimalistic", locale.s("ds.sidebar.delete"), function()
						menu_open:set(false)
						sections_edit(function(all: { Section }): { Section }
							local kept: { Section } = {}
							for _, other in all do
								if other.id ~= id then
									table.insert(kept, other)
								end
							end
							return kept
						end)
					end, true),
				}
			end,
		}, "menu"))
		return ui.row {
			class = "section-header",
			w_full = true,
			h = TITLE,
			shrink = 0,
			pl = "$space.2",
			pr = "$space.1",
			gap = "$space.1",
			items = "center",
			color = "$text.tertiary",
			cursor = "pointer",
			hover = { color = "$text.secondary" },
			on_hover = function(inside: boolean)
				hovered:set(inside)
			end,
			on_click = function()
				if editing() then
					return
				end
				fold_set(f(), not open())
			end,
			on_drag = function()
				held:set(true)
				return { section = id }
			end,
			on_drag_end = function()
				held:set(false)
				drag_y = nil
			end,
			drag_preview = function()
				return lifted(TITLE, ui.text { sec_now().name, literal = true, text_size = "$font.size.xs", weight = "$font.weight.medium", color = "$text.secondary" })
			end,
			ui.show(
				editing,
				ui.input(EditorMenu.on({
					value = name,
					label = locale.t("ds.sidebar.section_name"),
					placeholder = locale.t("ds.sidebar.section_name"),
					grow = 1,
					h = 20,
					px = 0,
					border = 0,
					bg = "#00000000",
					focus = { border = 0, outline = 0 },
					text_size = "$font.size.xs",
					weight = "$font.weight.medium",
					color = "$text.primary",
					select_on_focus = true,
					on_key = function(e): boolean
						if e.key == "Enter" then
							rename_done(true)
							return true
						elseif e.key == "Escape" then
							rename_done(false)
							return true
						end
						return false
					end,
					on_blur = function()
						rename_done(true)
					end,
					on_mount = function(el)
						ui.after(0, function()
							el:focus()
						end)
					end,
				})),
				ui.text {
					function(): string
						return sec_now().name
					end,
					grow = 1,
					text_size = "$font.size.xs",
					weight = "$font.weight.medium",
				}
			),
			ui.show(function(): boolean
				return not open() and not editing() and not hovered:get() and not menu_open:get()
			end, hint(function(): string
				local _, n = now()
				return tostring(n)
			end)),
			ui.show(function(): boolean
				return (hovered:get() or menu_open:get()) and not editing()
			end, menu),
		}
	end

	-- A group's header in the list: its tile and name; a click folds it
	-- (folded, how many it holds, and its tile's badge for the most pressing
	-- of them); under the pointer, a project's new chat.
	-- (`now`: the group as it stands; the row is the group's while it is
	-- there, its count and badge following its sessions.)
	-- (`tree_of`: an inner group's place on its outer group's trunk: its
	-- header indented a step, a branch to its tile.)
	local function group_head(g: Group, now: () -> Group, tree_of: (() -> Tree?)?): Node
		local function sessions(): { Session }
			return listed(now().sessions)
		end
		local open = opened[g.key]
		local hovered = ui.use_state(false)
		local function hidden(): boolean
			return not open:get()
		end
		-- (Its name as the group now has it: a project renamed.)
		local function name(): string
			local n = now().name
			return named(g.key, if type(n) == "function" then n() else n)
		end
		local header = row {
			class = "section-header",
			provide = if g.project then { project = g.project } else nil,
			name = name,
			hovered = hovered,
			lead = g.tile(hidden, sessions),
			leads = g.shows_tile,
			indent = if tree_of
				then function(): number
					local t = (tree_of :: () -> Tree?)()
					return if t then gutter(t) else 0
				end
				else nil,
			-- (The shared header's words, ds/session_row: folded, how many;
			-- a project's new chat under the pointer, in their place.)
			words = SessionRow.header_words(
				name,
				function(): string
					return tostring(#sessions())
				end,
				hidden,
				if g.new_in then new_corner(g, now) else nil,
				if g.new_in
					then function(): boolean
						return hovered:get()
					end
					else nil
			),
			on_click = function()
				fold_set(group_fold(g.key), not open:peek())
			end,
		}
		-- A project's: its menu on a right click (Zeron's Rename…, Remove…).
		local project, menu = g.project, props.project_menu
		if project and menu then
			header = ContextMenu {
				label = locale.t("ds.sidebar.project_actions"),
				ring = "$radius.md",
				w_full = true,
				shrink = 0,
				items = function()
					return menu(project)
				end,
				header,
			}
		end
		-- An inner group's: off its outer group's trunk, the branch reaching
		-- its tile (centred in its slot), or its name with no tile.
		if tree_of then
			header = SessionRow.on_tree(header, tree_of, function(): number
				return if g.shows_tile == nil or g.shows_tile() then (SLOT - 18) / 2 else words_inset()
			end, list_gap())
		end
		return header
	end

	-- The list. Its shape (which rows, in order) as the view, the places and
	-- a drag make it; its rows' heights as they stand now (read every frame
	-- something moves, and only then).
	local list_total = 0
	-- A row's height as the list lays it out (zgx's `ui.list`: a height
	-- above 0 as it is, however small; 0 or none, a point). Where rows stood,
	-- for their slides and glides, must be where the list put them: rows
	-- counted otherwise start a point or two off and move back after they
	-- land (a bounce under a fold that just closed, a dropped row's
	-- neighbours stepping back).
	local function laid_h(h: number?): number
		local x = h or 0
		return if x > 0 then x else 1
	end
	-- Its sessions in the list's order, as plugins under it read them.
	local order = ui.use_state({} :: { string })
	local live: { shape: Shape?, heights: { number } } = { shape = nil, heights = {} }
	local function the_list(by: Organize): Node
		-- (A computed holds plain data: the shape, with its rows' functions,
		-- is kept beside it, the computed counting its versions for what
		-- follows it.)
		local current: Shape = { items = {}, at = {}, counts = {} }
		local version = 0
		-- Its rows take new places smoothly (a session or a group going
		-- first, rows coming or going with "Show N more"): each row that
		-- moves glides from where it stood (the column motion's curve and
		-- pace, ds/motion: 180 to 240 ms by how far); one arriving opens and
		-- fades in where it lands, one leaving closes and fades where it was
		-- (kept in the list, as it was, until it's gone). Every row stays its
		-- node: its focus, hover and an inline rename go with it. (Reduce
		-- motion: all of it at once.) Not while a drag holds the list (its
		-- own slides), nor with a fold moving (its own motion).
		-- The list as last laid out, so a new shape knows where each stood.
		local laid: { shape: Shape?, heights: { number } } = { shape = nil, heights = {} }
		local grows: { [string]: Anim } = {}
		local going: { [string]: { it: Item, after: string? } } = {}
		local pending: { [string]: number }? = nil
		local GLIDES: { [string]: boolean } = { session = true, group = true, pager = true, empties = true }
		local function quiet(): boolean
			return drag:peek() == nil and starting == nil
		end
		local function settled(it: Item, t: number): boolean
			for _, f in it.chain do
				if t < f.h.at + f.h.delay + f.h.dur then
					return false
				end
			end
			return true
		end
		local function folding(s: Shape): boolean
			for _, it in s.items do
				if it.kind == "rest" then
					return true
				end
			end
			return false
		end
		-- Where each row of `s` stood (its top, its slide's offset with it).
		local function stood(s: Shape, hs: { number }, t: number): { [string]: number }
			local out: { [string]: number } = {}
			local y = 0
			for i, it in s.items do
				local a = slides[it.key]
				out[it.key] = y + (if a then value(a, t) else 0)
				y += laid_h(hs[i])
			end
			return out
		end
		local function ease_to(a: Anim?, from: number, to: number, on_done: (() -> ())?): Anim
			local n = anim(a or still(from), to, motion.BASE, 0, on_done)
			n.css = true
			return n
		end
		-- The new shape with the rows arriving and leaving: those come in
		-- from nothing; those gone stay where they followed, closing.
		local function reshaped(old: Shape?, hs: { number }, next: Shape): Shape
			local t = clock:peek()
			if old and quiet() and not folding(old) and not folding(next) then
				local was_at = old.at
				local after: string? = nil
				for i, it in old.items do
					local key = it.key
					if next.at[key] then
						-- (Back while it was going: it comes again from where it is.)
						if going[key] then
							going[key] = nil
							grows[key] = ease_to(grows[key], 1, 1, nil)
						end
					elseif not going[key] and GLIDES[it.kind] and not it.leaving and (hs[i] or 0) > 0.5 and settled(it, t) then
						local left = table.clone(it)
						left.leaving = true
						local g: Anim
						g = ease_to(it.grow, 1, 0, function()
							if grows[key] == g then
								grows[key] = nil
								going[key] = nil
								shape:set(shape:peek() + 1)
							end
						end)
						grows[key] = g
						going[key] = { it = left, after = after }
					end
					if next.at[key] or going[key] then
						after = key
					end
				end
				for _, it in next.items do
					if not was_at[it.key] and GLIDES[it.kind] and settled(it, t) then
						local key = it.key
						local g: Anim
						g = ease_to(nil, 0, 1, function()
							if grows[key] == g then
								grows[key] = nil
							end
						end)
						grows[key] = g
					end
				end
			end
			-- Those going, each after the row it followed (one itself going,
			-- in turn); with that row nowhere, it goes at once.
			local items = table.clone(next.items)
			local placed: { [string]: boolean } = {}
			for _, it in items do
				placed[it.key] = true
			end
			local left = 0
			for key in going do
				if placed[key] then
					going[key] = nil
				else
					left += 1
				end
			end
			while left > 0 do
				local moved = false
				for key, g in going do
					if not placed[key] and (g.after == nil or placed[g.after]) then
						local at = 1
						if g.after then
							for i, it in items do
								if it.key == g.after then
									at = i + 1
									break
								end
							end
						end
						table.insert(items, at, g.it)
						placed[key] = true
						left -= 1
						moved = true
					end
				end
				if not moved then
					for key in going do
						if not placed[key] then
							going[key] = nil
							grows[key] = nil
						end
					end
					break
				end
			end
			local at: { [string]: number } = {}
			for i, it in items do
				it.grow = grows[it.key]
				at[it.key] = i
			end
			return { items = items, at = at, counts = next.counts }
		end
		local shaped = ui.computed(function(): number
			local old = laid.shape
			local t = clock:peek()
			local before = if old and quiet() then stood(old, laid.heights, t) else nil
			current = reshaped(old, laid.heights, build_shape(by))
			pending = before
			version += 1
			return version
		end)
		local sh = {
			get = function(_): Shape
				shaped:get()
				return current
			end,
			peek = function(_): Shape
				shaped:peek()
				return current
			end,
		}
		-- (The list showing now's: one crossfading out leaves it be.)
		ui.effect(function()
			local s = sh:get()
			if by ~= by_peek() then
				return
			end
			local ids: { string } = {}
			local shown: { string } = {}
			for _, it in s.items do
				-- (One leaving is gone already, for the keys.)
				if it.kind == "session" and not it.leaving then
					local id = (it.s :: Session).id
					table.insert(ids, id)
					local open = true
					for _, f in it.chain or {} do
						open = open and (f :: Fold).is_open()
					end
					if open then
						table.insert(shown, id)
					end
				end
			end
			local function put(into: State<{ string }>, list: { string })
				local was = into:peek()
				local same = #was == #list
				for i = 1, if same then #list else 0 do
					if was[i] ~= list[i] then
						same = false
						break
					end
				end
				if not same then
					into:set(list)
				end
			end
			put(order, ids)
			if props.order then
				put(props.order, shown)
			end
			-- (Their jumps: the first nine shown, as ⌘1–9 go.)
			local jumps: { [string]: number } = {}
			for i = 1, math.min(#shown, 9) do
				jumps[shown[i]] = i
			end
			local was = slots:peek()
			local same = true
			for id, n in jumps do
				same = same and was[id] == n
			end
			for id in was do
				same = same and jumps[id] ~= nil
			end
			if not same then
				slots:set(jumps)
			end
		end)
		local heights = ui.computed(function(): { number }
			local s = sh:get()
			local t = clock:get()
			local d = drag:get()
			local out = table.create(#s.items) :: { number }
			local total = 0
			for i, it in s.items do
				local h = row_height(it, t, d)
				out[i] = h
				total += h
			end
			list_total = total
			live.shape, live.heights = s, out
			laid.shape, laid.heights = s, out
			return out
		end)
		local function item_of(key: string): Item?
			local s = sh:get()
			local i = s.at[key]
			return if i then s.items[i] else nil
		end
		-- The empty projects' row (the Archived title's own frame, no
		-- caret: the whole row folds): "Empty", a stack of their tiles
		-- overlapping (as many as fit, at most EMPTY_TILES: whole tiles
		-- dropped, never the word or the count cut), and how many at the
		-- end. Its full phrase, "21 empty projects", names it and is its
		-- tooltip. Opened, each tile of the stack flies down to be its
		-- project's icon in the row under it (and back up as it closes),
		-- on the fold's own motion; the stack's tiles are made once and
		-- follow the projects in place.
		local EMPTY_TILES = 5
		local EMPTY_TILE, EMPTY_OVERLAP, EMPTY_GAP = 18, 6, 1.5
		-- What the row keeps before the stack, points: its insets, the word
		-- (an estimate, the longer of its locales'), the space, and the
		-- count's digits.
		local EMPTY_FIXED = 8 * 2 + 40 + 8 + 8
		local EMPTY_DIGIT = 7
		-- A tile's cut: a rounded square the next one's size, and a gap round it
		-- (params[0]: its centre and half-side as fractions of the width,
		-- params[1]: its corner), so the tile behind is notched to the shape of
		-- the one over it, with no shadow, whatever shows through.
		local NOTCH = ui.layer_shader([[
fn layer(input:LayerInput)->vec4<f32> {
 let w=input.size.x;
 let c=vec2(input.params[0].x*w,.5*input.size.y);
 let h=input.params[0].y*w;
 let r=input.params[0].z*w;
 let q=abs(input.p-c)-vec2(h-r);
 let d=length(max(q,vec2(0.0)))+min(max(q.x,q.y),0.0)-r;
 return vec4(input.color.rgb,input.color.a*smoothstep(-.5,.5,d));
}
]])
		local function empties_row(now: () -> Item): Node
			local function says(): string
				return locale.s("ds.sidebar.empty_projects", { n = now().n or 0 })
			end
			local function group_at(i: number): Group?
				return (now().gs or {})[i]
			end
			local function project_at(i: number): Project?
				local g = group_at(i)
				return if g then index:get().by_project[g.project :: string] else nil
			end
			-- How many tiles the width keeps.
			local function fit(): number
				local free = room() - PAD * 2 - EMPTY_FIXED - EMPTY_DIGIT * #tostring(now().n or 0)
				if free < EMPTY_TILE then
					return 0
				end
				return math.min(EMPTY_TILES, 1 + math.floor((free - EMPTY_TILE) / (EMPTY_TILE - EMPTY_OVERLAP)))
			end
			local function shows(i: number): boolean
				return i <= fit() and group_at(i) ~= nil
			end
			-- The fold's own motion (its height's progress, its ink): 0 closed,
			-- 1 open.
			local function flown(): number
				return value(empties_fold().h, clock:get())
			end
			local stack_ref = ui.ref()
			local STEP = EMPTY_TILE - EMPTY_OVERLAP
			-- Where tile `i` lands, from where it stands: its project's row
			-- icon (the rows' own tile slot, centred on its slot).
			local function lands(i: number): (number, number)
				local g = group_at(i)
				local sh0 = sh:peek()
				local from, to = sh0.at["empties"], if g then sh0.at[`group:{g.key}`] else nil
				local box, list = stack_ref:peek(), list_view:peek()
				if not (from and to and box and list) then
					return 0, 0
				end
				local dy = (SLOT - TITLE) / 2
				for k = from, to - 1 do
					dy += sh0.items[k].base()
				end
				local dx = list.x + PAD + (SLOT - EMPTY_TILE) / 2 - (box.x + (i - 1) * STEP)
				return dx, dy
			end
			local function tile(i: number, cut: boolean): Node
				local held: { [any]: any } = {
					shrink = 0,
					w = EMPTY_TILE,
					h = EMPTY_TILE,
					Tile {
						size = EMPTY_TILE,
						label = function(): string
							local p = project_at(i)
							return if p then p.name else ""
						end,
						hue = function(): string?
							local p = project_at(i)
							return (p and p.hue) or "gray"
						end,
						src = function(): string?
							local p = project_at(i)
							return if p then p.src else nil
						end,
						emoji = function(): string?
							local p = project_at(i)
							return if p then p.emoji else nil
						end,
					},
				}
				if cut then
					held.layer_shader = NOTCH
					local half = EMPTY_TILE / 2 + EMPTY_GAP
					local radius = ((ui.theme.get("radius.sm") :: number?) or 4) + EMPTY_GAP
					held.layer_params = {
						{ (STEP + EMPTY_TILE / 2) / EMPTY_TILE, half / EMPTY_TILE, radius / EMPTY_TILE, 0 },
						{ 0, 0, 0, 0 },
						{ 0, 0, 0, 0 },
						{ 0, 0, 0, 0 },
					}
				end
				return ui.div(held)
			end
			local function slot(i: number): Node
				return ui.show(function(): boolean
					return shows(i)
				end, ui.div {
					shrink = 0,
					ml = if i == 1 then 0 else ui.raw(-EMPTY_OVERLAP, "tiles overlap by a third"),
					-- (Flying: down to its row's icon as the fold opens, back as
					-- it closes; whole while it flies, it hands over to the
					-- row's own icon, fading in under it, over the last stretch.)
					x = function(): number
						local p = flown()
						return if p > 0 then p * (lands(i)) else 0
					end,
					y = function(): number
						local p = flown()
						if p <= 0 then
							return 0
						end
						local _, dy = lands(i)
						return p * dy
					end,
					opacity = function(): number
						return 1 - math.clamp((flown() - 0.75) / 0.25, 0, 1)
					end,
					-- (The notch only while it sits in the stack.)
					ui.show(function(): boolean
						return not (i < EMPTY_TILES and shows(i + 1)) or flown() > 0
					end, tile(i, false)),
					if i < EMPTY_TILES
						then ui.show(function(): boolean
							return shows(i + 1) and flown() <= 0
						end, tile(i, true))
						else nil,
				})
			end
			local slots: { Node } = {}
			for i = 1, EMPTY_TILES do
				table.insert(slots, slot(i))
			end
			local frame: { [any]: any } = {
				class = "empty-projects",
				role = "button",
				label = says,
				w = function(): number
					return room() - PAD * 2
				end,
				h = TITLE,
				shrink = 0,
				pl = "$space.2",
				pr = "$space.2",
				items = "center",
				color = "$text.tertiary",
				cursor = "pointer",
				hover = { color = "$text.secondary" },
				on_click = function()
					fold_set(empties_fold(), not empties_open:peek())
				end,
				ui.text {
					locale.t("ds.sidebar.empty_projects_word"),
					shrink = 0,
					text_size = "$font.size.xs",
					weight = "$font.weight.medium",
				},
				-- (Gone from the header once they've landed in the rows.)
				ui.show(function(): boolean
					return flown() < 1 or not empties_open:get()
				end, ui.row {
					class = "empty-projects-stack",
					ref = stack_ref,
					ml = "$space.2",
					shrink = 1,
					min_w = 0,
					items = "center",
					table.unpack(slots),
				}),
				ui.div { grow = 1 },
				ui.text {
					function(): string
						return tostring(now().n or 0)
					end,
					shrink = 0,
					text_size = "$font.size.xs",
					tabular = true,
				},
				ui.presence(says, function(name: string): Node
					return ui.div { absolute = true, left = 0, top = 0, w_full = true, h_full = true, tooltip = name, tooltip_delay = 150, pointer_events = "none" }
				end),
			}
			frame.relative = true
			-- (No folding frame round it, which would clip: its tiles fly out.)
			return ui.div {
				w_full = true,
				h = TITLE,
				ui.row(fades(frame :: any, function(): boolean
					return not railed()
				end)),
			}
		end

		local function build(key: string): Node
			local it = item_of(key)
			if not it then
				return ui.div {}
			end
			local node: Node
			-- (Built once for its key: what it shows of its item that can
			-- change, it reads from the item as it stands.)
			local function now(): Item
				return item_of(key) or (it :: Item)
			end
			-- Its place on a group's tree as the item has it now (its trunk, the
			-- last on it, outer trunks running past it); nil, out of any.
			-- (`last`: always, a group's pager.)
			local function tree_at(last: boolean?): () -> Tree?
				return function(): Tree?
					local cur = item_of(key)
					local trunk = if cur then cur.trunk else nil
					if not trunk then
						return nil
					end
					return {
						trunk = trunk :: () -> number,
						last = function(): boolean
							if last then
								return true
							end
							local c = item_of(key)
							return c ~= nil and c.last == true
						end,
						through = function(): { number }
							local c = item_of(key)
							return if c and c.through then c.through() else {}
						end,
					}
				end
			end
			if it.kind == "title" then
				-- (Its title its message's key: the items are data.)
				node = section(locale.t(it.title :: string), fold_switch(it.fold :: () -> Fold, it.open :: State<boolean>), function(): number
					return now().n or 0
				end)
			elseif it.kind == "own" then
				node = own_title((it.sec :: Section).id, function(): (Section, number)
					local cur = now()
					return cur.sec :: Section, cur.n or 0
				end)
			elseif it.kind == "group" then
				-- (An inner group's header hangs off its outer group's trunk.)
				node = group_head(it.g :: Group, function(): Group
					return now().g :: Group
				end, tree_at())
			elseif it.kind == "session" then
				local s = it.s :: Session
				local function at(): number
					local now = item_of(key)
					return if now and now.index then now.index else 1
				end
				node = session_row(s, it.by :: Organize, {
					list = it.list,
					list_now = function(): string
						local now = item_of(key)
						return if now and now.list then now.list else it.list :: string
					end,
					at = at,
					-- Its trunk while it sits in a group that draws one.
					tree_of = tree_at(),
				})
			elseif it.kind == "drop" then
				node = drop_here(it.text :: string)
			elseif it.kind == "pager" then
				-- A group's "Show N more" (its rest folded under it), and once
				-- they show, "Show less": quiet, the group's last leaf, its
				-- chevron where its sessions' marks stand.
				-- (A group's sessions' pager, or its inner groups': `pkey`.)
				local gkey = it.pkey or (it.g :: Group).key
				local function all_shown(): boolean
					return expanded:get()[gkey] == true
				end
				local says = SessionRow.pager_says(all_shown, function(): number
					return now().n or 0
				end)
				local tree_now = tree_at(true)
				node = on_tree(
					row {
						class = "group-pager",
						name = says,
						indent = function(): number
							local t = tree_now()
							return if t then gutter(t) else 0
						end,
						leads = function(): boolean
							return details.harness:get()
						end,
						lead = SessionRow.pager_lead(all_shown),
						words = SessionRow.pager_words(says),
						on_click = function()
							fold_set(more_fold(gkey), not all_shown())
						end,
					},
					tree_now
				)
			elseif it.kind == "empties" then
				-- The Archived row's own (a section's title: its height, its
				-- words, its ink) with a chevron before: "Empty", the first
				-- projects' tiles stacked, the count at its end.
				node = empties_row(now)
			elseif it.kind == "more" then
				-- "Show N more": the shelf's next few.
				node = row {
					name = function(): string
						return locale.s("ds.sidebar.show_more", { n = now().n or 0 })
					end,
					lead = Icon { name = "alt-arrow-down", size = 14 },
					words = line(label(function(): string
						return locale.s("ds.sidebar.show_more", { n = now().n or 0 })
					end)),
					on_click = function()
						shelf_shown:set(shelf_shown:peek() + SHELF_MORE)
					end,
				}
			else
				node = ui.div {}
			end
			-- Each row as tall as its place in the list, clipped to it (a
			-- folding place's rows close over), its ink with its folds.
			return ui.div {
				w_full = true,
				h = function(): number
					local s = sh:get()
					local i = s.at[key]
					return if i then heights:get()[i] or 0 else 0
				end,
				y = function(): number
					slid:get()
					local a = slides[key]
					return if a then value(a, clock:get()) else 0
				end,
				-- (The empty projects' row lets its tiles fly out of it.)
				overflow = if it.kind == "empties" then "visible" else "hidden",
				opacity = function(): number
					local now = item_of(key)
					local t = clock:get()
					local o = if now then row_ink(now, t) else 1
					-- A row making way (gliding down as another goes first)
					-- recedes as it passes, the rising one staying whole, so
					-- the two never read as one jumble of words.
					slid:get()
					local a = slides[key]
					if a and a.css and a.from < -SLOT / 2 then
						local p = 1 - value(a, t) / a.from
						o *= 1 - 0.65 * math.sin(math.pi * math.clamp(p, 0, 1))
					end
					return o
				end,
				node,
			}
		end
		-- Where each row stands now, by key, down the list's content.
		local function positions(): { [string]: number }
			local out: { [string]: number } = {}
			local s, hs = sh:peek(), heights:peek()
			local y = 0
			for i, it in s.items do
				out[it.key] = y
				y += laid_h(hs[i])
			end
			return out
		end
		-- The list has its new shape: each row that moved slides there from
		-- where it stood (`before`).
		local function slide_from(before: { [string]: number })
			for key, y in positions() do
				local was = before[key]
				if was and math.abs(was - y) > 0.5 then
					local a = anim(still(was - y), 0, DRAG_SLIDE.duration)
					-- (On the folds' curve: a drop's slide and the place it
					-- leaves closing run together, one motion.)
					a.css = true
					a.on_done = function()
						if slides[key] == a then
							slides[key] = nil
						end
					end
					slides[key] = a
				end
			end
			-- (A drop's own slides: none of the list's after them.)
			pending = nil
			slid:set(slid:peek() + 1)
		end
		-- A new shape (not a drop's): each row that moved glides from where
		-- it stood, as far as it moved taking longer (ds/motion's column
		-- pace), on CSS `ease`.
		ui.effect(function()
			shaped:get()
			local before = pending
			if not before then
				return
			end
			pending = nil
			local moved = false
			for key, y in positions() do
				local was = before[key]
				if was and math.abs(was - y) > 0.5 then
					local a: Anim
					a = anim(still(was - y), 0, motion.duration(was - y), 0, function()
						if slides[key] == a then
							slides[key] = nil
						end
					end)
					a.css = true
					slides[key] = a
					moved = true
				end
			end
			if moved then
				slid:set(slid:peek() + 1)
			end
		end)
		return ui.list {
			ref = list_view,
			-- (Filling the box its crossfade keeps: a list is 240 tall unless
			-- told.)
			w_full = true,
			h_full = true,
			offset = list_offset,
			heights = function(): { number }
				return heights:get()
			end,
			-- (Read tracked, so rows moving with the count and heights the same
			-- (a group, or a session in one, going first) move in the list.)
			key = function(i: number): string
				return sh:get().items[i].key
			end,
			row = function(key: string): Node
				return build(key)
			end,
			fade = edge_fades(list_offset, function(): number
				heights:get()
				return math.max(0, list_total - (list_view.height or 0))
			end),
			-- Sessions dropped on it land where it shows; sections, among yours.
			-- (A split pane's head carried over it isn't a row to place.)
			drop_when = function(payload: any): boolean
				return type(payload) == "table" and payload.split_pane == nil and (payload.session ~= nil or payload.section ~= nil)
			end,
			on_drag_over = function(payload: any, e)
				drag_y = e.y
				local el = list_view:peek()
				local top_y = if el then el.y else nil
				if payload.session and top_y then
					hover_at(sh:peek(), heights:peek(), top_y, e.y)
				end
			end,
			on_drag_leave = function(payload: any)
				if payload.session then
					set_over(nil)
				end
			end,
			on_drop = function(payload: any, e)
				local el = list_view:peek()
				local top_y = if el then el.y else nil
				if payload.section then
					if top_y then
						section_drop(sh:peek(), heights:peek(), top_y, e.y, payload.section)
					end
					return
				end
				if top_y then
					hover_at(sh:peek(), heights:peek(), top_y, e.y)
				end
				local o = over:peek()
				-- Where everything stands as it's let go, the dropped row where
				-- its preview is; then the list's new shape, everything sliding
				-- to it from there in one move (the drop zone's room closing
				-- under the rows as they come up).
				local before = positions()
				if top_y then
					before[`s:{payload.session}`] = e.y - top_y + list_offset:peek() - grab_dy
				end
				end_drag()
				if o then
					place(payload.session, if o.list == "regular" then nil else o.list, o.index)
				end
				slide_from(before)
			end,
		}
	end
	local REGROUP: PresenceOptions = {
		w_full = true,
		grow = 1,
		min_h = 0,
		enter = { opacity = { 0, 1 }, duration = COME, delay = GO },
		exit = { opacity = 0, duration = GO },
	}
	local middle: Node = ui.presence(function(): string
		return `{by_now()}/{sort:get()}`
	end, function(): Node
		return the_list(by_peek())
	end, REGROUP)

	-- A new section of your own, at the end of yours, its name being typed.
	local function new_section()
		local id = `section-{os.time()}-{math.random(1, 1e6)}`
		sections_edit(function(all: { Section }): { Section }
			-- (Its name is the user's data from now: a string, in the words of
			-- the moment.)
			local fresh: Section = { id = id, name = locale.s("ds.sidebar.new_section"), sessions = {} }
			table.insert(all, fresh)
			return all
		end)
		renaming:set(id)
	end

	-- The foot: you (your avatar and your name: your profile), and beside
	-- you settings' own button, a toggle on while they're open.
	local foot: Node? = nil
	if props.user then
		local function who(): User
			return follow(props.user, { name = "" } :: User)
		end
		local function name(): string
			return who().name
		end
		local you = row {
			name = name,
			on_click = props.on_user,
			selected = props.user_open,
			-- (Room for settings' button beside it, and the list's gap.)
			less = if props.on_settings then SLOT + ((ui.theme.get("zeron.list.gap") :: number?) or 2) else nil,
			-- Your avatar: a gradient of your own, from your id (another
			-- account, another avatar).
			lead = ui.presence(function(): string
				local u = who()
				return u.id or u.name
			end, function(seed: string): Node
				return Avatar {
					seed = seed,
					size = 20,
					label = who().name,
					-- Your photo, once you have one.
					src = function(): string?
						return who().avatar
					end,
				}
			end),
			words = line(label(name, nil, "$font.weight.medium")),
		}
		if props.on_settings then
			local on = ui.use_state(props.settings_open ~= nil and props.settings_open())
			if props.settings_open then
				local open = props.settings_open
				ui.effect(function()
					local v = open()
					if on:peek() ~= v then
						on:set(v)
					end
				end)
			end
			local on_settings = props.on_settings
			foot = ui.row {
				gap = "$zeron.list.gap",
				items = "center",
				you,
				ToggleButton {
					on = on,
					-- (ToggleButton's label is its tooltip: a string.)
					label = locale.s("ds.sidebar.settings"),
					icon = "settings",
					tone = "quiet",
					w = SLOT,
					h = SLOT,
					rounded = "$radius.lg",
					shrink = 0,
					-- (Hidden on the rail, as your name is: the rail has no foot.)
					opacity = shown_wide,
					on_change = function()
						on_settings()
					end,
				},
			}
		else
			foot = you
		end
	end

	-- The rail: an island of the menus' glass at the window's bottom left
	-- (as the title bar's island at its top), its icons in a column: new
	-- chat and search, pinned, your sections, each group's tile, each
	-- opening its flyout. It comes as the sidebar goes, and goes as it comes
	-- back.
	if is_island then
		local function stack(n: number): number
			return n * RAIL_SLOT + math.max(n - 1, 0) * im.gap
		end
		-- The scroll's room round its icons: enough for the badge on a tile's
		-- corner, which its edge would otherwise cut.
		local ROOM = 8
		-- What scrolls under the actions: pinned (a group of its own), your
		-- sections, then each group's tile, as the view groups them; at most
		-- four showing, its ends fading while more runs past them.
		local function tiles(): Node
			local by = by_peek()
			-- What the rail holds, as plain data: pinned (while it holds any),
			-- your sections, then the groups as the view makes them. Each tile
			-- is its key's for as long as it is there: a session pinned or
			-- moved, a section renamed, its tile follows in place (its flyout
			-- open, the rail where it was scrolled).
			type Held = { key: string, name: string, sessions: { Session } }
			local held = ui.computed(function(): { Held }
				local p = places:get()
				local out: { Held } = {}
				if #p.pinned > 0 then
					table.insert(out, { key = "pinned", name = "Pinned", sessions = sessions_of(p.pinned) })
				end
				local narrow = props.narrowed ~= nil and props.narrowed()
				for _, sec in p.sections do
					local own = sessions_of(sec.sessions)
					if narrow and #own == 0 then
						continue
					end
					table.insert(out, { key = `section:{sec.id}`, name = sec.name, sessions = own })
				end
				for _, g in (groups_of(by, placed_in(p))) do
					-- (The empty projects aren't on the rail: their fold is
					-- the list's.)
					if not g.empty then
						table.insert(out, { key = g.key, name = g.name :: string, sessions = listed(g.sessions) })
					end
				end
				return out
			end)
			local keys = ui.computed(function(): { string }
				local out: { string } = {}
				for _, h in held:get() do
					table.insert(out, h.key)
				end
				return out
			end)
			local function now(key: string): Held?
				for _, h in held:get() do
					if h.key == key then
						return h
					end
				end
				return nil
			end
			local function sessions(key: string): () -> { Session }
				return function(): { Session }
					local h = now(key)
					return if h then h.sessions else {}
				end
			end
			-- Each group's tile and what it says of itself, as the view makes it
			-- (a group the data brings later, found as its tile is made).
			local made: { [string]: Group } = {}
			local function made_of(key: string): Group?
				if not made[key] then
					-- (Built untracked: this reads what stands.)
					for _, g in (groups_of(by, {})) do
						made[g.key] = made[g.key] or g
					end
				end
				return made[key]
			end
			local function tile(key: string): Node
				if key == "pinned" then
					-- (Pinned, a group of its own: the room after it a group's.)
					return ui.column {
						gap = "$zeron.island.gap",
						items = "center",
						pin_icon(sessions(key)),
						ui.div { h = RAIL_GROUP - im.gap * 2, shrink = 0 },
					}
				end
				local section_id = string.match(key, "^section:(.*)$")
				if section_id then
					local function name(): string
						local h = now(key)
						return if h then h.name else ""
					end
					return (group({
						key = key,
						name = name,
						sessions = sessions(key),
						tile = function(hidden: () -> boolean, live: Sessions?): Node
							return group_tile(
								QUIET,
								ui.text {
									function(): string
										return string.upper(string.sub(name(), 1, 1))
									end,
									text_size = "$font.size.xs",
									weight = "$font.weight.semibold",
								},
								live or {},
								hidden
							)
						end,
					}, "none"))
				end
				local g = made_of(key)
				if not g then
					return ui.div {}
				end
				return (group({
					key = key,
					name = g.name,
					sessions = sessions(key),
					tile = g.tile,
					new_in = g.new_in,
					-- (Its project and device: where its new chat starts.)
					project = g.project,
					device = g.device,
					project_name = g.project_name,
					shows_tile = g.shows_tile,
				}, by))
			end
			-- How tall its tiles stand: four at most showing.
			local function pinned_room(): number
				local list = held:get()
				return if list[1] and list[1].key == "pinned" then RAIL_GROUP - im.gap else 0
			end
			local function tall(): number
				return stack(#held:get()) + pinned_room()
			end
			local function most(): number
				return stack(4) + pinned_room()
			end
			local rail_offset = ui.use_state(0)
			return ui.scroll {
				-- Wider than the rail (it has no surface to overhang), so a
				-- tile's badge isn't cut at its sides either. Its room is its
				-- content's (a scroll clips inside its own padding, where its
				-- edge fades would fall on nothing), so its clip and its fades
				-- are at its edge, reaching through the room into the icons.
				w = RAIL_SLOT + ROOM * 2,
				h = function(): number
					return math.min(tall(), most()) + ROOM * 2
				end,
				offset = rail_offset,
				fade = edge_fades(rail_offset, function(): number
					return tall() - math.min(tall(), most())
				end, ROOM + RAIL_SLOT / 2),
				ui.column {
					w_full = true,
					shrink = 0,
					p = ui.raw(ROOM, "the rail's room, counted in its height and fades"),
					items = "center",
					ui.each(keys, tile, {
						key = function(key: string): string
							return key
						end,
						-- (Tiles taking new places glide there, as the list's rows
						-- do: a group going first.)
						animate = motion.layout({ duration = motion.BASE, ease = motion.EASE }),
						w_full = true,
						gap = "$zeron.island.gap",
						items = "center",
					}),
				},
			}
		end
		return ui.column {
			w = island.thickness(),
			p = "$zeron.island.pad",
			-- (The scroll's room round its icons is part of the gap above it.)
			gap = RAIL_GROUP - ROOM,
			items = "center",
			opacity = ui.tween(ui.computed(function(): number
				return if collapsed:get() then 1 else 0
			end), { duration = 220, ease = "out(3)" }),
			scale = motion.tween(ui.computed(function(): number
				return if collapsed:get() then 1 else 0.96
			end), { duration = 260, ease = "out(3)" }),
			pointer_events = function()
				return if collapsed:get() then "auto" else "none"
			end,
			-- New chat and search: always there.
			top,
			-- (Made again, the old fading over the new, as the view's grouping
			-- changes: another set of groups.)
			ui.presence(function(): string
				return by_now()
			end, tiles, {
				enter = { opacity = { 0, 1 }, duration = COME, delay = GO },
				exit = { opacity = 0, duration = GO },
			}),
		}
	end

	-- Its view options, as Zeron's: a "…" at its top right, on the title
	-- bar's line (centred where its controls are), opening how it groups and
	-- what its sessions show. A grouping or a sort is a choice (the menu
	-- closes on it); what shows, toggles (it stays open, for several); then
	-- compact, and a new section of your own.
	local view_open = ui.use_state(false)
	-- (The menu is built as it opens: its words, read then, are the locale's.
	-- Each row's `label` its message's key.)
	local function menu_row(icon: Icon.Name, name: string, on: () -> boolean, pick: () -> ()): Node
		return Menu.Row { icon = icon, label = name, checked = on, on_click = pick }
	end
	local ORGANIZE: { { value: Organize, label: string, icon: Icon.Name } } = {
		{ value = "project", label = "ds.sidebar.view.by_project", icon = "folder" },
		{ value = "device", label = "ds.sidebar.view.by_device", icon = "laptop" },
		{ value = "none", label = "ds.sidebar.view.none", icon = "list" },
	}
	local SORT: { { value: Sort, label: string, icon: Icon.Name } } = {
		{ value = "updated", label = "ds.sidebar.view.last_updated", icon = "clock-circle" },
		{ value = "created", label = "ds.sidebar.view.created", icon = "calendar" },
	}
	local SHOW: { { key: string, label: string, icon: Icon.Name } } = {
		{ key = "branch", label = "ds.sidebar.view.branch", icon = "git-branch" },
		{ key = "pr", label = "ds.sidebar.view.pr", icon = "pull-request" },
		{ key = "harness", label = "ds.sidebar.view.harness", icon = "bot" },
		{ key = "project_icon", label = "ds.sidebar.view.project_icon", icon = "project-default" },
		{ key = "location", label = "ds.sidebar.view.location", icon = "remote-server" },
	}
	if (details :: any).keep_active then
		table.insert(SHOW, { key = "keep_active", label = "ds.sidebar.view.keep_active", icon = "bell" })
	end
	if (details :: any).project_filter then
		table.insert(SHOW, { key = "project_filter", label = "ds.sidebar.view.project_filter", icon = "folder" })
	end
	if (details :: any).archived then
		table.insert(SHOW, { key = "archived", label = "ds.sidebar.view.archived", icon = "archive-minimalistic" })
	end
	if (details :: any).empty_projects then
		table.insert(SHOW, { key = "empty_projects", label = "ds.sidebar.view.empty_projects", icon = "inbox" })
	end
	-- Opens it. (A press anywhere outside it closes it, this one too, as
	-- zgx closes any popover: toggling here would open it again.)
	local view_trigger = IconButton {
		icon = "more-horizontal",
		label = locale.t("ds.sidebar.view.options"),
		tooltip = locale.s("ds.sidebar.view.options"),
		lit = view_open,
		on_click = function()
			view_open:set(true)
		end,
	}
	local view_menu = ui.popover(Glass.style({
		open = view_open,
		anchor = view_trigger,
		label = locale.t("ds.sidebar.view.options"),
		-- Beside the sidebar, not over it, so what it changes shows as it's
		-- changed: past its edge as far as the rail's flyouts are (the
		-- trigger sits its inset in from it).
		placement = { side = "right", align = "start", offset = PAD + 4 },
		w = "$zeron.menu.width",
		p = 0,
		gap = 0,
		rounded = "$radius.lg",
		build = function()
			-- Its groups: how it groups, how it sorts, what shows, then compact
			-- and a new section, rules between.
			-- How it groups: by project, by device, both (a tree, its levels
			-- as the rows stand: the upper one outermost), or neither
			-- ("None"). The two rows are toggles (the menu stays open for the
			-- next), each led by a handle: dragged past the other's middle,
			-- or moved with ⌥↑ ⌥↓, it takes its place, and the tree with it.
			local function levels(): { string }
				return SessionRow.levels(organize:get())
			end
			-- The rows' order (both levels, on or not), and Organize as it
			-- follows: those on, in that order.
			local function order_now(): { string }
				return full_order(organize_order:peek())
			end
			local function apply(order: { string }, on: { [string]: boolean })
				organize_order:set(order)
				local next: { string } = {}
				for _, l in order do
					if on[l] then
						table.insert(next, l)
					end
				end
				organize:set(next)
			end
			local function on_set(): { [string]: boolean }
				local on: { [string]: boolean } = {}
				for _, l in SessionRow.levels(organize:peek()) do
					on[l] = true
				end
				return on
			end
			local function toggle(level: string)
				local on = on_set()
				on[level] = not on[level] or nil
				apply(order_now(), on)
			end
			-- `level` to place `to` among the rows (1, 2).
			local function move(level: string, to: number)
				local order = order_now()
				local from = table.find(order, level)
				if not from or from == to or to < 1 or to > #order then
					return
				end
				table.remove(order, from)
				table.insert(order, to, level)
				apply(order, on_set())
			end
			local LABEL: { [string]: string } = { project = "ds.sidebar.view.by_project", device = "ds.sidebar.view.by_device" }
			local dragging = ui.use_state(nil :: string?)
			local function level_row(level: string): Node
				local function name(): string
					return locale.s(LABEL[level])
				end
				-- Its grip stands where the menu's rows have their glyphs, at
				-- their size: quiet at rest, brighter under the pointer and
				-- while it's held.
				local hovered = ui.use_state(false)
				local function held(): boolean
					return dragging:get() == level
				end
				local function grip(lit: () -> boolean): Node
					return ui.div {
						w = "$zeron.menu.glyph.box",
						h = "$zeron.menu.glyph.box",
						shrink = 0,
						items = "center",
						justify = "center",
						color = function(): string
							return if lit() then "$text.secondary" else "$text.tertiary"
						end,
						cursor = function(): string
							return if held() then "grabbing" else "grab"
						end,
						Icon { name = "grip", size = (ui.theme.get("zeron.menu.glyph.size") :: number?) or 16 },
					}
				end
				return Menu.Row {
					class = "organize-level",
					lead = grip(function(): boolean
						return hovered:get() or held()
					end),
					on_hover = function(inside: boolean)
						hovered:set(inside)
					end,
					label = name(),
					checked = function(): boolean
						return table.find(levels(), level) ~= nil
					end,
					-- (Held: faint where it was, its lifted copy under the pointer.)
					opacity = function(): number
						return if held() then 0.35 else 1
					end,
					on_click = function()
						toggle(level)
					end,
					-- Dragged: it takes the other's place as the pointer crosses
					-- the other's middle (as the panel's tabs do); where it shows
					-- is where it lands.
					on_drag = function(): any
						dragging:set(level)
						return { organize_level = level }
					end,
					-- The row lifted: as it stands in the menu (its grip, its name,
					-- its check), its height, its width and its corner the menu
					-- row's (concentric with the menu's), on the menu's glass and
					-- shadow.
					drag_preview = function()
						local inset = (ui.theme.get("space.1") :: number?) or 4
						local menu_w = (ui.theme.get("zeron.menu.width") :: number?) or 232
						local card: { [any]: any } = {
							w = menu_w - inset * 2,
							h = "$zeron.menu.row",
							px = "$space.2",
							gap = "$space.2",
							items = "center",
							rounded = "$zeron.menu.row_radius",
							color = "$text.primary",
							grip(function(): boolean
								return true
							end),
							ui.text { name(), grow = 1, text_size = "$font.size.md" },
							-- (The menu's own check.)
							Menu.check(function(): boolean
								return table.find(levels(), level) ~= nil
							end),
						}
						Glass.style(card, "menu")
						return ui.row(card)
					end,
					on_drag_end = function()
						dragging:set(nil)
					end,
					drop_when = function(payload: any): boolean
						return type(payload) == "table" and payload.organize_level ~= nil and payload.organize_level ~= level
					end,
					on_drag_over = function(payload: any, e)
						local order = order_now()
						local from, here = table.find(order, payload.organize_level), table.find(order, level)
						if not from or not here then
							return
						end
						local past_middle = e.local_y > e.h / 2
						if (from < here and past_middle) or (from > here and not past_middle) then
							move(payload.organize_level, here)
						end
					end,
					on_drop = function(_payload: any) end,
					-- ⌥↑ ⌥↓: up or down a place.
					on_key = function(e: any): boolean?
						if e.alt and (e.key == "ArrowUp" or e.key == "ArrowDown") then
							local here = table.find(order_now(), level) or 1
							move(level, if e.key == "ArrowUp" then here - 1 else here + 1)
							return true
						end
						return nil
					end,
				}
			end
			-- (The rows' list as a state, as ui.each takes one.)
			local rows_order = ui.use_state(full_order(organize_order:peek()))
			ui.effect(function()
				local now = full_order(organize_order:get())
				local was = rows_order:peek()
				if was[1] ~= now[1] or was[2] ~= now[2] then
					rows_order:set(now)
				end
			end)
			local organize_rows: { Node } = { heading = locale.s("ds.sidebar.view.organize") } :: any
			table.insert(
				organize_rows,
				ui.each(
					rows_order,
					level_row,
					{
						key = function(level: string): string
							return level
						end,
						w_full = true,
						gap = 0,
						-- (Rows taking new places slide there.)
						animate = motion.layout({ duration = motion.BASE, ease = motion.EASE }),
					}
				)
			)
			for _, o in ORGANIZE do
				if o.value == "none" then
					table.insert(
						organize_rows,
						menu_row(o.icon, locale.s(o.label), function(): boolean
							return #levels() == 0
						end, function()
							organize:set({})
						end)
					)
				end
			end
			local sort_rows: { Node } = { heading = locale.s("ds.sidebar.view.sort") } :: any
			for _, o in SORT do
				table.insert(
					sort_rows,
					menu_row(o.icon, locale.s(o.label), function(): boolean
						return sort:get() == o.value
					end, function()
						sort:set(o.value)
						view_open:set(false)
					end)
				)
			end
			local show_rows: { Node } = { heading = locale.s("ds.sidebar.view.show") } :: any
			for _, d in SHOW do
				local st = (details :: any)[d.key] :: State<boolean>
				table.insert(
					show_rows,
					menu_row(d.icon, locale.s(d.label), function(): boolean
						return st:get()
					end, function()
						st:set(not st:peek())
					end)
				)
			end
			return ui.column {
				w_full = true,
				gap = 0,
				Menu.Group(organize_rows :: any),
				Menu.Rule(),
				Menu.Group(sort_rows :: any),
				Menu.Rule(),
				Menu.Group(show_rows :: any),
				Menu.Rule(),
				Menu.Group {
					-- Compact: its switch says; the row itself takes no click.
					Menu.Row {
						label = locale.s("ds.sidebar.view.compact"),
						cursor = "default",
						hover = {},
						active = {},
						trail = { Switch { value = compact, label = locale.s("ds.sidebar.view.compact") } },
					},
					menu_item("plus", locale.s("ds.sidebar.view.create_section"), function()
						view_open:set(false)
						new_section()
					end),
				},
			}
		end,
	}, "menu"))

	-- Dragging near the list's top or bottom scrolls it, as Zeron's does:
	-- within 48 of an edge, faster the nearer, up to 12 every 16 ms. Where
	-- a held session would land is worked out again as its list moves under
	-- it.
	local SCROLL_BAND, SCROLL_MAX = 48, 12
	local function step_scroll(): boolean
		if drag:peek() == nil and not held:peek() then
			return false
		end
		local view = list_view:peek()
		local y = drag_y
		local view_y = if view then view.y else nil
		local view_h = if view then view.height else nil
		if not view_y or not view_h or not y then
			return true
		end
		local top, bottom = view_y, view_y + view_h
		local step = 0
		if y < top + SCROLL_BAND then
			step = -SCROLL_MAX * math.clamp((top + SCROLL_BAND - y) / SCROLL_BAND, 0, 1)
		elseif y > bottom - SCROLL_BAND then
			step = SCROLL_MAX * math.clamp((y - (bottom - SCROLL_BAND)) / SCROLL_BAND, 0, 1)
		end
		if step == 0 then
			return true
		end
		local most = math.max(0, list_total - (list_view.height or 0))
		local from = list_offset:peek()
		local to = math.clamp(from + step, 0, most)
		if to ~= from then
			list_offset:set(to)
			if drag:peek() and live.shape then
				hover_at(live.shape :: Shape, live.heights, view_y, y)
			end
		end
		return true
	end
	-- (A timer while something is dragged; it stops itself as the drag ends.)
	ui.effect(function()
		if drag:get() ~= nil or held:get() then
			ui.every(16, step_scroll)
		end
	end)
	-- Its column: the top, a notice if it has one, the list, you at the foot
	-- (collapsed, folded away: the rail keeps to what you work in, and what
	-- you start).
	local body: { [any]: any } = {
		w = function(): number
			return room()
		end,
		h_full = true,
		shrink = 0,
		px = ui.raw(PAD, "its inset, counted in its rows' widths"),
		pt = ui.raw((props.top or 0) + 4, "the title bar's strip over it, then its own inset"),
		gap = "$space.2",
		top,
	}
	if props.notice then
		table.insert(body, props.notice)
	end
	-- (The list's region: what plugins place in the sidebar by, and its
	-- sessions, in order, under them.)
	table.insert(body, ui.column {
		id = if is_island then nil else "sidebar",
		provide = { sessions = order },
		w_full = true,
		grow = 1,
		min_h = 0,
		-- None at all yet: said so, faintly, at the list's head (Zeron's);
		-- under projects, their groups (each with its new chat) say it.
		ui.show(function(): boolean
			local ix = index:get()
			local grouped = SessionRow.levels(organize:get())[1] == "project" and #ix.projects > 0
			return not is_island and not grouped and #ix.everything == 0 and #ix.pinned_ids == 0
		end, ui.text {
			locale.t("ds.sidebar.empty"),
			id = "sidebar-empty",
			px = "$space.2",
			pb = "$space.2",
			shrink = 0,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		}),
		middle,
	})
	-- The foot: a tip over you, a row's gap between them as between the
	-- rows at the top, so the two read as one list.
	local feet: { any } = {}
	if props.tip then
		table.insert(feet, props.tip)
	end
	if foot then
		-- (The sidebar's own foot: it never folds, the rail having none, so
		-- it's as tall as it is from the start, a reload included.)
		table.insert(feet, ui.column {
			-- You: out of plugins' reach.
			hook = false,
			w_full = true,
			h = SLOT + PAD,
			shrink = 0,
			overflow = "hidden",
			foot,
		})
	end
	if #feet > 0 then
		table.insert(body, ui.column { w_full = true, shrink = 0, gap = "$zeron.list.gap", table.unpack(feet) })
	end
	return ui.column {
		id = if is_island then "rail" else nil,
		w = function(): number
			return shown_w:get()
		end,
		-- Folded away, out of layout and out of reach (no row to tab to),
		-- its scroll and folds kept for when it opens.
		hidden = function(): boolean
			return collapsed:get() and shown_w:get() <= 0
		end,
		h_full = true,
		shrink = 0,
		overflow = "hidden",
		relative = true,
		ui.column(body),
		-- Under a title bar laid over it, its head is the window's to move it
		-- by (a double click zooms it), as a title bar is: under its "…" and
		-- its edge, which keep their presses.
		if props.top
			then ui.div {
				id = "sidebar-drag",
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				h = props.top,
				window_drag = true,
			}
			else ui.div {},
		-- Its view options, at its top right: on the title bar's line when one
		-- lies over it (its controls centred halfway down it, under its top
		-- inset: Zeron's 38 and 4), else at its own inset. Where it stands
		-- open, whatever its width as it folds or comes back: its edge clips
		-- it there, as it clips the rest, so it never slides under the title
		-- bar's controls and the window's buttons on the way.
		ui.div {
			absolute = true,
			left = function(): number
				local button = (ui.theme.get("zeron.icon_button.md") :: number?) or 24
				return room() - PAD - button
			end,
			top = if props.top then (props.top + 4) / 2 - 12 else PAD,
			opacity = shown_wide,
			pointer_events = function()
				return if collapsed:get() then "none" else "auto"
			end,
			view_menu,
		},
		-- A hairline down its right, between it and what it opens, full
		-- height (stronger under the pointer at its edge, and as it's
		-- dragged); collapsed, gone (the rail sits on the window as its
		-- icons).
		ui.div {
			absolute = true,
			right = 0,
			top = 0,
			bottom = 0,
			w = 1,
			pointer_events = "none",
			bg = function()
				-- (The cards beside it are its edge at rest; the
				-- hairline shows only under the pointer and as it's dragged.)
				return if edge_over:get() or dragging:get() then "$border.strong" else "#00000000"
			end,
			opacity = shown_wide,
		},
		-- Its edge: dragged, it resizes (242 to 400), following the pointer at
		-- once. (Pressed itself, so it keeps the pointer as it's dragged.)
		ui.div {
			absolute = true,
			right = 0,
			top = 0,
			bottom = 0,
			w = 6,
			cursor = "col_resize",
			label = locale.t("ds.sidebar.resize"),
			pointer_events = function()
				return if collapsed:get() then "none" else "auto"
			end,
			on_hover = function(inside: boolean)
				edge_over:set(inside)
			end,
			on_press = function(e)
				from_x, from_w = e.x, wide:peek()
				resizing = true
				dragging:set(true)
				edge_dragging:set(true)
			end,
			on_pointer_move = function(e)
				if resizing then
					wide:set(math.clamp(from_w + e.x - from_x, MIN_W, MAX_W))
				end
			end,
			on_release = function()
				resizing = false
				dragging:set(false)
				edge_dragging:set(false)
			end,
		},
	}
end)
