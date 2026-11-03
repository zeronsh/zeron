--!strict
-- Pull requests, from GitHub's own API through gh (gh.luau): one pull
-- request whole (what the viewer shows), its diff, what you say on it
-- (comments, line comments, replies, resolving, editing and deleting your
-- own), a repository's open pull requests (the board), and which repository
-- a folder on this Mac is.
--
-- What's read is kept (the plugin's database): a pull request shows at once
-- from what was kept, then as GitHub has it now; a diff between two commits
-- never changes, so it's asked once.

local Gh = require("./gh")

-- A pull request as GraphQL gives it, whole.
local DETAIL = [[
query($owner: String!, $name: String!, $number: Int!) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed deleteBranchOnMerge viewerPermission
    pullRequest(number: $number) {
      id number title body url state isDraft createdAt reviewDecision additions deletions
      mergeable mergeStateStatus merged mergedAt mergedBy { login } closed closedAt
      isCrossRepository viewerDidAuthor allCommits: commits { totalCount }
      author { login avatarUrl(size: 96) } baseRefName headRefName headRefOid baseRefOid
      files(first: 100) { nodes { path additions deletions changeType } }
      comments(first: 100) { nodes { id author { login avatarUrl(size: 96) } body createdAt url viewerDidAuthor } }
      reviews(first: 100) { nodes { author { login avatarUrl(size: 96) } body state submittedAt } }
      reviewThreads(first: 100) {
        nodes {
          id path line originalLine diffSide isResolved isOutdated
          comments(first: 100) { nodes { id author { login avatarUrl(size: 96) } body createdAt url replyTo { id } viewerDidAuthor } }
        }
      }
      commits(last: 1) {
        nodes { commit { statusCheckRollup { state contexts(first: 100) { nodes {
          __typename
          ... on CheckRun { name status conclusion detailsUrl }
          ... on StatusContext { context state targetUrl }
        } } } } }
      }
    }
  }
}]]

-- A repository's pull requests, as a search finds them.
local LIST = [[
query($q: String!, $after: String) {
  search(query: $q, type: ISSUE, first: 30, after: $after) {
    issueCount
    pageInfo { hasNextPage endCursor }
    nodes { ... on PullRequest {
      number title url isDraft reviewDecision mergeable additions deletions
      repository { nameWithOwner } author { login avatarUrl(size: 96) }
      commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
    } }
  }
}]]

local FILE_STATUS: { [string]: string } = { ADDED = "added", DELETED = "deleted", RENAMED = "renamed", COPIED = "added" }

-- Where checks stand, in one word (the board's and the badge's).
local function ci_of(state: string?): string?
	local s = string.upper(state or "")
	if s == "SUCCESS" then
		return "passed"
	elseif s == "FAILURE" or s == "ERROR" then
		return "failed"
	elseif s == "PENDING" or s == "EXPECTED" then
		return "pending"
	end
	return nil
end

local function nodes(of: any): { any }
	return if type(of) == "table" and type(of.nodes) == "table" then of.nodes else {}
end

-- A pull request's owner, repository and number, from its address.
local function parse(url: string): (string?, string?, number?)
	local owner, name, number = string.match(url or "", "^https://github%.com/([%w%-%._]+)/([%w%-%._]+)/pull/(%d+)")
	return owner, name, tonumber(number)
end

