-- Building a page in stages: what's given a rank past 0 holds `h` points of
-- room and builds in a frame of its own after the page's first, lowest
-- rank first, one a frame (a page built all at once spends its whole build
-- in one frame, the window not answering meanwhile). A timer of its own
-- builds it a little later should the pump's owner go first.
--
--   stage.Later(rank, build, h?)   build() now if rank <= 0, else later
--
-- Building in the background (a page kept hidden, built before it's
-- opened): what's given `stage.Idle` builds only once the stages above are
-- all done, `PER_FRAME` a frame (a piece's build runs as its frame lays it
-- out, past where a script can time it: so a count, the pieces made small
-- enough), lowest rank first. `now()` true (the page shown before it was
-- done) builds it at once.
--
--   stage.Idle { rank = 3, build = build, h = 120, now = function() … end,
--     box = { grow = 1 },    -- (the room it takes, built or not)
--     urgent = function() … end }  -- (its page shown: `URGENT_PER_FRAME` a
--                            -- frame, before the rest; never all at once)

local M = {}

type Entry = { rank: number, go: () -> () }
local queue: { Entry } = {}

local function forget(entry: Entry)
	local at = table.find(queue, entry)
	if at then
		table.remove(queue, at)
	end
end

-- One a frame, lowest rank first. (Each waiting entry keeps a timer of its
-- own, before each frame; the one at the head takes the frame's step. A
-- pump of the module's, its timer set by whichever component came first,
-- went with that component: a page made again, or laid out anew, left the
-- rest to their fallback, 0.4 s on.)
local later_out = 0
local later_blocked = 0
local later_stepped = false
local function later_step()
	if later_stepped or later_out > 0 then
		later_blocked += 1
		if later_blocked < 16 then
			return
		end
	end
	later_out, later_blocked = 0, 0
	later_stepped = true
	ui.after(0, function()
		later_stepped = false
	end)
	table.sort(queue, function(a: Entry, b: Entry): boolean
		return a.rank < b.rank
	end)
	local entry = table.remove(queue, 1)
	if entry then
		later_out += 1
		entry.go()
	end
end

local Later = ui.component(function(props: { rank: number, build: () -> Node, h: number? }): Node
	local ready = ui.use_state(false)
	local released = false
	local entry: Entry = {
		rank = props.rank,
		go = function()
			released = true
			ready:set(true)
		end,
	}
	table.insert(queue, entry)
	local alive = true
	ui.on_cleanup(function()
		alive = false
		forget(entry)
	end)
	local function tick()
		if not alive or ready:peek() then
			return
		end
		if table.find(queue, entry) then
			table.sort(queue, function(a: Entry, b: Entry): boolean
				return a.rank < b.rank
			end)
			if queue[1] == entry then
				later_step()
			end
		end
		if not ready:peek() then
			ui.after(0, tick)
		end
	end
	ui.after(0, tick)
	ui.after(400 + 60 * props.rank, function()
		if not ready:peek() then
			forget(entry)
			ready:set(true)
		end
	end)
	return ui.presence(function(): string
		return if ready:get() then "built" else "waiting"
	end, function(state: string): Node
		if state == "built" then
			if released then
				released = false
				later_out = math.max(0, later_out - 1)
			end
			return props.build()
		end
		return ui.div { class = "stage-later", w_full = true, h = props.h or 120 }
	end, { w_full = true })
end)

function M.Later(rank: number, build: () -> Node, h: number?): Node
	if rank <= 0 then
		return build()
	end
	return Later { rank = rank, build = build, h = h }
end

