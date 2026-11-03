-- The git journey, third page: the review. The pull request as its
-- reviewers see it, in the app's own three tabs (pull_request_detail.rs:
-- Summary, Code, Activity). Summary is where to act: what it is, its checks
-- folded to a line, the verdicts so far, and the one place to approve, ask
-- for changes, or just comment. Code is the change itself: its files and
-- the diff of the one chosen, with a thread on a line and a + on any
-- other to start one. Activity is what happened, in order: the commits as
-- one event, each comment, the checks, each verdict. Then the checks
-- running, a check that failed, changes asked for, and the request
-- approved; each at the window's width and a side pane's. The pieces are
-- in git_parts.luau.

local ds = require("@ds")
local Markdown = require("@ds/markdown")
local git = require("@demos/git_parts")
local Comment = require("@ds/comment")
local Changes = require("@demos/git_changes")

local shown = ui.state("ex_gr_state", "running")
local tab = ui.state("ex_gr_tab", "summary")
local draft = ui.state("ex_gr_draft", "")
local review = ui.state("ex_gr_review", "")
local resolved = ui.state("ex_gr_resolved", false)
-- The Code tab: the file chosen for the diff (the window's width), the
-- files open in place and the folders folded (a pane's), the filter, the
-- line a comment is being written on, its words, and the comments made
-- (by file and line).
local chosen = ui.state("ex_gr_chosen", "ui/ds/badge.luau")
local open = ui.state("ex_gr_open", { ["ui/ds/badge.luau"] = true } :: { [string]: boolean })
local folded = ui.state("ex_gr_folded", {} :: { [string]: boolean })
local filter = ui.state("ex_gr_filter", "")
local list_offset = ui.state("ex_gr_list_scroll", 0)
-- Where the diff's pane is scrolled to, and the pane's frame as an element
-- (kept from its `on_resize`: that handle reports a live `y`, a ref's
-- doesn't): a comment opening under a line brings itself into view
-- through them.
local pane_offset = ui.state("ex_gr_code_scroll", 0)
local pane_frame: any = nil
local composing = ui.state("ex_gr_composing", "")
local new_draft = ui.state("ex_gr_new", "")
local reply_draft = ui.state("ex_gr_reply", "")
local extra = ui.state("ex_gr_extra", {} :: { [string]: string })

local function tab_now(): string
	local t = tab:get()
	return if t == "code" or t == "activity" then t else "summary"
end

local function checks_for(state: string): { git.Check }
	if state == "running" then
		return {
			{ name = "Build", state = "passing", detail = "Passed in 2m 14s" },
			{ name = "Tests", state = "running", detail = "Running, 3m so far" },
			{ name = "Lint", state = "passing", detail = "Passed in 41s" },
		}
	elseif state == "failed" then
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

-- The failing check's log, open in the block when something failed.
local FAILED_LOG = {
	"FAIL badge renders a pull request's state (ui/ds/badge.test.luau:88)",
	"  expected ink $zeron.pr.open, got $zeron.tone.neutral.ink",
	"FAIL badge keeps a colour given outright (ui/ds/badge.test.luau:102)",
	"  expected #2ea043, got #8b949e",
	"316 passed, 2 failed in 4m 02s",
}

local function verdicts_for(state: string): { git.Verdict }
	local now = os.time()
	if state == "changes" then
		return { { who = git.THEO, verdict = "changes", at = now - 2400 } }
	elseif state == "approved" then
		return {
			{ who = git.THEO, verdict = "approved", at = now - 720 },
			{ who = git.PRIYA, verdict = "approved", at = now - 60 },
		}
	end
	return {}
end

return {
	id = "ex_git_review",
	title = "Review",
	about = "The git journey, 3 of 4: the pull request as its reviewers see it, in the app's three tabs. Summary to act (checks folded to a line, verdicts, the review box), Code to read the change and comment on a line, Activity for what happened in order. Then the checks running, one that failed, changes asked for, and the request approved; at the window's width and a side pane's.",
	demo = function()
		-- The verdicts so far, over the review box: what each reviewer said.
		local function verdicts(state: string): Node?
			if state == "changes" then
				return ds.Settings.Group {
					ds.Settings.Row {
						label = git.THEO.name,
						description = "Asked for changes, 40 minutes ago",
						lead = ds.Avatar { seed = git.THEO.id, size = 32, label = git.THEO.name },
						badge = ds.ReviewBadge { state = "changes", size = "sm" },
					},
					ds.Settings.Row {
						label = git.PRIYA.name,
						description = "Hasn't looked yet",
						lead = ds.Avatar { seed = git.PRIYA.id, size = 32, label = git.PRIYA.name },
						badge = ds.ReviewBadge { state = "pending", size = "sm" },
					},
				}
			elseif state == "approved" then
				return ds.Settings.Group {
					ds.Settings.Row {
						label = git.THEO.name,
						description = "Approved, 12 minutes ago",
						lead = ds.Avatar { seed = git.THEO.id, size = 32, label = git.THEO.name },
						badge = ds.ReviewBadge { state = "approved", size = "sm" },
					},
					ds.Settings.Row {
						label = git.PRIYA.name,
						description = "Approved, just now",
						lead = ds.Avatar { seed = git.PRIYA.id, size = 32, label = git.PRIYA.name },
						badge = ds.ReviewBadge { state = "approved", size = "sm" },
					},
				}
			end
			return nil
		end
		-- The review box: a note, then the verdict it goes with. Narrow, the
		-- three ways stack, the way on first.
		local function review_box(state: string, narrow: boolean): Node
			if state == "approved" then
				return ds.Callout {
					tone = "success",
					title = "Approved by Theo and Priya",
					body = "Every check passed. It's ready to merge.",
					action = { label = "Merge", primary = true },
				}
			end
			local approve = ds.Button { "Approve", ds.Icon { name = "check", size = 16 }, w_full = narrow }
			local changes = ds.Button { "Request changes", tone = "secondary", w_full = narrow }
			local comment = ds.Button { "Comment", tone = "ghost", w_full = narrow }
			return ui.column {
				w_full = true,
				gap = "$space.3",
				ds.TextArea { value = review, placeholder = "Leave a review", name = "Review", rows = 3, max_rows = 8, w_full = true },
				if narrow
					then ui.column { w_full = true, gap = "$space.2", approve, changes, comment }
					else ui.row { w_full = true, justify = "end", gap = "$space.2", comment, changes, approve },
			}
		end
		-- Summary: what it is and where it stands, and the way to say so. A
		-- failure is said once, by the checks block, which then comes first:
		-- it is what a reviewer needs before the description, and opening it
		-- shows the failing check and its log without a scroll.
		local function summary(state: string, checks: { git.Check }, narrow: boolean): Node
			local failed = state == "failed"
			local block = git.checks_block { list = checks, open = git.checks_open, log = if failed then git.log(FAILED_LOG) else nil }
			local description = ds.Settings.Section {
				title = "Description",
				ui.column { w_full = true, px = "$space.4", py = "$space.3", rounded = "$radius.xl", bg = "$zeron.settings.card", border = 1, border_color = "$zeron.settings.edge", Markdown { text = git.PR.body } },
			}
			return ui.column {
				w_full = true,
				gap = "$space.5",
				if failed then block else description,
				if failed then description else block,
				ds.Settings.Section { title = "Review", verdicts(state), review_box(state, narrow) },
			}
		end
		-- What stands under a line of a file, top to bottom: the thread
		-- already there, one made here, and the comment being written. Each
		-- is the design system's band, built with the diff's gutter as its
		-- inset, joined to the block above it. A comment is keyed by file and
		-- line ("ui/ds/badge.luau|n45").
		local function blocks_under(state: string, path: string, key: string): { git.Block }?
			local out: { git.Block } = {}
			local at = `{path}|{key}`
			local made = extra:get()
			if path == "ui/ds/badge.luau" and key == "n42" then
				table.insert(out, { kind = "thread", build = function(ctx: git.BlockCtx): Node
					return git.thread { inset = ctx.inset, joined = ctx.joined, draft = draft, resolved = resolved, changes = state == "changes" }
				end })
			end
			if made[at] then
				table.insert(out, { kind = "made", build = function(ctx: git.BlockCtx): Node
					local comments: { Comment.Comment } = { { author = git.ME, at = os.time(), body = made[at], mine = true } }
					return ds.InlineThread {
						inset = ctx.inset,
						joined = ctx.joined,
						location = `{path}:{string.sub(key, 2)}`,
						draft = reply_draft,
						me = git.ME,
						comments = comments,
					}
				end })
			end
			if composing:get() == at then
				table.insert(out, { kind = "composer", build = function(ctx: git.BlockCtx): Node
					return ds.InlineComposer {
						inset = ctx.inset,
						joined = ctx.joined,
						reveal = ctx.reveal,
						location = `{path}:{string.sub(key, 2)}`,
						value = new_draft,
						placeholder = "Comment on this line",
						submit = "Comment",
						on_submit = function(text: string)
							extra:set_key(at, text)
							composing:set("")
						end,
						on_cancel = function()
							composing:set("")
						end,
					}
				end })
			end
			return if #out > 0 then out else nil
		end
		-- Code: the changed files as one list (four, or a thousand), in their
		-- folders, with a filter and what there is over it. At the window's
		-- width the diff of the file chosen stands beside the list in a pane
		-- of its own, with the thread on its line 42 and a + on any line to
		-- start one; at a pane's width a file opens its diff in place under
		-- its row, threads and all, and the list is the one scroll surface.
		local function code(state: string, narrow: boolean): Node
			local files = Changes.files(git.many:peek())
			local by_path: { [string]: Changes.File } = {}
			for _, f in files do
				by_path[f.path] = f
			end
			-- The chosen file is one of these; the first, if not.
			if not by_path[chosen:peek()] and files[1] then
				ui.after(0, function()
					chosen:set(files[1].path)
				end)
			end
			if narrow then
				-- The files in their card (what there is and the filter as its
				-- head), each opening its diff in place; a comment opening under
				-- a line brings itself into the list's frame.
				local list_frame: any = nil
				return Changes.panel {
					files = files,
					filter = filter,
					narrow = true,
					folded = folded,
					open = open,
					after = function(path: string, key: string): { git.Block }?
						return blocks_under(state, path, key)
					end,
					on_comment = function(path: string, key: string)
						composing:set(`{path}|{key}`)
					end,
					reveal = {
						offset = list_offset,
						frame = function()
							return list_frame
						end,
					},
					offset = list_offset,
					on_frame = function(el)
						list_frame = el
					end,
				}
			end
			local function diff_now(): Node
				local f = by_path[chosen:get()] or files[1]
				return git.diff(f.path, f.status, f.add, f.del, {
					narrow = false,
					after = function(key: string): { git.Block }?
						return blocks_under(state, f.path, key)
					end,
					on_comment = function(key: string)
						composing:set(`{f.path}|{key}`)
					end,
					reveal = {
						offset = pane_offset,
						frame = function()
							return pane_frame
						end,
					},
				})
			end
			-- Another file, another diff; a comment opened or made under a
			-- line, a block in place under it.
			local diff = ui.presence(function(): string
				return chosen:get()
			end, function(_: string): Node
				return diff_now()
			end, { w_full = true, enter = { opacity = { 0, 1 }, duration = 100 }, exit = { opacity = 0, duration = 50 }, mode = "wait" })
			local pane = ui.div {
				grow = 1,
				min_w = 0,
				min_h = 0,
				h_full = true,
				on_resize = function(_size, el)
					pane_frame = el
				end,
				ui.scroll {
					id = "diff-scroll",
					offset = pane_offset,
					w_full = true,
					h_full = true,
					fade = { 16, 16 },
					fade_overflow = true,
					diff,
				},
			}
			-- The list's card and the diff's start on one line, their heads of
			-- one height, so their first rows share a line.
			local panel = Changes.panel {
				files = files,
				filter = filter,
				folded = folded,
				chosen = chosen,
				on_choose = function(f: Changes.File)
					chosen:set(f.path)
				end,
				offset = list_offset,
			}
			return ui.row {
				w_full = true,
				grow = 1,
				min_h = 0,
				gap = "$space.4",
				items = "stretch",
				ui.column { w = 300, shrink = 0, min_h = 0, panel },
				pane,
			}
		end
		local function page(state: string, narrow: boolean, fixed: boolean): Node
			local checks = checks_for(state)
			local passing = 0
			for _, c in checks do
				passing += if c.state == "passing" then 1 else 0
			end
			local review_state = if state == "changes" then "changes" elseif state == "approved" then "approved" else "pending"
			local checks_state = if state == "running" then "running" elseif state == "failed" then "failing" else "passing"
			local events = git.events { checks = checks, verdicts = verdicts_for(state) }
			-- The Code tab is built in place at the window's height; the
			-- other two come and go under the tabs.
			local content: Node = if fixed
				then code(state, narrow)
				else ui.presence(tab_now, function(t: string): Node
					if t == "activity" then
						return git.activity(events)
					end
					return summary(state, checks, narrow)
				end, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" })
			return ui.column {
				w_full = true,
				grow = if fixed then 1 else nil,
				min_h = if fixed then 0 else nil,
				gap = "$space.5",
				git.header("open", narrow),
				git.tabs {
					value = tab,
					narrow = narrow,
					options = {
						{ value = "summary", label = "Summary" },
						{ value = "code", label = "Code", count = #git.FILES },
						{ value = "activity", label = "Activity", count = #events },
					},
					badges = {
						ds.ChecksBadge { state = checks_state :: any, passed = passing, total = #checks, size = "md" },
						ds.ReviewBadge { state = review_state :: any, size = "md" },
					},
				},
				content,
			}
		end
		return git.window {
			states = {
				{ value = "running", label = "Checks running" },
				{ value = "failed", label = "A check failed" },
				{ value = "changes", label = "Changes requested" },
				{ value = "approved", label = "Approved" },
			},
			state = shown,
			fixed = function(): boolean
				return tab_now() == "code"
			end,
			body = function(narrow: boolean, fixed: boolean): Node
				return ui.presence(function(): string
					return shown:get()
				end, function(state: string): Node
					return page(state, narrow, fixed)
				end, { w_full = true, h_full = fixed, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" })
			end,
		}
	end,
}
