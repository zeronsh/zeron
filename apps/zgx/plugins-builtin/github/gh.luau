--!strict
-- GitHub, through its CLI (gh) as you're signed in to it on this Mac: the
-- plugin runs it (`zeron.run`, "commands": ["gh"]) and asks GitHub's own API,
-- REST and GraphQL. Nothing of Zeron's stands between: what GitHub can do,
-- this can. What it reads is kept in the plugin's database (`zeron.db`), so a
-- pull request opens at once and is asked again behind it.
--
--   gh.graphql(query, variables, { background })
--                                     a task of the answer's `data`
--   gh.rest(method, path, body?)      a task of the answer, decoded
--   gh.text(path, accept)             a task of the answer as text (a diff),
--                                     asked again only if it changed (ETag)
--   gh.cached(key, ok)                what's kept for `key` (nil, or the value
--                                     and when it was kept), then `ok`
--   gh.keep(key, value)               keeps it
--
-- A task is GitHub's answer to come: `task:after(ok, failed)`, `failed`
-- told what went wrong in words a person can act on.

export type Task = { after: (self: Task, ok: (value: any) -> (), failed: ((why: string) -> ())?) -> Task }

return function(zeron: any)
	local M = {}

	-- A value to come: `start(resolve, reject)` runs at once; whoever asks
	-- after it settles hears straight away.
	local function task(start: (resolve: (value: any) -> (), reject: (why: string) -> ()) -> ()): Task
		local settled, ok, value = false, false, nil :: any
		local waiting: { { ok: (value: any) -> (), failed: ((why: string) -> ())? } } = {}
		local function settle(good: boolean, v: any)
			if settled then
				return
			end
			settled, ok, value = true, good, v
			for _, w in waiting do
				if good then
					w.ok(v)
				elseif w.failed then
					(w.failed :: (why: string) -> ())(v)
				end
			end
			waiting = {}
		end
		local t = {} :: any
		function t.after(self: Task, on_ok: (value: any) -> (), on_failed: ((why: string) -> ())?): Task
			if not settled then
				table.insert(waiting, { ok = on_ok, failed = on_failed })
			elseif ok then
				on_ok(value)
			elseif on_failed then
				on_failed(value)
			end
			return self
		end
		start(function(v: any)
			settle(true, v)
		end, function(why: string)
			settle(false, why)
		end)
		return t :: Task
	end
	M.task = task

	-- What went wrong, in words to act on: gh missing, not signed in, or
	-- GitHub's own message.
	local function plain(why: string): string
		local text = string.gsub(why, "^%s+", "")
		text = string.gsub(text, "%s+$", "")
		if string.find(text, "isn't installed", 1, true) then
			return zeron.s("gh.no_cli")
		elseif string.find(text, "gh auth login", 1, true) or string.find(text, "not logged in", 1, true) then
			return zeron.s("gh.sign_in")
		elseif string.find(text, "HTTP 404", 1, true) or string.find(text, "Could not resolve to a", 1, true) then
			return zeron.s("gh.not_found")
		elseif string.find(text, "rate limit", 1, true) then
			return zeron.s("gh.rate_limit")
		end
		-- gh's own prefix off: GitHub's words.
		text = string.gsub(text, "^gh: ", "")
		return if text ~= "" then text else zeron.s("gh.no_answer")
	end

	-- GitHub's rate limits are tight, so as Zeron's own GitHub code does
	-- (crates/engine/src/source_control.rs): once GitHub says its limit is
	-- reached, nothing is asked for 15 minutes (every ask fails at once,
	-- saying so); and the same read asked again while it's on its way is
	-- that one ask, not another.
	local RATE_LIMIT_PAUSE = 15 * 60
	local limited_until = 0
	local in_flight: { [string]: Task } = {}

	-- gh runs two at a time at most: one machine's GitHub, and GitHub's
	-- own advice (no concurrent requests). What you asked for goes ahead
	-- of what the plugin asks on its own (`background`: the badges' watch).
	local MAX_RUNNING = 2
	local running = 0
	local waiting: { { start: () -> (), background: boolean } } = {}
	local function next_waiting()
		while running < MAX_RUNNING and #waiting > 0 do
			local w = table.remove(waiting, 1) :: any
			running += 1
			w.start()
		end
	end
	local function queued(start: (finished: () -> ()) -> (), background: boolean?)
		local function go()
			local done = false
			start(function()
				if not done then
					done = true
					running -= 1
					next_waiting()
				end
			end)
		end
		local entry = { start = go, background = background == true }
		if background then
			table.insert(waiting, entry)
		else
			-- (Ahead of the background ones, behind what you asked before.)
			local at = #waiting + 1
			for i, w in waiting do
				if w.background then
					at = i
					break
				end
			end
			table.insert(waiting, at, entry)
		end
		next_waiting()
	end

	local function limited(text: string): boolean
		local lower = string.lower(text)
		return string.find(lower, "rate limit", 1, true) ~= nil or string.find(lower, "secondary rate", 1, true) ~= nil
	end

	-- gh with `args` (and `stdin`): a task of what it printed. `share`: a
	-- read, joined by the same read asked meanwhile. `background`: asked
	-- by the plugin on its own, after what you asked.
	local function run(args: { string }, stdin: string?, share: boolean?, background: boolean?): Task
		if os.time() < limited_until then
			local at = os.date("%H:%M", limited_until)
			return task(function(_, reject)
				reject(zeron.s("gh.rate_limit_until", { at = at }))
			end)
		end
		local key = if share then table.concat(args, "\0") .. "\0" .. (stdin or "") else nil
		if key and in_flight[key] then
			return in_flight[key]
		end
		local t = task(function(resolve, reject)
			queued(function(finished: () -> ())
			zeron.run("gh", args, { stdin = stdin, timeout = 60 }):after(function(r: any)
				finished()
				if key then
					in_flight[key] = nil
				end
				if type(r) == "table" and r.status == 0 then
					resolve(r.stdout or "")
				else
					local said = tostring((if type(r) == "table" then (if r.stderr and r.stderr ~= "" then r.stderr else r.stdout) else nil) or "gh didn't answer")
					if limited(said) then
						limited_until = os.time() + RATE_LIMIT_PAUSE
					end
					reject(plain(said))
				end
			end, function(err: any)
				finished()
				if key then
					in_flight[key] = nil
				end
				reject(plain(if type(err) == "table" and err.message then tostring(err.message) else tostring(err)))
			end)
			end, background)
		end)
		if key then
			in_flight[key] = t
		end
		return t
	end

	local function decoded(text: string): any
		if string.match(text, "^%s*$") then
			return {}
		end
		local ok, value = pcall(ui.json.decode, text)
		return if ok then value else nil
	end

	function M.graphql(query: string, variables: { [string]: any }?, opts: { background: boolean? }?): Task
		return task(function(resolve, reject)
			local body = ui.json.encode({ query = query, variables = variables or {} })
			local read = not string.match(query, "^%s*mutation")
			run({ "api", "graphql", "--input", "-" }, body, read, opts and opts.background):after(function(out: string)
				local answer = decoded(out)
				if type(answer) ~= "table" then
					reject(zeron.s("gh.unreadable"))
				elseif answer.data == nil and answer.errors and answer.errors[1] then
					local message = tostring(answer.errors[1].message or zeron.s("gh.said_no"))
					-- (GraphQL's limit comes as an answer, not a failure.)
					if limited(message) or answer.errors[1].type == "RATE_LIMITED" then
						limited_until = os.time() + RATE_LIMIT_PAUSE
					end
					reject(plain(message))
				else
					resolve(answer.data or {})
				end
			end, function(why: string)
				-- (GraphQL errors arrive as JSON on stdout too.)
				local answer = decoded(why)
				if type(answer) == "table" and answer.errors and answer.errors[1] then
					reject(plain(tostring(answer.errors[1].message)))
				else
					reject(why)
				end
			end)
		end)
	end

	function M.rest(method: string, path: string, body: { [string]: any }?): Task
		return task(function(resolve, reject)
			local args = { "api", "--method", method, path }
			local stdin: string? = nil
			if body then
				table.insert(args, "--input")
				table.insert(args, "-")
				stdin = ui.json.encode(body)
			end
			run(args, stdin):after(function(out: string)
				resolve(decoded(out) or {})
			end, function(why: string)
				local answer = decoded(why)
				reject(if type(answer) == "table" and answer.message then plain(tostring(answer.message)) else why)
			end)
		end)
	end

	-- REST reads asked before: each one's ETag and answer. Asked again with
	-- `If-None-Match`, an unchanged one is GitHub's 304, which costs none of
	-- the hour's quota, and the answer kept is the answer. (A few at most:
	-- a diff can be large.)
	local MAX_TAGGED = 16
	local tagged: { [string]: { etag: string, body: string } } = {}
	local tagged_order: { string } = {}

	-- `gh api -i`'s output: its status, its headers (lower-case), its body.
	local function with_headers(out: string): (number?, { [string]: string }, string)
		local head, body = string.match(out, "^(.-)\r?\n\r?\n(.*)$")
		if not head then
			return nil, {}, out
		end
		local status = tonumber(string.match(head, "^HTTP/[%d%.]+ (%d+)") or "")
		if not status then
			return nil, {}, out
		end
		local headers: { [string]: string } = {}
		for line in string.gmatch(head, "[^\r\n]+") do
			local k, v = string.match(line, "^([%w%-]+):%s*(.*)$")
			if k then
				headers[string.lower(k)] = v
			end
		end
		return status, headers, body
	end

	function M.text(path: string, accept: string): Task
		local key = `{accept}\0{path}`
		local known = tagged[key]
		local args = { "api", "-i", "-H", `Accept: {accept}`, path }
		if known then
			table.insert(args, "-H")
			table.insert(args, `If-None-Match: {known.etag}`)
		end
		return task(function(resolve, reject)
			run(args, nil, true):after(function(out: string)
				local status, headers, body = with_headers(out)
				if status == 304 and known then
					resolve(known.body)
					return
				end
				local etag = headers["etag"]
				if etag and etag ~= "" then
					if not tagged[key] then
						table.insert(tagged_order, key)
						if #tagged_order > MAX_TAGGED then
							tagged[table.remove(tagged_order, 1) :: string] = nil
						end
					end
					tagged[key] = { etag = etag, body = body }
				end
				resolve(body)
			end, function(why: string)
				-- (gh may call GitHub's 304 a failure: unchanged, all the same.)
				if known and string.find(why, "304", 1, true) then
					resolve(known.body)
				else
					reject(why)
				end
			end)
		end)
	end

	-- The cache: one table, made once; every read and write waits for it.
	local ready = task(function(resolve, reject)
		zeron.db.exec("CREATE TABLE IF NOT EXISTS cache (key TEXT PRIMARY KEY, value TEXT NOT NULL, at INTEGER NOT NULL)"):after(function()
			resolve(true)
		end, function(err: any)
			reject(tostring(if type(err) == "table" and err.message then err.message else err))
		end)
	end)

	-- What's been read or kept this run, by key: told at once, so what was
	-- open a moment ago opens again without a beat on the database.
	local memory: { [string]: { value: any, at: number } } = {}

	function M.cached(key: string, ok: (value: any?, at: number?) -> ())
		local known = memory[key]
		if known then
			ok(known.value, known.at)
			return
		end
		ready:after(function()
			zeron.db.query("SELECT value, at FROM cache WHERE key = ?", { key }):after(function(rows: any)
				local row = type(rows) == "table" and rows[1] or nil
				if row then
					local ok_decode, value = pcall(ui.json.decode, row.value)
					if ok_decode and value ~= nil and not memory[key] then
						memory[key] = { value = value, at = row.at }
					end
					ok(if ok_decode then value else nil, row.at)
				else
					ok(nil, nil)
				end
			end, function()
				ok(nil, nil)
			end)
		end, function()
			ok(nil, nil)
		end)
	end

	function M.keep(key: string, value: any)
		memory[key] = { value = value, at = os.time() }
		ready:after(function()
			zeron.db.exec("INSERT OR REPLACE INTO cache (key, value, at) VALUES (?, ?, ?)", { key, ui.json.encode(value), os.time() })
		end)
	end

	return M
end
