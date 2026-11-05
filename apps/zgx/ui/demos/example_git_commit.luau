-- The git journey, first page: committing. Drawn for a thousand files
-- (the window's Many switch) as much as for four: one list of the changed
-- files, in their folders, with a filter over it and what there is above
-- that, and the commit docked under it, so the message and the button are
-- there at any scroll. At the window's width the list stands beside the
-- diff of the file chosen in it; at a pane's width a file opens its diff
-- in place under its row, and nothing else moves (Zeron's own Changes
-- panel is one list at line granularity, changes.rs; this follows it).
-- Then nothing to commit, two files in conflict, the push under way, and
-- the push that failed. The list is git_changes.luau; the rest of the
-- pieces are git_parts.luau.

local ds = require("@ds")
local git = require("@demos/git_parts")
local Changes = require("@demos/git_changes")

local shown = ui.state("ex_gc_state", "ready")
-- Staging: every file is staged unless its path is here.
local unstaged = ui.state("ex_gc_unstaged", {} :: { [string]: boolean })
local message = ui.state("ex_gc_message", "Add the design system's primitives")
-- The file chosen for the diff (the window's width), the files open in
-- place (a pane's), the folders folded, and the filter.
local chosen = ui.state("ex_gc_chosen", "ui/ds/badge.luau")
local open = ui.state("ex_gc_open", {} :: { [string]: boolean })
local folded = ui.state("ex_gc_folded", {} :: { [string]: boolean })
local filter = ui.state("ex_gc_filter", "")
local list_offset = ui.state("ex_gc_list_scroll", 0)

-- The files as the state has them: four, a thousand, or four with two in
-- conflict.
local function files_for(state: string): { Changes.File }
	if state == "conflicts" and not git.many:peek() then
		local out: { Changes.File } = {}
		for _, f in git.CONFLICTED do
			local dir, name = string.match(f.path, "^(.*)/([^/]+)$")
			table.insert(out, { path = f.path, dir = dir or "", name = name or f.path, status = f.status, add = f.add, del = f.del })
		end
		return out
	end
	return Changes.files(git.many:peek())
end

local function staged_count(files: { Changes.File }): number
	local n = 0
	local off = unstaged:get()
	for _, f in files do
		if not off[f.path] and f.status ~= "conflict" then
			n += 1
		end
	end
	return n
end

-- The commit's subject: the message's first line.
local function subject(): string
	return string.match(message:get(), "^[^\n]*") or ""
end

-- The commit, docked: the message and, under it, who commits and the
-- button (commit, commit and push, open a pull request, amend); or what
-- stands in its place while it pushes or after the push failed.
local function dock(state: string, files: { Changes.File }, conflicts: boolean, narrow: boolean): Node
	local inner: Node
	if state == "pushing" then
		inner = ui.column {
			w_full = true,
			gap = "$space.2",
			ui.row {
				gap = "$space.2",
				items = "center",
				ds.ProgressRing { size = 14, tone = "neutral", label = "Pushing" },
				ds.FadeText { `Pushing 1 commit to origin/{git.PR.branch}`, grow = 1, text_size = "$font.size.md", color = "$text.secondary" },
			},
			ds.ProgressBar { size = "thin", w_full = true, label = "Push" },
		}
	elseif state == "push_failed" then
		inner = ds.Callout {
			tone = "danger",
			title = `Couldn't push to origin/{git.PR.branch}`,
			body = "The branch moved since you last pulled. Your commit is safe on this Mac.",
			actions = {
				{ label = "Pull, then push", primary = true },
				{ label = "Try again" },
			},
		}
	else
		local commit = ds.SplitButton {
			function()
				local n = staged_count(files)
				return if n == 0 then "Commit" elseif n == 1 then "Commit 1 file" else `Commit {Changes.count(n)} files`
			end,
			size = "sm",
			disabled = function()
				return conflicts or staged_count(files) == 0 or message:get() == ""
			end,
			items = {
				{ label = "Commit and push", icon = "arrow-up", shortcut = "cmd+shift+enter", on_click = function() shown:set("pushing") end },
				{ label = "Commit and open a pull request", icon = "pull-request" },
				{ label = "Amend the last commit", icon = "pen" },
			},
			on_click = function()
				shown:set("pushing")
			end,
		}
		-- (Who commits, by name alone: the branch is in the header's chip.)
		local who = ds.Person { id = git.ME.id, name = git.ME.name, size = 18 }
		inner = ui.column {
			w_full = true,
			gap = "$space.2",
			ds.TextArea {
				value = message,
				placeholder = "What changed, and why",
				name = "Commit message",
				subject = { soft = 50, max = 72 },
				rows = if narrow then 1 else 2,
				max_rows = 6,
				w_full = true,
				hint = if conflicts then "Resolve the conflicts first." else nil,
			},
			if narrow
				then ui.row { w_full = true, gap = "$space.2", items = "center", ds.Avatar { seed = git.ME.id, size = 18, label = git.ME.name }, ui.div { grow = 1 }, commit }
				else ui.row { w_full = true, gap = "$space.2", items = "center", who, ui.div { grow = 1 }, commit },
		}
	end
	return ui.div { w_full = true, shrink = 0, pt = "$space.3", border_sides = { 1, 0, 0, 0 }, border_color = "$zeron.settings.rule", inner }
end

return {
	id = "ex_git_commit",
	title = "Commit",
	about = "The git journey, 1 of 4: the changed files as one list (four, or a thousand: the Many switch), in their folders, with a filter and what there is over it, and the commit docked under it. At the window's width the diff of the chosen file stands beside the list; at a pane's width a file opens its diff in place. Then a clean tree, two files in conflict, the push under way, and a push that failed.",
	demo = function()
		local function body(state: string, narrow: boolean): Node
			if state == "clean" then
				return ds.EmptyState {
					icon = "git-commit",
					title = "Nothing to commit",
					body = `Everything on {git.PR.branch} is committed and pushed.`,
					action = { label = `Open pull request #{git.PR.number}`, icon = "pull-request" },
					fill = true,
				}
			end
			local conflicts = state == "conflicts"
			local files = files_for(state)
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
			local notice: Node? = nil
			if conflicts then
				local props = {
					tone = "warning",
					title = "2 files have conflicts",
					body = "main moved under them. Resolve each, then commit.",
					action = { label = "Open in editor" },
				}
				notice = if narrow then ds.Callout(props :: any) else ds.Banner(props :: any)
			end
			-- A box before each file; none before a conflict.
			local function box(f: Changes.File): Node?
				if f.status == "conflict" then
					return ui.div { w = 16, h = 16, shrink = 0 }
				end
				return ds.Checkbox {
					value = function()
						return unstaged:get()[f.path] ~= true
					end,
					on_change = function(on)
						local now = table.clone(unstaged:peek())
						now[f.path] = if on then nil else true
						unstaged:set(now)
					end,
					name = f.path,
				}
			end
			local function all(): boolean
				return staged_count(files) == #files
			end
			local function some(): boolean
				local n = staged_count(files)
				return n > 0 and n < #files
			end
			local stage_all = if conflicts
				then ui.div { w = 16, h = 16, shrink = 0 }
				else ds.Checkbox {
					value = all,
					mixed = some,
					on_change = function(on)
						if on then
							unstaged:set({})
						else
							local now: { [string]: boolean } = {}
							for _, f in files do
								now[f.path] = true
							end
							unstaged:set(now)
						end
					end,
					name = "Stage all",
				}
			-- The files in their card: what there is and the filter as its
			-- head, the list under them.
			local panel = Changes.panel {
				files = files,
				filter = filter,
				lead = stage_all,
				extra = if narrow then "3 ahead of main" else nil,
				narrow = narrow,
				folded = folded,
				open = open,
				chosen = chosen,
				on_choose = function(f: Changes.File)
					chosen:set(f.path)
				end,
				box = box,
				offset = list_offset,
			}
			if narrow then
				return ui.column {
					w_full = true,
					grow = 1,
					min_h = 0,
					gap = "$space.3",
					notice,
					panel,
					dock(state, files, conflicts, true),
				}
			end
			-- The diff of the chosen file, in a pane of its own beside the list.
			local diff = ui.presence(function(): string
				return chosen:get()
			end, function(path: string): Node
				local f = by_path[path] or files[1]
				return git.diff(f.path, f.status, f.add, f.del, { narrow = false })
			end, { w_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" })
			return ui.column {
				w_full = true,
				grow = 1,
				min_h = 0,
				gap = "$space.4",
				notice,
				ui.row {
					w_full = true,
					grow = 1,
					min_h = 0,
					gap = "$space.5",
					items = "stretch",
					-- The list's card and the diff's start on one line, their
					-- heads of one height, so their first rows share a line.
					ui.column {
						w = 372,
						shrink = 0,
						min_h = 0,
						gap = "$space.3",
						panel,
						dock(state, files, conflicts, false),
					},
					ui.scroll { grow = 1, min_w = 0, min_h = 0, h_full = true, fade = { 16, 16 }, fade_overflow = true, diff },
				},
			}
		end
		return git.window {
			states = {
				{ value = "ready", label = "Ready" },
				{ value = "clean", label = "Nothing to commit" },
				{ value = "conflicts", label = "Conflicts" },
				{ value = "pushing", label = "Pushing" },
				{ value = "push_failed", label = "Push failed" },
			},
			state = shown,
			fixed = function(): boolean
				return true
			end,
			body = function(narrow: boolean): Node
				return ui.column {
					w_full = true,
					grow = 1,
					min_h = 0,
					gap = "$space.4",
					-- A lean header: the page's name as a heading, the branch
					-- beside it, and at the window's width how far ahead it is.
					ui.row {
						w_full = true,
						gap = "$space.3",
						items = "center",
						ds.Text { "Changes", style = "heading", heading = 1 },
						ds.Badge { git.PR.branch, icon = "git-branch", mono = true },
						if narrow then nil else ui.div { grow = 1 },
						if narrow then nil else ds.Badge { "3 commits ahead of main" },
					},
					ui.presence(function(): string
						return shown:get()
					end, function(state: string): Node
						return body(state, narrow)
					end, { w_full = true, grow = 1, min_h = 0, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" }),
				}
			end,
		}
	end,
}
