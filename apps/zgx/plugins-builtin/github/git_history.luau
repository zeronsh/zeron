--!strict
-- What GitHub knows of the commits in the Git plugin's History, put where
-- the Git plugin leaves room for it (its slots, `git_commit` given in each:
-- { sha, subject, author, email, at, parents, refs, remote_url, session }):
--
--   - `git.commit.face`: the author's GitHub picture over Git's face for
--     them (their initial on their gradient until it's here, or if never);
--   - `git.commit`: the pull request a commit came in by, its state and
--     number (a merge's own number, from its subject, while GitHub hasn't
--     said); a click opens it in the chat's panel;
--   - `git.commit.head`: the same in an opened commit's head, and View on
--     GitHub;
--   - the row's menu (`git.commit`, a menu the Git plugin offers): View on
--     GitHub, Copy GitHub link.
--
-- Asked through gh, a GraphQL ask for up to 40 commits at once (each one's
-- author and its first pull request), batched as rows appear, and kept in
-- the plugin's database: a commit's author never changes; a commit with no
-- pull request yet is asked again after an hour. A repository that isn't
-- GitHub's, or GitHub saying no: nothing shows, and History is git's alone.

local Badge = require("@ds/badge")
local IconButton = require("@ds/icon_button").IconButton
local Tooltip = require("@ds/tooltip")
local people = require("@ds/people")

-- How many commits one ask covers; how long "no pull request" holds.
local BATCH, NO_PR_FOR = 40, 3600

-- `owner/name` of a GitHub remote's URL (ssh or https), or nil.
local function repository(url: string?): string?
	if type(url) ~= "string" then
		return nil
	end
	local owner, name = string.match(url, "github%.com[:/]([^/]+)/([^/]+)$")
	if not owner then
		return nil
	end
	name = string.gsub(name :: string, "%.git$", "")
	return `{owner}/{name}`
end

-- A merge commit's pull request number, as GitHub writes its subject.
local function merged_number(subject: string?): number?
	local n = string.match(subject or "", "^Merge pull request #(%d+)") or string.match(subject or "", "%(#(%d+)%)$")
	return tonumber(n)
end

return function(zeron: any, pulls: any)
	local M = {}
	M.repository = repository

	-- What's known of each commit, by `repo|sha`: { login, avatar, pr } or
	-- false (nothing on GitHub).
	local known = ui.state("github.git_commits", {} :: { [string]: any })
	local asked: { [string]: boolean } = {}
	local waiting: { [string]: { [string]: boolean } } = {}
	local scheduled = false

	local function settle(repo: string, sha: string, record: any)
		local now = table.clone(known:peek())
		now[`{repo}|{sha}`] = record
		known:set(now)
	end

	local function ask(repo: string, shas: { string })
		local owner, name = string.match(repo, "^([^/]+)/(.+)$")
		if not owner then
			return
		end
		local parts: { string } = {}
		for i, sha in shas do
			table.insert(parts, `c{i}: object(oid: "{sha}") \{ ... on Commit \{ author \{ user \{ login avatarUrl(size: 64) \} \} associatedPullRequests(first: 1) \{ nodes \{ number title url state merged \} \} \} \}`)
		end
		local query = `query($owner: String!, $name: String!) \{ repository(owner: $owner, name: $name) \{ {table.concat(parts, " ")} \} \}`
		pulls.graphql(query, { owner = owner, name = name }):after(function(data: any)
			local r = (type(data) == "table" and data.repository) or {}
			local now = table.clone(known:peek())
			for i, sha in shas do
				local o = r[`c{i}`]
				local record: any = false
				if type(o) == "table" then
					local user = o.author and o.author.user
					local nodes = (o.associatedPullRequests and o.associatedPullRequests.nodes) or {}
					local pr = nodes[1]
					record = {
						login = user and user.login,
						avatar = user and user.avatarUrl,
						pr = if pr
							then {
								number = pr.number,
								title = pr.title,
								url = pr.url,
								state = if pr.merged then "merged" else string.lower(pr.state or "open"),
							}
							else nil,
					}
				end
				now[`{repo}|{sha}`] = record
				pulls.keep(`commit:{repo}|{sha}`, record)
			end
			known:set(now)
		end, function(why: any)
			-- (Not GitHub's, not signed in, offline: History stays git's.)
			print(`github: commits of {repo}: {why}`)
		end)
	end

	local function flush()
		scheduled = false
		local all = waiting
		waiting = {}
		for repo, set in all do
			local list: { string } = {}
			for sha in set do
				table.insert(list, sha)
			end
			table.sort(list)
			for i = 1, #list, BATCH do
				local chunk: { string } = {}
				for j = i, math.min(i + BATCH - 1, #list) do
					table.insert(chunk, list[j])
				end
				ask(repo, chunk)
			end
		end
	end

	-- What GitHub knows of `sha` in `repo`, read as it changes: asked (from
	-- the database first) the first time.
	function M.commit(repo: string, sha: string): () -> any
		local key = `{repo}|{sha}`
		if not asked[key] then
			asked[key] = true
			pulls.cached(`commit:{key}`, function(value: any?, at: number?)
				local fresh = value ~= nil and (value ~= false and (value.pr ~= nil or os.time() - (at or 0) < NO_PR_FOR))
				if value ~= nil then
					settle(repo, sha, value)
				end
				if not fresh then
					waiting[repo] = waiting[repo] or {}
					waiting[repo][sha] = true
					if not scheduled then
						scheduled = true
						ui.after(80, flush)
					end
				end
			end)
		end
		return function(): any
			local v = known:get()[key]
			return if type(v) == "table" then v else nil
		end
	end

	local function commit_url(repo: string, sha: string): string
		return `https://github.com/{repo}/commit/{sha}`
	end

	-- A pull request as a chip, a click opening it in the chat's panel.
	local function pr_chip(pr: any, session: string?, panel: any?): Node
		return ui.row {
			class = "github-commit-pr",
			role = "link",
			label = zeron.s("git_history.open_pr", { number = pr.number, title = pr.title or "" }),
			tooltip = if pr.title then Tooltip { pr.title } else nil,
			cursor = "pointer",
			shrink = 0,
			on_click = function()
				if panel and pr.url then
					panel.open_tab {
						kind = "github-pr",
						title = if pr.title and pr.title ~= "" then pr.title else `#{pr.number}`,
						icon = "pull-request",
						data = { url = pr.url, number = pr.number, session = session },
					}
				end
			end,
			Badge.PullRequest { state = (pr.state or "open") :: any, number = pr.number, size = "sm" },
		}
	end

	-- The author's picture over Git's face.
	zeron.place {
		at = "slot:git.commit.face",
		after = function(): Node
			local c = zeron.context("git_commit")
			local repo = c and repository(c.remote_url)
			if not repo then
				return ui.div {}
			end
			local size = tonumber(zeron.context("git_face")) or 18
			local known_of = M.commit(repo, c.sha)
			return ui.show(function(): boolean
				local k = known_of()
				return k ~= nil and k.avatar ~= nil
			end, ui.div {
				class = "github-commit-face",
				role = "img",
				label = function(): string
					local k = known_of()
					return zeron.s("git_history.picture", { login = (k and k.login) or c.author })
				end,
				people.face({
					id = c.email or c.author,
					name = c.author,
					src = function(): string?
						local k = known_of()
						return k and k.avatar
					end,
				}, size),
			})
		end,
	}

	-- The pull request a commit came in by, after its refs.
	zeron.place {
		at = "slot:git.commit",
		after = function(): Node
			local c = zeron.context("git_commit")
			local repo = c and repository(c.remote_url)
			if not repo then
				return ui.div {}
			end
			local panel = zeron.context("panel")
			local known_of = M.commit(repo, c.sha)
			local fallback = merged_number(c.subject)
			return ui.presence(function(): string
				local k = known_of()
				if k and k.pr then
					return `pr:{k.pr.number}:{k.pr.state}`
				end
				return if fallback then `n:{fallback}` else ""
			end, function(key: string): Node
				local k = known_of()
				if k and k.pr then
					return pr_chip(k.pr, c.session, panel)
				elseif fallback then
					return pr_chip({ number = fallback, state = "merged", url = `https://github.com/{repo}/pull/{fallback}` }, c.session, panel)
				end
				return ui.div {}
			end, { shrink = 0 })
		end,
	}

	-- An opened commit's head: its pull request, and its page on GitHub.
	zeron.place {
		at = "slot:git.commit.head",
		after = function(): Node
			local c = zeron.context("git_commit")
			local repo = c and repository(c.remote_url)
			if not repo then
				return ui.div {}
			end
			local panel = zeron.context("panel")
			local known_of = M.commit(repo, c.sha)
			return ui.row {
				id = "github-commit-head",
				gap = "$space.1",
				items = "center",
				ui.presence(function(): string
					local k = known_of()
					return if k and k.pr then `{k.pr.number}:{k.pr.state}` else ""
				end, function(key: string): Node
					local k = known_of()
					return if key ~= "" and k then pr_chip(k.pr, c.session, panel) else ui.div {}
				end),
				IconButton {
					id = "github-view-commit",
					icon = "arrow-up-right",
					label = zeron.s("git_history.view"),
					size = "sm",
					tone = "quiet",
					on_click = function()
						zeron.open_url(commit_url(repo, c.sha))
					end,
				},
			}
		end,
	}

	-- The row's menu (a menu the Git plugin offers): its page, its link.
	local function with_repo(context: any, run: (repo: string, sha: string) -> ())
		local repo = type(context) == "table" and repository(context.remote_url)
		if not repo or type(context.sha) ~= "string" then
			zeron.toast(zeron.s("git_history.not_github"))
			return
		end
		run(repo, context.sha)
	end
	zeron.menu {
		kind = "git.commit",
		id = "github-view-commit",
		label = zeron.s("git_history.view"),
		run = function(context: any)
			with_repo(context, function(repo: string, sha: string)
				zeron.open_url(commit_url(repo, sha))
			end)
		end,
	}
	zeron.menu {
		kind = "git.commit",
		id = "github-copy-commit-link",
		label = zeron.s("git_history.copy_link"),
		run = function(context: any)
			with_repo(context, function(repo: string, sha: string)
				ui.clipboard.write(commit_url(repo, sha))
				zeron.toast(zeron.s("git_history.copied"))
			end)
		end,
	}

	return M
end
