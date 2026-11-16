-- Building a page in stages: what's given a rank past 0 holds `h` points of
-- room and builds in a frame of its own after the page's first, lowest
-- rank first, one a frame (a page built all at once spends its whole build
-- in one frame, the window not answering meanwhile). A timer of its own
-- builds it a little later should the pump's owner go first.
--
--   stage.Later(rank, build, h?)   build() now if rank <= 0, else later

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

-- (For tests: how many wait.)
function M.waiting(): number
	return #queue
end

return M
