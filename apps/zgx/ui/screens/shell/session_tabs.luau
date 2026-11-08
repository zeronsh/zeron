-- The tabs on the top bar over the page (`splits` keeps them): one for each two chats
-- or more composed together, and one for a chat alone, which navigating replaces. Each
-- shows its panes' tiles stacked (three at most) with the pane focused last in front,
-- and its title. A chat dragged onto one joins it. A click goes there, a
-- drag reorders them, its × closes the tab, and a split with it (the sessions go on,
-- in the sidebar).
--
--   session_tabs.Strip { left = fn, right = fn }

local ds = require("@ds")
local ContextMenu = require("@ds/context_menu")
local SessionRow = require("@ds/session_row")
local motion = require("@ds/motion")
local look = require("@app/screens/shell/project_look")
local route = require("@app/screens/shell/route")
local sessions = require("@account/sessions")
local splits = require("@app/screens/shell/split")

local M = {}

local TAB_H, SLOT, LABEL_MAX, TILE = 28, 20, 160, 20
local SLIDE = { duration = 150, ease = "out(3)" }
local FADE, FADE_RATE = 36, 12
-- A tab's stacked tiles, as a fanned hand of cards: at most this many, and for each
-- place how far along it stands and how far it's turned. Alone, a tile stands square.
local STACK = 3
local FAN = { { x = 0, turn = -3 }, { x = 6, turn = 3 }, { x = 10, turn = 7 } }

-- Each tab's node, for its width (`Strip` keeps the one showing in view).
local tab_refs: { [string]: any } = {}

local function chat_of(id: string): any
	for _, c in sessions.now:get().chats do
		if c.id == id then
			return c
		end
	end
	return nil
end
local function project_of(c: any): any
	if not c or not c.project then
		return nil
	end
	for _, p in sessions.now:get().projects do
		if p.id == c.project then
			return p
		end
	end
	return nil
end

-- The ones focused, latest first.
local recent = ui.persist("shell_session_tabs_recent", {} :: { string })

-- The tabs with a chat still there, by key. (`splits` keeps them.)
local shown = ui.computed(function(): { string }
	local out: { string } = {}
	for _, key in splits.tabs:get() do
		for _, id in splits.panes(key) do
			if chat_of(id) then
				table.insert(out, key)
				break
			end
		end
	end
	return out
end)

-- A tab's panes, as many as show (`STACK`), the one focused last first: the ones
-- focused in that order, then the rest as they stand in the split.
local function stack_of(key: string): { string }
	local peers = splits.panes(key)
	local out: { string } = {}
	for _, r in recent:get() do
		if #out < STACK and table.find(peers, r) and chat_of(r) then
			table.insert(out, r)
		end
	end
	for _, peer in peers do
		if #out < STACK and not table.find(out, peer) and chat_of(peer) then
			table.insert(out, peer)
		end
	end
	return out
end

-- What a tab's corner says: of its chats, any awaiting a person first, else any
-- at work or waiting on its background work (the sidebar's order of what's
-- most pressing).
local TAB_STATES = { approval = true, input = true, working = true, waiting = true, connecting = true }
local function tab_state(key: string): string?
	local best: string? = nil
	for _, id in splits.panes(key) do
		local c = chat_of(id)
		local kind = if c then c.status else nil
		if kind and TAB_STATES[kind] and (not best or SessionRow.URGENCY[kind] < SessionRow.URGENCY[best]) then
			best = kind
		end
	end
	return best
end

-- What a tab shows: of its chats, the one focused last.
local function face(key: string): string
	return stack_of(key)[1] or splits.panes(key)[1] or key
end

-- A chat a drag carries, for the tabs: not a tab of them being moved.
local function carried(payload: any): string?
	if type(payload) ~= "table" or payload.session_tab then
		return nil
	end
	return splits.dragged(payload)
end

-- A chat focused goes first among the focused. (Its tab: `splits`.)
ui.effect(function()
	local s = route.session:get()
	if not s then
		return
	end
	local was = recent:peek()
	if was[1] ~= s.id then
		local now = { s.id }
		for _, r in was do
			if r ~= s.id and #now < 64 then
				table.insert(now, r)
			end
		end
		recent:set(now)
	end
end)

