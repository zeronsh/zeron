-- A dropdown: a compact trigger (an icon, the chosen item, a chevron) that
-- opens a frosted panel: a search field, the items it filters (each with a
-- letter tile, the chosen one in the selected state), then actions below a
-- rule. Items
-- may carry an icon and a line saying what they do (a mode's), the search
-- may be left out (`search = false`, for a few choices), and `head` and
-- `extra` put something of the caller's at the panel's top and foot, a
-- rule between (a setting beside the choice: where to work, a switch).
--
-- `Dropdown { value = state, items = { { value = "zgx", label = "zgx" }, … },
-- actions = { { label = "New project", icon = "plus", on_click = … } },
-- icon = "folder", placeholder = "No project", search = "Search projects…" }`

local Icon = require("@self/icon")
local Mark = require("@self/mark")
local AgentMark = require("@self/agent_mark")
local RollText = require("@self/roll_text")
local FadeText = require("@self/fade_text")
local PrivateText = require("@self/private_text")
local Avatar = require("@self/avatar")
local Text = require("@self/text")
local Tile = require("@self/tile")
local ascii = require("@self/ascii")
local Menu = require("@self/menu")
local Skeleton = require("@self/skeleton")
local control = require("@self/control")
local locale = require("@self/locale")
local EditorMenu = require("@self/editor_menu")

-- An item: its name, and the harness mark or icon it shows in place of its
-- letter tile (a model's, a mode's), what it does (under its name), a muted
-- tag at its end ("You"), and a warning that it is out of reach (a device
-- offline).
export type Item = {
	value: string,
	label: string,
	mark: Mark.Name?,
	-- An agent from the ACP registry's SVG, in a mark's place, in the
	-- agent's colours (ds.AgentMark: `svg_seed`, the agent's id, its hue).
	svg: string?,
	svg_seed: string?,
	icon: Icon.Name?,
	-- Its tile on a palette hue (a project's: its initial in the hue's 50 on
	-- its 500), in place of the plain letter tile.
	hue: string?,
	-- A project's id, where the item is one: its tile is then
	-- `.project-icon` (ds/tile).
	project: string?,
	-- Its own artwork (a project's favicon), in place of its hue's tile.
	src: string?,
	-- A picture chosen for it (a project's emoji), on its hue's wash.
	emoji: string?,
	description: string?,
	-- Its label is an email address: drawn as ds.PrivateText draws one, its
	-- tile no letter of it (`seed`'s avatar).
	private: boolean?,
	tag: string?,
	warn: boolean?,
	-- Shown but not to be picked (a device out of reach): dimmed, passed
	-- over by the keys, its `description` saying why.
	disabled: boolean?,
}
-- What a caller's own trigger is given: whether the panel is open, and how
-- to open or close it. A submenu trigger forwards its hover changes so
-- leaving both the row and its panel can close it.
export type Menu = { open: State<boolean>, toggle: () -> (), on_hover: ((inside: boolean) -> ())? }
-- (The panel's own words, an action's, an empty list's, the placeholder,
-- the search's and the detail, may be messages, `ui.t`. An item's label
-- stays a string: the search reads it, and its tile its first letter.)
-- `shown`: whether it's offered now (followed as it changes); none, always.
export type Action = { label: string | Message | () -> string, icon: Icon.Name, on_click: (() -> ())?, shown: (() -> boolean)? }
-- What an empty list shows: what is missing, and how to fill it.
export type Empty = { title: string | Message, hint: (string | Message)? }
-- The trigger's weight: `filled` (a control on its own, with a chevron),
-- `ghost` (a chip among a bar's controls: no fill until touched), `chip` (a
-- footer's small quiet label). Only `filled` shows a chevron.
export type Tone = "filled" | "ghost" | "chip"
export type Props = {
	value: State<string?>,
	-- The items: a list, or a state or function of one (items that come
	-- and go, followed in place).
	items: { Item } | State<{ Item }> | () -> { Item },
	actions: { Action }?,
	tone: Tone?,
	-- The trigger's leading icon (or the chosen item's mark, with `mark`;
	-- with neither, the chosen item's icon, if items have them), and what it
	-- says with nothing chosen.
	icon: (Icon.Name | () -> Icon.Name)?,
	mark: boolean?,
	placeholder: (string | Message)?,
	-- A muted second half to the trigger's label (a model's effort).
	detail: (string | Message | () -> string)?,
	-- After the detail, at the trigger's end (the model chip's usage ring).
	trail: Node?,
	-- The label and detail roll only for a change made in the open panel; one
	-- from elsewhere (another chat's own choice shown) is there at once.
	roll_in_panel: boolean?,
	-- The search field's placeholder (a function follows what it names);
	-- `false`, no search.
	search: (string | Message | boolean | () -> string)?,
	-- Which items show at all (a tab's), before the search narrows them.
	filter: ((item: Item) -> boolean)?,
	-- Called as the panel opens.
	on_open: (() -> ())?,
	-- Bumped, the panel opens, as a press on its trigger opens it (a
	-- shortcut that opens this menu).
	open_with: State<number>?,
	-- Set to whether the panel is open (the caller holding still what it
	-- hangs from while it is).
	showing: State<boolean>?,
	-- Whether `extra` (and its rule) shows; always, by default.
	extra_when: (() -> boolean)?,
	-- A row's own control at its end (a star to favourite it), given the
	-- item and whether its row is highlighted (under the pointer or the
	-- keys); and what an empty list shows, given what is searched for.
	trailing_item: ((item: Item, lit: () -> boolean) -> Node?)?,
	-- A row wrapped in something of the caller's (a submenu opening beside
	-- it as the pointer rests there), given the item and its row.
	wrap_item: ((item: Item, row: Node) -> Node)?,
	-- While false, no search and no list (and no rules for them): the head
	-- alone, in place (the panel stays open as it turns).
	list_when: (() -> boolean)?,
	-- What the trigger says instead of the chosen item, while it says
	-- anything (a chip with nothing to choose: "No project").
	say: (() -> (string | Message)?)?,
	empty: ((query: string) -> Empty)?,
	-- A panel that opens folded on its chosen item: while `expanded` is
	-- false the head and the search fold away and every item but the chosen
	-- one folds to nothing (clipped, out of sight), and they open around it
	-- as it turns true (at once: see `folding`). `on_expand` asks to open out (a click on the chosen
	-- item, ↑/↓, typing). `keep_open`: a pick leaves the panel open
	-- (`on_choose` hears it: the caller folds it again); `on_escape` takes
	-- Escape first (true: handled, the panel stays).
	expanded: (() -> boolean)?,
	-- What it lists is still on its way: its chosen item's name and its rows
	-- hold their shape rather than saying there's nothing.
	loading: (() -> boolean)?,
	on_expand: (() -> ())?,
	keep_open: boolean?,
	on_choose: ((value: string) -> ())?,
	on_escape: (() -> boolean)?,
	-- At the panel's top, above a rule, and under a rule at its foot: the
	-- caller's own rows.
	head: Node?,
	extra: Node?,
	-- The panel's width (232 by default).
	width: number?,
	-- The caller's own trigger in place of the chip (a row of another
	-- panel opening this one beside it, as a submenu).
	trigger: ((menu: Menu) -> Node)?,
	-- A side-opening menu whose parent stays reachable by the pointer.
	submenu: boolean?,
	-- How far the trigger folds (a footer short of room): 0, all of it; 1,
	-- without its detail; 2, its leading icon alone. And while `accent` is
	-- true, that icon is in the accent (a mode on).
	fold: (() -> number)?,
	accent: (() -> boolean)?,
	-- What assistive technology calls it.
	label: Label?,
	-- Where the panel opens (zgx's popover placement); below, from the
	-- trigger's start, by default.
	placement: { side: string?, align: string?, offset: number? }?,
	-- The panel read bottom up (opening above its trigger): the head nearest
	-- the trigger, the search, then the list.
	bottom_up: boolean?,
}

-- A row of the panel: compact, full width. Actions wash under the pointer;
-- items wash where the highlight is (the pointer's or the keys'), so the
-- two never disagree.
-- (ROW is the menus' row, `zeron.menu.row`, here as a number: the list's
-- geometry is worked out from it.)
local ROW = 28
local function row(props: Merge<ContainerProps, { tall: boolean? }>, hover: boolean?): Node
	local p = table.clone(props)
	-- A row with a description grows to hold it, its tile on its title's
	-- line (the tile is a line high) and its check centred.
	local tall = p.tall == true
	p.tall = nil
	if hover == false then
		p.hover = {}
	end
	Menu.look(p, tall)
	return ui.row(p)
end

-- The list's geometry: its height before it scrolls, the inset above its
-- first row, and the room kept between the highlight and either edge as
-- the keys move it (a row: what comes next always shows).
local VIEW, INSET, GUTTER = 220, 4, ROW
-- Between rows: the theme's list gap (`zeron.list.gap`, read as the list
-- lays out: the themes load after this module); and a row's step down the
-- list.
local function gap(): number
	return (ui.theme.get("zeron.list.gap") :: number?) or 2
end
local function step(): number
	return ROW + gap()
end
-- The list's height for `n` rows: its insets, the rows, the gaps between.
local function list_h(n: number): number
	return INSET * 2 + math.max(n * step() - gap(), 0)
end
-- An empty list's height: room for its glyph, title and a two-line hint.
local EMPTY_H = 128

local function text(label: string | Message | () -> string, tone: string?): Node
	return FadeText {
		label,
		text_size = "$font.size.md",
		color = tone or "$text.primary",
		grow = 1,
	}
end

-- An item's tile: the first letter of its name, or its mark or icon.
local function tile(item: Item): Node
	if item.private then
		return Avatar { seed = item.label, size = 18, label = locale.t("ds.provider_card.account") }
	end
	if item.hue or item.src or item.emoji then
		return Tile { label = item.label, hue = item.hue, src = item.src, emoji = item.emoji, size = 18, project = item.project }
	end
	if item.icon then
		return Menu.glyph(item.icon)
	end
	if item.svg then
		return ui.div {
			w = 18,
			h = 18,
			shrink = 0,
			items = "center",
			justify = "center",
			AgentMark { label = item.label, seed = item.svg_seed, icon = item.svg, size = 14, bare = true },
		}
	end
	if item.mark then
		return ui.div { w = 18, h = 18, shrink = 0, items = "center", justify = "center", Mark { name = item.mark, size = 14, bare = true } }
	end
	local label = item.label
	return ui.div {
		w = 18,
		h = 18,
		shrink = 0,
		rounded = "$radius.sm",
		bg = "$zeron.secondary.bg",
		items = "center",
		justify = "center",
		ui.text {
			string.upper(string.sub(label, 1, 1)),
			font = "$font.mono",
			text_size = "$font.size.xs",
			weight = "$font.weight.semibold",
			color = "$accent",
		},
	}
end

-- Each tone's trigger: its size, its fill and ink at rest, under the pointer
-- (and open) and pressed, its label's ink (nil: the trigger's own), its
-- detail's (a step quieter than the label) and its leading icon's size.
local TRIGGER = {
	filled = {
		h = 26,
		px = "$space.2",
		gap = "$space.1",
		rounded = "$radius.md",
		text_size = "$font.size.sm",
		bg = "$zeron.secondary.bg",
		color = "$text.secondary",
		hover = { bg = "$zeron.secondary.hover", color = "$text.primary" },
		active = { bg = "$zeron.secondary.active", color = "$text.primary" },
		label = "$text.primary",
		detail = "$text.tertiary",
		glyph = 14,
	},
	ghost = {
		h = 32,
		px = "$zeron.space.tight",
		gap = "$zeron.space.tight",
		rounded = "$radius.md",
		text_size = "$font.size.sm",
		bg = "#00000000",
		color = "$text.primary",
		hover = { bg = "$hover" },
		active = { bg = "$pressed" },
		label = nil,
		detail = "$text.tertiary",
		glyph = 16,
	},
	chip = {
		-- (The composer's footer chips: its 24 and 8, its glyph 6 in all
		-- round.)
		h = 24,
		px = "$zeron.space.tight",
		gap = "$zeron.space.tight",
		rounded = "$radius.lg",
		text_size = "$font.size.sm",
		-- Its words lifted a point: centred by their capitals, their
		-- lowercase (most of what a chip says) reads low beside its glyph.
		-- (A margin under them of twice that.)
		lift = "$zeron.space.hair",
		bg = "#00000000",
		color = "$text.tertiary",
		hover = { bg = "$hover", color = "$text.secondary" },
		active = { bg = "$pressed", color = "$text.secondary" },
		label = nil,
		detail = "$text.disabled",
		glyph = 12,
	},
}

-- A chip's label: how wide it may grow, and how long its end fades when
-- it is longer.
local CHIP_LABEL, CHIP_FADE = 160, 24

-- Folding and opening out are one step, no easing and no fading: the panel
-- is a popup window of its own, so a height easing over frames resized it
-- on every one (on Wayland each a new configure; before xdg_popup moved in
-- place, a new surface the compositor faded in, the whole panel flickering
-- see-through as it opened out), and parts of it fading in read as the
-- panel going transparent.

-- What folds: `height` tall while `open`, clipped to nothing (and out of
-- sight and reach: the search no field to find) while not.
local function folding(open: () -> boolean, height: number, ...: Node): Node
	return ui.column {
		-- (Named for tests: what opens out with the list.)
		class = "dropdown-fold",
		w_full = true,
		gap = 0,
		shrink = 0,
		overflow = "hidden",
		h = function(): number
			return if open() then height else 0
		end,
		opacity = function(): number
			return if open() then 1 else 0
		end,
		...,
	}
end

-- A rule across the whole panel, edge to edge: the menus'.
local function rule(): Node
	return Menu.Rule()
end

return ui.component(function(props: Props): Node
	-- The items now, however they were given (read where they're shown, so
	-- what shows them follows them).
	local function items(): { Item }
		local given: any = props.items
		if type(given) == "function" then
			local call: () -> { Item } = given
			return call()
		elseif type(given) == "userdata" then
			local state: State<{ Item }> = given
			return state:get()
		end
		return given
	end
	local open = ui.use_state(false)
	local query = ui.use_state("")
	local value = props.value
	-- Opening upward (above its trigger): its far edge, the top, is the one
	-- that moves as it folds and opens.
	local up = props.placement ~= nil and props.placement.side == "top"
	-- Reading bottom up (`bottom_up`): the head and search by the trigger,
	-- the list over them, so a list changing length moves only the far edge.
	local bottom_up = props.bottom_up == true

	local function chosen(): string
		if props.say then
			local said = control.text(props.say())
			if said then
				return said
			end
		end
		local v = value:get()
		for _, item in items() do
			if item.value == v then
				return item.label
			end
		end
		return control.text(props.placeholder) or locale.s("ds.dropdown.placeholder")
	end

	local shown = ui.computed(function(): { Item }
		local q = string.lower(query:get())
		local out: { Item } = {}
		local keep = props.filter
		for _, item in items() do
			if (keep == nil or keep(item)) and (q == "" or string.find(string.lower(item.label), q, 1, true)) then
				table.insert(out, item)
			end
		end
		return out
	end)

	-- The highlighted row: an item (by value), or an action ("action:<n>");
	-- the keys run through the items, then on into the actions, and round. And the
	-- list's scroll offset.
	local hi = ui.use_state(nil :: string?)
	local offset = ui.use_state(0)
	-- The item under the pointer alone (the highlight follows the keys too):
	-- a folded panel's one row washes only under it.
	local under = ui.use_state(nil :: string?)
	local function index_of(v: string?): number?
		for i, item in shown:peek() do
			if item.value == v then
				return i
			end
		end
		return nil
	end
	-- The list's height: its rows up to the view (a short list leaves no
	-- space under it), or an empty one's message.
	-- (Its rows as laid out: a row with a description under it is taller
	-- than a row, so the list is as tall as its rows really are.)
	local content_h = ui.use_state(0)
	local function list_height(): number
		local n = #shown:get()
		if n == 0 then
			return EMPTY_H
		end
		local measured = content_h:get()
		return math.min(VIEW, if measured > 0 then measured else list_h(n))
	end
	-- Scroll so row `i` keeps a gutter from either edge.
	local function reveal(i: number)
		local n = #shown:peek()
		local top = INSET + (i - 1) * step()
		local o = offset:peek()
		local most = math.max(0, list_h(n) - VIEW)
		if top - GUTTER < o then
			offset:set(math.max(0, top - GUTTER))
		elseif top + ROW + GUTTER > o + VIEW then
			offset:set(math.min(most, top + ROW + GUTTER - VIEW))
		end
	end
	local actions: { Action } = props.actions or {}
	-- Every row the keys stop at, in order: the items shown, then the actions.
	local function stops(): { string }
		local out: { string } = {}
		for _, item in shown:peek() do
			if not item.disabled then
				table.insert(out, item.value)
			end
		end
		for n, action in actions do
			if not action.shown or action.shown() then
				table.insert(out, `action:{n}`)
			end
		end
		return out
	end
	local function move(by: number)
		local list = stops()
		if #list == 0 then
			return
		end
		local at = 0
		for i, v in list do
			if v == hi:peek() then
				at = i
			end
		end
		if at == 0 then
			at = if by > 0 then 0 else #list + 1
		end
		-- Round the ends: past the last, the first; before the first, the last.
		local i = (at + by - 1) % #list + 1
		hi:set(list[i])
		local n = #shown:peek()
		-- Into the actions: the list at its end, so the order reads on.
		reveal(math.min(i, math.max(n, 1)))
	end
	local function run(n: number)
		open:set(false)
		local action = actions[n]
		if action and action.on_click then
			action.on_click()
		end
	end
	local function choose(v: string)
		local n = string.match(v, "^action:(%d+)$")
		if n then
			run(tonumber(n) :: number)
			return
		end
		-- (One not to be picked stays as it is.)
		for _, item in shown:peek() do
			if item.value == v and item.disabled then
				return
			end
		end
		-- At once: the pick and what the caller does on it (folding the
		-- panel), so the list eases from where it was.
		ui.batch(function()
			value:set(v)
			if props.on_choose then
				props.on_choose(v)
			end
			if not props.keep_open then
				open:set(false)
			end
		end)
	end
	-- A search: the first match highlighted, the list at its top. No search
	-- (as it opens): the chosen item highlighted, in view.
	local function settle(list: { Item })
		local chosen_at = if query:peek() == "" then index_of(value:peek()) else nil
		offset:set(0)
		if chosen_at then
			hi:set(list[chosen_at].value)
			reveal(chosen_at)
		else
			hi:set(if list[1] then list[1].value else nil)
		end
	end

	-- Folding (a panel that opens folded). Its rows never fold: the list is
	-- a window over them, its height and scroll set together, folded to the
	-- one row of the chosen item, opening out from where that row is
	-- (upward, the rows over it come into view), folding back onto the pick.
	-- The search clears as it folds; folded, typing opens it out.
	local view_h = ui.use_state(0)
	local function folded(): boolean
		return props.expanded ~= nil and not props.expanded()
	end
	-- Where the folded window sits: on row `i`, its insets either side.
	local function on_row(i: number): number
		return (i - 1) * step()
	end
	-- The list's height: folded, one row; opened out, one height whatever
	-- the tab or the search shows (as tall as every item, up to the view,
	-- and room for the empty list's message), a short list leaving room
	-- under it: switching tabs or searching never moves the panel's edge.
	-- (It began as a fix: a popup that changed size was shown anew on
	-- Wayland, a new `xdg_popup` the compositor faded in as the last faded
	-- out, so a list easing to each tab's height flickered the panel
	-- see-through. zgx moves and resizes a popup in place since 0e7cd810;
	-- the one height stays, as the picker's look.)
	local function height_for(list: { Item }, now: boolean, i: number?): number
		if now and i then
			return list_h(1)
		end
		return math.min(VIEW, math.max(EMPTY_H, list_h(#items())))
	end
	local was_folded: boolean? = nil
	local back_on_chosen: () -> () = function() end
	if props.expanded then
		ui.effect(function()
			local list = shown:get()
			local now = folded()
			local v = value:get()
			local i = index_of(v)
			local h = height_for(list, now, i)
			if was_folded == nil then
				view_h:set(h)
				if now and i then
					offset:set(on_row(i))
					hi:set(v)
				else
					settle(list)
				end
			elseif now ~= was_folded then
				local to: number
				if now then
					to = if i then on_row(i) else 0
					query:set("")
				else
					-- Opening out: the chosen row stays where it is (upward,
					-- by the search, the list growing over it), as far as the
					-- list's ends allow.
					local most = math.max(0, list_h(#list) - h)
					local o = offset:peek()
					to = math.clamp(if up then o - (h - view_h:peek()) else o, 0, most)
					if i then
						hi:set(v)
					end
				end
				ui.batch(function()
					view_h:set(h)
					offset:set(to)
				end)
			else
				-- A tab, a search (opened out, one height: see height_for).
				view_h:set(h)
				if now then
					if i then
						offset:set(on_row(i))
					end
				else
					settle(list)
				end
			end
			was_folded = now
		end)
		-- An offset set before the rows are laid out is clamped to the top by the
		-- list, and said again unchanged it's no news to it: the chosen row is
		-- out of the window, and the panel's head is blank. So once the rows have
		-- their height, it's said again, a hair different (well under a point).
		local hair = false
		back_on_chosen = function()
			local i = index_of(value:peek())
			if folded() and i then
				hair = not hair
				offset:set(on_row(i) + (if hair then 0.01 else 0))
			end
		end
		-- Folded, the window stays on the chosen row: whatever else moves it
		-- (a wheel over it, the list clamping an offset set before it had its
		-- rows) is put back. Put back a
		-- frame later, once at a time (never in the same update: the list's
		-- own write back would answer it, and so on without end).
		local function astray(): boolean
			local i = index_of(value:peek())
			return folded() and i ~= nil and math.abs(offset:peek() - on_row(i)) > 0.5
		end
		local mending = false
		ui.effect(function()
			offset:get()
			if mending or not astray() then
				return
			end
			mending = true
			ui.after(16, function()
				mending = false
				if astray() then
					offset:set(on_row(index_of(value:peek()) :: number))
				end
			end)
		end)
		ui.effect(function()
			if query:get() ~= "" and props.on_expand and folded() then
				props.on_expand()
			end
		end)
	else
		ui.effect(function()
			settle(shown:get())
		end)
	end
	local tone = props.tone or "filled"
	local function chosen_mark(): Mark.Name?
		local v = value:get()
		for _, item in items() do
			if item.value == v then
				return if item.svg then nil else item.mark
			end
		end
		return nil
	end
	-- (A registry agent's icon, in the mark's place: its seed, then the SVG.)
	local function chosen_svg(): string
		local v = value:get()
		for _, item in items() do
			if item.value == v then
				return if item.svg then `{item.svg_seed or item.label}\0{item.svg}` else ""
			end
		end
		return ""
	end
	-- Open, the trigger keeps the wash it has under the pointer.
	local trigger_props = table.clone(TRIGGER[tone]) :: any
	trigger_props.bg = function()
		return if open:get() then TRIGGER[tone].hover.bg else TRIGGER[tone].bg
	end
	local leading: Child = nil
	if props.mark then
		local glyph = TRIGGER[tone].glyph
		leading = ui.row {
			shrink = 0,
			ui.show(function()
				return chosen_mark() ~= nil
			end, Mark {
				name = function(): Mark.Name
					return chosen_mark() or "claude"
				end,
				size = glyph,
				bare = true,
			}),
			ui.presence(chosen_svg, function(key: string): Node
				local seed, code = string.match(key, "^(.-)%z(.*)$")
				if not code or code == "" then
					return ui.div {}
				end
				return AgentMark { label = seed :: string, seed = seed, icon = code, size = glyph, bare = true }
			end),
		}
	elseif props.icon then
		leading = Icon { name = props.icon, size = TRIGGER[tone].glyph }
	else
		local function chosen_icon(): Icon.Name?
			local v = value:get()
			for _, item in items() do
				if item.value == v then
					return item.icon
				end
			end
			return nil
		end
		local any_icon = false
		for _, item in items() do
			any_icon = any_icon or item.icon ~= nil
		end
		if any_icon then
			local function glyph(): Node
				return Icon {
					name = function(): Icon.Name
						return chosen_icon() or "check"
					end,
					size = TRIGGER[tone].glyph,
				}
			end
			local accent = props.accent
			leading = ui.show(function()
				return chosen_icon() ~= nil
			end, if accent
				then ui.div {
					shrink = 0,
					-- Two glyphs, so the plain one keeps following the
					-- trigger's own ink (and its hover).
					ui.show(accent, ui.div { color = "$accent", glyph() }),
					ui.show(function()
						return not accent()
					end, glyph()),
				}
				else glyph())
		end
	end
	local rolls = if props.roll_in_panel
		then function(): boolean
			return open:peek()
		end
		else nil
	-- The detail, shown only while it says something (an empty one would
	-- still take the trigger's gap) and the trigger hasn't folded it away.
	local detail: Child = nil
	if props.detail then
		local d = props.detail
		local function said(): string
			return control.text(d) or ""
		end
		local fold = props.fold
		detail = ui.show(function()
			return said() ~= "" and (fold == nil or fold() < 1)
		end, FadeText {
			said,
			mb = TRIGGER[tone].lift,
			roll = true,
			rolls = rolls,
			color = TRIGGER[tone].detail,
		})
	end
	-- The chosen item's name. A chip's is held to a width and, past it, fades
	-- out at its end rather than cutting to "…" (only while it overflows).
	local label_node: Node
	if tone == "chip" then
		local clip, text = ui.ref(), ui.ref()
		-- A row, so the text keeps its own width inside (a column would
		-- stretch it to the box's, and nothing would overflow).
		label_node = ui.row {
			ref = clip,
			-- (Lifted by a margin under it: the row stays centred, its words
			-- a point up, nothing outside its clip.)
			mb = TRIGGER[tone].lift,
			min_w = 0,
			items = "center",
			max_w = CHIP_LABEL,
			overflow = "hidden",
			fade_x = function()
				local over = (text.width or 0) > (clip.width or 0) + 0.5
				return { 0, if over then CHIP_FADE else 0 }
			end,
			RollText { chosen, rolls = rolls, ref = text, shrink = 0, color = TRIGGER[tone].label },
		}
	else
		label_node = FadeText { chosen, roll = true, rolls = rolls, color = TRIGGER[tone].label }
	end
	if props.loading then
		label_node = ui.show(props.loading, Skeleton.Bar { w = ui.raw(64, "about a model's name"), h = 10 }, label_node)
	end
	-- Opening: the search cleared, on the chosen item, in view. (Open
	-- already, it stays as it is: asked again, by its shortcut or its
	-- trigger, it was put back folded, the search and tab lost.)
	local function show_panel()
		if open:peek() then
			return
		end
		if props.on_open then
			props.on_open()
		end
		query:set("")
		local v = value:peek()
		if v then
			hi:set(v)
		end
		open:set(true)
		local i = index_of(v)
		if props.expanded then
			-- Folded on the chosen item from the start.
			local now = folded()
			view_h:set(height_for(shown:peek(), now, i))
			offset:set(if now and i then on_row(i) else 0)
			was_folded = now
		elseif i then
			reveal(i)
		end
	end
	if props.open_with then
		local asked = props.open_with
		local seen = asked:peek()
		ui.effect(function()
			local n = asked:get()
			if n ~= seen then
				seen = n
				if not open:peek() then
					show_panel()
				end
			end
		end)
	end
	if props.showing then
		local showing = props.showing
		ui.effect(function()
			local now = open:get()
			if showing:peek() ~= now then
				showing:set(now)
			end
		end)
		ui.on_cleanup(function()
			if showing:peek() then
				showing:set(false)
			end
		end)
	end
	local function toggle()
		if open:peek() then
			open:set(false)
		else
			show_panel()
		end
	end
	-- A hover-opened submenu stays open across the gap between its row
	-- and panel. Leaving both starts a short, cancellable closing grace.
	local over_trigger = false
	local over_panel = false
	local cancel_close: (() -> ())? = nil
	local function stop_close()
		if cancel_close then
			cancel_close()
			cancel_close = nil
		end
	end
	local function hover(inside: boolean, panel: boolean)
		if panel then
			over_panel = inside
		else
			over_trigger = inside
		end
		stop_close()
		if not over_trigger and not over_panel and open:peek() then
			cancel_close = ui.after(150, function()
				cancel_close = nil
				if not over_trigger and not over_panel then
					open:set(false)
				end
			end)
		end
	end
	if props.submenu then
		ui.on_cleanup(stop_close)
		ui.effect(function()
			if not open:get() then
				stop_close()
				over_panel = false
			end
		end)
	end
	local trigger_ref = ui.ref()
	-- (Its trigger gone from view, as Settings stand over the page: closed, not left
	-- with nothing to hang from.)
	ui.effect(function()
		local w = trigger_ref.width
		if w ~= nil and w <= 0 and open:peek() then
			open:set(false)
		end
	end)
	local trigger = if props.trigger then props.trigger({
		open = open,
		toggle = toggle,
		on_hover = if props.submenu
			then function(inside: boolean)
				hover(inside, false)
			end
			else nil,
	}) else ui.button {
		ref = trigger_ref,
		label = props.label,
		h = trigger_props.h,
		px = trigger_props.px,
		gap = trigger_props.gap,
		items = "center",
		rounded = trigger_props.rounded,
		text_size = trigger_props.text_size,
		weight = "$font.weight.medium",
		min_w = 0,
		bg = trigger_props.bg,
		color = trigger_props.color,
		hover = trigger_props.hover,
		active = trigger_props.active,
		-- (Open, a press on it closes it, as a menu's trigger does.)
		on_click = toggle,
		leading,
		if props.fold
			then ui.show(function()
				return (props.fold :: () -> number)() < 2
			end, label_node)
			else label_node,
		detail,
		props.trail,
		if tone == "filled" then Icon { name = "alt-arrow-down", size = 12 } else nil,
	}

	-- The menus' glass (frosted: a light tint over the platform's own
	-- popover material; opaque: a solid panel). No padding of its own: its
	-- rules run edge to edge; each section insets itself.
	local panel = Menu.panel {
		-- (Named for tests.)
		class = "dropdown-panel",
		open = open,
		anchor = trigger,
		placement = props.placement or { side = "bottom", align = "start", offset = 6 },
		w = props.width or "$zeron.menu.width",
		build = function()
			-- The keys: up and down move the highlight, Enter takes it.
			-- The search field, once made: where the keys go as the panel is
			-- opened out by the pointer (typing filters, Escape folds it).
			local search_el: any = nil
			local function keys(e): boolean
				-- Folded, the arrows open it out.
				if
					props.expanded
					and not props.expanded()
					and (e.key == "ArrowDown" or e.key == "ArrowUp")
				then
					if props.on_expand then
						props.on_expand()
					end
					return true
				end
				if e.key == "ArrowDown" then
					move(1)
					return true
				elseif e.key == "ArrowUp" then
					move(-1)
					return true
				elseif e.key == "Enter" then
					local v = hi:peek()
					if v then
						choose(v)
					end
					return true
				elseif e.key == "Escape" and props.on_escape and props.on_escape() then
					return true
				elseif e.key == "Tab" then
					-- Tab leaves it, as it leaves a menu: closed, the keys
					-- back on its trigger, to go on from there.
					open:set(false)
					local el = trigger_ref:peek()
					if el then
						el:focus()
					end
					return true
				end
				return false
			end
			-- What a search browses: the head, the search, the list, the
			-- actions.
			local function browse(): { Node }
				local rows: { Node } = {}
				-- The head and the search, folding together: above the list,
				-- each over its rule; opening upward, below it (the search,
				-- then the head, by the trigger), each under its rule.
				local top: { Node } = {}
				-- No list at all (no search, nothing to list, nothing to say
				-- for it): the head alone, no rule under it.
				local listless = props.search == false and #items() == 0 and props.empty == nil
				-- Behind `list_when`: all but the head.
				local function gated(node: Node): Node
					return if props.list_when then ui.show(props.list_when, node) else node
				end
				local function section(node: Node, is_head: boolean?)
					local shown = if is_head then node else gated(node)
					if listless then
						table.insert(top, shown)
					elseif bottom_up then
						table.insert(top, gated(rule()))
						table.insert(top, shown)
					else
						table.insert(top, shown)
						table.insert(top, gated(rule()))
					end
				end
				local head = if props.head then ui.column { w_full = true, p = "$space.1", props.head } else nil
				if head and not bottom_up then
					section(head, true)
				end
				-- The search, filtering as it is typed.
				if props.search ~= false then
					local given: any = props.search
					local named = type(given) == "string" or control.is_message(given)
					local search = if named or type(given) == "function"
						then given
						else locale.t("ds.dropdown.search_placeholder")
					local field = ui.row {
						h = 36,
						px = "$space.3",
						gap = "$space.2",
						items = "center",
						color = "$text.tertiary",
						Icon { name = "search", size = 14 },
						ui.input(EditorMenu.on({
							value = query,
							-- (A function's search is named "Search": the tests
							-- and assistive technology find it by that name.)
							label = if named or type(given) ~= "function" then search else locale.t("common.search"),
							placeholder = search,
							grow = 1,
							h = 24,
							px = 0,
							border = 0,
							bg = "#00000000",
							-- No ring or border of its own: the panel shows where
							-- focus is.
							focus = { border = 0, outline = 0 },
							text_size = "$font.size.md",
							on_key = keys,
							-- Typing filters as the panel opens, whatever is
							-- above the search (a head's rows): focused after
							-- the popover gives focus to its first row.
							on_mount = function(el)
								search_el = el
								ui.after(0, function()
									el:focus()
								end)
							end,
						})),
					}
					section(field)
				end
				if head and bottom_up then
					section(head, true)
				end
				-- Put in place above the list (or, upward, below it).
				local function place_top()
					if props.expanded then
						-- The head (a 32-point row inset 4 each way) and the
						-- search, each with its rule.
						local tall = (if props.head then 40 + 1 else 0) + (if props.search ~= false then 36 + 1 else 0)
						table.insert(rows, folding(props.expanded, tall, table.unpack(top)))
					else
						for _, node in top do
							table.insert(rows, node)
						end
					end
				end
				if not bottom_up then
					place_top()
				end
				-- A row's own control at its end; not while folded (the one row
				-- there only opens the panel).
				local function trailing(item: Item): Node?
					local node = if props.trailing_item
						then props.trailing_item(item, function(): boolean
							return hi:get() == item.value
						end)
						else nil
					local expanded = props.expanded
					if not node or not expanded then
						return node
					end
					return ui.show(expanded, node)
				end
				local items: { Node } = {}
				table.insert(
					items,
					ui.each(shown, function(item: Item)
						local r = row({
							role = "option",
							-- What assistive technology hears the selected
							-- state as (the check it no longer shows).
							selected = function(): boolean
								return value:get() == item.value
							end,
							bg = function()
								-- Folded, the one row is a control of its own: a
								-- wash under the pointer, nothing else.
								if props.expanded and not props.expanded() then
									return if under:get() == item.value then "$hover" else "#00000000"
								end
								-- The chosen item in the selected state (as a
								-- sidebar's row), the highlight (pointer or keys) a
								-- wash on the rest.
								if value:get() == item.value then
									return "$zeron.menu.selected"
								end
								return if hi:get() == item.value then "$hover" else "#00000000"
							end,
							on_hover = function(inside: boolean)
								if inside then
									hi:set(item.value)
									under:set(item.value)
								elseif under:peek() == item.value then
									under:set(nil)
								end
							end,
							on_click = function()
								-- Folded, the chosen item opens it out.
								if props.expanded and not props.expanded() then
									if props.on_expand then
										props.on_expand()
									end
									return
								end
								choose(item.value)
							end,
							tall = item.description ~= nil,
							opacity = if item.disabled then ui.raw(0.45, "a choice out of reach, dimmed") else nil,
							cursor = if item.disabled then "default" else nil,
							tile(item),
							if item.description
								then ui.column {
									grow = 1,
									min_w = 0,
									gap = "$zeron.space.hair",
									if item.private
										then PrivateText {
											item.label,
											reveal = false,
											truncate = true,
											text_size = "$font.size.md",
											line_height = "$font.line.md",
											weight = "$font.weight.medium",
											color = "$text.primary",
										}
										else ui.text {
											item.label,
											text_size = "$font.size.md",
											line_height = "$font.line.md",
											weight = "$font.weight.medium",
											color = "$text.primary",
										},
									ui.text {
										item.description,
										text_size = "$font.size.sm",
										line_height = "$font.line.sm",
										color = "$text.tertiary",
										wrap = true,
									},
								}
								else text(item.label),
							if item.tag
								then ui.text {
									item.tag,
									self = "center",
									shrink = 0,
									text_size = "$font.size.xs",
									color = "$text.tertiary",
								}
								else nil,
							-- Out of reach: Wi-Fi off, in the warning's ink (a glyph,
							-- not the word, as Zeron's device menu shows it).
							if item.warn
								then ui.div { self = "center", shrink = 0, color = "$warning", Icon { name = "wifi-off", size = 12 } }
								else nil,
							trailing(item),
							-- Folded, a chevron where the check is: it opens.
							if props.expanded
								then ui.show(function()
									return value:get() == item.value and not (props.expanded :: () -> boolean)()
								end, ui.div {
									self = "center",
									color = "$text.tertiary",
									Icon { name = "alt-arrow-down", size = 14 },
								})
								else nil,
						}, false)
						if props.wrap_item then
							r = props.wrap_item(item, r)
						end
						if not props.expanded then
							return r
						end
						-- Folded, the others are out of sight (the edges of
						-- the rows either side would show in the window's
						-- insets), at once as it folds and opens out.
						local expanded = props.expanded
						return ui.div {
							w_full = true,
							opacity = function(): number
								return if expanded() or value:get() == item.value then 1 else 0
							end,
							r,
						}
					end, {
						key = function(item: Item)
							return item.value
						end,
						w_full = true,
					})
				)
				-- Nothing to list: the list's room as a quiet field of
				-- characters (the slider's, in one faint ink), clearing in the
				-- middle for what is missing (a mono label, as the panel's
				-- section titles) and how to fill it.
				local function said(): Empty
					local q = query:get()
					if props.empty then
						return props.empty(q)
					end
					return {
						title = locale.s("ds.dropdown.no_matches"),
						hint = if q ~= "" then locale.s("ds.dropdown.nothing_matches", { query = q }) else nil,
					}
				end
				table.insert(
					items,
					ui.show(function()
						return #shown:get() == 0 and props.loading ~= nil and props.loading()
					end, ui.column {
						w_full = true,
						gap = "$zeron.list.gap",
						Skeleton.Bar { w_full = true, h = ROW, rounded = Menu.row_radius() },
						Skeleton.Bar { w_full = true, h = ROW, rounded = Menu.row_radius() },
						Skeleton.Bar { w_full = true, h = ROW, rounded = Menu.row_radius() },
					})
				)
				table.insert(
					items,
					ui.show(function()
						return #shown:get() == 0 and not (props.loading ~= nil and props.loading())
					end, ui.column {
						w_full = true,
						-- (Opened out, the list's whole window: its message in
						-- the middle of it.)
						h = if props.expanded
							then function(): number
								return math.max(EMPTY_H, view_h:get()) - INSET * 2
							end
							else EMPTY_H - INSET * 2,
						relative = true,
						overflow = "hidden",
						rounded = Menu.row_radius(),
						-- The field thins out toward the list's top and foot.
						fade = { 16, 16 },
						px = "$space.4",
						gap = "$zeron.space.tight",
						items = "center",
						justify = "center",
						ui.div {
							absolute = true,
							left = 0,
							right = 0,
							top = 0,
							bottom = 0,
							pointer_events = "none",
							bg_shader = ascii.QUIET,
							shader_params = ascii.quiet("$text.tertiary", 0.15, { 100, 20 }),
							on_mount = ascii.drift,
						},
						ui.text {
							function(): string
								return string.upper(control.text(said().title) or "")
							end,
							font = "$font.mono",
							text_size = "$font.size.xs",
							letter_spacing = 1,
							color = "$text.secondary",
						},
						ui.show(function(): boolean
							return said().hint ~= nil
						end, Text {
							function(): string
								return control.text(said().hint) or ""
							end,
							style = "caption",
							tone = "tertiary",
							align = "center",
							wrap = true,
							w_full = true,
						}),
					})
				)
				-- The items scroll past about seven rows; the search and the
				-- actions stay put. Without a search there are only a few: they
				-- sit as they are.
				if listless then
					-- (Nothing to list.)
				elseif props.search == false then
					table.insert(rows, gated(ui.column { w_full = true, p = "$space.1", gap = "$zeron.list.gap", table.unpack(items) }))
				else
					local list = ui.scroll {
						w_full = true,
						-- As tall as its rows, up to the view: a short list leaves
						-- no empty space under it.
						-- (Folding: the window's height, eased with its scroll.)
						h = if props.expanded then view_h else list_height,
						offset = offset,
						-- Each edge fades only where there is more to scroll that way
						-- (in the list as it will stand), easing out as the end
						-- comes; folded, neither.
						fade = function()
							if folded() then
								return { 0, 0 }
							end
							local list = shown:get()
							local measured = content_h:get()
							local whole = if measured > 0 then measured else list_h(#list)
							local most = math.max(0, whole - list_height())
							local o = offset:get()
							return { math.min(o, 12), math.min(math.max(0, most - o), 12) }
						end,
						ui.column {
							w_full = true,
							p = "$space.1",
							gap = "$zeron.list.gap",
							on_resize = function(size: { w: number, h: number })
								if math.abs(size.h - content_h:peek()) >= 0.5 then
									content_h:set(size.h)
								end
								back_on_chosen()
							end,
							table.unpack(items),
						},
					}
					if props.expanded then
						-- Folded, the one row is a control, not a list: a layer
						-- over it takes the pointer, so a wheel there scrolls
						-- nothing (no list under it to scroll), a click opens it
						-- out, and the row washes as if under the pointer itself.
						local function chosen_value(): string?
							local v = value:peek()
							return if index_of(v) then v else nil
						end
						list = ui.div {
							w_full = true,
							relative = true,
							list,
							ui.show(
								folded,
								ui.div {
									-- (Named for tests: the press that opens it out.)
									class = "folded",
									absolute = true,
									left = 0,
									right = 0,
									top = 0,
									bottom = 0,
									cursor = "pointer",
									-- (The pointer's way to open it out: the keys have the row.)
									aria_hidden = true,
									focusable = false,
									on_hover = function(inside: boolean)
										under:set(if inside then chosen_value() else nil)
									end,
									on_click = function()
										if props.on_expand then
											props.on_expand()
										end
										-- (The press left the keys on the panel: the
										-- search takes them, once it shows.)
										ui.after(0, function()
											if search_el then
												search_el:focus()
											end
										end)
									end,
								}
							),
						}
					end
					table.insert(rows, gated(list))
				end
				if props.actions and #props.actions > 0 then
					table.insert(rows, rule())
					local acts: { Node } = {}
					for n, action in props.actions do
						local key = `action:{n}`
						local made = row({
							color = "$text.secondary",
							bg = function()
								return if hi:get() == key then "$hover" else "#00000000"
							end,
							on_hover = function(inside: boolean)
								if inside then
									hi:set(key)
								end
							end,
							on_click = function()
								run(n)
							end,
							Menu.glyph(action.icon),
							text(action.label),
						}, false)
						table.insert(acts, if action.shown then ui.show(action.shown, made) else made)
					end
					table.insert(rows, ui.column { w_full = true, p = "$space.1", gap = "$zeron.list.gap", table.unpack(acts) })
				end
				if bottom_up then
					place_top()
				end
				return rows
			end
			local rows: { Node } = {}
			for _, row in browse() do
				table.insert(rows, row)
			end
			if props.extra then
				local foot = ui.column { w_full = true, rule(), ui.column { w_full = true, p = "$space.1", props.extra } }
				table.insert(rows, if props.extra_when then ui.show(props.extra_when, foot) else foot)
			end
			if props.search == false then
				-- No field to type in: the panel itself takes the keys, focused
				-- as it opens.
				return ui.column {
					-- The panel's full width, so its rules reach both edges
					-- (not the width of its widest row).
					w_full = true,
					gap = 0,
					-- (It has the keys: what it is, for assistive technology.)
					role = "listbox",
					label = props.label,
					focusable = true,
					focus = { outline = 0 },
					on_mount = function(el)
						el:focus()
					end,
					on_key = keys,
					table.unpack(rows),
				}
			end
			-- A list that can go (`list_when`): the panel itself holds the focus
			-- when it does, as the search it was in goes with it (a popover
			-- loses its focus, and closes, otherwise), and takes the keys.
			if props.list_when then
				local list_when = props.list_when
				local root: any = nil
				ui.effect(function()
					if not list_when() and root then
						-- At once (before the search can take the focus with it),
						-- and again once the list has gone.
						local el = root
						el:focus()
						ui.after(0, function()
							el:focus()
						end)
					end
				end)
				return ui.column {
					w_full = true,
					gap = 0,
					role = "listbox",
					label = props.label,
					focusable = true,
					focus = { outline = 0 },
					on_key = keys,
					on_mount = function(el)
						root = el
						if not list_when() then
							ui.after(0, function()
								el:focus()
							end)
						end
					end,
					table.unpack(rows),
				}
			end
			-- Escape from whatever in it has the keys (the slider, a tab, a
			-- star, not only the search) is the panel's first: opened out,
			-- it folds it back, rather than reaching the popover, which
			-- closes.
			local on_escape = props.on_escape
			return ui.column {
				w_full = true,
				gap = 0,
				on_key = if on_escape
					then function(e): boolean
						return e.key == "Escape" and on_escape()
					end
					else nil,
				table.unpack(rows),
			}
		end,
	}
	if props.submenu then
		local build = panel.build
		panel.build = nil
		panel.anchor = nil
		panel.trigger = trigger
		panel.label = props.label
		panel[1] = ui.presence(function(): boolean
			return open:get()
		end, function(visible: boolean): Node
			if not visible then
				return ui.div {}
			end
			return ui.column {
				w_full = true,
				on_hover = function(inside: boolean)
					hover(inside, true)
				end,
				build(),
			}
		end)
		return ui.submenu(panel)
	end
	return ui.popover(panel)
end)