-- Background pieces built a frame; and while any is `urgent` (its page
-- shown: a board opened, its cards coming), that many of those first.
M.PER_FRAME = 1
M.URGENT_PER_FRAME = 3
-- Tries (a frame's time apart) a step waits on the last one's pieces.
M.GIVE_UP = 120
-- How many background pieces each `group` (a page) still waits on: a page
-- can stay out of layout until they're all in. (Counted from the queue.)
M.pending = ui.state("stage_idle_pending", {} :: { [string]: number })
type IdleEntry = { rank: number, go: () -> (), group: string?, urgent: (() -> boolean)?, wake: () -> () }
local idle: { IdleEntry } = {}

local recount_due = false
local function recount()
	if recount_due then
		return
	end
	recount_due = true
	ui.after(0, function()
		recount_due = false
		local n: { [string]: number } = {}
		for _, e in idle do
			if e.group then
				n[e.group] = (n[e.group] or 0) + 1
			end
		end
		local was = M.pending:peek()
		for k, v in n do
			if was[k] ~= v then
				M.pending:set(n)
				return
			end
		end
		for k in was do
			if n[k] == nil then
				M.pending:set(n)
				return
			end
		end
	end)
end

local function urgent(e: IdleEntry): boolean
	return e.urgent ~= nil and e.urgent()
end

-- The queue in the order it builds: what's urgent first, then by rank.
local function order()
	table.sort(idle, function(a: IdleEntry, b: IdleEntry): boolean
		local ua, ub = urgent(a), urgent(b)
		if ua ~= ub then
			return ua
		end
		return a.rank < b.rank
	end)
end

-- One frame's pieces: `URGENT_PER_FRAME` while any is urgent, else
-- `PER_FRAME`, and none while the page's own stages wait (unless urgent).
-- (One step at a time: the next only once the last one's pieces have been
-- built, as a frame lays them out. Timers run between frames too: at rest
-- a window draws a frame only now and then, and a step a timer would take
-- every time would pile up for that one frame. Pieces let go but gone before
-- they built: after a while, not waited on.)
local outstanding = 0
local blocked = 0
-- (And once a frame at most: the token is let go before the next frame.)
local stepped = false
-- (In the background, a frame's time of the clock apart too: a window at
-- rest draws a frame only now and then, and runs every timer come due in
-- that frame, so pieces a timer's time apart would all land in it.)
local last_step = -math.huge
local function step()
	if not (idle[1] ~= nil and urgent(idle[1])) and ui.now() - last_step < 12 then
		return
	end
	-- (A piece let go and gone before it built gives its turn back; held
	-- back past `GIVE_UP` tries all the same, a token whose timer went with
	-- the component that set it.)
	if stepped or outstanding > 0 then
		blocked += 1
		if blocked < M.GIVE_UP then
			return
		end
	end
	outstanding, blocked = 0, 0
	last_step = ui.now()
	stepped = true
	ui.after(0, function()
		stepped = false
	end)
	order()
	local any_urgent = idle[1] ~= nil and urgent(idle[1])
	if #queue > 0 and not any_urgent then
		return
	end
	for _ = 1, if any_urgent then M.URGENT_PER_FRAME else M.PER_FRAME do
		local entry = table.remove(idle, 1)
		if not entry then
			break
		end
		outstanding += 1
		entry.go()
	end
	recount()
end

local function forget_idle(entry: IdleEntry)
	local at = table.find(idle, entry)
	if at then
		table.remove(idle, at)
		recount()
		-- (The head gone: the next one takes over.)
		if at == 1 and idle[1] then
			idle[1].wake()
		end
	end
end

-- (The piece at the head of the queue keeps a timer, a frame's time apart,
-- and takes the frame's step; the others wait. A timer goes with the
-- component that set it, so each sets its own: the head gone or changed,
-- the new head is woken (a state of its own) and sets one. No pump of the
-- module's to lose, and no timer a waiting piece.)
M.Idle = ui.component(function(props: { rank: number?, build: () -> Node, h: number?, now: (() -> boolean)?, box: { [string]: any }?, group: string?, urgent: (() -> boolean)? }): Node
	local ready = ui.use_state(false)
	local poke = ui.use_state(0)
	local released = false
	local entry: IdleEntry = {
		rank = props.rank or 0,
		group = props.group,
		urgent = props.urgent,
		go = function()
			released = true
			ready:set(true)
		end,
		wake = function() end,
	}
	entry.wake = function()
		poke:set(poke:peek() + 1)
	end
	table.insert(idle, entry)
	recount()
	local alive = true
	ui.on_cleanup(function()
		alive = false
		forget_idle(entry)
		if released then
			released = false
			outstanding = math.max(0, outstanding - 1)
		end
	end)
	local function tick()
		if not alive or ready:peek() or not table.find(idle, entry) then
			return
		end
		order()
		if idle[1] ~= entry then
			-- (Not the head: the head ticks.)
			idle[1].wake()
			return
		end
		step()
		if idle[1] == entry and not ready:peek() then
			entry.wake()
		elseif idle[1] then
			idle[1].wake()
		end
	end
	-- Woken (the head, or new): its next tick, a frame's time on, on a timer
	-- of its own effect's (an effect run again lets go of the last one's).
	ui.effect(function()
		poke:get()
		if alive and not ready:peek() then
			ui.after(16, tick)
		end
	end)
	local now = props.now
	return ui.presence(function(): string
		return if ready:get() or (now ~= nil and now()) then "built" else "waiting"
	end, function(state: string): Node
		if state == "built" then
			forget_idle(entry)
			-- (Its turn done once a frame has laid it out: timers run between
			-- frames, and a piece built by each would all land in one.)
			if released then
				return ui.column {
					w_full = true,
					grow = 1,
					min_h = 0,
					on_resize = function()
						if released then
							released = false
							outstanding = math.max(0, outstanding - 1)
						end
					end,
					props.build(),
				}
			end
			return props.build()
		end
		return ui.div { class = "stage-idle", w_full = true, h = props.h or 120 }
	end, props.box or { w_full = true })
end)

-- (For tests: how many wait in the background.)
function M.idle_waiting(): number
	return #idle
end

-- (For tests: how many wait.)
function M.waiting(): number
	return #queue
end

return M