local function close(key: string)
	local before = shown:peek()
	local at = table.find(before, key)
	local alone = #splits.panes(key) == 1
	local was_showing = splits.current_key() == key
	splits.close_tabs({ key })
	if was_showing then
		-- The one beside it, else a new chat. (A chat alone: always a new chat.)
		local after = shown:peek()
		local next_key = if at and not alone then after[math.min(at, #after)] else nil
		if next_key then
			route.open_session(face(next_key))
		else
			route.new_chat()
		end
	end
end

-- The tab showing closed, or the chat alone (⌘W): true if there was one.
function M.close_showing(): boolean
	local key = splits.current_key()
	if not key then
		return false
	end
	close(key)
	return true
end

-- Those tabs closed at once, `keep` gone to if the one showing was among them.
-- (Each of `close` in turn would open every chat on the way.)
local function close_many(list: { string }, keep: string)
	if #list == 0 then
		return
	end
	local was_showing = table.find(list, splits.current_key() or "") ~= nil
	splits.close_tabs(list)
	if was_showing then
		route.open_session(face(keep))
	end
end

-- The tabs other than this one: all of them, or those to one side.
local function others(id: string, side: string?): { string }
	local list = shown:peek()
	local me = table.find(list, id) or 0
	local out: { string } = {}
	for i, other in list do
		if other ~= id and (side == nil or (side == "left" and i < me) or (side == "right" and i > me)) then
			table.insert(out, other)
		end
	end
	return out
end

local tile_enter = { opacity = { 0, 1 }, scale = { 0.6, 1 }, duration = 220, ease = "outBack(1.6)" }
local tile_exit = { opacity = 0, scale = 0.6, duration = 120, ease = "inQuad" }

-- The tile for a pane's place in the stack: its project's. The focus moving
-- swaps each place's pane, the new one springing up there.
local function pane_tile(id: string, place: number): Node
	return ui.presence(function(): string
		local chat = stack_of(id)[place]
		if not chat then
			return ""
		end
		-- (A chat with no project shows the side chat's glyph: a real swap.)
		return if project_of(chat_of(chat)) then chat else `{chat}\0loose`
	end, function(which: string): Node
		local chat = string.match(which, "^[^%z]*") or ""
		if which ~= chat then
			return ds.Tile { size = TILE, inside = ds.Icon { name = "side-chat", size = 14 } }
		end
		local function now(): any
			return project_of(chat_of(chat))
		end
		return ds.Tile {
			size = TILE,
			label = function(): string?
				local p = now()
				return if p then p.name else "Chat"
			end,
			hue = function(): string?
				local p = now()
				return if p then look.of(p).hue else nil
			end,
			src = function(): string?
				local p = now()
				return if p then look.of(p).src else nil
			end,
			emoji = function(): string?
				local p = now()
				return if p then look.of(p).emoji else nil
			end,
		}
	end, { w = TILE, h = TILE, shrink = 0, enter = tile_enter, exit = tile_exit })
end

-- A tab's tiles: the pane focused last's in front at the far left, turned a little,
-- its shadow on the others fanned out behind it to the right. (A tab of one pane
-- has its tile alone and square: a turned node is a layer of its own.)
local function tile(id: string): Node
	local function count(): number
		return #stack_of(id)
	end
	local function stacked(): boolean
		return count() >= 2
	end
	local function place(at: number): Node
		local fan = FAN[at]
		return ui.div {
			id = `session-tab-{id}-tile-{at}`,
			absolute = true,
			top = 0,
			left = fan.x,
			w = TILE,
			h = TILE,
			rounded = "$radius.sm",
			rotate = motion.tween(ui.computed(function(): number
				return if stacked() then fan.turn else 0
			end), SLIDE),
			-- (Solid under its wash, so what's behind it doesn't show through; the
			-- one in front's shadow falls on those behind.)
			bg = function(): string
				return if stacked() then "$zeron.menu.solid" else "#00000000"
			end,
			shadow = function(): any
				return if stacked() and at == 1 then "$shadow.sm" else false
			end,
			pane_tile(id, at),
		}
	end
	local shown_places: { any } = {
		relative = true,
		shrink = 0,
		h = TILE,
		w = motion.tween(ui.computed(function(): number
			return TILE + FAN[math.clamp(count(), 1, STACK)].x
		end), SLIDE),
	}
	-- (Those behind first: each paints under the one after it.)
	for at = STACK, 2, -1 do
		table.insert(shown_places, ui.show(function(): boolean
			return count() >= at
		end, place(at)))
	end
	table.insert(shown_places, place(1))
	return ui.div(shown_places)
end

local function title(id: string): string
	local c = chat_of(face(id))
	return if c then c.title else ""
end

local Tab = ui.component(function(props: { chat: string }): Node
	local id = props.chat
	local hovered = ui.use_state(false)
	-- A chat held over it, that dropping would bring in.
	local held = ui.use_state(false)
	local own = ui.ref()
	tab_refs[id] = own
	ui.on_cleanup(function()
		if tab_refs[id] == own then
			tab_refs[id] = nil
		end
	end)
	local function active(): boolean
		local s = route.session:get()
		return s ~= nil and splits.current_key() == id
	end
	return ContextMenu {
		label = ui.t("panel.tab.actions"),
		ring = "$radius.lg",
		shrink = 0,
		items = function(): { any }
			return {
				{ label = ui.t("panel.tab.close"):get(), icon = "close-thin", on_select = function()
					close(id)
				end },
				{ label = ui.t("panel.tab.close_others"):get(), on_select = function()
					close_many(others(id), id)
				end },
				{ label = ui.t("panel.tab.close_left"):get(), on_select = function()
					close_many(others(id, "left"), id)
				end },
				{ label = ui.t("panel.tab.close_right"):get(), on_select = function()
					close_many(others(id, "right"), id)
				end },
			}
		end,
		ui.row {
			id = `session-tab-{id}`,
			ref = own,
			role = "tab",
			label = function(): string
				return title(id)
			end,
			selected = active,
			h = TAB_H,
			shrink = 0,
			pl = ui.raw(4, "as far in as the tile is from its top"),
			pr = ui.raw(4, "the tab's inset"),
			gap = "$zeron.space.tight",
			items = "center",
			rounded = "$radius.lg",
			cursor = "pointer",
			bg = function(): string
				if active() then
					return "$zeron.selected.flat.bg"
				end
				return if hovered:get() or held:get() then "$hover" else "#00000000"
			end,
			color = function(): string
				return if active() or hovered:get() then "$text.primary" else "$text.secondary"
			end,
			on_hover = function(inside: boolean)
				hovered:set(inside)
			end,
			on_click = function()
				route.open_session(face(id))
			end,
			on_aux_click = function(e: any)
				if e.button == "middle" then
					close(id)
				end
			end,
			-- (`session`, as a sidebar row's: dropped on a pane, it joins the split.)
			on_drag = function(): any
				return { session_tab = id, session = face(id) }
			end,
			-- The tab as it stands, on the window's colour. (Nothing here eases: a drag's
			-- preview has no clock for it.)
			drag_preview = function(): Node
				local box = own:peek()
				local count = #stack_of(id)
				local places: { any } = { relative = true, shrink = 0, h = TILE, w = TILE + FAN[math.clamp(count, 1, STACK)].x }
				for at = math.min(count, STACK), 1, -1 do
					local fan = FAN[at]
					table.insert(places, ui.div {
						absolute = true,
						top = 0,
						left = fan.x,
						w = TILE,
						h = TILE,
						rounded = "$radius.sm",
						rotate = if count >= 2 then fan.turn else 0,
						bg = if count >= 2 then "$zeron.menu.solid" else "#00000000",
						shadow = if count >= 2 and at == 1 then "$shadow.sm" else false,
						pane_tile(id, at),
					})
				end
				return ui.div {
					rounded = "$radius.lg",
					bg = "$background",
					shadow = "$shadow.sm",
					ui.row {
						w = if box then box.width else nil,
						h = TAB_H,
						pl = ui.raw(4, "as far in as the tile is from its top"),
						pr = ui.raw(4, "the tab's inset"),
						gap = "$zeron.space.tight",
						items = "center",
						rounded = "$radius.lg",
						bg = if active() then "$zeron.selected.flat.bg" else "#00000000",
						color = if active() then "$text.primary" else "$text.secondary",
						ui.div(places),
						ds.FadeText { title(id), max_w = LABEL_MAX, min_w = 0, grow = 1, text_size = "$font.size.sm", weight = "$font.weight.medium" },
					},
				}
			end,
			on_drag_over = function(payload: any, e: any)
				local other = if type(payload) == "table" then payload.session_tab else nil
				if not other then
					local chat = carried(payload)
					if not chat then
						return
					end
					-- Dropped on it, the chat joins it.
					local can = splits.can_join(chat, id)
					if held:peek() ~= can then
						held:set(can)
					end
					return
				end
				if other == id then
					return
				end
				local list = shown:peek()
				local from, here = table.find(list, other), table.find(list, id)
				if not from or not here then
					return
				end
				local past_middle = e.local_x > e.w / 2
				if (from < here and past_middle) or (from > here and not past_middle) then
					splits.move_tab(other, id)
				end
			end,
			on_drag_leave = function()
				held:set(false)
			end,
			-- (A tab dropped on a tab has moved already. A chat, a sidebar's row or
			-- a pane's head, joins it.)
			on_drop = function(payload: any)
				held:set(false)
				local chat = carried(payload)
				if not chat then
					return
				end
				splits.join_tab(chat, id)
			end,
			-- A sidebar's chat resting on its middle shows it, so it can go among its
			-- panes. (Not a pane's: its source would go, ending the drag.)
			on_drag_spring = function(payload: any): boolean
				local chat = carried(payload)
				if not chat or payload.split_pane or not held:peek() or active() then
					return false
				end
				route.open_session(face(id))
				return true
			end,
			tile(id),
			ds.FadeText {
				function(): string
					return title(id)
				end,
				max_w = LABEL_MAX,
				min_w = 0,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
			},
			ui.row {
				w = SLOT,
				h = SLOT,
				shrink = 0,
				items = "center",
				justify = "center",
				-- Its × under the pointer; else whether a chat of it awaits a person or
				-- is at work.
				ui.presence(function(): string
					return if hovered:get() then "close" else tab_state(id) or ""
				end, function(which: string): Node
					if which == "" then
						return ui.div {}
					end
					if which ~= "close" then
						return ui.div { id = `session-tab-{id}-state`, SessionRow.state(which :: any) }
					end
					return ds.IconButton {
						label = "Close tab",
						tooltip = false,
						size = "sm",
						tone = "quiet",
						w = SLOT,
						h = SLOT,
						rounded = "$radius.sm",
						on_click = function()
							close(id)
						end,
						ds.Icon { name = "close", size = 12 },
					}
				end, { shrink = 0 }),
			},
		},
	}
end)

-- `window_x`: how far the page's room stands from the window's left, followed
-- as the sidebar moves. `start`: where the tabs start in the window.
function M.Strip(props: { window_x: () -> number, start: () -> number, right: () -> number }): Node
	-- Each edge fades in full as soon as tabs run a little past it: a band only
	-- as wide as the overflow (`fade_overflow`) is a hard edge while that's a
	-- few points. Over the last few, as the end comes to rest, it goes.
	local offset = ui.state("shell_session_tabs_offset", 0)
	local view, content = ui.ref(), ui.ref()
	local function bands(): { number }
		local at = offset:get()
		local past = (content.width or 0) - (view.width or 0) - at
		return { math.clamp(at * FADE_RATE, 0, FADE), math.clamp(past * FADE_RATE, 0, FADE) }
	end
	-- The tab showing, always clear of the overflow and its fade: as it's opened,
	-- as tabs come and go, and as the strip's room changes (the panel opening).
	-- (A person's own scrolling is left alone: this doesn't follow the offset.)
	ui.effect(function()
		local s = route.session:get()
		local list = shown:get()
		local total = content.width or 0
		local room = view.width or 0
		if not s or room <= 0 then
			return
		end
		for _, id in list do
			local r = tab_refs[id]
			if not r or (r.width or 0) <= 0 then
				return
			end
		end
		local left = 0
		-- (The tabs' own gap, the strip's hair.)
		local gap = (ui.theme.get("zeron.space.hair") :: number?) or 2
		for _, id in list do
			local w = tab_refs[id].width
			if splits.current_key() == id then
				local right = left + w
				local at = offset:peek()
				local want = at
				if w + 2 * FADE >= room then
					want = left - (room - w) / 2
				elseif left - FADE < at then
					want = left - FADE
				elseif right + FADE > at + room then
					want = right + FADE - room
				end
				want = math.clamp(want, 0, math.max(0, total - room))
				if math.abs(want - at) > 0.5 then
					offset:set(want)
				end
				return
			end
			left += w + gap
		end
	end)
	-- Where they start, eased as the sidebar's "…" before them comes and goes.
	local start = motion.tween(ui.computed(props.start), motion.sidebar)
	-- From the window's left whatever the sidebar is doing, so they stay put as
	-- it moves. (Only the tabs' own part takes the pointer: the sidebar's "…"
	-- lies under the rest, past the margin.)
	return ui.row {
		absolute = true,
		left = function(): number
			return -props.window_x()
		end,
		right = 0,
		-- (On the bar's line, wherever the platform has it.)
		top = function(): number
			return ui.raw(ds.TitleBar.middle() - 17, "on the top bar's line")
		end,
		h = 34,
		pointer_events = "none",
		ui.row {
			id = "session-tabs",
			role = "tablist",
			label = "Open chats",
			relative = true,
			grow = 1,
			min_w = 0,
			h_full = true,
			ml = start,
			items = "center",
			window_drag = true,
			pointer_events = "auto",
			pr = function(): any
				return ui.raw(props.right(), "clear of the title bar's controls at the right")
			end,
			ui.scroll_x {
				id = "session-tabs-scroll",
				ref = view,
				offset = offset,
				grow = 1,
				min_w = 0,
				h = TAB_H,
				fade_x = bands,
				ui.row {
					ref = content,
					gap = "$zeron.space.hair",
					items = "center",
					ui.each(shown, function(id: string): Node
						return Tab { chat = id }
					end, {
						key = function(id: string): string
							return id
						end,
						direction = "row",
						gap = "$zeron.space.hair",
						animate = motion.layout(SLIDE),
					}),
				},
			},
		},
	}
end

return M
