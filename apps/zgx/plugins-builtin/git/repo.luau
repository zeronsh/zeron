--!strict
-- A session's checkout as the window keeps it: what its device's service
-- (service.luau) last said of it, asked again as that device says it
-- changed while a Git tab shows it, and the files and diffs asked only as
-- they move. One
-- per session, shared by the tab's two views and by what other plugins
-- place in its slots, so switching views asks nothing again.
--
-- Every call goes to the service on the device the session's checkout is
-- on (`zeron.service(session.device)`); every failure is kept as a
-- Problem (`problem(err)`: the refusal's reason, the service's own code,
-- the words), for the view to say in plain words (words.luau).

export type Problem = { reason: string, code: string?, words: string }

export type Status = {
	branch: string?,
	oid: string?,
	upstream: string?,
	ahead: number,
	behind: number,
	pushed: boolean?,
	base: string?,
	base_ahead: number?,
	base_behind: number?,
	state: string?,
	conflicts: number,
	changes: number,
	tree: string,
	index: string,
	device: string?,
	remote: string?,
	who: string?,
}

-- A file as the list draws it (ds/git_changes' File), with its staging.
export type File = {
	path: string,
	dir: string,
	name: string,
	status: string,
	add: number,
	del: number,
	rev: string?,
	old: string?,
	stage: string?,
	binary: boolean?,
}

export type Commit = { sha: string, parents: { string }, author: string, email: string, at: number, refs: { string }, subject: string }

-- A file's diff as asked: its lines once here, whether it was cut, why it
-- failed.
export type Diff = { lines: { any }?, cut: boolean?, problem: Problem? }

-- How often a shown tab asks again with nothing heard (ms). The checkout's
-- device says when it changes (its service watches it: service.luau), so
-- this is only a net, for a change it couldn't tell (its link was down).
-- It also keeps the device's watch (`LEASE` there).
local SAFETY = 60000
-- A file's diff shows at most this many lines (and says so past them).
local MAX_LINES = 2000

return function(zeron: any)
	local gitds = require("@ds/git")
	local M = {}

	-- Why a call failed: the refusal's reason ("offline", "not_approved",
	-- …), and for a failure of the service's own ("failed"), its code
	-- ("rejected", "dirty", …) and words.
	function M.problem(err: any): Problem
		local text = if type(err) == "table" and err.message then tostring(err.message) else tostring(err)
		local reason = zeron.reason(text)
		local rest = string.match(text, "^[%a_]+: (.*)$") or text
		if reason == "failed" then
			local code, words = string.match(rest, "^([%a_]+): (.*)$")
			if code then
				return { reason = reason, code = code, words = words :: string }
			end
		end
		return { reason = reason, words = rest }
	end

	-- The service's files as the list draws them (`staging`: staging asked
	-- and not yet read back, laid over them).
	function M.to_files(list: { any }?, staging: { [string]: string }?): { File }
		local out: { File } = {}
		for _, f in list or {} do
			local dir, name = string.match(f.path, "^(.*)/([^/]+)$")
			-- (A file renamed says what it was: `old.rs → new.rs`.)
			local shown = name or f.path
			local title: string? = nil
			if type(f.old) == "string" and f.old ~= f.path then
				shown = `{string.match(f.old, "([^/]+)$") or f.old} → {shown}`
				title = `{f.old} → {f.path}`
			end
			table.insert(out, {
				path = f.path,
				dir = dir or "",
				name = shown,
				title = title,
				status = f.status,
				add = f.add or 0,
				del = f.del or 0,
				rev = f.rev,
				old = f.old,
				stage = (staging and staging[f.path]) or f.stage,
				binary = f.binary,
			})
		end
		return out
	end

	local models: { [string]: any } = {}

	-- The model of session `id`'s checkout.
	function M.get(id: string): any
		if models[id] then
			return models[id]
		end
		local m: any = {
			id = id,
			status = ui.state(`git.{id}.status`, nil :: Status?),
			-- Why the last ask for its status failed (nil while it answers).
			problem = ui.state(`git.{id}.problem`, nil :: Problem?),
			-- Asked at least once (a first answer, or a first failure).
			asked = ui.state(`git.{id}.asked`, false),
			-- What changed: in which scope, from which base; the answer.
			scope = ui.state(`git.{id}.scope`, "working"),
			base = ui.state(`git.{id}.base`, nil :: string?),
			changes = ui.state(`git.{id}.changes`, nil :: any),
			changes_problem = ui.state(`git.{id}.changes_problem`, nil :: Problem?),
			-- Staging asked and not yet read back: path → "all" | "none".
			staging = ui.state(`git.{id}.staging`, {} :: { [string]: string }),
			-- Each file's diff, by the trees it's between and its path.
			-- Which diffs have come (key → how many times one did, a change
			-- to read again by): the diffs themselves are kept in Lua
			-- (`diff_cache`), never in state, where a long one's every line
			-- would be converted and weighed against the plugin's memory on
			-- each write (a few hundred lines went past its 32 MB).
			diffs = ui.state(`git.{id}.diffs`, 0),
			diff_cache = {} :: { [string]: Diff },
			-- What's on its way (commit, push, pull, switch, …) and what
			-- came back no.
			acting = ui.state(`git.{id}.acting`, nil :: string?),
			act_problem = ui.state(`git.{id}.act_problem`, nil :: { what: string, problem: Problem }?),
			-- Discard every change, asked from the tab's ⋯ (Changes asks first).
			discard_ask = ui.state(`git.{id}.discard_ask`, nil :: number?),
			-- The commit's message, kept while the window is.
			message = ui.state(`git.{id}.message`, ""),
			-- History, a page at a time; the branches, as the switcher opens.
			history = ui.state(`git.{id}.history`, nil :: { commits: { Commit }, more: boolean }?),
			history_problem = ui.state(`git.{id}.history_problem`, nil :: Problem?),
			history_head = "",
			-- What History shows: every branch or this one's line, and a
			-- search ("" none), kept with the session.
			history_all = ui.state(`git.{id}.history_all`, false),
			history_query = ui.state(`git.{id}.history_query`, ""),
			-- The commit opened in History ("": none): the History tab's list
			-- and the way back at its band's start both read it.
			history_chosen = ui.state(`git.{id}.history_chosen`, ""),
			history_loading_state = ui.state(`git.{id}.history_loading`, false),
			-- Each commit's whole (body, committer, stats), asked as it opens.
			infos = ui.state(`git.{id}.infos`, {} :: { [string]: any }),
			branches = ui.state(`git.{id}.branches`, nil :: { any }?),
			-- The last word changes were asked for (scope, trees, index).
			seen = "",
			busy = false,
			-- Asked again while an ask was on its way: once more after it.
			again_status = false,
			loading_changes = false,
			-- The views showing it now (each its own key: on screen, in a
			-- window that shows); a change heard while none does is kept
			-- for when one does.
			showing = {} :: { [any]: boolean },
			stale = false,
			listening = false,
			-- When it last answered (ui.now()).
			answered_at = 0,
		}
		models[id] = m

		-- The session's record: its device and its folder.
		local function where(): (string?, string?)
			local s = zeron.sessions()[id]
			if not s then
				return nil, nil
			end
			return s.device, s.cwd
		end
		m.where = where

		function m.call(method: string, args: { [string]: any }?, timeout: number?): any
			local device, cwd = where()
			local a = if args then table.clone(args) else {}
			a.cwd = cwd
			return zeron.service(device):call(method, a, { timeout = timeout or 60 })
		end

		-- The files as the list draws them, staging asked laid over them.
		-- (Not `ui.computed`: a computed belongs to the component it's made
		-- in, and the model outlives every tab that shows it. These are
		-- worked out again only as the answer or the staging change, and
		-- read reactively through the states they read.)
		local memo: { changes: any, staging: any, files: { File }, by_path: { [string]: File } }? = nil
		local function worked(c: any, staging: any): any
			if not memo or memo.changes ~= c or memo.staging ~= staging then
				local files = M.to_files(if c then c.files else nil, staging)
				local map: { [string]: File } = {}
				for _, f in files do
					map[f.path] = f
				end
				memo = { changes = c, staging = staging, files = files, by_path = map }
			end
			return memo
		end
		m.files = {
			get = function(_self: any): { File }
				return worked(m.changes:get(), m.staging:get()).files
			end,
			peek = function(_self: any): { File }
				return worked(m.changes:peek(), m.staging:peek()).files
			end,
		}
		m.by_path = {
			get = function(_self: any): { [string]: File }
				return worked(m.changes:get(), m.staging:get()).by_path
			end,
			peek = function(_self: any): { [string]: File }
				return worked(m.changes:peek(), m.staging:peek()).by_path
			end,
		}

		-- What changed, asked again: in the scope shown, from the trees as
		-- they stand.
		function m.load_changes()
			if m.loading_changes then
				m.again = true
				return
			end
			m.loading_changes = true
			m.again = false
			local scope = m.scope:peek()
			m.call("changes", { scope = scope, base = m.base:peek(), chat = id }, 120):after(function(c: any)
				m.loading_changes = false
				-- (A scope chosen meanwhile asks for its own.)
				if scope == m.scope:peek() then
					m.changes:set(c)
					m.changes_problem:set(nil)
					m.staging:set({})
				end
				if m.again or scope ~= m.scope:peek() then
					m.load_changes()
				end
			end, function(err: any)
				m.loading_changes = false
				m.changes_problem:set(M.problem(err))
				if m.again then
					m.load_changes()
				end
			end)
		end

		-- Its status asked again; the files too when the work tree, the
		-- index, the commit or the scope moved (`force`: whatever moved).
		function m.refresh(force: boolean?)
			if m.busy then
				-- (What moved since it was asked is asked once more after.)
				m.again_status = true
				m.again_force = m.again_force or force
				return
			end
			local _, cwd = where()
			if not cwd then
				m.problem:set({ reason = "failed", code = "no_checkout", words = "" })
				m.asked:set(true)
				return
			end
			m.busy = true
			local function settled()
				m.busy = false
				if m.again_status then
					local again_force = m.again_force
					m.again_status, m.again_force = false, false
					m.refresh(again_force)
				end
			end
			-- (`watch`: its device keeps watching it, and says when it moves.)
			m.call("status", { watch = true }, 90):after(function(s: Status)
				local was = m.status:peek()
				if not was or ui.json.encode(was) ~= ui.json.encode(s) then
					m.status:set(s)
				end
				m.problem:set(nil)
				m.asked:set(true)
				local key = `{m.scope:peek()}|{m.base:peek() or ""}|{s.tree}|{s.index}|{s.oid or ""}|{s.branch or ""}`
				if force or key ~= m.seen or not m.changes:peek() then
					m.seen = key
					m.load_changes()
				end
				if m.history:peek() and (s.oid or "") .. (s.branch or "") ~= m.history_head then
					m.load_history(false)
				end
				m.answered_at = ui.now()
				settled()
			end, function(err: any)
				m.problem:set(M.problem(err))
				m.asked:set(true)
				settled()
			end)
		end

		local function shown(): boolean
			return next(m.showing) ~= nil and zeron.window(true).shown
		end

		-- Its device said the checkout changed: asked again if a view shows
		-- it, else when one does.
		function m.heard()
			if shown() then
				m.refresh()
			else
				m.stale = true
			end
		end

		-- What its device says of it (once per model; its listener lasts
		-- with the window, as the model does).
		local function listen()
			if m.listening then
				return
			end
			m.listening = true
			local device = where()
			zeron.service(device):on("changed", function(data: any)
				local _, cwd = where()
				if cwd and type(data) == "table" and type(data.cwds) == "table" and table.find(data.cwds, cwd) then
					m.heard()
				end
			end)
		end

		-- Asked as its device says it changed while a view shows it
		-- (`shows`: its node is on screen), on showing again if it changed
		-- meanwhile, as the window comes back in front, and once a minute
		-- with nothing heard. Nothing while it's hidden. (Call in a
		-- component: its effect, and the timer it makes, go with it.)
		function m.watch(shows: any)
			listen()
			local key = {}
			local front = zeron.window(true).active
			ui.effect(function()
				local window = zeron.window()
				local on = shows:get() and window.shown
				local now_front = window.active
				local came_front = now_front and not front
				front = now_front
				if not on then
					m.showing[key] = nil
					return
				end
				m.showing[key] = true
				ui.on_cleanup(function()
					m.showing[key] = nil
				end)
				-- (Long unasked, its device's watch may have lapsed too.)
				if m.stale or came_front or not m.asked:peek() or ui.now() - m.answered_at > SAFETY then
					m.stale = false
					m.refresh()
				end
				ui.every(SAFETY, function()
					m.refresh()
				end)
			end)
		end

		function m.set_scope(scope: string, base: string?)
			if m.scope:peek() == scope and m.base:peek() == base then
				return
			end
			m.scope:set(scope)
			m.base:set(base)
			m.changes:set(nil)
			m.changes_problem:set(nil)
			m.seen = ""
			m.load_changes()
		end

		-- A file's diff, between the trees the list was read from (`c`, a
		-- `changes` answer: the one shown, or a commit's): asked the first
		-- time it's wanted, its lines then.
		function m.diff_of(f: File, of: any?): Diff?
			local c = of or m.changes:peek()
			if not c then
				return nil
			end
			-- (Only a file it lists: a list on its way out may still ask.)
			local listed = false
			for _, x in c.files or {} do
				if x.path == f.path then
					listed = true
					break
				end
			end
			if not listed then
				return nil
			end
			local key = `{c.a}..{c.b}:{f.path}`
			-- (Read, so whoever asked is told as it comes.)
			m.diffs:get()
			local known = m.diff_cache[key]
			if known then
				return known
			end
			local function done(d: Diff)
				m.diff_cache[key] = d
				-- (After this run: a state isn't written while it's read.)
				ui.after(0, function()
					m.diffs:set(m.diffs:peek() + 1)
				end)
			end
			-- (Asked after this read, once: a placeholder from then until its
			-- answer. A timer made while a computed reads is dropped if it
			-- runs again first, so the placeholder is set as it fires.)
			ui.after(0, function()
				if m.diff_cache[key] then
					return
				end
				m.diff_cache[key] = {}
				m.call("diff", { a = c.a, b = c.b, path = f.path, old = f.old }, 60):after(function(d: any)
					local lines: { string } = {}
					local started = false
					for line in string.gmatch((d.text or "") .. "\n", "([^\n]*)\n") do
						if not started and string.sub(line, 1, 2) == "@@" then
							started = true
						end
						if started then
							table.insert(lines, line)
						end
					end
					if lines[#lines] == "" then
						table.remove(lines)
					end
					done({ lines = gitds.parse(lines, MAX_LINES), cut = d.cut or #lines > MAX_LINES })
				end, function(err: any)
					done({ problem = M.problem(err) })
				end)
			end)
			return {}
		end

		-- Something done to the checkout: `what` names it while it's on its
		-- way; then everything asked again. A no is kept, said where it was
		-- asked. A promise-like: `done(value)` after it went.
		function m.act(what: string, method: string, args: { [string]: any }?, done: ((value: any) -> ())?, failed: ((p: Problem) -> ())?)
			if m.acting:peek() then
				return
			end
			m.acting:set(what)
			m.act_problem:set(nil)
			m.call(method, args, 300):after(function(value: any)
				m.acting:set(nil)
				if done then
					done(value)
				end
				m.refresh(true)
			end, function(err: any)
				m.acting:set(nil)
				local p = M.problem(err)
				m.act_problem:set({ what = what, problem = p })
				if failed then
					failed(p)
				end
				m.refresh(true)
			end)
		end

		-- Staging: shown at once, read back with the files.
		function m.stage(paths: { string }, on: boolean)
			local now = table.clone(m.staging:peek())
			for _, p in paths do
				now[p] = if on then "all" else "none"
			end
			m.staging:set(now)
			m.call(if on then "stage" else "unstage", { paths = paths }):after(function()
				m.refresh(true)
			end, function(err: any)
				m.staging:set({})
				m.act_problem:set({ what = if on then "stage" else "unstage", problem = M.problem(err) })
			end)
		end
		function m.stage_all(on: boolean)
			local now: { [string]: string } = {}
			for _, f in m.files:peek() do
				if f.status ~= "conflict" then
					now[f.path] = if on then "all" else "none"
				end
			end
			m.staging:set(now)
			m.call(if on then "stage" else "unstage", { all = true }):after(function()
				m.refresh(true)
			end, function(err: any)
				m.staging:set({})
				m.act_problem:set({ what = if on then "stage" else "unstage", problem = M.problem(err) })
			end)
		end

		-- History: the first page again (`more`: the next one after it).
		-- (`asked`: what the last ask was for, so an answer to an older one
		-- — a search typed on — is let go.)
		local asked_for = ""
		function m.load_history(more: boolean)
			local all, query = m.history_all:peek(), m.history_query:peek()
			local what = `{all}|{query}`
			if m.history_loading and what == asked_for and more then
				return
			end
			m.history_loading = true
			m.history_loading_state:set(true)
			asked_for = what
			local have = m.history:peek()
			local skip = if more and have then #have.commits else 0
			local s = m.status:peek()
			m.history_head = if s then (s.oid or "") .. (s.branch or "") else ""
			m.call("history", { skip = skip, limit = 200, all = all, query = if query ~= "" then query else nil }, 60):after(function(h: any)
				if asked_for ~= what then
					return
				end
				m.history_loading = false
				m.history_loading_state:set(false)
				local commits = if more and have then table.clone(have.commits) else {}
				for _, c in h.commits or {} do
					table.insert(commits, c)
				end
				m.history:set({ commits = commits, more = h.more == true, query = query, all = all })
				m.history_problem:set(nil)
			end, function(err: any)
				if asked_for ~= what then
					return
				end
				m.history_loading = false
				m.history_loading_state:set(false)
				m.history_problem:set(M.problem(err))
			end)
		end

		-- A commit whole (its body, committer, stats), asked once.
		function m.commit_info(sha: string)
			if m.infos:peek()[sha] ~= nil then
				return
			end
			local now = table.clone(m.infos:peek())
			now[sha] = false
			m.infos:set(now)
			m.call("commit_info", { sha = sha }, 30):after(function(info: any)
				local next = table.clone(m.infos:peek())
				next[sha] = info
				m.infos:set(next)
			end, function(err: any)
				print(`git: commit {sha}: {err}`)
			end)
		end

		-- A commit's own files (asked once: a commit never changes).
		m.commits = {} :: { [string]: any }
		function m.commit_changes(sha: string, done: (c: any) -> (), failed: (p: Problem) -> ())
			local known = m.commits[sha]
			if known then
				done(known)
				return
			end
			m.call("changes", { scope = "commit", sha = sha }, 60):after(function(c: any)
				m.commits[sha] = c
				done(c)
			end, function(err: any)
				failed(M.problem(err))
			end)
		end

		function m.load_branches()
			m.call("branches", nil, 30):after(function(b: any)
				m.branches:set(b.branches or {})
			end, function(err: any)
				m.act_problem:set({ what = "branches", problem = M.problem(err) })
			end)
		end

		return m
	end

	-- A session's Changes or History, each a tab of its own (kind
	-- "git-changes", "git-history"): the panel's own, if it has one for the
	-- session already (the panel brings it forward), else a new one.
	-- (`scope`: what Changes shows, "working", "branch" or "turn": set on
	-- the session's model first, so a tab brought forward shows it too.)
	M.KINDS = { changes = "git-changes", history = "git-history", commit = "git-commit" }
	function M.open_tab(panel: any, session: string, view: string, scope: string?)
		local history = view == "history"
		if scope == "working" or scope == "branch" or scope == "turn" then
			M.get(session).set_scope(scope, nil)
		end
		panel.open_tab {
			kind = if history then M.KINDS.history else M.KINDS.changes,
			title = zeron.s(if history then "tab.history" else "tab.changes"),
			icon = if history then "git-commit" else "file-diff",
			data = { session = session },
		}
	end

	-- A commit in a tab of its own, as Zeron's commit diff tab: titled by
	-- its subject (its short id with none), the one already open for it
	-- brought forward (`data.key`: the panel finds it by its id, not its
	-- title, which two commits may share).
	function M.open_commit(panel: any, session: string, c: any)
		local subject = if type(c.subject) == "string" then string.match(c.subject, "^%s*(.-)%s*$") or "" else ""
		panel.open_tab {
			kind = M.KINDS.commit,
			title = if subject ~= "" then subject else string.sub(c.sha, 1, 7),
			icon = "git-commit",
			data = {
				session = session,
				key = `commit:{c.sha}`,
				commit = {
					sha = c.sha,
					subject = c.subject or "",
					parents = c.parents or {},
					author = c.author,
					email = c.email,
					at = c.at,
					refs = c.refs or {},
					remotes = c.remotes,
				},
			},
		}
	end

	return M
end
