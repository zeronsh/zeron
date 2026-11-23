--!strict
-- How the board moves (step 3 of the board's design, and its review's
-- decisions 38-44): where each card is *presented*, lagging the rows by a
-- hold, and what plays as it goes. One per board (ds/orch_board_ctx makes
-- it with the board's slot); the columns, the drawers and the list view all
-- read the presented places through `ctx.ids`, so they move together.
--
-- A move has three beats:
--   1. At once: the card's status line changes where it stands (its glyph
--      and words, its clock, its dot) and the tabs' counts change (they read
--      the rows, `ctx.counts`). Its summary and its size stay as they were
--      (`phase(id)` is "hold": ds/orch_board_card keeps the summary it had).
--   2. Hold, HOLD ms: it stays in its old place, dimming to DIM over DIM_MS.
--   3. Travel, TRAVEL ms (outQuint): it is presented where it stands. The
--      old list closes its gap and the new one opens a slot (their `each`
--      slides its rows); the card's node in the new list, built with its new
--      summary and size, starts where the old one showed and is translated
--      home (never scaled), at full opacity within FADE ms and on a solid
--      fill until it lands. The column heads count it as it goes.
-- Cards that leave together leave STAGGER ms apart, the one that lands
-- lowest first.
--
-- What is a move: the same card, its row's `seq` higher than the last one
-- drawn, `from` other than `place`. Anything else that changes a card's
-- list (the board mounting, a filter, a row with `seq` 0, a day going by)
-- presents it at once, unanimated.
--
-- Limits: at most CAP cards between their hold and their landing; a move
-- that would be one more, and every move with Reduce Motion
-- (ds/motion.reduced), crossfades: out FADE ms where it was, in FADE ms
-- where it goes. A card whose row moves again in its hold holds again once,
-- then travels to wherever it stands; one moved again in its travel goes on
-- from where it shows.
--
-- The board to its list and back (`begin_switch`): the leaving view fades
-- out in place (SWITCH_OUT); each card or row of the arriving view whose
-- counterpart showed (at most SWITCH_CAP) starts at the counterpart's top
-- left, fades in over SWITCH_IN and slides home over SWITCH, translated
-- only; the rest fade in where they are.
--
-- Transforms and opacity only while cards move: the dim and the comings in
-- are `anime` animations of an element's opacity and translation; the gap
-- and the slot are `each`'s, on the compositor. The beats' ends are timers
-- (`ui.after`), which sleep between: once every card has landed nothing
-- asks for a frame.
--
-- What the tests read (tests/orch_board_motion.test.luau): the named state
-- `orch_board_motion` (the last board that changed): `flying` (in their
-- hold or travel), `timers`, `phase` ({ [id] = phase }), `hold_ms`
-- ({ [id] = its hold, stagger included }), `switched` (the nodes the last
-- switch slid from their counterparts).

local motion = require("@self/motion")

