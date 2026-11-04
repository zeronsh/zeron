-- The service side of Zeron's plugin SDK: what `require("./zeron")(id)`
-- gives a plugin's service.luau. The engine runs it (plugins/service/vm.rs)
-- with `host`, its natives, and keeps `zeron_service` for zeron.luau to
-- hand out. Handlers, tools, timers and `after` callbacks each run as a
-- task (a coroutine), so they can `:await()` a promise.

local host = ...

-- ── Tasks and promises ─────────────────────────────────────────────────────

-- The running tasks, by coroutine: each one's step (resumes it).
local tasks: { [thread]: () -> () } = {}

local function report(what: string, why: any)
	host.log(`{what}: {tostring(why)}`)
end

-- Run `fn(...)` as a task; `finish(ok, value)` once it ends.
local function task(fn: (...any) -> any, finish: ((boolean, any) -> ())?, ...: any)
	local co = coroutine.create(fn)
	local function step(...: any)
		local ok, value = coroutine.resume(co, ...)
		if coroutine.status(co) == "dead" then
			tasks[co] = nil
			if finish then
				finish(ok, value)
			elseif not ok then
				report("a task failed", value)
			end
		end
	end
	tasks[co] = step
	step(...)
end

local Promise = {}
Promise.__index = Promise

local function promise(): any
	return setmetatable({ state = "pending", waiters = {} }, Promise)
end

local function settle(p: any, ok: boolean, value: any)
	if p.state ~= "pending" then
		return
	end
	p.state = if ok then "done" else "failed"
	if ok then
		p.value = value
	else
		p.error = value
	end
	local waiters = p.waiters
	p.waiters = {}
	for _, wake in waiters do
		wake()
	end
end

-- `fn(value)` once it's done (as a task), `failed(why)` if it fails.
function Promise.after(self: any, fn: ((any) -> ())?, failed: ((any) -> ())?): any
	local function fire()
		if self.state == "done" then
			if fn then
				task(fn, nil, self.value)
			end
		elseif failed then
			task(failed, nil, self.error)
		else
			report("a promise failed and nothing caught it", self.error)
		end
	end
	if self.state == "pending" then
		table.insert(self.waiters, fire)
	else
		fire()
	end
	return self
end

-- Its value, waiting for it (in a task); a failure is raised.
function Promise.await(self: any): any
	if self.state == "pending" then
		local co = coroutine.running()
		local step = co and tasks[co]
		if not step then
			error("await works in a task: a handler, a tool, a timer, or zeron.task(fn)", 2)
		end
		table.insert(self.waiters, step)
		coroutine.yield()
	end
	if self.state == "failed" then
		error(self.error, 0)
	end
	return self.value
end

-- Host work in flight, by op id.
local ops: { [number]: any } = {}

local function op(kind: string, args: any): any
	local p = promise()
	ops[host.op(kind, args)] = p
	return p
end

-- ── What the service serves ────────────────────────────────────────────────

local served: { [string]: { fn: (any, any) -> any, safe: boolean } } = {}
local tools: { [string]: (any, any) -> any } = {}
local timers: { { seconds: number, fn: (any) -> () } } = {}
local stops: { () -> () } = {}
-- `zeron.watch`es, by id: each one's folders and listener.
local watches: { [number]: { folders: { string }, fn: (any) -> () } } = {}

local function dispatch(kind: string, name: string, args: any, call: any, reply: number)
	local fn
	if kind == "method" then
		fn = served[name] and served[name].fn
	elseif kind == "tool" then
		fn = tools[name]
	elseif kind == "timer" then
		local timer = timers[tonumber(string.sub(name, 2)) or 0]
		fn = timer and timer.fn
	end
	if not fn then
		host.unknown(reply)
		return
	end
	task(fn, function(ok: boolean, value: any)
		host.reply(reply, ok, value)
	end, args, call)
end

local function describe(): any
	local methods = {}
	for name, entry in served do
		methods[name] = entry.safe
	end
	local tool_names = {}
	for name in tools do
		table.insert(tool_names, name)
	end
	local timer_list = {}
	for i, timer in timers do
		table.insert(timer_list, { key = `#{i}`, seconds = timer.seconds })
	end
	return { methods = methods, tools = tool_names, timers = timer_list }
end

local function done(id: number, ok: boolean, value: any)
	local p = ops[id]
	ops[id] = nil
	if p then
		settle(p, ok, value)
	end
end

-- A watch's burst, by folder's place: its paths, or `true` (anything).
local function heard(id: number, changed: { [number]: any })
	local w = watches[id]
	if not w then
		return
	end
	local by_folder = {}
	for place, paths in changed do
		local folder = w.folders[place]
		if folder then
			by_folder[folder] = paths
		end
	end
	task(w.fn, nil, by_folder)
end

local function stopping()
	for _, fn in stops do
		local ok, why = pcall(fn)
		if not ok then
			report("on_stop failed", why)
		end
	end
end

-- ── The SDK ────────────────────────────────────────────────────────────────

local made: { [string]: any } = {}

