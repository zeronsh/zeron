-- Minimum presence: a status or an activity, once shown, stays at least
-- 300 ms before the next one takes its place, so a state that comes and
-- goes in a blink is still seen, and one that flickers doesn't. A change
-- that arrives inside the hold waits for its end; changes after it replace
-- it as they come (the latest wins), and a change back to what shows
-- cancels the wait.
--
--   local hold = require("@ds/orch_hold")
--   local kind = hold.value(function() return chat().status end)
--   StatusIcon { kind = kind }                     -- kind() is held
--   hold.value(source, { ms = 500, same = function(a, b) … end })
--
-- Called in a component's, row's or presence's build: what it holds goes
-- with what made it.

local M = {}

M.MIN = 300

-- Whether two values are the same for showing: equal, or tables with the
-- same fields (an activity `{ kind, label, since }` sent again).
local function same(a: any, b: any): boolean
	if a == b then
		return true
	end
	if type(a) ~= "table" or type(b) ~= "table" then
		return false
	end
	for k, v in a do
		if type(v) ~= "table" and b[k] ~= v then
			return false
		end
	end
	for k, v in b do
		if type(v) ~= "table" and a[k] ~= v then
			return false
		end
	end
	return true
end
M.same = same

export type Options = { ms: number?, same: ((a: any, b: any) -> boolean)? }

-- `source` held: a function reading what shows, at least `ms` (300) since
-- the last thing it showed.
function M.value<T>(source: () -> T, opts: Options?): () -> T
	local ms = (opts and opts.ms) or M.MIN
	local eq = (opts and opts.same) or same
	local shown = ui.use_state(source())
	local since = ui.now()
	ui.effect(function()
		local v = source()
		if eq(v, shown:peek()) then
			-- (Back to what shows: a change waiting is dropped with this run.)
			return
		end
		local left = ms - (ui.now() - since)
		if left <= 0 then
			since = ui.now()
			shown:set(v)
			return
		end
		-- (Made in this run: the next change cancels it, and its own wait
		-- counts from the same moment.)
		ui.after(left, function()
			since = ui.now()
			shown:set(v)
		end)
	end)
	return function(): T
		return shown:get()
	end
end

return M
