--!strict
-- In the Git plugin's Changes head (its `git.branch` slot, which gives
-- `git_branch`: the session, its branch, remote and base, whether it's
-- pushed, how far ahead): the branch's pull request, a click opening it in
-- the chat's panel; or, for a branch on GitHub with commits its base
-- hasn't and no pull request yet, Create pull request, which opens
-- GitHub's own page for it where you open links. The Git plugin knows only
-- git; what GitHub is, this plugin says.

local Button = require("@ds/button")
-- The sidebar's pull request badge, a link to it.
local PrBadge = require("./badge")

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

-- A branch's name in a URL's path.
local function encoded(s: string): string
	return (string.gsub(s, "[^%w%-%._~/]", function(c: string): string
		return string.format("%%%02X", string.byte(c))
	end))
end

-- GitHub's page for opening a pull request from `branch` into its base.
local function compare_url(b: any): string?
	local repo = repository(b.remote_url)
	if not repo or not b.branch then
		return nil
	end
	local base = b.base and string.match(b.base, "^[^/]+/(.+)$") or b.base or "main"
	if base == b.branch then
		return nil
	end
	return `https://github.com/{repo}/compare/{encoded(base)}...{encoded(b.branch)}?expand=1`
end

return function(zeron: any, watch: any)
	local M = {}
	M.compare_url = compare_url

	-- What goes in the slot: built once per Changes head, following the
	-- branch and its pull request as they change.
	function M.build(): Node
		local branch = zeron.live("git_branch")
		local here = zeron.context("session")
		local first = branch()
		local session = (first and first.session) or (here and here.id)
		if type(session) ~= "string" then
			return ui.div {}
		end
		local pr = watch.pr(session)
		local panel = zeron.context("panel")
		local function open_pr()
			local now = pr()
			if now and panel then
				panel.open_tab {
					kind = "github-pr",
					title = if now.title and now.title ~= "" then now.title else `#{now.number}`,
					icon = "pull-request",
					data = { url = now.url, number = now.number, session = session },
				}
			end
		end
		-- Which to show: the pull request, the way to make one, or nothing.
		local function which(): string
			local now = pr()
			if now then
				return "pr"
			end
			local b = branch()
			if b and b.upstream and (b.base_ahead or 0) > 0 and compare_url(b) then
				return "create"
			end
			return ""
		end
		return ui.presence(which, function(key: string): Node
			if key == "pr" then
				return ui.row { id = "github-git-pr", shrink = 0, items = "center", PrBadge { words = zeron, pr = pr, on_open = open_pr } }
			elseif key == "create" then
				return Button {
					zeron.t("git_slot.create"),
					id = "github-git-create-pr",
					tone = "secondary",
					size = "xs",
					on_click = function()
						local url = compare_url(branch())
						if url then
							zeron.open_url(url)
						end
					end,
				}
			end
			return ui.div {}
		end, { shrink = 0 })
	end

	return M
end