local function service(id: string): any
	if made[id] then
		return made[id]
	end
	if id ~= host.plugin then
		error(`this service is {host.plugin}'s: require("./zeron")("{host.plugin}")`, 2)
	end
	local M = {}
	M.device = host.device
	M.json = host.json
	M.null = host.null

	-- A method callers reach with `zeron.service(device?):call(name, args)`.
	-- `fn(args, call)` returns the answer (or awaits, then returns it);
	-- error() fails the call. `safe = true`: running it twice does no harm,
	-- so Zeron may run it again when the service stopped during the call.
	function M.serve(name: string, fn: (any, any) -> any, opts: { safe: boolean? }?)
		assert(type(name) == "string" and name ~= "", "zeron.serve: a method's name")
		assert(type(fn) == "function", "zeron.serve: a function to answer it")
		assert(string.sub(name, 1, 1) ~= "@", "zeron.serve: names starting with @ are Zeron's")
		served[name] = { fn = fn, safe = (opts and opts.safe) == true }
	end

	-- A tool agents call (named in the manifest's "tools"), answered here
	-- with no window open.
	function M.tool(tool: { name: string, run: (any, any) -> any })
		assert(type(tool) == "table" and type(tool.name) == "string", "zeron.tool { name, run }")
		assert(type(tool.run) == "function", "zeron.tool { name, run }: run is a function")
		tools[tool.name] = tool.run
	end

	-- Now and then, 30 seconds at least, even while nothing calls the
	-- service (Zeron starts it to run `fn`). Register timers at the top
	-- level, in the same order each time: they're known by their place.
	function M.every(seconds: number, fn: (any) -> ())
		assert(type(seconds) == "number" and type(fn) == "function", "zeron.every(seconds, fn)")
		table.insert(timers, { seconds = math.max(30, seconds), fn = fn })
	end

	-- Told before the service stops (idle, an update, Zeron quitting).
	function M.on_stop(fn: () -> ())
		table.insert(stops, fn)
	end

	-- Changes under `folders` on this device, each all the way down
	-- ("files.read"): `fn(changed)` once a burst of them has been quiet for
	-- `quiet` ms (250 unless said), `changed` by folder: the paths that
	-- changed in it (relative, `/`-separated), or `true` when anything may
	-- have (as the watch starts, and when changes were too many or lost).
	-- Reads aren't changes. Returns a function that stops watching. A
	-- watch lasts while the service runs (5 minutes after its last call).
	function M.watch(folders: { string }, fn: (any) -> (), opts: { quiet: number? }?): () -> ()
		assert(type(folders) == "table" and #folders > 0, "zeron.watch(folders, fn)")
		assert(type(fn) == "function", "zeron.watch(folders, fn)")
		local id = host.watch(folders, opts and opts.quiet)
		watches[id] = { folders = table.clone(folders), fn = fn }
		return function()
			if watches[id] then
				watches[id] = nil
				host.unwatch(id)
			end
		end
	end

	-- To this plugin's windows (on this device, and those watching it from
	-- others): `zeron.service(device):on(event, fn)`.
	function M.emit(event: string, data: any)
		host.emit(event, data)
	end

	-- This device's chats' and projects' folders, whole paths ("sessions.read"):
	-- a promise of a list. Where a service's work on them belongs.
	function M.folders(): any
		return op("folders", {})
	end

	-- A program its manifest names ("commands"), as you, on this device:
	-- a promise of { status, stdout, stderr }.
	function M.run(program: string, args: { string }?, opts: { cwd: string?, stdin: string?, timeout: number?, env: { [string]: string }? }?): any
		local o = opts or {}
		return op("run", { program = program, args = args or {}, cwd = o.cwd, stdin = o.stdin, timeout = o.timeout, env = o.env })
	end

	-- The internet ("network"): a promise of { status, headers, body }.
	function M.fetch(url: string, opts: { method: string?, headers: { [string]: string }?, body: string?, timeout: number? }?): any
		local o = opts or {}
		return op("fetch", { url = url, method = o.method, headers = o.headers, body = o.body, timeout = o.timeout })
	end

	-- A notification, as Zeron's own ("notifications"): a promise of
	-- "posted", "off", "held", "paced" or "unavailable".
	function M.notify(n: { title: string, body: string?, session: string? }): any
		return op("notify", { title = n.title, body = n.body, session = n.session })
	end

	-- A promise done after `seconds`.
	function M.sleep(seconds: number): any
		return op("sleep", { seconds = seconds })
	end

	-- Run `fn` as a task of its own (so it can await).
	function M.task(fn: () -> ())
		task(fn)
	end

	-- What the service keeps between runs, on this device (1 MB at most).
	function M.store(key: string, default: any): any
		local box = {}
		function box.get(_self: any): any
			local v = host.store_get(key)
			return if v == nil then default else v
		end
		box.peek = box.get
		function box.set(_self: any, value: any)
			host.store_set(key, value)
		end
		return box
	end

	made[id] = M
	return M
end

return service, dispatch, describe, done, stopping, heard
