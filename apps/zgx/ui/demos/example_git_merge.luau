-- The git journey, last page: merging. Whether the pull request can go in
-- and why not yet, its checks folded to a line under that, how it goes in
-- (squash, a merge commit, rebase), what happens to the branch, and the
-- one button. Then blocked, in conflict, merging, merged, and the merge
-- that failed. At a side pane's width the header stacks and the button
-- fills the line. The pieces are in git_parts.luau.

local ds = require("@ds")
local git = require("@demos/git_parts")
local Radio = require("@ds/radio")

local shown = ui.state("ex_gm_state", "ready")
local method = ui.state("ex_gm_method", "squash")
local delete_branch = ui.state("ex_gm_delete", true)

local METHODS: { Radio.Option } = {
	{ value = "squash", label = "Squash and merge", description = "The 3 commits become one on main, with the title as its message." },
	{ value = "merge", label = "Merge commit", description = "The commits as they are, joined by a merge commit." },
	{ value = "rebase", label = "Rebase and merge", description = "The commits replayed onto main, one by one." },
}

-- The checks as they stand: all passed, or, blocked, Tests failed.
local function checks_for(state: string): { git.Check }
	if state == "blocked" then
		return {
			{ name = "Build", state = "passing", detail = "Passed in 2m 14s" },
			{ name = "Tests", state = "failing", detail = "2 of 318 failed: badge renders a pull request's state" },
			{ name = "Lint", state = "passing", detail = "Passed in 41s" },
		}
	end
	return {
		{ name = "Build", state = "passing", detail = "Passed in 2m 14s" },
		{ name = "Tests", state = "passing", detail = "318 passed in 4m 02s" },
		{ name = "Lint", state = "passing", detail = "Passed in 41s" },
	}
end

return {
	id = "ex_git_merge",
	title = "Merge",
	about = "The git journey, 4 of 4: merging. Whether the pull request can go in and what's still in the way, its checks folded to a line, how it goes in, what becomes of the branch, and the one button. Then blocked by its review and checks, in conflict with main, merging, merged, and the merge that failed; at the window's width and a side pane's.",
	demo = function()
		-- What stands between the request and main, as a list; none, and it
		-- is ready.
		local function standing(state: string, narrow: boolean): Node
			if state == "ready" or state == "merging" then
				return ds.Settings.Row {
					label = "Ready to merge",
					description = "Approved by Theo and Priya, every check passed, no conflicts with main.",
					lead = git.glyph("check-circle", "success"),
				}
			elseif state == "conflicts" then
				return ds.Settings.Row {
					label = "Conflicts with main",
					description = "ui/ds/tokens.luau and ui/ds/sidebar.luau changed on main too. Resolve them on the branch, then merge.",
					lead = git.glyph("danger-triangle", "danger"),
					-- Narrow, the way to resolve under the words, not beside them.
					control = if narrow then nil else ds.Button { "Resolve in Zeron", tone = "secondary", size = "sm" },
					badge = nil,
				}
			end
			return ds.Settings.Row {
				label = "Not yet",
				description = "Theo asked for changes, and Tests failed. Both have to clear first.",
				lead = git.glyph("clock-circle", "warning"),
			}
		end
		local function body(state: string, narrow: boolean): Node
			if state == "merged" then
				return ui.column {
					w_full = true,
					gap = "$space.4",
					ds.Callout {
						tone = "success",
						title = `Merged #{git.PR.number} into main`,
						body = `Squashed into one commit. {git.PR.branch} was deleted; your checkout is on main.`,
						action = { label = "Start a chat on main", primary = true },
					},
				}
			end
			local blocked = state == "blocked" or state == "conflicts"
			local merging = state == "merging"
			local merge = ds.Button {
				function()
					if merging then
						return "Merging…"
					end
					local m = method:get()
					return if m == "squash" then "Squash and merge" elseif m == "rebase" then "Rebase and merge" else "Merge pull request"
				end,
				disabled = blocked or merging,
				w_full = narrow,
				ds.Icon { name = "git-merge", size = 16 },
				on_click = function()
					shown:set("merging")
				end,
			}
			return ui.column {
				w_full = true,
				gap = "$space.4",
				ds.Settings.Group {
					standing(state, narrow),
					if state == "conflicts" and narrow
						then ui.div { w_full = true, px = "$space.4", pb = "$space.4", ds.Button { "Resolve in Zeron", tone = "secondary", size = "sm", w_full = true } }
						else nil,
				},
				git.checks_block { list = checks_for(state), open = git.checks_open },
				ds.Settings.Section {
					title = "How it goes in",
					ds.Settings.Group {
						ui.column {
							w_full = true,
							px = "$space.4",
							py = "$space.4",
							ds.RadioGroup { value = method, label = "How it goes in", options = METHODS, disabled = blocked },
						},
						ds.Settings.Row {
							label = "Delete the branch after",
							description = `{git.PR.branch} goes from GitHub and from this Mac. Its chats keep their history.`,
							control = ds.Switch { value = delete_branch, label = "Delete the branch after", disabled = blocked },
						},
					},
				},
				if state == "failed"
					then ds.Callout {
						tone = "danger",
						title = "Couldn't merge",
						body = "main moved while this was merging. Nothing changed on GitHub. Update the branch, then try again.",
						actions = { { label = "Update branch", primary = true }, { label = "Try again" } },
					}
					else nil,
				if narrow
					then ui.column {
						w_full = true,
						gap = "$space.2",
						items = "center",
						ui.show(function()
							return merging
						end, ds.ProgressRing { size = 16, tone = "neutral", label = "Merging" }),
						merge,
					}
					else ui.row {
						w_full = true,
						justify = "end",
						gap = "$space.2",
						items = "center",
						ui.show(function()
							return merging
						end, ds.ProgressRing { size = 16, tone = "neutral", label = "Merging" }),
						merge,
					},
			}
		end
		return git.window {
			states = {
				{ value = "ready", label = "Ready" },
				{ value = "blocked", label = "Blocked" },
				{ value = "conflicts", label = "Conflicts" },
				{ value = "merging", label = "Merging" },
				{ value = "merged", label = "Merged" },
				{ value = "failed", label = "Failed" },
			},
			state = shown,
			body = function(narrow: boolean): Node
				return ui.presence(function(): string
					return shown:get()
				end, function(state: string): Node
					return ui.column {
						w_full = true,
						gap = "$space.5",
						git.header(if state == "merged" then "merged" else "open", narrow),
						body(state, narrow),
					}
				end, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" })
			end,
		}
	end,
}
