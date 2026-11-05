-- The git journey, second page: opening the pull request. Where it goes
-- (the branch into main), a title that starts as the commit's subject, a
-- description, who should look at it, and whether it opens as a draft.
-- Then the request under way, opened, and the one GitHub didn't take. At
-- a side pane's width the two branches stack and the buttons fill the
-- line, the way on first. The pieces are in git_parts.luau.

local ds = require("@ds")
local git = require("@demos/git_parts")

local shown = ui.state("ex_gp_state", "ready")
local title = ui.state("ex_gp_title", git.PR.title)
local body = ui.state("ex_gp_body", git.PR.body)
local reviewers = ui.state("ex_gp_reviewers", { "Theo Lindqvist", "Priya Raman" })
local draft = ui.state("ex_gp_draft", false)
local base = ui.state("ex_gp_base", "main")
local compare = ui.state("ex_gp_compare", git.PR.branch)

local BRANCHES = {
	{ value = "main", label = "main" },
	{ value = git.PR.branch, label = git.PR.branch },
	{ value = "zeron/composer", label = "zeron/composer" },
}

return {
	id = "ex_git_pr",
	title = "Pull request",
	about = "The git journey, 2 of 4: opening the pull request. The branch into main, a title that starts as the commit's subject, the description, who reviews, and whether it opens as a draft; then the request under way, opened, and one GitHub didn't take; at the window's width and a side pane's.",
	demo = function()
		ui.effect(function()
			draft:set(shown:get() == "draft")
		end)
		local function outcome(state: string, narrow: boolean): Node
			if state == "created" then
				return ds.Callout {
					tone = "success",
					title = `Pull request #{git.PR.number} opened`,
					body = "Theo and Priya were asked to review. You'll hear when they do.",
					action = { label = `Open #{git.PR.number}` },
				}
			elseif state == "failed" then
				return ds.Callout {
					tone = "danger",
					title = "Couldn't open the pull request",
					body = "GitHub didn't answer. Your title and description are kept here.",
					action = { label = "Try again", on_click = function() shown:set("ready") end },
				}
			end
			local creating = state == "creating"
			local open = ds.Button {
				function()
					if creating then
						return "Opening…"
					end
					return if draft:get() then "Open as a draft" else "Open pull request"
				end,
				disabled = function()
					return creating or title:get() == ""
				end,
				w_full = narrow,
				on_click = function()
					shown:set("creating")
				end,
			}
			local cancel = ds.Button { "Cancel", tone = "ghost", disabled = creating, w_full = narrow }
			local ring = ui.show(function()
				return creating
			end, ds.ProgressRing { size = 16, tone = "neutral", label = "Opening" })
			if narrow then
				return ui.column { w_full = true, gap = "$space.2", open, cancel }
			end
			return ui.row {
				w_full = true,
				justify = "end",
				gap = "$space.2",
				items = "center",
				ring,
				cancel,
				open,
			}
		end
		-- Where it goes: the branch, into its base; side by side, or
		-- stacked.
		local function branches(narrow: boolean): Node
			if narrow then
				return ui.column {
					w_full = true,
					gap = "$space.3",
					ds.Select { value = compare, label = "Branch", options = BRANCHES, w_full = true },
					ds.Select { value = base, label = "Into", options = BRANCHES, w_full = true },
				}
			end
			return ui.row {
				gap = "$space.3",
				items = "end",
				ds.Select { value = compare, label = "Branch", options = BRANCHES, w = 240 },
				ui.div { pb = "$space.2", color = "$text.tertiary", ds.Icon { name = "arrow-right", size = 16 } },
				ds.Select { value = base, label = "Into", options = BRANCHES, w = 200 },
			}
		end
		return git.window {
			states = {
				{ value = "ready", label = "Ready" },
				{ value = "draft", label = "Draft" },
				{ value = "creating", label = "Opening" },
				{ value = "created", label = "Opened" },
				{ value = "failed", label = "Failed" },
			},
			state = shown,
			body = function(narrow: boolean): Node
				return ui.column {
					w_full = true,
					-- (A theme number, read as it is built: max_w takes points.)
					max_w = if narrow then nil else ui.theme.get("zeron.settings.measure") :: number?,
					gap = "$space.5",
					ui.column {
						gap = "$space.1",
						ds.Text { "New pull request", style = "title", heading = 1 },
						ui.text { "3 commits on the branch, reviewed and merged on GitHub.", text_size = "$font.size.md", line_height = 20, color = "$text.secondary", wrap = true },
					},
					branches(narrow),
					ds.TextField { value = title, label = "Title", placeholder = "What this change does", w_full = true, count = { of = function() return #title:get() end, soft = 72 } },
					ds.TextArea { value = body, label = "Description", placeholder = "Why, and anything a reviewer should know", rows = 6, max_rows = 12, w_full = true, hint = "Markdown. Mention an issue to link it." },
					ds.TokenField { value = reviewers, label = "Reviewers", placeholder = "Add a teammate", w_full = true, hint = "They're asked when it opens, unless it's a draft." },
					ds.Checkbox { value = draft, name = "Open as a draft", label = "Open as a draft", description = "No review asked for yet. Mark it ready when it is." },
					ui.presence(function(): string
						return shown:get()
					end, function(state: string): Node
						return outcome(state, narrow)
					end, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
				}
			end,
		}
	end,
}
