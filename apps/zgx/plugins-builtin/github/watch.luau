--!strict
-- Each session's pull request, for the sidebar's badges, the hover card and
-- the notifications: the pull request of the branch the session works on,
-- from GitHub through gh, kept up to date by the plugin itself.
--
-- A session's repository is its folder's on this Mac (or its project's
-- folder's), found once and kept, and its pull request is that
-- repository's own from the session's branch: never one found by its
-- branch's name alone, since every repository has a `main`. A session whose
-- repository can't be told here (its folder elsewhere, no GitHub remote)
-- shows none from gh; the engine's watch, on the session's own device, may
-- still tell its. What was last heard is kept, so badges show at launch.
--
-- How often a session is asked depends on how much it matters now: one on
-- screen (its row, its card, its panel) or working, every half minute;
-- one touched in the last fortnight, every twelve minutes; older ones out
-- of sight, never. A repository asked for one of its sessions answers for
-- all of them, and one GraphQL ask covers many repositories (aliases), so
-- five hundred repositories are a handful of asks, not five hundred. As
-- GitHub's quota for the hour runs low (`rateLimit`, in every answer),
-- everything is asked less often; near its end, nothing until it resets.
-- A pull request the engine already watches for a session on screen
-- (`zeron.github.watch_pr`, Zeron's own sign-in) comes from there, pushed;
-- gh asks of it only at the slow pace.
--
--   watch.pr(session_id)   in a build function: a function reading that
--                          session's pull request as it changes (nil: none),
--                          the session asked often while the build stands.
--                          It follows that session alone: another's moving
--                          tells it nothing.
--   watch.on_change(fn)    `fn(session_id)` as a session's is heard to move
--   watch.current(id)      that session's as it stands (nil: none), unread

-- The watch's beat (seconds): what's due is asked on it. Between asks, in
-- beats: a session on screen or working; one touched lately; how lately
-- (days).
local BEAT = 30
local HOT = 1
local WARM = 24
local RECENT_DAYS = 14
-- One ask covers this many repositories, and branches, at most.
local BATCH_REPOS, BATCH_BRANCHES = 50, 150
-- Repositories looked up (`gh repo view`) per round, at most: the rest the
-- next round, a moment later.
local LOOKUPS = 8
-- After a failed ask, the next waits longer each time (a beat, doubling,
-- at most 15 minutes), as Zeron's own watcher does.
local BACKOFF_FIRST, BACKOFF_MAX = 1, 30
-- Where what was last heard is kept. (`watch` was written while a session
-- whose repository wasn't known took any pull request of yours from a
-- branch of its name, in any repository: not read again.)
local KEPT = "watch.v2"

-- What a badge and a notification read of a pull request.
local FIELDS = [[number title url state isDraft reviewDecision baseRefName headRefName isCrossRepository
  reviewRequests(first: 10) { nodes { requestedReviewer { ... on User { login } } } }
  commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }]]

-- Seconds since 1970 of GitHub's "2026-11-02T18:04:05Z" (UTC).
local function epoch(iso: string?): number?
	local y, mo, d, h, mi, s = string.match(iso or "", "^(%d+)-(%d+)-(%d+)T(%d+):(%d+):(%d+)")
	if not y then
		return nil
	end
	local year, month, day = tonumber(y) :: number, tonumber(mo) :: number, tonumber(d) :: number
	-- (Days from the civil date: Howard Hinnant's.)
	year -= if month <= 2 then 1 else 0
	local era = (if year >= 0 then year else year - 399) // 400
	local yoe = year - era * 400
	local doy = (153 * (month + (if month > 2 then -3 else 9)) + 2) // 5 + day - 1
	local doe = yoe * 365 + yoe // 4 - yoe // 100 + doy
	local days = era * 146097 + doe - 719468
	return days * 86400 + (tonumber(h) :: number) * 3600 + (tonumber(mi) :: number) * 60 + (tonumber(s) :: number)
end

return function(zeron: any, pulls: any)
	local M = {}
	-- Each session's pull request (or false: none) as gh last told it: in
	-- plain Lua for the watch's own work, and for what draws it in a state
	-- of each session's own (`cell`), so one moving tells its own badge and
	-- nothing else.
	local known: { [string]: any } = {}
	local cells: { [string]: any } = {}
	local function cell(id: string): any
		local c = cells[id]
		if not c then
			c = ui.state(`github_pr.{id}`, if known[id] == nil then false else known[id])
			cells[id] = c
		end
		return c
	end
	local listeners: { (id: string) -> () } = {}
	-- Each folder's repository (or false: none), and each session's.
	local repos: { [string]: any } = {}
	local viewer: string? = nil
	-- What shows each session now (its badge, its card, its panel).
	local shown: { [string]: number } = {}
	-- The beat it is, and when each session was last asked of GitHub.
	local beat = 0
	local asked_at: { [string]: number } = {}

	-- GitHub's pull request as a badge reads it.
	local function record(pr: any): any
		local state = string.lower(pr.state or "open")
		if state == "open" and pr.isDraft then
			state = "draft"
		end
		local rollup = nil
		for _, n in pulls.nodes(pr.commits) do
			rollup = n.commit and n.commit.statusCheckRollup
		end
		local asked = false
		for _, r in pulls.nodes(pr.reviewRequests) do
			if viewer and r.requestedReviewer and r.requestedReviewer.login == viewer then
				asked = true
			end
		end
		local review = string.upper(pr.reviewDecision or "")
		return {
			number = pr.number,
			title = pr.title,
			url = pr.url,
			state = state,
			base = pr.baseRefName,
			head = pr.headRefName,
			review = if review == "APPROVED" then "approved" elseif review == "CHANGES_REQUESTED" then "changesRequested" elseif review == "REVIEW_REQUIRED" then "reviewRequired" else nil,
			requested_of_you = asked,
			ci = pulls.ci_of(rollup and rollup.state),
		}
	end

	-- The engine's pull request (github.rs `summary`) as a badge reads it;
	-- a draft as gh last said, the engine not saying.
	local function from_engine(e: any, mine: any?): any
		local state = e.state
		if state == "open" and type(mine) == "table" and mine.number == e.number and mine.state == "draft" then
			state = "draft"
		end
		local ci = if e.ci == "passed" or e.ci == "failed" or e.ci == "pending" then e.ci else nil
		local review = if e.review == "approved" or e.review == "changesRequested" or e.review == "reviewRequired" then e.review else nil
		return {
			number = e.number,
			title = e.title,
			url = e.url,
			state = state,
			base = e.base,
			head = e.head,
			review = review,
			requested_of_you = e.requested_of_you == true,
			ci = ci,
		}
	end

	local failures = 0
	local resume_at = 0
	local function failed_ask()
		failures += 1
		resume_at = beat + math.min(BACKOFF_MAX, BACKOFF_FIRST * 2 ^ (failures - 1))
	end

	-- GitHub's quota for the hour, as its last answer said: how much
	-- slower to ask (1: as usual), and until when nothing is asked.
	local quota = { remaining = 5000, limit = 5000, reset = 0 }
	local function slower(): number
		local share = quota.remaining / math.max(1, quota.limit)
		if share < 0.1 then
			return 8
		elseif share < 0.25 then
			return 4
		elseif share < 0.5 then
			return 2
		end
		return 1
	end
	local function heard_quota(rate: any)
		if type(rate) ~= "table" or type(rate.remaining) ~= "number" then
			return
		end
		quota.remaining, quota.limit = rate.remaining, (rate.limit or quota.limit)
		quota.reset = epoch(rate.resetAt) or quota.reset
		-- (Near its end: nothing until it resets, a few left for what you
		-- open yourself.)
		if quota.remaining < math.max(20, quota.limit * 0.02) and quota.reset > os.time() then
			resume_at = math.max(resume_at, beat + math.ceil((quota.reset - os.time()) / BEAT))
		end
	end

	local function publish(changes: { [string]: any })
		local moved: { string } = {}
		for id, pr in changes do
			-- (Never heard counts as moved: a session's state outlives a
			-- reload, holding what was shown before it.)
			if known[id] == nil or ui.json.encode(known[id]) ~= ui.json.encode(pr) then
				known[id] = pr
				if cells[id] then
					cells[id]:set(pr)
				end
				table.insert(moved, id)
			end
		end
		if #moved > 0 then
			pulls.keep(KEPT, known)
			for _, id in moved do
				for _, listen in listeners do
					local ok, why = pcall(listen, id)
					if not ok then
						print(`github: a pull request's listener failed: {why}`)
					end
				end
			end
		end
	end

	-- The branch a session works on: the one its latest run started on,
	-- else the one it was made on.
	local function branch_of(s: any): string?
		local b = s.source_branch or s.branch
		return if type(b) == "string" and b ~= "" then b else nil
	end

	-- What the engine told of session `id` (a table), if anything.
	local function engine_of(id: string): any
		local ok, all = pcall(zeron.github.heard)
		local e = ok and type(all) == "table" and all[id] or nil
		return if type(e) == "table" then e else nil
	end
	-- Whether the engine watches session `id` and has told its pull request
	-- (it shows: the engine watches what shows).
	local function engine_has(id: string): boolean
		return (shown[id] or 0) > 0 and engine_of(id) ~= nil
	end

	-- How often session `id` is asked (in beats), or nil: not at all.
	local function every(id: string, s: any, now: number): number?
		local on_screen = (shown[id] or 0) > 0
		local working = s.status == "working" or s.status == "input"
		-- (The engine watching it pushes it: gh only now and then.)
		if (on_screen or working) and not engine_has(id) then
			return HOT * (if slower() >= 4 then slower() / 2 else 1)
		end
		local recent = type(s.updated) == "number" and now - s.updated < RECENT_DAYS * 86400
		if recent or on_screen then
			return WARM * slower()
		end
		return nil
	end

	type Ask = { id: string, branch: string }

	-- One ask for many repositories' branches: each session's pull request.
	local function ask(batch: { { repo: string, sessions: { Ask } } }, done: () -> ())
		local vars: { [string]: any } = {}
		local defs: { string } = {}
		local parts: { string } = {}
		for r, entry in batch do
			local owner, name = string.match(entry.repo, "^([^/]+)/([^/]+)$")
			vars[`o{r}`], vars[`n{r}`] = owner, name
			table.insert(defs, `$o{r}: String!`)
			table.insert(defs, `$n{r}: String!`)
			local branches = {}
			for k, s in entry.sessions do
				vars[`b{r}_{k}`] = s.branch
				table.insert(defs, `$b{r}_{k}: String!`)
				table.insert(branches, `b{r}_{k}: pullRequests(headRefName: $b{r}_{k}, first: 5, orderBy: \{ field: UPDATED_AT, direction: DESC \}, states: [OPEN, MERGED, CLOSED]) \{ nodes \{ {FIELDS} \} \}`)
			end
			table.insert(parts, `r{r}: repository(owner: $o{r}, name: $n{r}) \{ {table.concat(branches, " ")} \}`)
		end
		local query = `query({table.concat(defs, ", ")}) \{ viewer \{ login \} rateLimit \{ cost remaining limit resetAt \} {table.concat(parts, " ")} \}`
		pulls.graphql(query, vars, { background = true }):after(function(data: any)
			failures = 0
			viewer = data.viewer and data.viewer.login or viewer
			heard_quota(data.rateLimit)
			local changes: { [string]: any } = {}
			for r, entry in batch do
				local repository = data[`r{r}`] or {}
				for k, s in entry.sessions do
					-- Its own branch's: one opened from a fork's branch of the
					-- same name (anyone's `main`) is someone else's.
					local found = nil
					for _, pr in pulls.nodes(repository[`b{r}_{k}`]) do
						if not pr.isCrossRepository and pr.headRefName == s.branch then
							found = pr
							break
						end
					end
					-- (gh's own answer: what the engine tells is laid over it
					-- where it's read, so the two never take turns.)
					changes[s.id] = if found then record(found) else false
				end
			end
			publish(changes)
			done()
		end, function()
			failed_ask()
			done()
		end)
	end

	-- The rounds: each asks what's due, on the beat and as something new
	-- shows.
	local round: () -> ()
	-- (A burst of rows mounting, sessions arriving: one round after it.)
	local soon_armed = false
	local function soon()
		if not soon_armed then
			soon_armed = true
			ui.after(500, function()
				soon_armed = false
				round()
			end)
		end
	end

	-- Session `id`'s repository, from its folder on this Mac or its
	-- project's: "known" (now kept), "looking" (asked, or `start` false:
	-- to ask later), or "none" here.
	local looking: { [string]: boolean } = {}
	local function lookup(id: string, s: any, start: boolean): string
		local project = if s.project then zeron.context_of("project", s.project) else nil
		for _, f in { s.cwd, project and project.path } do
			if type(f) == "string" and f ~= "" and repos[f] ~= false then
				if repos[f] then
					repos[`session:{id}`] = repos[f]
					return "known"
				end
				if not looking[f] and start then
					looking[f] = true
					pulls.repository(f):after(function(repo: string)
						looking[f] = nil
						repos[f] = repo
						soon()
					end, function()
						looking[f] = nil
						repos[f] = false
						soon()
					end)
				end
				return "looking"
			end
		end
		return "none"
	end

	local busy = false
	function round()
		if busy then
			return
		end
		local now = os.time()
		if beat < resume_at then
			return
		end
		-- Each repository's sessions worth asking, and which repositories
		-- are due (a session of theirs is).
		local by_repo: { [string]: { Ask } } = {}
		-- (Each due repository's shortest period: while repositories are
		-- still being found, only the often-asked go, the rest wait to go
		-- together.)
		local due: { [string]: number } = {}
		local lookups = 0
		local later = false
		-- (Sessions whose repository isn't known here.)
		local unknown: { [string]: any } = {}
		for id, s in zeron.sessions() do
			local branch = branch_of(s)
			local period = branch and every(id, s, now)
			if not branch or not period then
				continue
			end
			local at = (asked_at[id] or -math.huge) + period
			local repo = repos[`session:{id}`]
			if not repo then
				local was = lookups
				local found = lookup(id, s, lookups < LOOKUPS)
				if found == "looking" then
					if lookups == was then
						later = later or lookups >= LOOKUPS
					end
					lookups += if lookups < LOOKUPS then 1 else 0
					continue
				end
				repo = repos[`session:{id}`]
			end
			if repo then
				-- (What the engine tells isn't asked of gh too, unless it's
				-- due itself.)
				if at <= beat or not engine_has(id) then
					by_repo[repo] = by_repo[repo] or {}
					table.insert(by_repo[repo], { id = id, branch = branch :: string })
				end
				if at <= beat then
					due[repo] = math.min(due[repo] or math.huge, period)
				end
			elseif known[id] ~= false then
				-- No repository to tell here: none from gh, rather than a
				-- guess from its branch's name.
				unknown[id] = false
			end
		end
		if next(unknown) then
			publish(unknown)
		end
		-- The due repositories, a batch at a time.
		local batches: { { { repo: string, sessions: { Ask } } } } = {}
		local batch: { { repo: string, sessions: { Ask } } } = {}
		local branches = 0
		local list: { string } = {}
		local settling = later or next(looking) ~= nil
		for repo, period in due do
			if not settling or period < WARM then
				table.insert(list, repo)
			end
		end
		table.sort(list)
		for _, repo in list do
			local sessions = by_repo[repo]
			if #batch >= BATCH_REPOS or (branches + #sessions > BATCH_BRANCHES and #batch > 0) then
				table.insert(batches, batch)
				batch, branches = {}, 0
			end
			table.insert(batch, { repo = repo, sessions = sessions })
			branches += #sessions
			for _, s in sessions do
				asked_at[s.id] = beat
			end
		end
		if #batch > 0 then
			table.insert(batches, batch)
		end
		if #batches == 0 then
			-- (Lookups on their way start a round as they come; those left
			-- for later, a moment from now.)
			if later and next(looking) == nil then
				soon()
			end
			return
		end
		busy = true
		local left = #batches
		for _, b in batches do
			ask(b, function()
				left -= 1
				if left == 0 then
					busy = false
					round()
				end
			end)
		end
	end

	-- What was heard last time, at once; then GitHub.
	pulls.cached(KEPT, function(value: any?)
		if type(value) == "table" and next(known) == nil then
			publish(value)
		end
	end)
	soon()
	ui.every(BEAT * 1000, function()
		beat += 1
		round()
	end)
	-- Asked again now (Zeron's menu bar: GitHub › Refresh pull requests).
	ui.command {
		id = "refresh",
		title = zeron.s("command.refresh"),
		run = function()
			table.clear(asked_at)
			resume_at = 0
			round()
		end,
	}
	-- A session arriving, or its branch moving, is asked soon.
	local shape: { [string]: string } = {}
	ui.effect(function()
		local moved = false
		for id in zeron.sessions() do
			local s = zeron.context_of("session", id) or {}
			local b = `{branch_of(s) or ""}|{s.status or ""}`
			if shape[id] ~= b then
				shape[id] = b
				asked_at[id] = nil
				moved = true
			end
		end
		if moved then
			soon()
		end
	end)

	function M.pr(session: string): () -> any
		-- (On screen while this build stands: asked often; seen at once
		-- if it hadn't been lately. Plain counts: nothing is written.)
		shown[session] = (shown[session] or 0) + 1
		local first = shown[session] == 1
		pcall(ui.on_cleanup, function()
			local n = (shown[session] or 1) - 1
			shown[session] = if n > 0 then n else nil
		end)
		if first and beat - (asked_at[session] or -math.huge) >= HOT then
			soon()
		end
		-- Its own state, and what the engine says of it alone (where
		-- Zeron's sign-in lets it watch, "github": pushed as it moves). The
		-- engine's stands; gh adds a draft, which the engine doesn't say.
		local mine = cell(session)
		local ok, theirs = pcall(zeron.github.watch_pr, session)
		local engine: (() -> any)? = if ok and type(theirs) == "function" then theirs :: any else nil
		return function(): any
			local m = mine:get()
			local e = engine and engine()
			if e then
				return from_engine(e, m)
			end
			return if m == false then nil else m
		end
	end

	function M.on_change(listen: (id: string) -> ())
		table.insert(listeners, listen)
	end

	function M.current(id: string): any
		local e = engine_of(id)
		if e then
			return from_engine(e, known[id])
		end
		return if known[id] then known[id] else nil
	end

	-- (For the tests: how round would ask, and the quota as heard.)
	M.epoch = epoch
	M.slower = slower
	M.quota = quota

	return M
end
