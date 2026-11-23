-- What a page kept built but hidden reads: live while it shows, the value
-- it had last while it's hidden. A prop function wrapped here reads only
-- `live()` while hidden, so nothing upstream (a push of sessions, the
-- board's items) wakes it or what's built from it; shown again, `live()`
-- changing runs it again, and it catches up at once.
--
--   local hold = require("@ds/hold")
--   needs = hold.fn(live, function() return data.needs(…) end)
--   thread_of = hold.by(live, function(chat) return chats[chat] end)
--   local now = hold.computed(live, function() return orchestration.now:get() end)
--
-- `name` (optional) counts each run that read upstream (ds/orch_perf_trace,
-- `hold.<name>`): what a test reads to see a hidden page do nothing.

local trace = require("@self/orch_perf_trace")

local M = {}

local NONE = {}

-- `f()` while `live()`; its last value while not (run once if it never has).
function M.fn<T>(live: () -> boolean, f: () -> T, name: string?): () -> T
	local last: any = NONE
	return function(): T
		if not live() and last ~= NONE then
			return last
		end
		if name then
			trace.hit(`hold.{name}`)
		end
		last = f()
		return last
	end
end

-- A computed of `read()` while `live()`, its last value while not: what
-- reads it is woken only when it changes, never by `live` alone (a page's
-- roots, read by hundreds of bindings). Made in a component's build (a
-- computed belongs to the build that makes it).
function M.computed<T>(live: () -> boolean, read: () -> T, name: string?): any
	local last: any = NONE
	return ui.computed(function(): T
		if not live() and last ~= NONE then
			return last
		end
		if name then
			trace.hit(`hold.{name}`)
		end
		last = read()
		return last
	end)
end

-- The same for a function of one key (a chat's, a card's): each key's
-- last value kept.
function M.by<K, T>(live: () -> boolean, f: (K) -> T, name: string?): (K) -> T
	local last: { [any]: any } = {}
	return function(k: K): T
		local was = last[k]
		if not live() and was ~= nil then
			return if was == NONE then nil :: any else was
		end
		if name then
			trace.hit(`hold.{name}`)
		end
		local v = f(k)
		last[k] = if v == nil then NONE else v
		return v
	end
end

return M
