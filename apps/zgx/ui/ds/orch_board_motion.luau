--!strict
-- How the board moves (step 3 of the board's design): where each card is
-- *presented*, lagging the rows by a hold, and what plays as it goes. One
-- per board (ds/orch_board_ctx makes it with the board's slot); the
-- columns, the drawers and the list view all read the presented places
-- through `ctx.ids`, so they move together.
--
-- A move has three beats:
--   1. At once: the card changes where it stands (its glyph and words
--      crossfade, its clock restarts) and the tabs' counts change (they
--      read the rows, `ctx.counts`).
--   2. Hold, HOLD ms: it stays in its old place, dimming to DIM over DIM_MS.
--   3. Travel, TRAVEL ms (outQuint): it is presented where it stands; the
--      old list closes its gap and the new one opens a slot (their `each`
--      slides its rows), and the card goes there in a straight line (its
--      `layout_id`: the node mounting in the new list starts at the box of
--      the one leaving the old), its opacity coming back. The column heads'
--      counts change as it lands (they count the presented lists).
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
-- Transforms and opacity only while cards move: the dim and the comings in
-- are `anime` animations of an element's opacity and translation, and the
-- travel and the gap are layout animations zgx runs on the compositor. The
-- hold's and travel's ends are timers (`ui.after`), which sleep between:
-- once every card has landed nothing asks for a frame.
--
-- What the tests read (tests/orch_board_motion.test.luau): the named state
-- `orch_board_motion` (the last board that changed): `flying` (in their
-- hold or travel), `timers`, `phase` ({ [id] = phase }), `morphed` (the
-- rows the last switch to the list paired with a card).

local motion = require("@self/motion")

-- (anime's overloads, for elements, refs and their params alike.)
local play: (any, any) -> any = anime.animate :: any

local M = {}

M.HOLD = 800
M.DIM = 0.6
M.DIM_MS = 200
M.TRAVEL = 350
M.EASE = "outQuint"
M.FADE = 120
M.CAP = 3
-- The board to its list and back: each card in view becomes its row.
M.MORPH = 280
M.MORPH_CAP = 24
M.MORPH_OUT = 120
M.MORPH_IN = 160
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
-- The places that are a column's drawer: their cards never morph.
M.IN_DRAWER = { snoozed = true, planned = true, earlier = true, dropped = true } :: { [string]: boolean }

M.debug = ui.state("orch_board_motion", { flying = 0, timers = 0, phase = {}, morphed = {} } :: any)

-- (Phases: "hold", "travel", "out" (to a folded drawer's head), "fade_out",
-- "fade_in". The first three count against CAP.)
local FLYING: { [string]: boolean } = { hold = true, travel = true, out = true }

type Flight = {
	phase: string,
	-- The list it shows in while it holds (or fades out), and its index there.
	from: string,
	index: number,
	restarted: boolean,
	-- Bumped with each phase: a timer of an older one does nothing.
	token: number,
}

export type Built = { layout_id: string, duration: number, how: string? }

export type Motion = {
	-- The presented list of a card held away from its row's, or nil.
	where: { [string]: string },
	flights: { [string]: Flight },
	-- Seeds what was drawn first: no animation for any of it.
	seed: (lists: { [string]: { string } }, by_id: { [string]: any }) -> (),
	-- A push: `lists` the rows' (by place), `shown` the presented lists as
	-- they stand, `by_id` the rows' entries.
	update: (lists: { [string]: { string } }, shown: { [string]: { string } }, by_id: { [string]: any }) -> (),
	-- A card's node (its card, or its row) built: its `layout_id` (the
	-- card's and its row's the same, so one hands its box to the other),
	-- how long a move handed to it takes, and how it comes in ("travel",
	-- "fade", "drawer", "morph" or nil). `drawer`: it stands in a drawer.
	build: (id: string, drawer: boolean) -> Built,
	-- Its node mounted (built with `start`'s opacities, so its first frame
	-- shows none of it): `el` what moves and dims, `parts.content` what it
	-- holds (a morph fades it alone), `parts.ghost` a row's card-coloured
	-- backing (a morph fades it out as the row takes the card's place).
	attach: (id: string, el: any, parts: { content: any?, ghost: any? }?) -> (),
	detach: (id: string, el: any) -> (),
	mounted: (id: string) -> boolean,
	-- The board says which drawers are folded, and gives their heads.
	folded: (place: string) -> boolean,
	set_folded: (f: (place: string) -> boolean) -> (),
	head: (place: string, el: any?) -> any?,
	-- The board to its list, or back: for MORPH ms, the first MORPH_CAP
	-- cards' nodes built whose other node showed pair with it.
	begin_morph: () -> (),
	-- The opacities a node built as `b` says starts at: its own, its
	-- content's, its ghost's.
	start: (b: Built) -> (number?, number?, number?),
	-- The timers' host: a node the board (or the list) mounts once.
	host: () -> Node,
	-- What is in flight, for the tests.
	flying: () -> number,
	-- Whether a card travels into `place` now (its column paints over the
	-- others meanwhile, so the card goes over them). Tracked.
	raised: (place: string) -> boolean,
}

function M.make(slot: number, filter: () -> string?, republish: () -> ()): Motion
	local m = {} :: Motion
	m.where = {}
	m.flights = {}
	-- The last `seq` drawn of each card.
	local seq: { [string]: number } = {}
	-- Bumped when a card is placed again without travelling (a crossfade, at
	-- once): its next node starts no layout pair with the last.
	local pair: { [string]: number } = {}
	-- Each card's mounted element (its card's, or its row's), and what
	-- animates it.
	local elements: { [string]: any } = {}
	local arrivals: { [string]: string } = {}
	local heads: { [string]: any } = {}
	local folded_of: (place: string) -> boolean = function(_place: string): boolean
		return false
	end
	-- The timers: one per card in flight, each the row of an `each` that owns
	-- its `ui.after` (a timer an effect made would end with its next run).
	local timers = ui.use_state({} :: { any })
	-- The places a card travels into now.
	local landing = ui.use_state({} :: { [string]: boolean })
	local morph_left = 0
	local morph_on = false
	local morph_token = 0
	-- What showed as the switch began (each card's node), and what paired.
	local morph_from: { [string]: boolean } = {}
	local morphed: { string } = {}

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
		for id, f in m.flights do
			phase[id] = f.phase
		end
		M.debug:set({ flying = flying(), timers = #timers:peek(), phase = phase, morphed = table.clone(morphed) })
	end

	local function schedule()
		local out: { any } = {}
		for id, f in m.flights do
			local ms = if f.phase == "hold"
				then M.HOLD
				elseif f.phase == "fade_out" or f.phase == "fade_in" then M.FADE
				else M.TRAVEL
			table.insert(out, { key = `{id}:{f.token}`, id = id, token = f.token, ms = ms })
		end
		if morph_on then
			table.insert(out, { key = `morph:{morph_token}`, token = morph_token, ms = M.MORPH })
		end
		table.sort(out, function(a: any, b: any): boolean
			return a.key < b.key
		end)
		timers:set(out)
		local into: { [string]: boolean } = {}
		local row_list = (m :: any).row_list or {}
		for id, f in m.flights do
			if f.phase == "travel" or f.phase == "fade_in" then
				local to = row_list[id]
				if to then
					into[M.COLUMN_OF[to] or to] = true
				end
			end
		end
		local was = landing:peek()
		local same = true
		for k in into do
			if not was[k] then
				same = false
			end
		end
		for k in was do
			if not into[k] then
				same = false
			end
		end
		if not same then
			landing:set(into)
		end
		publish_debug()
	end

	local function animate(id: string, params: { [string]: any })
		local el = elements[id]
		if el then
			play(el, params :: any)
		end
	end

	local function phase(id: string, f: Flight, name: string)
		f.phase = name
		f.token += 1
	end

	local function done(id: string)
		m.flights[id] = nil
		arrivals[id] = nil
	end

	-- A timer's end: the next beat.
	local function advance(id: string?, token: number)
		if id == nil then
			if token == morph_token then
				morph_on = false
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
				phase(id, f, "out")
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
				m.where[id] = nil
				phase(id, f, "travel")
				arrivals[id] = if folded_of(f.from) then "drawer" else "travel"
				republish()
			end
		elseif p == "out" then
			m.where[id] = nil
			pair[id] = (pair[id] or 0) + 1
			done(id)
			republish()
		elseif p == "fade_out" then
			m.where[id] = nil
			pair[id] = (pair[id] or 0) + 1
			phase(id, f, "fade_in")
			arrivals[id] = "fade"
			republish()
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
		for place, ids in lists do
			for _, id in ids do
				row_list[id] = place
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
						f.token += 1
						changed = true
					end
				elseif f.phase == "travel" or f.phase == "fade_in" then
					if to ~= at then
						-- Moved again on its way: on from where it shows.
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
					pair[id] = (pair[id] or 0) + 1
				elseif reduced or flying() >= M.CAP then
					m.where[id] = at
					m.flights[id] = { phase = "fade_out", from = at, index = index[id], restarted = false, token = 1 }
					animate(id, { opacity = { to = 0 }, duration = M.FADE, ease = "linear" })
					changed = true
				else
					m.where[id] = at
					m.flights[id] = { phase = "hold", from = at, index = index[id], restarted = false, token = 1 }
					animate(id, { opacity = { to = M.DIM }, duration = M.DIM_MS, ease = "out(3)" })
					changed = true
				end
			end
		end
		-- Gone from the board: let go.
		for id in seq do
			if row_list[id] == nil then
				seq[id] = nil
				m.where[id] = nil
				pair[id] = nil
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
		local duration = M.TRAVEL
		if morph_on and how == nil then
			if not drawer and morph_left > 0 and morph_from[id] then
				morph_left -= 1
				table.insert(morphed, id)
				how = "morph"
				duration = M.MORPH
				publish_debug()
			else
				-- (Past the cap, in a drawer, or no node to come from: no
				-- pair, a crossfade.)
				pair[id] = (pair[id] or 0) + 1
				how = "fade"
			end
		end
		arrivals[id] = how
		return { layout_id = `orch-card-{slot}-{filter() or ""}-{pair[id] or 0}-{id}`, duration = duration, how = how }
	end

	function m.attach(id: string, el: any, parts: { content: any?, ghost: any? }?)
		elements[id] = el
		local how = arrivals[id]
		if how == "travel" then
			play(el, { opacity = { from = M.DIM, to = 1 }, duration = M.TRAVEL, ease = M.EASE })
		elseif how == "morph" then
			-- (Its box moves from its other node's; what it holds comes in
			-- once that one's would have gone.)
			local content = (parts and parts.content) or el
			play(content, { opacity = { from = 0, to = 1 }, delay = M.MORPH - M.MORPH_IN, duration = M.MORPH_IN, ease = "linear" })
			local ghost = parts and parts.ghost
			if ghost then
				play(ghost, { opacity = { from = 1, to = 0 }, duration = M.MORPH, ease = M.EASE })
			end
		elseif how == "fade" then
			play(el, { opacity = { from = 0, to = 1 }, duration = M.FADE, ease = "linear" })
		elseif how == "drawer" then
			-- From its drawer's head line, in over the first FADE ms.
			local f = m.flights[id]
			local head = f and heads[f.from]
			ui.after(0, function()
				local dy = 0
				if head then
					local ok, d = pcall(function(): number
						return (head :: any).y - (el :: any).y
					end)
					dy = if ok then d else 0
				end
				play(el, {
					y = { from = dy, to = 0, duration = M.TRAVEL, ease = M.EASE },
					opacity = { from = 0, to = 1, duration = M.FADE, ease = "linear" },
				})
			end)
		end
		-- (Once: a node built again later comes in plainly.)
		arrivals[id] = nil
	end

	function m.raised(place: string): boolean
		return landing:get()[place] == true
	end

	function m.start(b: Built): (number?, number?, number?)
		local how = b.how
		if how == "travel" then
			return M.DIM, nil, 0
		elseif how == "fade" or how == "drawer" then
			return 0, nil, 0
		elseif how == "morph" then
			return nil, 0, 1
		end
		return nil, nil, 0
	end

	function m.detach(id: string, el: any)
		if elements[id] == el then
			elements[id] = nil
		end
	end

	function m.mounted(id: string): boolean
		return elements[id] ~= nil
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

	function m.begin_morph()
		morph_left = if motion.reduced_now() then 0 else M.MORPH_CAP
		morph_on = true
		morph_token += 1
		morph_from = {}
		for id in elements do
			morph_from[id] = true
		end
		morphed = {}
		schedule()
	end

	return m
end

return M