-- (anime's overloads, for elements and their params alike.)
local play: (any, any) -> any = anime.animate :: any
local set: (any, any) -> () = anime.utils.set :: any

local M = {}

M.HOLD = 800
M.DIM = 0.6
M.DIM_MS = 200
M.TRAVEL = 350
M.EASE = "outQuint"
M.FADE = 120
M.CAP = 3
M.STAGGER = 60
-- A travelling card is opaque within FADE ms of its first frame: its fade
-- begins the frame after it appears, so it takes a frame less.
M.TRAVEL_IN = 100
-- The board to its list and back.
M.SWITCH = 280
M.SWITCH_IN = 100
M.SWITCH_OUT = 100
M.SWITCH_CAP = 24
-- A card's detail: in with a slide from the right, out quicker.
M.DETAIL_IN = 180
M.DETAIL_OUT = 130
M.DETAIL_SLIDE = 12
-- A drawer opening or closing.
M.DRAWER = 200
-- Each list's column.
M.COLUMN_OF = { snoozed = "needs_you", planned = "running", earlier = "done", dropped = "done" } :: { [string]: string }
-- Every list, in the board's order.
M.ORDER = { "needs_you", "snoozed", "running", "planned", "done", "earlier" }
-- The places that are a column's drawer: their cards never slide across a
-- switch.
M.IN_DRAWER = { snoozed = true, planned = true, earlier = true, dropped = true } :: { [string]: boolean }

M.debug = ui.state("orch_board_motion", { flying = 0, timers = 0, phase = {}, hold_ms = {}, switched = {} } :: any)

-- (Phases: "hold", "travel", "out" (to a folded drawer's head), "fade_out",
-- "fade_in". The first three count against CAP.)
local FLYING: { [string]: boolean } = { hold = true, travel = true, out = true }

type Flight = {
	phase: string,
	-- The list it shows in while it holds (or fades out), and its index there.
	from: string,
	index: number,
	restarted: boolean,
	-- Its hold, the stagger included.
	hold_ms: number,
	-- Bumped with each phase: a timer of an older one does nothing.
	token: number,
}

export type Built = { how: string? }

export type Motion = {
	-- The presented list of a card held away from its row's, or nil.
	where: { [string]: string },
	flights: { [string]: Flight },
	-- Seeds what was drawn first: no animation for any of it.
	seed: (lists: { [string]: { string } }, by_id: { [string]: any }) -> (),
	-- A push: `lists` the rows' (by place), `shown` the presented lists as
	-- they stand, `by_id` the rows' entries.
	update: (lists: { [string]: { string } }, shown: { [string]: { string } }, by_id: { [string]: any }) -> (),
	-- A card's node (its card, or its row) built: how it comes in
	-- ("travel", "fade", "drawer", "switch" or nil). `drawer`: it stands in
	-- a drawer.
	build: (id: string, drawer: boolean) -> Built,
	-- The opacity a node built so starts at (nil: its own).
	start: (b: Built) -> number?,
	-- Its node mounted (built with `start`'s opacity, so its first frame
	-- shows nothing of where it doesn't belong): `el` what moves and dims.
	attach: (id: string, el: any) -> (),
	-- Its node's first layout (its `on_resize`): an arrival that starts
	-- from where something else stood starts now.
	placed: (id: string, el: any) -> (),
	detach: (id: string, el: any) -> (),
	-- A card's phase, "" at rest (tracked: its reader wakes when its own
	-- card's changes).
	phase: (id: string) -> string,
	-- The board says which drawers are folded, and gives their heads.
	folded: (place: string) -> boolean,
	set_folded: (f: (place: string) -> boolean) -> (),
	head: (place: string, el: any?) -> any?,
	-- Whether a card travels into the column `place` now (it paints over
	-- the others meanwhile, so the card goes over them). Tracked.
	raised: (place: string) -> boolean,
	-- Whether a card that left the column `place` is still on its way
	-- (its empty line waits). Tracked.
	leaving: (place: string) -> boolean,
	-- The board to its list, or back: what shows now is what the arriving
	-- nodes start from.
	begin_switch: () -> (),
	-- The timers' host: a node the board (or the list) mounts once.
	host: () -> Node,
	flying: () -> number,
}

-- A number from what anime or an element answers (a number, or a string
-- with one in it).
local function num(v: any): number
	if type(v) == "number" then
		return v
	end
	return tonumber(string.match(tostring(v or ""), "-?[%d%.]+") or "") or 0
end

-- Where an element shows: its laid-out box's top left and its translation.
local function shown_at(el: any): (number?, number?)
	local ok, x, y = pcall(function(): (number, number)
		return (el :: any).x, (el :: any).y
	end)
	if not ok or type(x) ~= "number" or type(y) ~= "number" then
		return nil, nil
	end
	local okt, tx, ty = pcall(function(): (any, any)
		return anime.utils.get(el, "x"), anime.utils.get(el, "y")
	end)
	if okt then
		x += num(tx)
		y += num(ty)
	end
	return x, y
end

function M.make(slot: number, filter: () -> string?, republish: () -> ()): Motion
	local m = {} :: Motion
	m.where = {}
	m.flights = {}
	-- The last `seq` drawn of each card.
	local seq: { [string]: number } = {}
	-- Each card's mounted element (its card's, or its row's).
	local elements: { [string]: any } = {}
	local arrivals: { [string]: string } = {}
	-- Where a card's last node showed as the next took its place (a
	-- travel's start, a switch's).
	local from_at: { [string]: { x: number, y: number } } = {}
	local heads: { [string]: any } = {}
	local folded_of: (place: string) -> boolean = function(_place: string): boolean
		return false
	end
	-- The timers: one per card in flight, each the row of an `each` that owns
	-- its `ui.after` (a timer an effect made would end with its next run).
	local timers = ui.use_state({} :: { any })
	-- The columns a card travels into, and those a card left still on its way.
	local landing = ui.use_state({} :: { [string]: boolean })
	local left = ui.use_state({} :: { [string]: boolean })
	-- Each card's phase, a small state its card reads.
	local phases: { [string]: any } = {}
	local function phase_state(id: string): any
		local st = phases[id]
		if not st then
			st = ui.state(`orch_board_phase:{slot}:{id}`, "" :: string) :: any
			phases[id] = st
		end
		return st
	end
	local switch_left = 0
	local switch_on = false
	local switch_token = 0
	local switched: { string } = {}

	local function flying(): number
		local n = 0
		for _, f in m.flights do
			if FLYING[f.phase] then
				n += 1
			end
		end
		return n
	end
	m.flying = flying

	local function publish_debug()
		local phase: { [string]: string } = {}
		local hold_ms: { [string]: number } = {}
		for id, f in m.flights do
			phase[id] = f.phase
			hold_ms[id] = f.hold_ms
		end
		M.debug:set({ flying = flying(), timers = #timers:peek(), phase = phase, hold_ms = hold_ms, switched = table.clone(switched) })
	end

	-- The marks that follow the flights: each card's phase, the columns
	-- landed in and left, the timers.
	local function schedule()
		local out: { any } = {}
		local into: { [string]: boolean } = {}
		local from: { [string]: boolean } = {}
		local row_list = (m :: any).row_list or {}
		for id, f in m.flights do
			local ms = if f.phase == "hold"
				then f.hold_ms
				elseif f.phase == "fade_out" or f.phase == "fade_in" then M.FADE
				else M.TRAVEL
			table.insert(out, { key = `{id}:{f.token}`, id = id, token = f.token, ms = ms })
			if f.phase == "travel" or f.phase == "fade_in" or f.phase == "out" then
				local to = row_list[id]
				if to then
					into[M.COLUMN_OF[to] or to] = true
				end
				from[M.COLUMN_OF[f.from] or f.from] = true
			end
		end
		if switch_on then
			table.insert(out, { key = `switch:{switch_token}`, token = switch_token, ms = M.SWITCH })
		end
		table.sort(out, function(a: any, b: any): boolean
			return a.key < b.key
		end)
		local function differ(a: { [string]: boolean }, b: { [string]: boolean }): boolean
			for k in a do
				if not b[k] then
					return true
				end
			end
			for k in b do
				if not a[k] then
					return true
				end
			end
			return false
		end
		ui.batch(function()
			timers:set(out)
			if differ(into, landing:peek()) then
				landing:set(into)
			end
			if differ(from, left:peek()) then
				left:set(from)
			end
			for id, st in phases do
				local f = m.flights[id]
				local p = if f then f.phase else ""
				if st:peek() ~= p then
					st:set(p)
				end
			end
		end)
		publish_debug()
	end

	local function animate(id: string, params: { [string]: any })
		local el = elements[id]
		if el then
			play(el, params)
		end
	end

	local function phase(f: Flight, name: string)
		f.phase = name
		f.token += 1
	end

	local function done(id: string)
		m.flights[id] = nil
		arrivals[id] = nil
		from_at[id] = nil
	end

	-- A timer's end: the next beat.
	local function advance(id: string?, token: number)
		if id == nil then
			-- (The switch's while over: what is built later comes in
			-- plainly, and where nodes stood is let go.)
			if token == switch_token then
				switch_on = false
				for k in from_at do
					if not m.flights[k] then
						from_at[k] = nil
					end
				end
				schedule()
			end
			return
		end
		local f = m.flights[id]
		if not f or f.token ~= token then
			return
		end
		local p = f.phase
		if p == "hold" then
			local to = m.where[id] and (m :: any).row_list[id]
			if not to or to == f.from then
				-- (Back where it was before it left: it stays, lit again.)
				m.where[id] = nil
				done(id)
				animate(id, { opacity = { to = 1 }, duration = M.DIM_MS, ease = M.EASE })
			elseif folded_of(to) and not folded_of(f.from) and elements[id] then
				-- Into a folded drawer: to its head line, fading at the end.
				phase(f, "out")
				local el, head = elements[id], heads[to]
				local dy = 0
				if head then
					local ok, d = pcall(function(): number
						return (head :: any).y - (el :: any).y
					end)
					dy = if ok then d else 0
				end
				animate(id, {
					y = { to = dy, duration = M.TRAVEL, ease = M.EASE },
					opacity = { to = 0, duration = M.FADE, delay = M.TRAVEL - M.FADE, ease = "linear" },
				})
			else
				-- (Where its old node shows, for its new one to start from.)
				local el = elements[id]
				local x, y = nil, nil
				if el and not folded_of(f.from) then
					x, y = shown_at(el)
				end
				from_at[id] = if x and y then { x = x, y = y } else nil
				m.where[id] = nil
				phase(f, "travel")
				arrivals[id] = if folded_of(f.from) then "drawer" else "travel"
				schedule()
				republish()
				return
			end
		elseif p == "out" then
			m.where[id] = nil
			done(id)
			schedule()
			republish()
			return
		elseif p == "fade_out" then
			m.where[id] = nil
			phase(f, "fade_in")
			arrivals[id] = "fade"
			schedule()
			republish()
			return
		else
			done(id)
		end
		schedule()
	end

	function m.host(): Node
		return ui.div {
			class = "orch-board-timers",
			absolute = true,
			w = 0,
			h = 0,
			ui.each(timers, function(t: any): Node
				ui.after(t.ms, function()
					advance(t.id, t.token)
				end)
				return ui.div { w = 0, h = 0 }
			end, {
				key = function(t: any): string
					return t.key
				end,
			}),
		}
	end

	function m.seed(lists: { [string]: { string } }, by_id: { [string]: any })
		for id, e in by_id do
			seq[id] = (e.card and e.card.seq) or 0
		end
		(m :: any).row_list = {}
		for place, ids in lists do
			for _, id in ids do
				(m :: any).row_list[id] = place
			end
		end
	end

	function m.update(lists: { [string]: { string } }, shown: { [string]: { string } }, by_id: { [string]: any })
		local row_list: { [string]: string } = {}
		local row_index: { [string]: number } = {}
		for place, ids in lists do
			for i, id in ids do
				row_list[id] = place
				row_index[id] = i
			end
		end
		(m :: any).row_list = row_list
		local shown_in: { [string]: string } = {}
		local index: { [string]: number } = {}
		for place, ids in shown do
			for i, id in ids do
				shown_in[id] = place
				index[id] = i
			end
		end
		local reduced = motion.reduced_now()
		local changed = false
		-- (In the board's order, so which of many moves are the first CAP is
		-- the same each time.)
		local order: { string } = {}
		for _, place in M.ORDER do
			for _, id in lists[place] or {} do
				table.insert(order, id)
			end
		end
		local holding: { string } = {}
		for _, id in order do
			local to = row_list[id]
			local e = by_id[id]
			local s = (e and e.card and e.card.seq) or 0
			local last = seq[id]
			seq[id] = math.max(s, last or 0)
			local at = shown_in[id]
			local f = m.flights[id]
			local moved = last ~= nil and s > last
			if at == nil then
				-- New here (made, or shown by a filter): there at once.
				if f then
					done(id)
					changed = true
				end
				m.where[id] = nil
			elseif f then
				if f.phase == "hold" then
					-- Moved again while it holds: holds again, once.
					if moved and not f.restarted then
						f.restarted = true
						f.hold_ms = M.HOLD
						f.token += 1
						changed = true
					end
				elseif f.phase == "travel" or f.phase == "fade_in" then
					if to ~= at then
						-- Moved again on its way: on from where it shows.
						local el = elements[id]
						local x, y = nil, nil
						if el then
							x, y = shown_at(el)
						end
						from_at[id] = if x and y then { x = x, y = y } else nil
						arrivals[id] = if f.phase == "travel" then "travel" else "fade"
						m.where[id] = nil
						f.token += 1
						changed = true
					end
				end
				-- ("out" and "fade_out" end where the row stands then.)
			elseif to ~= at then
				local card = e and e.card
				local move = moved and s > 0 and card ~= nil and card.from ~= nil and card.from ~= card.place
				if not move then
					m.where[id] = nil
				elseif reduced or flying() >= M.CAP then
					m.where[id] = at
					m.flights[id] = { phase = "fade_out", from = at, index = index[id], restarted = false, hold_ms = 0, token = 1 }
					animate(id, { opacity = { to = 0 }, duration = M.FADE, ease = "linear" })
					changed = true
				else
					m.where[id] = at
					m.flights[id] = { phase = "hold", from = at, index = index[id], restarted = false, hold_ms = M.HOLD, token = 1 }
					animate(id, { opacity = { to = M.DIM }, duration = M.DIM_MS, ease = "out(3)" })
					table.insert(holding, id)
					changed = true
				end
			end
		end
		-- Leaving together, they leave STAGGER ms apart: the one that lands
		-- lowest first (the furthest down its new list, then the rightmost
		-- column).
		if #holding > 1 then
			local column: { [string]: number } = { needs_you = 1, running = 2, done = 3 }
			table.sort(holding, function(a: string, b: string): boolean
				local ia, ib = row_index[a] or 0, row_index[b] or 0
				if ia ~= ib then
					return ia > ib
				end
				local ca = column[M.COLUMN_OF[row_list[a]] or row_list[a]] or 0
				local cb = column[M.COLUMN_OF[row_list[b]] or row_list[b]] or 0
				if ca ~= cb then
					return ca > cb
				end
				return a < b
			end)
			for i, id in holding do
				m.flights[id].hold_ms = M.HOLD + (i - 1) * M.STAGGER
			end
		end
		-- Gone from the board: let go.
		for id in seq do
			if row_list[id] == nil then
				seq[id] = nil
				m.where[id] = nil
				if m.flights[id] then
					done(id)
					changed = true
				end
			end
		end
		if changed then
			schedule()
		end
	end

	function m.build(id: string, drawer: boolean): Built
		local how = arrivals[id]
		if switch_on and how == nil then
			if not drawer and switch_left > 0 and from_at[id] then
				switch_left -= 1
				table.insert(switched, id)
				how = "switch"
				publish_debug()
			else
				how = "switch_fade"
			end
		end
		arrivals[id] = how
		return { how = how }
	end

	function m.start(b: Built): number?
		return if b.how ~= nil then 0 else nil
	end

	function m.phase(id: string): string
		return phase_state(id):get()
	end

	-- A node arriving: from `from_at[id]` to its own place, translated, its
	-- opacity from `from_opacity` to 1 over `fade_ms`. (At its first layout:
	-- where it is is known then, before the frame is drawn.)
	local function slide_in(id: string, el: any, from_opacity: number, fade_ms: number, ms: number)
		local from = from_at[id]
		local dx, dy = 0, 0
		if from then
			local ok, x, y = pcall(function(): (number, number)
				return (el :: any).x, (el :: any).y
			end)
			if ok and type(x) == "number" and type(y) == "number" then
				dx, dy = from.x - x, from.y - y
			end
		end
		set(el, { x = dx, y = dy, opacity = from_opacity })
		play(el, {
			x = { to = 0, duration = ms, ease = M.EASE },
			y = { to = 0, duration = ms, ease = M.EASE },
			opacity = { to = 1, duration = fade_ms, ease = "linear" },
		})
	end

	-- Arrivals waiting for their node's first layout.
	local pending: { [string]: { how: string, el: any, head: any? } } = {}

	function m.attach(id: string, el: any)
		elements[id] = el
		local how = arrivals[id]
		if how == "travel" or how == "switch" then
			pending[id] = { how = how, el = el }
		elseif how == "drawer" then
			local f = m.flights[id]
			pending[id] = { how = how, el = el, head = f and heads[f.from] }
		elseif how == "switch_fade" then
			play(el, { opacity = { from = 0, to = 1 }, duration = if motion.reduced_now() then M.FADE else M.SWITCH_IN, ease = "linear" })
		elseif how == "fade" then
			play(el, { opacity = { from = 0, to = 1 }, duration = M.FADE, ease = "linear" })
		end
		-- (Once: a node built again later comes in plainly.)
		arrivals[id] = nil
	end

	function m.placed(id: string, el: any)
		-- (The element `on_resize` hands is another handle on the node
		-- `on_mount` gave: either moves it.)
		local p = pending[id]
		if not p then
			return
		end
		pending[id] = nil
		if p.how == "travel" then
			slide_in(id, el, M.DIM, M.TRAVEL_IN, M.TRAVEL)
		elseif p.how == "switch" then
			slide_in(id, el, 0, M.SWITCH_IN, M.SWITCH)
		else
			-- From its drawer's head line, in over the first FADE ms.
			local dy = 0
			local head = p.head
			if head then
				local ok, d = pcall(function(): number
					return (head :: any).y - (el :: any).y
				end)
				dy = if ok then d else 0
			end
			set(el, { y = dy, opacity = 0 })
			play(el, {
				y = { to = 0, duration = M.TRAVEL, ease = M.EASE },
				opacity = { to = 1, duration = M.FADE, ease = "linear" },
			})
		end
	end

	function m.detach(id: string, el: any)
		if elements[id] == el then
			elements[id] = nil
		end
		local p = pending[id]
		if p and p.el == el then
			pending[id] = nil
		end
	end

	function m.folded(place: string): boolean
		return folded_of(place)
	end
	function m.set_folded(f: (place: string) -> boolean)
		folded_of = f
	end
	function m.head(place: string, el: any?): any?
		if el ~= nil then
			heads[place] = el
		end
		return heads[place]
	end
	function m.raised(place: string): boolean
		return landing:get()[place] == true
	end
	function m.leaving(place: string): boolean
		return left:get()[place] == true
	end

	function m.begin_switch()
		switch_left = if motion.reduced_now() then 0 else M.SWITCH_CAP
		switch_on = true
		switch_token += 1
		switched = {}
		-- (Where each card or row shows as the switch begins: in view
		-- alone, those the window shows.)
		local win_h = 1e9
		local ok, size = pcall(function(): any
			return ((ui :: any).window.size :: any):peek()
		end)
		if ok and type(size) == "table" and type(size.height) == "number" and size.height > 0 then
			win_h = size.height
		end
		for id, el in elements do
			if not m.flights[id] then
				local x, y = shown_at(el)
				if x and y and y < win_h then
					from_at[id] = { x = x, y = y }
				else
					from_at[id] = nil
				end
			end
		end
		schedule()
	end

	return m
end

return M
