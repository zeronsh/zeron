-- A context menu: what a secondary click (a Control-click on macOS), the
-- Menu key or Shift+F10 opens on the thing it is over (a session, a
-- message, a file, the empty list around them), at the pointer, or under
-- what has focus from the keys. The dropdown's frosted panel and rows, so
-- it is one design system: an icon and a name, the keys that do the same
-- at its end, headings and rules between groups, items that toggle (a
-- check at the end), items in the danger ink that ask once more before
-- they act, items out of reach that say why, and submenus.
--
-- `ContextMenu { items = function(at) return { … } end, open = state, ring =
-- "$radius.md", w_full = true, row }`: `items` is asked as the menu opens
-- (returning nil opens none, and the click goes on to what is around it);
-- `open`, the caller's, says while it shows. Opened from the keys, the thing
-- it is about keeps a ring meanwhile (with `ring`, its corner), so focus can
-- be followed; from the pointer, none (the pointer already says which).
-- Other props go to the box around the thing.
--
-- The keys: ↑ ↓ (round the ends), Home, End, typing a name's first letters
-- (again and again, a letter steps through those it starts), → into a
-- submenu and ← back out, Enter or Space to choose, Escape to close one
-- level, Tab to close them all. The pointer: a submenu opens as its row is
-- reached and stays open while the pointer heads for it, across the rows
-- in its way (Zeron's hover intent: crates/ui/src/popover/hover_intent.rs).

local Icon = require("@self/icon")
local FadeText = require("@self/fade_text")
local RollText = require("@self/roll_text")
local Kbd = require("@self/kbd")
local Menu = require("@self/menu")
local glass_of = require("@self/glass")
local control = require("@self/control")
local locale = require("@self/locale")

-- An entry: an item (a `label`), a `heading` over the items after it, or a
-- `separator`. An item may have its glyph, its keys ("cmd+shift+c", shown
-- as ⇧⌘C), `danger` (the danger ink), `confirm` (what it says asking once
-- more, as a first choice arms it and a second does it), `disabled` (true,
-- or why: said under its name), `checked` (a check at its end while true;
-- `keep_open` leaves the menu open as it toggles) and `submenu` (its own
-- entries, beside it).
-- (Its words may be messages, `ui.t`: read as text as the menu opens.)
export type Entry = {
	label: (string | Message)?,
	icon: Icon.Name?,
	shortcut: string?,
	on_select: (() -> ())?,
	danger: boolean?,
	confirm: (string | Message)?,
	disabled: (boolean | string | Message)?,
	checked: (() -> boolean)?,
	keep_open: boolean?,
	submenu: { Entry }?,
	heading: (string | Message)?,
	separator: boolean?,
}
-- Where it was asked for: in window points, and from the box's top left.
export type At = { x: number, y: number, local_x: number, local_y: number }
export type Props = {
	items: (at: At) -> { Entry }?,
	open: State<boolean>?,
	ring: string?,
	-- What assistive technology calls the menu ("Session actions").
	label: Label?,
	-- The panel's width (232, the dropdown's).
	width: number?,
	-- In place of all that: these entries drawn as the open panel, in place
	-- (the gallery's specimen, to see it without opening it).
	still: { Entry }?,
	-- Handed what opens it at a point in the window (`x`, `y`), its items
	-- asked then as from a click there: for a host showing menus for what
	-- has no box to hang one from (ds/menu_at, a link in a paragraph).
	-- False back when it didn't open (none to show, or another took it).
	hook: ((open_at: (x: number, y: number) -> boolean) -> ())?,
	[any]: any,
}

-- The panel's width: the menus' (`zeron.menu.width`, 232). A row's look
-- (its height, padding and corner) is the menus' (`Menu.look`).
local WIDTH = "$zeron.menu.width"
-- How long the pointer may take crossing other rows to an open submenu
-- before the row it rests on takes over (Zeron's grace, renewed while it
-- keeps moving toward it), how far past the submenu's edges its corridor
-- reaches, and how far the pointer must move to count as heading somewhere.
local GRACE, SLACK, NUDGE = 300, 8, 2
-- Typing: how long between letters they still make one name (as zgx's own
-- menus and AppKit's).
local TYPEAHEAD = 750
-- A second choice of an armed item within this long is the first one's
-- double click, not a decision.
local ARM_GUARD = 300
-- A choice blinks before the menu closes, as AppKit's do: off, then on.
local BLINK = 50

-- Whether the pointer at `p`, having last rested on the trigger at `o`, is
-- still heading for the submenu `b` (on the left of it, `left`): inside the
-- triangle from `o` to the submenu's near edge, its ends `SLACK` past the
-- submenu's top and foot (Zeron's `corridor`).
local function toward(o: { x: number, y: number }, p: { x: number, y: number }, b: Element, left: boolean): boolean
	local bx, by, bw, bh = b.x, b.y, b.width, b.height
	if not (bx and by and bw and bh) then
		return false
	end
	local edge = if left then bx + bw else bx
	local dir = if left then -1 else 1
	local distance = (edge - o.x) * dir
	local advance = (p.x - o.x) * dir
	if distance <= 0 or advance <= 0 or advance > distance + SLACK then
		return false
	end
	local f = math.min(advance / distance, 1)
	local top = o.y + (by - SLACK - o.y) * f
	local bottom = o.y + (by + bh + SLACK - o.y) * f
	return p.y >= top and p.y <= bottom
end

-- Rules between groups (edge to edge, the groups inset themselves) and a
-- group's title: the menus' own (`Menu.Rule`, `Menu.Heading`).
local function rule(): Node
	return Menu.Rule()
end
local function heading(name: string): Node
	-- (It opens its group: no room over it but the group's inset.)
	return Menu.Heading { name, first = true }
end

-- One open menu's panel (the root, or a submenu): its rows, the keys among
-- them, and the submenus they open. `chain` is shared by every level of one
-- menu: how to close it all.
type Chain = { close: () -> () }
-- A level's props: its entries; `back`, a submenu's, out to its trigger;
-- `keyed`, opened from the keys (its first item lit); and, a submenu's,
-- telling its trigger's level where it is and that the pointer reached it.
type PanelProps = {
	entries: { Entry },
	chain: Chain,
	back: (() -> ())?,
	keyed: () -> boolean,
	placed: ((el: Element) -> ())?,
	entered: (() -> ())?,
	-- Drawn in place, not opened (the gallery's specimen): its submenus'
	-- rows only, nothing focused as it shows.
	still: boolean?,
}
-- The items the keys stop at, in order: each its element (once mounted),
-- whether it can be chosen, and a submenu's open state.
-- A submenu's trigger also keeps its panel's box, once built.
type Stop = { entry: Entry, el: Element?, enabled: boolean, open: State<boolean>?, panel: Element? }

-- A level, as a component: made below, once its builder (which makes its
-- submenus' levels with it) is defined.
local panel_component: ((props: PanelProps) -> Node)? = nil
local function Panel(props: PanelProps): Node
	local make = panel_component
	if not make then
		error("context_menu: a level was drawn before its component was made")
	end
	return make(props)
end

-- The glass every level shares: the menus', frosted or opaque, its groups
-- and rules edge to edge.
local function glass(props: { [any]: any }): { [any]: any }
	props.p = 0
	props.gap = 0
	return glass_of.style(props, "menu")
end

-- An entry's words as strings, read as its level is built (it opens anew
-- each time): a label, confirm, heading or reason may be a message (`ui.t`),
-- and the keys' typeahead and the armed confirm's roll read them as text.
local function plain(list: { Entry }): { Entry }
	local out: { Entry } = {}
	for i, e in list do
		local any_message = control.is_message(e.label) or control.is_message(e.confirm) or control.is_message(e.heading) or control.is_message(e.disabled)
		if any_message then
			local c = table.clone(e) :: any
			c.label = control.text(e.label)
			c.confirm = control.text(e.confirm)
			c.heading = control.text(e.heading)
			if control.is_message(e.disabled) then
				c.disabled = control.text(e.disabled)
			end
			out[i] = c
		else
			out[i] = e
		end
	end
	return out
end

local function build_panel(props: PanelProps): Node
	local entries, chain, back, keyed = plain(props.entries), props.chain, props.back, props.keyed
	local stops: { Stop } = {}
	local box: Element? = nil
	-- The item with the focus (the highlight), by its place in `stops`.
	local focused = ui.use_state(0)
	-- An item asking once more (its place), and when it began to.
	local armed = ui.use_state(0)
	local armed_at = 0
	-- The item blinking as it is chosen.
	local blinking = ui.use_state(0)
	local blink_on = ui.use_state(true)

	-- Hover intent: the open submenu's trigger (its place), where the
	-- pointer last was on that trigger, and a row waiting for the grace to run out (its place, where the
	-- pointer was, and how to stop the wait).
	local sub_at = 0
	local origin: { x: number, y: number }? = nil
	local pending = 0
	local pending_at: { x: number, y: number }? = nil
	local stop_wait: (() -> ())? = nil
	-- The row under the pointer (its place; 0: none).
	local under = 0

	local function focus(i: number)
		local s = stops[i]
		if s and s.el then
			s.el:focus()
			-- (Set here too: called from a row's own pointer handler, zgx skips
			-- the row's `on_focus`, its listener being the one running.)
			focused:set(i)
		end
	end
	local function rest()
		-- (Already resting: nothing lit.)
		if box and focused:peek() ~= 0 then
			box:focus()
			focused:set(0)
		end
	end
	local function cancel_wait()
		if stop_wait then
			stop_wait()
			stop_wait = nil
		end
		pending = 0
		pending_at = nil
	end
	local function close_sub()
		cancel_wait()
		local s = stops[sub_at]
		if s and s.open then
			s.open:set(false)
		end
		sub_at = 0
		origin = nil
	end
	-- The pointer reached row `i` (or moved on it, to `p`).
	local function reached(i: number, p: { x: number, y: number }?)
		under = i
		local s = stops[i]
		if s.open then
			-- A submenu's trigger: zgx opens its submenu as it is reached
			-- (and closes the others); the trigger keeps the focus, so the
			-- keys stay with this panel until → goes in.
			if sub_at ~= i then
				cancel_wait()
				sub_at = i
			end
			if p then
				origin = p
			end
			if focused:peek() ~= i then
				focus(i)
				-- (Again once zgx has moved the focus into the submenu it
				-- opened.)
				ui.after(0, function()
					if under == i then
						focus(i)
					end
				end)
			end
			return
		end
		-- (Closed meanwhile, by ← or Escape: nothing to head for.)
		if sub_at ~= 0 and not (stops[sub_at].open :: State<boolean>):peek() then
			cancel_wait()
			sub_at = 0
			origin = nil
		end
		-- Just entered, a submenu open: where the pointer is heading is for
		-- its first move to say.
		if sub_at ~= 0 and not p and origin then
			return
		end
		local sub_el = if sub_at ~= 0 then stops[sub_at].panel else nil
		if sub_at ~= 0 and p and origin and sub_el then
			local trig = stops[sub_at].el
			local left = trig ~= nil and sub_el.x ~= nil and trig.x ~= nil and (sub_el.x :: number) < (trig.x :: number)
			if toward(origin, p, sub_el, left) then
				-- Heading for the submenu across this row: wait, renewing the
				-- wait while the pointer keeps on toward it; stopping, or
				-- turning back, hands over to this row.
				local dir = if left then -1 else 1
				local was = pending_at
				local forward = if was then (p.x - was.x) * dir else NUDGE
				if pending == i and forward <= -NUDGE then
					cancel_wait()
				elseif pending ~= i or forward >= NUDGE then
					cancel_wait()
					pending = i
					pending_at = p
					stop_wait = ui.after(GRACE, function()
						stop_wait = nil
						local waiting = pending
						pending = 0
						if waiting ~= 0 and under == waiting then
							close_sub()
							if stops[waiting].enabled then
								focus(waiting)
							else
								rest()
							end
						end
					end)
					return
				else
					return
				end
			end
		end
		if sub_at ~= 0 then
			close_sub()
		end
		if s.enabled then
			if focused:peek() ~= i then
				focus(i)
			end
		else
			rest()
		end
	end
	local function left_row(i: number)
		if under == i then
			under = 0
		end
		-- Off every row (out of the panel, or into a rule): nothing lit,
		-- unless a submenu is open (its trigger stays lit).
		ui.after(0, function()
			if under == 0 and sub_at == 0 and pending == 0 and focused:peek() == i then
				rest()
			end
		end)
	end

	-- Choosing item `i`: a submenu opens (and its first item takes the
	-- focus); an item asking once more arms; otherwise it blinks, the menu
	-- closes (all of it, unless it stays open to toggle again) and then it
	-- runs.
	local function choose(i: number, from_keys: boolean)
		local s = stops[i]
		if not s or not s.enabled or blinking:peek() ~= 0 then
			return
		end
		local e = s.entry
		if s.open then
			sub_at = i
			s.open:set(true)
			return
		end
		if e.confirm and armed:peek() ~= i then
			armed:set(i)
			armed_at = ui.now()
			return
		end
		if e.confirm and ui.now() - armed_at < ARM_GUARD then
			return
		end
		if e.keep_open then
			if e.on_select then
				e.on_select()
			end
			return
		end
		blinking:set(i)
		blink_on:set(false)
		ui.after(BLINK, function()
			blink_on:set(true)
			ui.after(BLINK, function()
				blinking:set(0)
				chain.close()
				if e.on_select then
					e.on_select()
				end
			end)
		end)
	end

	-- Typing: what has been typed toward a name, and when last.
	local typed = ""
	local typed_at = 0
	local function seek(ch: string)
		local now = ui.now()
		if now - typed_at > TYPEAHEAD then
			typed = ""
		end
		typed_at = now
		typed ..= string.lower(ch)
		-- One letter again and again: step through the names it starts.
		local same = string.rep(string.sub(typed, 1, 1), #typed) == typed
		local want = if same then string.sub(typed, 1, 1) else typed
		local from = focused:peek()
		local n = #stops
		local start = if same or from == 0 then from else from - 1
		for k = 1, n do
			local i = (start + k - 1) % n + 1
			local s = stops[i]
			local name = string.lower(s.entry.label or "")
			if s.enabled and string.sub(name, 1, #want) == want then
				focus(i)
				return
			end
		end
	end
	local function step(by: number)
		local n = #stops
		if n == 0 then
			return
		end
		local i = focused:peek()
		if i == 0 then
			i = if by > 0 then 0 else n + 1
		end
		for _ = 1, n do
			i = (i + by - 1) % n + 1
			if stops[i].enabled then
				focus(i)
				return
			end
		end
	end
	local function edge(first: boolean)
		local n = #stops
		for k = 1, n do
			local i = if first then k else n - k + 1
			if stops[i].enabled then
				focus(i)
				return
			end
		end
	end

	local function keys(e: KeyEvent): boolean
		local k = e.key
		local i = focused:peek()
		if k == "ArrowDown" then
			step(1)
		elseif k == "ArrowUp" then
			step(-1)
		elseif k == "Home" or k == "PageUp" then
			edge(true)
		elseif k == "End" or k == "PageDown" then
			edge(false)
		elseif k == "ArrowRight" then
			local s = stops[i]
			if s and s.open then
				choose(i, true)
			end
		elseif k == "ArrowLeft" then
			if back then
				back()
			end
		elseif k == "Enter" or k == " " or k == "Space" then
			if i ~= 0 then
				choose(i, true)
			end
		elseif k == "Escape" then
			if back then
				back()
			else
				chain.close()
			end
		elseif k == "Tab" then
			chain.close()
		elseif #k == 1 and not e.cmd and not e.ctrl and not e.alt and not e.meta then
			seek(k)
		else
			return false
		end
		return true
	end

	-- The rows, in groups between the rules (each inset from the panel's
	-- edge, so the rules run edge to edge), each group under its heading.
	local groups: { Node } = {}
	local group: { Node } = {}
	local title: string? = nil
	local function flush()
		if #group > 0 or title then
			local rows = table.clone(group)
			if title then
				table.insert(rows, 1, heading(title))
			end
			if #groups > 0 then
				table.insert(groups, rule())
			end
			table.insert(groups, ui.column {
				w_full = true,
				p = "$space.1",
				gap = "$zeron.list.gap",
				role = if title then "group" else nil,
				label = title,
				table.unpack(rows),
			})
		end
		group = {}
		title = nil
	end

	for _, e in entries do
		if e.separator then
			flush()
			continue
		end
		if e.heading then
			flush()
			title = e.heading
			continue
		end
		local reason = if type(e.disabled) == "string" then e.disabled else nil
		local enabled = not e.disabled
		local stop: Stop = { entry = e, el = nil, enabled = enabled, open = nil }
		table.insert(stops, stop)
		local i = #stops
		local ink = if not enabled then "$text.disabled" elseif e.danger then "$danger" else "$text.primary"
		local glyph_ink = if not enabled then "$text.disabled" elseif e.danger then "$danger" else nil
		local lit = function(): boolean
			if blinking:get() == i then
				return blink_on:get()
			end
			return focused:get() == i
		end
		-- What it says: its name, or, armed, what it asks.
		local said = function(): string
			return if armed:get() == i then e.confirm or "" else e.label or ""
		end
		local glyph = if e.icon then Menu.glyph(e.icon, glyph_ink) else nil
		local name: Node = if e.confirm then RollText { said, text_size = "$font.size.md", color = ink, grow = 1, min_w = 0 } else FadeText {
			e.label or "",
			text_size = "$font.size.md",
			color = ink,
			grow = 1,
		}
		if reason then
			-- Out of reach: why, under its name.
			name = ui.column {
				grow = 1,
				min_w = 0,
				gap = "$zeron.space.hair",
				ui.text { e.label or "", text_size = "$font.size.md", line_height = "$font.line.md", color = ink },
				ui.text { reason, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary", wrap = true },
			}
		end
		local trail: { Node } = {}
		-- A check beside keys stands before them, its slot kept on or off:
		-- every row's keys end on one line, and nothing moves as it toggles.
		if e.checked and e.shortcut then
			table.insert(trail, Menu.check(e.checked))
		end
		if e.shortcut then
			-- Its keys as a menu writes them (⇧⌘C), the design system's Kbd.
			table.insert(trail, Kbd {
				keys = e.shortcut,
				look = "plain",
				size = "md",
				tone = if enabled then "quiet" else "disabled",
			})
		end
		if e.checked and not e.shortcut then
			table.insert(trail, Menu.check(e.checked))
		end
		if e.submenu then
			table.insert(trail, Menu.chevron())
		end

		-- Its look: the dropdown's row; lit, the hover wash (armed, the
		-- danger's), whichever lit it (the pointer or the keys).
		local look: { [any]: any } = {
			cursor = if enabled then "pointer" else "default",
			label = e.label,
			description = reason,
			bg = function()
				if armed:get() == i then
					return "$zeron.menu.danger"
				end
				if lit() then
					return if e.danger then "$zeron.menu.danger" else "$hover"
				end
				-- Its submenu open: lit while the pointer is over it.
				return "#00000000"
			end,
			focus = { outline = 0, border = 0 },
			-- (No wash of zgx's own under the pointer or pressed: `bg` says.)
			hover = {},
			active = {},
			on_mount = function(el: Element)
				stop.el = el
			end,
			on_focus = function()
				focused:set(i)
			end,
			on_blur = function()
				if focused:peek() == i then
					focused:set(0)
				end
				if armed:peek() == i then
					armed:set(0)
				end
			end,
			on_hover = function(inside: boolean)
				if inside then
					reached(i, nil)
				else
					left_row(i)
				end
			end,
			on_pointer_move = function(ev: PointerEvent)
				reached(i, { x = ev.x, y = ev.y })
			end,
			on_click = function()
				choose(i, false)
			end,
		}
		-- The menus' row (its height, padding, corner); with a reason
		-- under its name, as tall as that needs.
		Menu.look(look, reason ~= nil)

		local row: Node
		if e.submenu and props.still then
			look.role = "menuitem"
			look.focusable = true
			table.insert(look, glyph or Menu.glyph_space())
			table.insert(look, name)
			for _, t in trail do
				table.insert(look, t)
			end
			row = ui.row(look)
		elseif e.submenu then
			-- A submenu: zgx's own, so this panel stays live beside it (its
			-- rows still take the pointer) and it opens beside its row,
			-- flipping to the other side short of room.
			local open = ui.use_state(false)
			stop.open = open
			local sub_entries = e.submenu
			look.bg = function()
				if lit() or open:get() then
					return "$hover"
				end
				return "#00000000"
			end
			look.active = { bg = "$hover" }
			local trigger_props = table.clone(look)
			table.insert(trigger_props, glyph or Menu.glyph_space())
			table.insert(trigger_props, name)
			for _, t in trail do
				table.insert(trigger_props, t)
			end
			local trigger = ui.menu_item(trigger_props :: any)
			row = ui.submenu(glass({
				open = open,
				label = e.label,
				trigger = trigger,
				w = WIDTH,
				Panel {
					entries = sub_entries,
					chain = chain,
					keyed = function(): boolean
						return false
					end,
					back = function()
						open:set(false)
						focus(i)
					end,
					-- Its box, for the corridor; reached, the wait for a row
					-- on the way there is over.
					placed = function(el: Element)
						stop.panel = el
					end,
					entered = cancel_wait,
				},
			}) :: any)
		else
			local row_props = table.clone(look)
			row_props.role = "menuitem"
			row_props.focusable = enabled
			row_props.disabled = not enabled
			if e.checked then
				row_props.checked = e.checked
			end
			table.insert(row_props, glyph or Menu.glyph_space())
			table.insert(row_props, name)
			for _, t in trail do
				table.insert(row_props, t)
			end
			row = ui.row(row_props)
			if not enabled then
				-- A disabled row hears no pointer: a box round it does, so
				-- the highlight leaves the row before it as the pointer
				-- reaches it.
				row = ui.div {
					w_full = true,
					on_hover = function(inside: boolean)
						if inside then
							reached(i, nil)
						else
							left_row(i)
						end
					end,
					on_pointer_move = function(ev: PointerEvent)
						reached(i, { x = ev.x, y = ev.y })
					end,
					row,
				}
			end
		end
		table.insert(group, row)
	end
	flush()

	return ui.column {
		w_full = true,
		gap = 0,
		focusable = true,
		focus = { outline = 0 },
		on_key = keys,
		on_hover = function(inside: boolean)
			if inside and props.entered then
				props.entered()
			end
		end,
		on_mount = function(el: Element)
			box = el
			if props.placed then
				props.placed(el)
			end
			-- (A submenu's panel is built with its menu, closed: zgx focuses
			-- its first item as it opens.)
			if props.still or back then
				return
			end
			-- From the keys: the first item lit; from the pointer, nothing
			-- until it moves or a key is pressed. (Again a few frames on,
			-- unless something is lit by then: zgx gives the panel itself
			-- the focus as it opens, after this.)
			local function settle()
				if focused:peek() ~= 0 then
					return
				end
				if keyed() then
					edge(true)
				else
					el:focus()
				end
			end
			ui.after(0, settle)
			ui.after(48, settle)
		end,
		table.unpack(groups),
	}
end
panel_component = ui.component(build_panel)

-- Only one thing takes a secondary click: the innermost (a session's row,
-- not the list around it). zgx asks every box with a context menu out to
-- the window in one go when each returns none, so the first to open one
-- says so until the next turn.
local claimed = false

return ui.component(function(props: Props): Node
	if props.still then
		local specimen: ContainerProps = {
			w = props.width or WIDTH,
			shrink = 0,
			Panel {
				entries = props.still,
				chain = { close = function() end },
				keyed = function(): boolean
					return false
				end,
				still = true,
			},
		}
		glass(specimen)
		return ui.column(specimen)
	end
	local open = props.open or ui.use_state(false)
	local at = ui.use_state({ x = 0, y = 0 })
	-- What it shows, built as it opens (plain: entries hold functions,
	-- which a state cannot).
	local entries: { Entry } = {}
	-- Opened from the keys (the Menu key, Shift+F10): its first item lit.
	local keyed = false
	-- The same, followed: the thing it is about is ringed only then.
	local ringed = ui.use_state(false)
	local items = props.items
	-- Whether this opening came through `show` (its items built for it).
	local asked = false

	local function show(e: At, from_keys: boolean): boolean
		if claimed then
			return false
		end
		local list = items(e)
		if not list or #list == 0 then
			return false
		end
		claimed = true
		ui.after(0, function()
			claimed = false
		end)
		ui.batch(function()
			-- Opened again elsewhere: closed first, so it opens anew there.
			open:set(false)
			entries = list
			at:set({ x = e.local_x, y = e.local_y })
		end)
		ui.after(0, function()
			asked = true
			keyed = from_keys
			ringed:set(from_keys)
			open:set(true)
		end)
		return true
	end
	-- Closed: the next opening starts afresh.
	local self_ref = ui.ref()
	if props.hook then
		props.hook(function(x: number, y: number): boolean
			local el = self_ref:peek()
			local ox, oy = if el then el.x or 0 else 0, if el then el.y or 0 else 0
			return show({ x = x, y = y, local_x = x - ox, local_y = y - oy }, false)
		end)
	end
	ui.effect(function()
		if not open:get() then
			keyed = false
			asked = false
			ringed:set(false)
		end
	end)

	local chain: Chain = {
		close = function()
			open:set(false)
		end,
	}

	-- The point it hangs from: the pointer's, or under what has focus.
	local point = ui.div { w = 1, h = 1 }
	local menu = ui.menu(glass({
		open = open,
		anchor = point,
		label = props.label or locale.t("ds.context_menu.label"),
		-- To the right of the point, its top at it; short of room on the
		-- right it flips to the left, and short of room below it moves up
		-- (as AppKit's context menus do).
		placement = { side = "right", align = "start", offset = 0 },
		autofocus = false,
		w = props.width or WIDTH,
		build = function()
			if not asked then
				-- Opened by the caller (its `open` set, not a click): under
				-- the box, from its start, its first item lit, as from the keys.
				local el = self_ref:peek()
				local h = if el then el.height or 0 else 0
				local x, y = if el then el.x or 0 else 0, if el then el.y or 0 else 0
				entries = items({ x = x, y = y + h, local_x = 0, local_y = h }) or {}
				at:set({ x = 0, y = h })
				keyed = true
				ringed:set(true)
			end
			return Panel {
				entries = entries,
				chain = chain,
				keyed = function(): boolean
					return keyed
				end,
			}
		end,
	}) :: any)

	local box: { [any]: any } = {}
	for k, v in props do
		if k ~= "items" and k ~= "open" and k ~= "ring" and k ~= "label" and k ~= "width" and k ~= "children" and k ~= "hook" then
			box[k] = v
		end
	end
	box.relative = true
	box.ref = self_ref
	box.context_menu = function(e: PointerEvent): { Node }?
		-- From the keys (the Menu key, Shift+F10), zgx asks at the foot of
		-- what has focus: on the box's bottom edge, where a pointer is
		-- never inside it. Its first item lit, then.
		local el = self_ref:peek()
		local h = if el then el.height or 0 else 0
		show({ x = e.x, y = e.y, local_x = e.local_x, local_y = e.local_y }, h > 0 and e.local_y >= h - 0.5)
		-- None of zgx's own: the menu is ours.
		return nil
	end
	if props.ring then
		-- The thing it is about, ringed while it shows: from the keys only.
		table.insert(box, ui.show(function(): boolean
			return open:get() and ringed:get()
		end, ui.div {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			rounded = props.ring,
			border = 1,
			border_color = "$focus.ring",
			pointer_events = "none",
		}))
	end
	table.insert(box, ui.div {
		absolute = true,
		left = function(): number
			return at:get().x
		end,
		top = function(): number
			return at:get().y
		end,
		w = 1,
		h = 1,
		pointer_events = "none",
		menu,
	})
	return ui.div(box)
end)
