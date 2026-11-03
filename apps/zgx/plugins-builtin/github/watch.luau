--!strict
-- Each session's pull request, for the sidebar's badges, the hover card and
-- the notifications: the pull request of the branch the session works on,
-- from GitHub through gh, kept up to date by the plugin itself.
--
-- A session's repository is its folder's on this Mac (or its project's
-- folder's), found once and kept. A session whose folder isn't on this Mac
-- is looked for on GitHub by its branch, among pull requests that involve
-- you. Then every two minutes (Zeron's own pace), one GraphQL ask a repository covers every
-- session's branch in it (aliases), and an unchanged answer changes
-- nothing. What was last heard is kept, so badges show at launch.
--
--   watch.pr(session_id)   in a build function: a function reading that
--                          session's pull request as it changes (nil: none)
--   watch.sessions()       every session's, by id (`{ [id] = pr | false }`)

local SECONDS = 2 * 60
-- After a failed ask, the next waits longer each time (20 s, doubling, at
-- most 15 minutes), as Zeron's own watcher does.
local BACKOFF_FIRST, BACKOFF_MAX = 20, 15 * 60

-- What a badge and a notification read of a pull request.
local FIELDS = [[number title url state isDraft reviewDecision baseRefName headRefName isCrossRepository
  reviewRequests(first: 20) { nodes { requestedReviewer { ... on User { login } } } }
  commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }]]

return function(zeron: any, pulls: any)
	local M = {}
	-- Each session's pull request (or false: none), as last heard.
	local heard = ui.state("github_watch", {} :: { [string]: any })
	-- Each folder's repository (or false: none), and each remote session's.
	local repos: { [string]: any } = {}
	local viewer: string? = nil

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

	local failures = 0
	local resume_at = 0
	local function failed_ask()
		failures += 1
		resume_at = os.time() + math.min(BACKOFF_MAX, BACKOFF_FIRST * 2 ^ (failures - 1))
	end

	local function publish(changes: { [string]: any })
		failures = 0
		local now = table.clone(heard:peek())
		local moved = false
		for id, pr in changes do
			if ui.json.encode(now[id] or false) ~= ui.json.encode(pr) then
				now[id] = pr
				moved = true
			end
		end
		if moved then
			heard:set(now)
			pulls.keep("watch", now)
		end
	end

	-- The branch a session works on: the one its latest run started on,
	-- else the one it was made on.
	local function branch_of(s: any): string?
		local b = s.source_branch or s.branch
		return if type(b) == "string" and b ~= "" then b else nil
	end

	-- One ask for every branch of `repo`: each session's pull request.
	local function ask_repo(repo: string, sessions: { { id: string, branch: string } })
		local owner, name = string.match(repo, "^([^/]+)/([^/]+)$")
		if not owner then
			return
		end
		local vars: { [string]: any } = { owner = owner, name = name }
		local defs = { "$owner: String!", "$name: String!" }
		local parts = {}
		for i, s in sessions do
			vars[`b{i}`] = s.branch
			table.insert(defs, `$b{i}: String!`)
			table.insert(parts, `b{i}: pullRequests(headRefName: $b{i}, first: 10, orderBy: \{ field: UPDATED_AT, direction: DESC \}, states: [OPEN, MERGED, CLOSED]) \{ nodes \{ {FIELDS} \} \}`)
		end
		local query = `query({table.concat(defs, ", ")}) \{ viewer \{ login \} repository(owner: $owner, name: $name) \{ {table.concat(parts, " ")} \} \}`
		pulls.graphql(query, vars):after(function(data: any)
			viewer = data.viewer and data.viewer.login or viewer
			local repository = data.repository or {}
			local changes: { [string]: any } = {}
			for i, s in sessions do
				-- Its own branch's: one opened from a fork's branch of the same
				-- name (anyone's `main`) is someone else's.
				local found = nil
				for _, pr in pulls.nodes(repository[`b{i}`]) do
					if not pr.isCrossRepository then
						found = pr
						break
					end
				end
				changes[s.id] = if found then record(found) else false
			end
			publish(changes)
		end, failed_ask)
	end

	-- A session whose folder isn't here: its branch's pull request among
	-- those that involve you, the repository named as its project first.
	local function search(id: string, branch: string, project: string?)
		local query = `query($q: String!) \{ viewer \{ login \} search(query: $q, type: ISSUE, first: 10) \{ nodes \{ ... on PullRequest \{ repository \{ nameWithOwner name \} {FIELDS} \} \} \} \}`
		pulls.graphql(query, { q = `is:pr head:{branch} involves:@me sort:updated-desc` }):after(function(data: any)
			viewer = data.viewer and data.viewer.login or viewer
			local pick = nil
			for _, pr in pulls.nodes(data.search) do
				if pr.headRefName == branch and not pr.isCrossRepository then
					local named = project and pr.repository and string.lower(pr.repository.name or "") == string.lower(project)
					if not pick or named then
						pick = pr
					end
					if named then
						break
					end
				end
			end
			if pick and pick.repository then
				-- (Its repository now known: asked with the rest from here on.)
				repos[`session:{id}`] = pick.repository.nameWithOwner
			end
			publish({ [id] = if pick then record(pick) else false })
		end, failed_ask)
	end

	-- When each session not found on this Mac was last looked for on
	-- GitHub (not again for a while if it had none).
	local searched: { [string]: number } = {}

	-- Every session's pull request, asked again.
	local busy = false
	-- (Not more than once in 20 seconds unless asked: sessions coming and
	-- going in a burst ask GitHub once.)
	local last_poll = 0
	local function poll(force: boolean?)
		if busy or (not force and (os.time() - last_poll < 20 or os.time() < resume_at)) then
			return
		end
		busy = true
		last_poll = os.time()
		local by_repo: { [string]: { { id: string, branch: string } } } = {}
		local function add(repo: string, id: string, branch: string)
			by_repo[repo] = by_repo[repo] or {}
			table.insert(by_repo[repo], { id = id, branch = branch })
		end
		-- (Each session's lookup counted out; when the last is in, each
		-- repository is asked once.)
		local waiting = 1
		local function done()
			waiting -= 1
			if waiting == 0 then
				for repo, list in by_repo do
					ask_repo(repo, list)
				end
				busy = false
			end
		end
		for id, s in zeron.sessions() do
			local branch = branch_of(s)
			if branch then
				local known = repos[`session:{id}`]
				if known then
					add(known, id, branch)
				else
					local project = if s.project then zeron.context_of("project", s.project) else nil
					-- Its folder on this Mac, then its project's; else GitHub,
					-- by its branch.
					local folders: { string } = {}
					for _, f in { s.cwd, project and project.path } do
						if type(f) == "string" and f ~= "" then
							table.insert(folders, f)
						end
					end
					waiting += 1
					local function lookup(i: number)
						local folder = folders[i]
						if not folder then
							if force or os.time() - (searched[id] or 0) >= 5 * SECONDS then
								searched[id] = os.time()
								search(id, branch, project and project.name)
							end
							done()
						elseif repos[folder] == false then
							lookup(i + 1)
						else
							pulls.repository(folder):after(function(repo: string)
								repos[folder] = repo
								repos[`session:{id}`] = repo
								add(repo, id, branch)
								done()
							end, function()
								repos[folder] = false
								lookup(i + 1)
							end)
						end
					end
					lookup(1)
				end
			end
		end
		done()
	end

	-- What was heard last time, at once; then GitHub, now and every minute,
	-- and again soon after a session comes or its branch moves.
	pulls.cached("watch", function(value: any?)
		if type(value) == "table" and next(heard:peek()) == nil then
			heard:set(value)
		end
	end)
	zeron.every(SECONDS, function()
		poll(false)
	end)
	-- Asked again now (Zeron's menu bar: GitHub › Refresh pull requests).
	ui.command {
		id = "refresh",
		title = zeron.s("command.refresh"),
		run = function()
			poll(true)
		end,
	}
	local shape = ""
	ui.effect(function()
		local parts = {}
		for id in zeron.sessions() do
			local s = zeron.context_of("session", id) or {}
			table.insert(parts, `{id}:{branch_of(s) or ""}`)
		end
		table.sort(parts)
		local now = table.concat(parts, ",")
		if now ~= shape then
			shape = now
			-- (Once the last ask is 20 seconds old, so a change isn't missed.)
			ui.after(math.max(500, (20 - (os.time() - last_poll)) * 1000 + 100), function()
				poll(false)
			end)
		end
	end)

	function M.pr(session: string): () -> any
		return function(): any
			local pr = heard:get()[session]
			return if pr == false then nil else pr
		end
	end

	function M.sessions(): { [string]: any }
		return heard:get()
	end

	return M
end