-- GitHub's answer as the viewer reads a pull request.
local function shape(pr: any, viewer: string?, repository: any?): any
	-- The repository's ways in, and what you may do there.
	local r = repository or {}
	local repo = {
		squash = r.squashMergeAllowed ~= false,
		merge = r.mergeCommitAllowed ~= false,
		rebase = r.rebaseMergeAllowed ~= false,
		deleteOnMerge = r.deleteBranchOnMerge == true,
		permission = r.viewerPermission,
	}
	local rollup = nil
	for _, n in nodes(pr.commits) do
		rollup = n.commit and n.commit.statusCheckRollup
	end
	local checks: { any } = {}
	for _, c in nodes(rollup and rollup.contexts) do
		if c.__typename == "StatusContext" then
			table.insert(checks, { name = c.context, context = c.context, state = c.state, targetUrl = c.targetUrl })
		else
			table.insert(checks, { name = c.name, status = c.status, conclusion = c.conclusion, detailsUrl = c.detailsUrl })
		end
	end
	local files: { any } = {}
	for _, f in nodes(pr.files) do
		table.insert(files, { path = f.path, additions = f.additions, deletions = f.deletions, status = FILE_STATUS[f.changeType or ""] or "modified" })
	end
	local threads: { any } = {}
	for _, t in nodes(pr.reviewThreads) do
		table.insert(threads, {
			id = t.id,
			path = t.path,
			line = t.line,
			originalLine = t.originalLine,
			diffSide = t.diffSide,
			isResolved = t.isResolved,
			isOutdated = t.isOutdated,
			comments = nodes(t.comments),
		})
	end
	local ci = ci_of(rollup and rollup.state)
	return {
		id = pr.id,
		title = pr.title or "",
		body = pr.body or "",
		url = pr.url,
		number = pr.number,
		author = pr.author,
		baseRefName = pr.baseRefName,
		headRefName = pr.headRefName,
		state = pr.state,
		isDraft = pr.isDraft,
		createdAt = pr.createdAt,
		reviewDecision = pr.reviewDecision,
		additions = pr.additions,
		deletions = pr.deletions,
		headRefOid = pr.headRefOid,
		baseRefOid = pr.baseRefOid,
		ci = { state = if #checks == 0 and rollup == nil then "noChecks" else ci, totalCount = #checks },
		statusCheckRollup = checks,
		files = files,
		comments = nodes(pr.comments),
		reviews = nodes(pr.reviews),
		reviewThreads = threads,
		viewerLogin = viewer,
		-- Whether and how it can go in (the merge section's).
		mergeable = pr.mergeable,
		mergeStateStatus = pr.mergeStateStatus,
		merged = pr.merged,
		mergedAt = pr.mergedAt,
		mergedBy = pr.mergedBy,
		closedAt = pr.closedAt,
		isCrossRepository = pr.isCrossRepository,
		viewerDidAuthor = pr.viewerDidAuthor,
		commitCount = pr.allCommits and pr.allCommits.totalCount,
		repo = repo,
	}
end

-- A search's pull request as a row of the board.
local function item(pr: any): any
	local state = nil
	for _, n in nodes(pr.commits) do
		state = n.commit and n.commit.statusCheckRollup and n.commit.statusCheckRollup.state
	end
	local review = string.upper(pr.reviewDecision or "")
	return {
		number = pr.number,
		title = pr.title,
		url = pr.url,
		repository = pr.repository and pr.repository.nameWithOwner,
		author = pr.author,
		isDraft = pr.isDraft,
		reviewDecision = if review == "APPROVED" then "approved" elseif review == "CHANGES_REQUESTED" then "changesRequested" else "unknown",
		mergeability = if pr.mergeable == "CONFLICTING" then "conflicting" else "mergeable",
		ci = { state = ci_of(state) },
		additions = pr.additions,
		deletions = pr.deletions,
	}
end

-- A path in an address: each part as an address keeps it.
local function encoded(path: string): string
	return (string.gsub(path, "[^%w%-%._~/]", function(c: string): string
		return string.format("%%%02X", string.byte(c))
	end))
end

return function(zeron: any)
	local gh = Gh(zeron)
	local M = {}
	M.task = gh.task
	M.parse = parse

	local function failed(why: string): Gh.Task
		return gh.task(function(_resolve, reject)
			reject(why)
		end)
	end

	-- The pull request at `url`, as GitHub has it now. Unless `refresh`,
	-- what was kept is told first (`kept`), so it shows at once.
	-- How long what was kept stands as GitHub's word without asking again
	-- (Zeron's own: five minutes).
	local FRESH_SECONDS = 5 * 60

	function M.pr(url: string, refresh: boolean?, kept: ((d: any) -> ())?): Gh.Task
		local owner, name, number = parse(url)
		if not owner then
			return failed(zeron.s("pulls.not_pr"))
		end
		local key = `pr:{owner}/{name}#{number}`
		return gh.task(function(resolve, reject)
			local function ask()
				gh.graphql(DETAIL, { owner = owner, name = name, number = number }):after(function(data: any)
					local pr = data.repository and data.repository.pullRequest
					if not pr then
						reject(zeron.s("pulls.not_found"))
						return
					end
					local d = shape(pr, data.viewer and data.viewer.login, data.repository)
					gh.keep(key, d)
					resolve(d)
				end, reject)
			end
			if refresh or not kept then
				ask()
				return
			end
			-- What was kept shows at once; GitHub is asked again only once
			-- it's five minutes old (its rate limits are tight: opening the
			-- same pull request again and again asks nothing).
			gh.cached(key, function(value: any?, at: number?)
				if value then
					(kept :: (d: any) -> ())(value)
					if at and os.time() - at < FRESH_SECONDS then
						resolve(value)
						return
					end
				end
				ask()
			end)
		end)
	end

	-- Its diff, unified, between `head` and `base` (asked once a pair).
	function M.diff(url: string, at: { head: string?, base: string? }): Gh.Task
		local owner, name, number = parse(url)
		if not owner then
			return failed(zeron.s("pulls.not_pr"))
		end
		local key = `diff:{owner}/{name}#{number}:{at.head or ""}:{at.base or ""}`
		return gh.task(function(resolve, reject)
			local function ask()
				gh.text(`repos/{owner}/{name}/pulls/{number}`, "application/vnd.github.v3.diff"):after(function(text: string)
					if at.head and at.base then
						gh.keep(key, text)
					end
					resolve(text)
				end, reject)
			end
			if at.head and at.base then
				gh.cached(key, function(value: any?)
					if type(value) == "string" then
						resolve(value)
					else
						ask()
					end
				end)
			else
				ask()
			end
		end)
	end

	-- A comment on the pull request, as you.
	function M.comment(url: string, body: string): Gh.Task
		local owner, name, number = parse(url)
		return if owner then gh.rest("POST", `repos/{owner}/{name}/issues/{number}/comments`, { body = body }) else failed(zeron.s("pulls.not_pr"))
	end

	-- A comment on a line of a file, at a commit, as you.
	function M.line_comment(url: string, at: { path: string, line: number, side: string, commit: string }, body: string): Gh.Task
		local owner, name, number = parse(url)
		if not owner then
			return failed(zeron.s("pulls.not_pr"))
		end
		return gh.rest("POST", `repos/{owner}/{name}/pulls/{number}/comments`, {
			body = body,
			commit_id = at.commit,
			path = at.path,
			line = at.line,
			side = at.side,
		})
	end

	function M.reply(_url: string, thread: string, body: string): Gh.Task
		return gh.task(function(resolve, reject)
			gh.graphql(
				"mutation($id: ID!, $body: String!) { addPullRequestReviewThreadReply(input: { pullRequestReviewThreadId: $id, body: $body }) { comment { id author { login avatarUrl(size: 96) } } } }",
				{ id = thread, body = body }
			):after(function(data: any)
				local reply = data.addPullRequestReviewThreadReply
				resolve(reply and reply.comment or {})
			end, reject)
		end)
	end

	function M.resolve(_url: string, thread: string, resolved: boolean): Gh.Task
		local verb = if resolved then "resolveReviewThread" else "unresolveReviewThread"
		return gh.graphql(`mutation($id: ID!) \{ {verb}(input: \{ threadId: $id \}) \{ thread \{ id isResolved \} \} \}`, { id = thread })
	end

	-- Your comment, edited or deleted: one on a line (`review`) or on the
	-- pull request.
	function M.edit(id: string, body: string, review: boolean): Gh.Task
		if review then
			return gh.graphql("mutation($id: ID!, $body: String!) { updatePullRequestReviewComment(input: { pullRequestReviewCommentId: $id, body: $body }) { pullRequestReviewComment { id } } }", { id = id, body = body })
		end
		return gh.graphql("mutation($id: ID!, $body: String!) { updateIssueComment(input: { id: $id, body: $body }) { issueComment { id } } }", { id = id, body = body })
	end

	function M.delete(id: string, review: boolean): Gh.Task
		if review then
			return gh.graphql("mutation($id: ID!) { deletePullRequestReviewComment(input: { id: $id }) { clientMutationId } }", { id = id })
		end
		return gh.graphql("mutation($id: ID!) { deleteIssueComment(input: { id: $id }) { clientMutationId } }", { id = id })
	end

	-- The pull request, merged: `method` "squash", "merge" or "rebase"; its
	-- branch deleted after (on GitHub; one from a fork is the fork's).
	function M.merge(url: string, d: any, method: string, delete_branch: boolean): Gh.Task
		local owner, name = parse(url)
		if not owner or type(d.id) ~= "string" then
			return failed(zeron.s("pulls.not_pr"))
		end
		local how = if method == "rebase" then "REBASE" elseif method == "merge" then "MERGE" else "SQUASH"
		return gh.task(function(resolve, reject)
			gh.graphql(
				"mutation($id: ID!, $how: PullRequestMergeMethod!, $head: GitObjectID) { mergePullRequest(input: { pullRequestId: $id, mergeMethod: $how, expectedHeadOid: $head }) { pullRequest { merged } } }",
				{ id = d.id, how = how, head = d.headRefOid }
			):after(function(data: any)
				local merged = data.mergePullRequest and data.mergePullRequest.pullRequest and data.mergePullRequest.pullRequest.merged
				if not merged then
					reject(zeron.s("pulls.not_merged"))
					return
				end
				if delete_branch and not d.isCrossRepository and type(d.headRefName) == "string" and d.headRefName ~= "" then
					-- (A branch that won't go leaves it merged: said, not failed.)
					gh.rest("DELETE", `repos/{owner}/{name}/git/refs/heads/{encoded(d.headRefName)}`):after(function()
						resolve({ merged = true, deleted = true })
					end, function(why: string)
						resolve({ merged = true, deleted = false, why = why })
					end)
				else
					resolve({ merged = true })
				end
			end, reject)
		end)
	end

	-- Closed without merging, or opened again.
	function M.close(id: string): Gh.Task
		return gh.graphql("mutation($id: ID!) { closePullRequest(input: { pullRequestId: $id }) { pullRequest { state } } }", { id = id })
	end

	function M.reopen(id: string): Gh.Task
		return gh.graphql("mutation($id: ID!) { reopenPullRequest(input: { pullRequestId: $id }) { pullRequest { state } } }", { id = id })
	end

	-- A draft marked ready for review, or a pull request back to a draft.
	function M.ready(id: string, ready: boolean): Gh.Task
		if ready then
			return gh.graphql("mutation($id: ID!) { markPullRequestReadyForReview(input: { pullRequestId: $id }) { pullRequest { isDraft } } }", { id = id })
		end
		return gh.graphql("mutation($id: ID!) { convertPullRequestToDraft(input: { pullRequestId: $id }) { pullRequest { isDraft } } }", { id = id })
	end

	-- Its branch brought up to date with its base (GitHub merges the base in).
	function M.update_branch(id: string): Gh.Task
		return gh.graphql("mutation($id: ID!) { updatePullRequestBranch(input: { pullRequestId: $id }) { pullRequest { id } } }", { id = id })
	end

	-- Your review: "APPROVE", "REQUEST_CHANGES" or "COMMENT", with a note.
	function M.review(id: string, event: string, body: string?): Gh.Task
		local vars: { [string]: any } = { id = id, event = event }
		if body and string.match(body, "%S") then
			vars.body = body
		end
		return gh.graphql(
			"mutation($id: ID!, $event: PullRequestReviewEvent!, $body: String) { addPullRequestReview(input: { pullRequestId: $id, event: $event, body: $body }) { pullRequestReview { id state } } }",
			vars
		)
	end

	-- A file of the pull request on GitHub: as it is at its head (or, gone
	-- in it, at its base).
	function M.file_url(url: string, path: string, commit: string?): string?
		local owner, name = parse(url)
		if not owner or not commit or commit == "" then
			return nil
		end
		return `https://github.com/{owner}/{name}/blob/{commit}/{encoded(path)}`
	end

	-- A repository's open pull requests, newest updates first, a page at a
	-- time: yours, waiting for your review, or all. `cached`: what was kept
	-- of the first page, if any (with when, `fetchedAt`), instead of asking.
	function M.list(repository: string, opts: { filter: string?, after: string?, refresh: boolean?, cached: boolean? }?): Gh.Task
		local o = opts or {}
		local filter = o.filter or "authored"
		local q = `repo:{repository} is:pr is:open sort:updated-desc`
		if filter == "authored" then
			q ..= " author:@me"
		elseif filter == "reviewing" then
			q ..= " review-requested:@me"
		end
		local key = `list:{repository}:{filter}`
		return gh.task(function(resolve, reject)
			local function ask()
				gh.graphql(LIST, { q = q, after = o.after }):after(function(data: any)
					local search = data.search or {}
					local items: { any } = {}
					for _, pr in nodes(search) do
						if pr.number then
							table.insert(items, item(pr))
						end
					end
					local info = search.pageInfo or {}
					local page = {
						items = items,
						nextCursor = if info.hasNextPage then info.endCursor else nil,
						totalCount = search.issueCount,
						fetchedAt = os.date("!%Y-%m-%dT%H:%M:%SZ"),
					}
					if not o.after then
						gh.keep(key, page)
					end
					resolve(page)
				end, reject)
			end
			if o.cached and not o.after and not o.refresh then
				gh.cached(key, function(value: any?)
					if type(value) == "table" then
						resolve(value)
					else
						ask()
					end
				end)
			elseif o.refresh and not o.after then
				-- (Refresh pressed again within 15 seconds of an answer asks
				-- nothing: that answer is GitHub's word still, as Zeron's.)
				gh.cached(key, function(value: any?, at: number?)
					if type(value) == "table" and at and os.time() - at < 15 then
						resolve(value)
					else
						ask()
					end
				end)
			else
				ask()
			end
		end)
	end

	-- Which GitHub repository the folder at `path` (on this Mac) is
	-- (`owner/name`), kept once known.
	function M.repository(path: string?): Gh.Task
		if type(path) ~= "string" or path == "" then
			return failed(zeron.s("pulls.no_folder"))
		end
		local key = `repo:{path}`
		return gh.task(function(resolve, reject)
			gh.cached(key, function(value: any?)
				if type(value) == "string" and value ~= "" then
					resolve(value)
					return
				end
				zeron.run("gh", { "repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner" }, { cwd = path, timeout = 30 }):after(function(r: any)
					local name = if type(r) == "table" and r.status == 0 then string.match(r.stdout or "", "^%s*(%S+)") else nil
					if name then
						gh.keep(key, name)
						resolve(name)
					else
						reject(zeron.s("pulls.no_repo"))
					end
				end, function(err: any)
					reject(tostring(if type(err) == "table" and err.message then err.message else err))
				end)
			end)
		end)
	end

	M.graphql = gh.graphql
	M.cached = gh.cached
	M.keep = gh.keep
	M.ci_of = ci_of
	M.nodes = nodes
	return M
end
