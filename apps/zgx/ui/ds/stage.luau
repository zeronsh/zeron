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
--     box = { grow = 1 } }   -- (the room it takes, built or not)

local M = {}

type Entry = { rank: number, go: () -> () }
local queue: { Entry } = {}
local pumping = false

local function pump()
	if #queue == 0 then
		pumping = false
		return
	end
	table.sort(queue, function(a: Entry, b: Entry): boolean
		return a.rank < b.rank
	end)
	local entry = table.remove(queue, 1) :: Entry
	entry.go()
	ui.after(0, pump)
end

local function forget(entry: Entry)
	local at = table.find(queue, entry)
	if at then
		table.remove(queue, at)
	end
end

local Later = ui.component(function(props: { rank: number, build: () -> Node, h: number? }): Node
	local ready = ui.use_state(false)
	local entry: Entry = {
		rank = props.rank,
		go = function()
			ready:set(true)
		end,
	}
	table.insert(queue, entry)
	ui.on_cleanup(function()
		forget(entry)
	end)
	if not pumping then
		pumping = true
		ui.after(0, pump)
	end
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

-- Background pieces built a frame.
M.PER_FRAME = 1
-- How many background pieces each `group` (a page) still waits on: a page
-- can stay out of layout until they're all in.
M.pending = ui.state("stage_idle_pending", {} :: { [string]: number })
local function count(group: string?, by: number)
	if not group then
		return
	end
	local n = (M.pending:peek()[group] or 0) + by
	M.pending:set_key({ group }, if n > 0 then n else nil)
end
type IdleEntry = { rank: number, go: () -> () }
local idle: { IdleEntry } = {}
local idle_pumping = false

local function idle_pump()
	if #idle == 0 then
		idle_pumping = false
		return
	end
	-- (The page's own stages first.)
	if #queue > 0 then
		ui.after(16, idle_pump)
		return
	end
	table.sort(idle, function(a: IdleEntry, b: IdleEntry): boolean
		return a.rank < b.rank
	end)
	for _ = 1, M.PER_FRAME do
		local entry = table.remove(idle, 1)
		if not entry then
			break
		end
		entry.go()
	end
	-- (A frame's time on: a timer due at once can run in the same frame.)
	ui.after(16, idle_pump)
end

local function forget_idle(entry: IdleEntry)
	local at = table.find(idle, entry)
	if at then
		table.remove(idle, at)
	end
end

M.Idle = ui.component(function(props: { rank: number?, build: () -> Node, h: number?, now: (() -> boolean)?, box: { [string]: any }?, group: string? }): Node
	local ready = ui.use_state(false)
	local counted = true
	count(props.group, 1)
	local function done()
		if counted then
			counted = false
			-- (After this frame's build: a frame on.)
			ui.after(0, function()
				count(props.group, -1)
			end)
		end
	end
	local entry: IdleEntry = {
		rank = props.rank or 0,
		go = function()
			ready:set(true)
		end,
	}
	table.insert(idle, entry)
	ui.on_cleanup(function()
		forget_idle(entry)
		done()
	end)
	if not idle_pumping then
		idle_pumping = true
		ui.after(16, idle_pump)
	end
	local now = props.now
	return ui.presence(function(): string
		return if ready:get() or (now ~= nil and now()) then "built" else "waiting"
	end, function(state: string): Node
		if state == "built" then
			forget_idle(entry)
			done()
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
