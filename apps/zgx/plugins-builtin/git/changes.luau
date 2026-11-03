--!strict
-- Changes: what changed in a session's checkout, the gallery's commit page
-- (demos/example_git_commit) on the design system's list
-- (ds/git_changes). Three scopes, as Zeron's Changes pane has them: the
-- work tree (against the last commit: staged or not, a box each), the
-- branch (since it left its base) and the last turn (since a chat's turn
-- started on its device). Under the list, the commit: its message and
-- Commit (and push, or amend); while a push goes, its progress; a push
-- refused, why, and what to do.
--
-- Its head holds the branch's slot (`slot:git.branch`, providing
-- `git_branch`: the session, its branch, its upstream, whether it's
-- pushed, how far ahead): the GitHub plugin puts "Create pull request" or
-- its pull request there. The Git plugin itself knows only git.
--
-- The list follows the checkout in place: files that move are drawn
-- again, the rest stay as they stood (ds/git_changes, `files` a function).
-- What's open, folded, chosen and where it's scrolled are by path, kept on
-- the session's model while the window is.

local Changes = require("@ds/git_changes")
local git = require("@ds/git")
local Badge = require("@ds/badge")
local Button = require("@ds/button")
local IconButton = require("@ds/icon_button").IconButton
local Checkbox = require("@ds/checkbox")
local TextArea = require("@ds/text_area")
local SplitButton = require("@ds/split_button")
local EmptyState = require("@ds/empty_state")
local FadeText = require("@ds/fade_text")
local Dialog = require("@ds/dialog").Dialog
local Menu = require("@ds/menu")
local Glass = require("@ds/glass")
local people = require("@ds/people")
local progress = require("@ds/progress")
local banner = require("@ds/banner")
local Inline = require("@ds/inline_block")


return function(zeron: any, Repo: any, words: any)
	-- A notice of what went wrong, with the way to try again if there is
	-- one, and a × to put it away.
	local function said_notice(p: any, status: any?, retry: (() -> ())?, dismiss: (() -> ())?): Node
		local w = words(p, status)
		return banner.Callout {
			tone = "danger",
			title = w.title,
			body = w.body,
			action = if retry and w.retry then { label = zeron.s("try_again"), on_click = retry } else nil,
			on_dismiss = dismiss,
		}
	end

	return ui.component(function(props: { model: any, wide: boolean }): Node
		local m = props.model
		local wide = props.wide
		local id = m.id
		local filter = ui.state(`git.{id}.filter`, "")
		local open = ui.state(`git.{id}.open`, {} :: { [string]: boolean })
		local folded = ui.state(`git.{id}.folded`, {} :: { [string]: boolean })
		local chosen = ui.state(`git.{id}.chosen`, "")
		local list_offset = ui.state(`git.{id}.list_scroll`, 0)
		local diff_offset = ui.use_state(0)
		-- Discarding, asked first: which files, while the question stands.
		local discard_open = ui.use_state(false)
		local discard_paths = ui.use_state({} :: { string })
		local push_after = ui.use_state(false)

		local function status(): any
			return m.status:get()
		end
		local function scope(): string
			return m.scope:get()
		end
		local function working(): boolean
			return m.scope:get() == "working"
		end

		-- The branch, as the slot gives it to other plugins: plain data, kept
		-- in a state of its own (what crosses into another plugin's node).
		local branch_info = ui.use_state(nil :: any)
		ui.effect(function()
			local s = m.status:get()
			local device, cwd = m.where()
			local info = if s
				then {
					session = id,
					device = device,
					cwd = cwd,
					branch = s.branch,
					upstream = s.upstream,
					remote = s.remote,
					remote_url = s.remote_url,
					pushed = s.pushed == true,
					ahead = s.ahead or 0,
					behind = s.behind or 0,
					base = s.base,
					base_ahead = s.base_ahead,
					head = s.oid,
				}
				else nil
			local was = branch_info:peek()
			if ui.json.encode(was) ~= ui.json.encode(info) then
				branch_info:set(info)
			end
		end)

		local function ask_discard(paths: { string })
			discard_paths:set(paths)
			discard_open:set(true)
		end

		-- -- the head: scope, how far ahead, the slot, pull and push --------

		-- (What Changes is of, and the Branch scope's base, are the tab's
		-- band's: view.luau.)
		-- Pull (behind its upstream) and push (ahead of it, or not on the
		-- remote yet), each a small button saying how many.
		local function sync_node(): Node
			return ui.row {
			shrink = 0,
			gap = "$space.1",
			items = "center",
			ui.show(function(): boolean
				local s = status()
				return s ~= nil and s.upstream ~= nil and (s.behind or 0) > 0
			end, Button {
				function(): string
					local s = status()
					return zeron.s("pull.button", { n = if s then s.behind or 0 else 0 })
				end,
				id = "git-pull",
				tone = "secondary",
				size = "xs",
				disabled = function(): boolean
					return m.acting:get() ~= nil
				end,
				on_click = function()
					m.act("pull", "pull")
				end,
			}),
			ui.show(function(): boolean
				local s = status()
				if not s or not s.branch or not s.oid then
					return false
				end
				return (s.upstream == nil and s.remote ~= nil) or (s.ahead or 0) > 0
			end, Button {
				function(): string
					local s = status()
					if s and s.upstream == nil then
						return zeron.s("push.publish")
					end
					return zeron.s("push.button", { n = if s then s.ahead or 0 else 0 })
				end,
				id = "git-push",
				tone = "secondary",
				size = "xs",
				disabled = function(): boolean
					return m.acting:get() ~= nil
				end,
				on_click = function()
					m.act("push", "push")
				end,
			}),
		}
		end
		-- What other plugins put beside the branch's way out (GitHub's
		-- Create pull request, or its pull request), given the branch.
		local function branch_slot(): Node
			return ui.row {
				class = "git-branch-actions",
				shrink = 0,
				items = "center",
				provide = { git_branch = branch_info },
				zeron.slot("branch", { direction = "row", gap = "$space.1", items = "center" }),
			}
		end

		-- Discard all, asked from the tab's ⋯ (view.luau) through the model.
		ui.effect(function()
			local ask = m.discard_ask:get()
			if ask then
				ui.after(0, function()
					m.discard_ask:set(nil)
					local paths: { string } = {}
					for _, f in m.files:peek() do
						if f.status ~= "conflict" then
							table.insert(paths, f.path)
						end
					end
					if #paths > 0 then
						ask_discard(paths)
					end
				end)
			end
		end)

		-- -- notices --------------------------------------------------------

		-- A merge, rebase or cherry-pick under way, and files in conflict.
		local state_notice = ui.show(function(): boolean
			local s = status()
			return s ~= nil and (s.state ~= nil or (s.conflicts or 0) > 0)
		end, ui.column { w_full = true, shrink = 0, pb = "$space.3", banner.Callout {
			tone = "warning",
			title = function(): string
				local s = status()
				if not s then
					return ""
				end
				if (s.conflicts or 0) > 0 then
					return zeron.s("conflicts.title", { n = s.conflicts })
				end
				return zeron.s(`state.{s.state or "merge"}`)
			end,
			body = function(): string
				local s = status()
				return if s and (s.conflicts or 0) > 0 then zeron.s("conflicts.body") else zeron.s("state.commit_to_finish")
			end,
		} })
		-- What came back no (staging, discarding, switching, fetching,
		-- pulling): over the list until it's put away. (A commit's and a
		-- push's are the dock's.)
		local act_notice = ui.presence(function(): string
			local a = m.act_problem:get()
			if not a or a.what == "commit" or a.what == "push" then
				return ""
			end
			return `{a.what}:{a.problem.code or a.problem.reason}:{a.problem.words}`
		end, function(key: string): Node
			if key == "" then
				return ui.div {}
			end
			local a = m.act_problem:peek()
			return ui.column {
				w_full = true,
				shrink = 0,
				pb = "$space.3",
				said_notice(a.problem, m.status:peek(), nil, function()
					m.act_problem:set(nil)
				end),
			}
		end, { w_full = true })

		-- -- the list -------------------------------------------------------

		local function files(): { Repo.File }
			return m.files:get()
		end
		local function stage_of(path: string): string
			local f = m.by_path:get()[path]
			return if f then f.stage or "none" else "none"
		end
		local function box(f: Repo.File): Node?
			if not working() then
				return nil
			end
			if f.status == "conflict" then
				return ui.div { w = 16, h = 16, shrink = 0 }
			end
			return Checkbox {
				value = function(): boolean
					return stage_of(f.path) == "all"
				end,
				mixed = function(): boolean
					return stage_of(f.path) == "some"
				end,
				on_change = function(on: boolean)
					m.stage({ f.path }, on)
				end,
				name = f.path,
			}
		end
		local function staged_count(): number
			local n = 0
			for _, f in m.files:get() do
				if f.stage == "all" or f.stage == "some" then
					n += 1
				end
			end
			return n
		end
		local stage_all = Checkbox {
			value = function(): boolean
				local list = m.files:get()
				return #list > 0 and staged_count() == #list
			end,
			mixed = function(): boolean
				local n = staged_count()
				return n > 0 and n < #m.files:get()
			end,
			on_change = function(on: boolean)
				m.stage_all(on)
			end,
			name = zeron.s("stage_all"),
		}
		local function lines_of(f: Repo.File): { any }?
			local d = m.diff_of(f)
			return if d then d.lines else nil
		end
		-- Under an open file's lines: why there are none (loading, binary,
		-- failed), or that it was cut short.
		local function foot_of(f: Repo.File): Node?
			if f.binary then
				return ui.text { zeron.t("diff.binary"), text_size = "$font.size.sm", color = "$text.tertiary" }
			end
			local d = m.diff_of(f)
			if d and d.problem then
				return ui.text { words(d.problem, m.status:peek()).body, text_size = "$font.size.sm", color = "$zeron.tone.danger.ink" }
			end
			if d and d.cut then
				return ui.text { zeron.t("diff.cut"), text_size = "$font.size.sm", color = "$text.tertiary" }
			end
			return ui.row {
				gap = "$space.2",
				items = "center",
				progress.ProgressRing { size = 12, tone = "neutral", label = zeron.s("loading") },
				ui.text { zeron.t("diff.loading"), text_size = "$font.size.sm", color = "$text.tertiary" },
			}
		end
		local function foot_when(f: Repo.File): boolean
			if f.binary then
				return true
			end
			local d = m.diff_of(f)
			return d == nil or d.lines == nil or d.cut == true or d.problem ~= nil
		end
		-- A file's menu (a secondary click): stage or unstage it, discard it.
		local function menu_of(f: Repo.File): { any }?
			if not working() or f.status == "conflict" then
				return nil
			end
			local staged = stage_of(f.path) ~= "none"
			return {
				{
					label = zeron.s(if staged then "file.unstage" else "file.stage"),
					icon = if staged then "minus" else "plus",
					on_select = function()
						m.stage({ f.path }, not staged)
					end,
				},
				{ separator = true },
				{
					label = zeron.s("file.discard"),
					icon = "trash-bin-minimalistic",
					danger = true,
					on_select = function()
						ask_discard({ f.path })
					end,
				},
			}
		end
		local function trail_of(f: Repo.File): Node?
			if not working() or f.status == "conflict" then
				return nil
			end
			return IconButton {
				icon = "trash-bin-minimalistic",
				label = zeron.s("file.discard"),
				size = "sm",
				tone = "quiet",
				class = "git-discard",
				on_click = function()
					ask_discard({ f.path })
				end,
			}
		end
		-- -- comments on the diff, for the chat ------------------------------

		-- A comment being written: under the line asked (a range's last),
		-- the range's lines lit. Sent, it's a chip on the chat's draft
		-- (`zeron.compose_comment`: Zeron's review comments block, its file,
		-- lines, side, the code and the note); the lines keep a mark while
		-- the chip is there, lit as it's pointed at.
		local composing = ui.use_state(nil :: { path: string, key: string, lines: { git.Line } }?)
		local note = ui.use_state("")
		-- A comment's key: its file and its lines' keys (so the marks find
		-- their lines again after a reload, with nothing kept but the chip).
		local function comment_key(path: string, keys: { string }): string
			return ui.json.encode({ p = path, k = keys })
		end
		-- The marks: by file, each line's ("on", or "lit" while its chip is
		-- pointed at), from this chat's comments still on its draft.
		local marked = ui.computed(function(): { [string]: { [string]: string } }
			local out: { [string]: { [string]: string } } = {}
			for key, at in zeron.composed() do
				local ok, d = pcall(ui.json.decode, key)
				if ok and type(d) == "table" and type(d.p) == "string" and type(d.k) == "table" and (at.session == nil or at.session == id) then
					local by = out[d.p] or {}
					out[d.p] = by
					for _, lk in d.k do
						if at.hovered or by[lk] == nil then
							by[lk] = if at.hovered then "lit" else "on"
						end
					end
				end
			end
			return out
		end)
		local function marks(path: string, lk: string): string?
			local c = composing:get()
			if c and c.path == path then
				for _, l in c.lines do
					if git.line_key(l) == lk then
						return "lit"
					end
				end
			end
			local by = marked:get()[path]
			return if by then by[lk] else nil
		end
		local function on_comment(path: string, key: string, line: git.Line, range: { git.Line }?)
			note:set("")
			composing:set({ path = path, key = key, lines = range or { line } })
		end
		-- Where a comment is, as git numbers it: the old file's lines when
		-- every line is one taken out ("L"), else the changed file's ("R").
		local function cited(path: string, lines: { git.Line }): (string, number, number, string)
			local old = true
			for _, l in lines do
				if l.kind ~= "-" then
					old = false
				end
			end
			local lo, hi = math.huge, -math.huge
			for _, l in lines do
				local n = if old then l.old else l.new
				if n then
					lo, hi = math.min(lo, n), math.max(hi, n)
				end
			end
			local f = m.by_path:peek()[path]
			local file = if old and f and f.old then f.old else path
			return file, lo, hi, if old then "L" else "R"
		end
		local function send_comment(text: string)
			local c = composing:peek()
			if not c or string.match(text, "%S") == nil then
				return
			end
			local file, lo, hi, side = cited(c.path, c.lines)
			if lo == math.huge then
				return
			end
			local code: { string } = {}
			local keys: { string } = {}
			for _, l in c.lines do
				table.insert(code, `{if l.kind == " " then " " else l.kind}{l.text}`)
				table.insert(keys, git.line_key(l))
			end
			zeron.compose_comment {
				file = file,
				line = lo,
				to = if hi > lo then hi else nil,
				side = side,
				snippet = table.concat(code, "\n"),
				note = text,
				key = comment_key(c.path, keys),
				session = id,
			}
			composing:set(nil)
			note:set("")
		end
		local function after(path: string, key: string): { git.Block }?
			local c = composing:get()
			if not c or c.path ~= path or c.key ~= key then
				return nil
			end
			local file, lo, hi, side = cited(path, c.lines)
			local where = if hi > lo then `{file}:{lo}–{hi}` else `{file}:{lo}`
			return {
				{
					kind = "composer",
					build = function(ctx: git.BlockCtx): Node
						return ui.div {
							class = "git-comment-writer",
							w_full = true,
							Inline.Composer {
								inset = ctx.inset,
								joined = ctx.joined,
								reveal = ctx.reveal,
								location = if side == "L" then zeron.s("comment.where_old", { where = where }) else where,
								value = note,
								placeholder = zeron.s("comment.placeholder"),
								submit = zeron.s("comment.add"),
								on_submit = send_comment,
								on_cancel = function()
									composing:set(nil)
								end,
							},
						}
					end,
				},
			}
		end

		local function panel(narrow: boolean): Node
			return Changes.panel {
				after = after,
				on_comment = on_comment,
				ranges = true,
				marks = marks,
				id = "git-changes-list",
				files = files,
				filter = filter,
				-- (A list cut short: how much of it shows.)
				extra = function(): string
					local c = m.changes:get()
					return if c and c.total then zeron.s("changes.partial", { n = #(c.files or {}), total = c.total }) else ""
				end,
				lead = if working() then stage_all else nil,
				narrow = narrow,
				columns = if narrow then 1 else 2,
				folded = folded,
				open = open,
				chosen = chosen,
				on_choose = function(f: Changes.File)
					chosen:set(f.path)
					diff_offset:set(0)
				end,
				box = if working() then box else nil,
				lines = lines_of,
				foot = foot_of,
				foot_when = foot_when,
				menu = menu_of,
				trail = trail_of,
				offset = list_offset,
				row_class = "git-file",
			}
		end
		-- The chosen file's diff in its card (the window's width).
		local function diff_card(): Node
			return ui.presence(function(): string
				local c = chosen:get()
				local f = m.by_path:get()[c]
				return if f then `{c}@{f.rev or ""}` else ""
			end, function(key: string): Node
				local f = m.by_path:peek()[chosen:peek()]
				if key == "" or not f then
					return EmptyState { icon = "document", title = zeron.s("diff.pick_title"), body = zeron.s("diff.pick_body"), size = "sm", fill = true }
				end
				return git.frame {
					fill = true,
					git.diff_head({ path = f.path, status = f.status, add = f.add, del = f.del }, nil),
					git.rule(),
					Changes.list {
						id = "git-diff",
						files = { f },
						bare = true,
						columns = 2,
						after = after,
						on_comment = on_comment,
						ranges = true,
						marks = marks,
						lines = lines_of,
						foot = foot_of,
						foot_when = foot_when,
						offset = diff_offset,
					},
				}
			end, { w_full = true, h_full = true, enter = { opacity = { 0, 1 }, duration = 100 } })
		end

		-- What the list shows: asking, nothing changed (the scope's own
		-- words), what failed, or the files.
		local function phase(): string
			if m.changes_problem:get() and not m.changes:get() then
				return "failed"
			end
			local c = m.changes:get()
			if not c then
				return "loading"
			end
			return if #(c.files or {}) == 0 then `empty:{c.scope}` else "list"
		end
		local function empty(scope_now: string): Node
			if scope_now == "working" then
				return EmptyState {
					icon = "git-commit",
					title = zeron.s("empty.working_title"),
					body = function(): string
						local s = status()
						if s and s.branch and (s.ahead or 0) > 0 then
							return zeron.s("empty.working_ahead", { n = s.ahead, branch = s.branch })
						end
						return zeron.s("empty.working_body", { branch = (s and s.branch) or "" })
					end,
					fill = true,
				}
			elseif scope_now == "branch" then
				return EmptyState {
					icon = "git-branch",
					title = zeron.s("empty.branch_title"),
					body = function(): string
						local c = m.changes:get()
						return zeron.s("empty.branch_body", { base = (c and c.base) or "" })
					end,
					fill = true,
				}
			end
			return EmptyState { icon = "chat-round-line", title = zeron.s("empty.turn_title"), body = zeron.s("empty.turn_body"), fill = true }
		end
		local body = ui.presence(function(): string
			return `{phase()}|{if wide then "wide" else "narrow"}`
		end, function(key: string): Node
			local p = string.match(key, "^([^|]+)") :: string
			if p == "loading" then
				return ui.column {
					w_full = true,
					grow = 1,
					items = "center",
					justify = "center",
					progress.ProgressRing { size = 18, tone = "neutral", label = zeron.s("loading") },
				}
			elseif p == "failed" then
				return said_notice(m.changes_problem:peek(), m.status:peek(), function()
					m.changes_problem:set(nil)
					m.load_changes()
				end)
			elseif string.sub(p, 1, 6) == "empty:" then
				return empty(string.sub(p, 7))
			end
			if not wide then
				return panel(true)
			end
			if chosen:peek() == "" or not m.by_path:peek()[chosen:peek()] then
				local first = m.files:peek()[1]
				if first then
					chosen:set(first.path)
				end
			end
			return ui.row {
				w_full = true,
				grow = 1,
				min_h = 0,
				gap = "$space.4",
				items = "stretch",
				ui.column { w = ui.raw(360, "the gallery's file list beside a diff"), shrink = 0, min_h = 0, panel(false) },
				ui.column { grow = 1, min_w = 0, min_h = 0, diff_card() },
			}
		end, { w_full = true, grow = 1, min_h = 0, enter = { opacity = { 0, 1 }, duration = 120 } })

		-- -- the dock: commit, pushing, a push refused ------------------------

		local function commit(push: boolean, amend: boolean)
			local message = m.message:peek()
			m.act("commit", "commit", { message = message, amend = amend }, function()
				m.message:set("")
				if push then
					-- (After the commit's own refresh has started: the push next.)
					ui.after(0, function()
						m.act("push", "push")
					end)
				end
			end)
		end
		local function commit_label(): string
			local n = staged_count()
			return if n == 0 then zeron.s("commit.button") else zeron.s("commit.files", { n = n })
		end
		local function conflicts(): boolean
			local s = status()
			return s ~= nil and (s.conflicts or 0) > 0
		end
		local function dock_phase(): string
			if not working() then
				return "none"
			end
			if m.acting:get() == "push" then
				return "pushing"
			end
			local a = m.act_problem:get()
			if a and (a.what == "push" or a.what == "commit") then
				return `failed:{a.what}:{a.problem.code or a.problem.reason}`
			end
			return "commit"
		end
		local dock = ui.presence(dock_phase, function(key: string): Node
			if key == "none" then
				-- (Another scope: no commit, the branch's way out alone; no
				-- room of its own while it says nothing.)
				return ui.row { id = "git-dock-foot", w_full = true, shrink = 0, pt = "$space.2", justify = "end", gap = "$space.1", items = "center", branch_slot(), sync_node() }
			end
			local inner: Node
			if key == "pushing" then
				inner = ui.column {
					w_full = true,
					gap = "$space.2",
					ui.row {
						gap = "$space.2",
						items = "center",
						progress.ProgressRing { size = 14, tone = "neutral", label = zeron.s("push.pushing_label") },
						FadeText {
							function(): string
								local s = status()
								local where = if s and s.upstream then s.upstream else `{(s and s.remote) or "origin"}/{(s and s.branch) or ""}`
								return zeron.s("push.pushing", { n = math.max(1, (s and s.ahead) or 1), where = where })
							end,
							grow = 1,
							text_size = "$font.size.md",
							color = "$text.secondary",
						},
					},
					progress.ProgressBar { size = "thin", w_full = true, label = zeron.s("push.pushing_label") },
				}
			elseif string.sub(key, 1, 7) == "failed:" then
				local a = m.act_problem:peek()
				local w = words(a.problem, m.status:peek())
				local actions: { any } = {}
				if a.what == "push" and a.problem.code == "rejected" then
					table.insert(actions, {
						label = zeron.s("push.pull_then_push"),
						primary = true,
						on_click = function()
							m.act("pull", "pull", nil, function()
								ui.after(0, function()
									m.act("push", "push")
								end)
							end)
						end,
					})
				end
				if w.retry or a.what == "push" then
					table.insert(actions, {
						label = zeron.s("try_again"),
						on_click = function()
							m.act_problem:set(nil)
							if a.what == "push" then
								m.act("push", "push")
							end
						end,
					})
				end
				inner = banner.Callout {
					tone = "danger",
					title = w.title,
					body = w.body,
					actions = actions,
					on_dismiss = function()
						m.act_problem:set(nil)
					end,
				}
			else
				local button = SplitButton {
					commit_label,
					-- (The foot's scale: the pull request's, push's, 24.)
					size = "xs",
					disabled = function(): boolean
						return conflicts() or staged_count() == 0 or m.message:get() == "" or m.acting:get() ~= nil
					end,
					items = {
						{
							label = zeron.s("commit.and_push"),
							icon = "arrow-up",
							on_click = function()
								commit(true, false)
							end,
						},
						{
							label = zeron.s("commit.amend"),
							icon = "pen",
							on_click = function()
								commit(false, true)
							end,
						},
					},
					on_click = function()
						commit(false, false)
					end,
				}
				-- Who commits (git's user.name there), drawn again if it changes.
				local who = ui.presence(function(): string
					local s = status()
					return (s and s.who ~= "" and s.who) or zeron.s("commit.you")
				end, function(name: string): Node
					return people.Person { id = `git-author-{name}`, name = name, size = 18 }
				end, { shrink = 1, min_w = 0 })
				inner = ui.column {
					w_full = true,
					gap = "$space.2",
					TextArea {
						value = m.message,
						placeholder = zeron.t("commit.placeholder"),
						name = zeron.s("commit.message"),
						subject = { soft = 50, max = 72 },
						rows = if wide then 2 else 1,
						max_rows = 6,
						w_full = true,
					},
					-- Who commits; then the branch's way out, in the order it goes:
					-- a pull request (another plugin's), pull, push, commit.
					ui.row {
						id = "git-dock-foot",
						w_full = true,
						gap = "$space.1",
						items = "center",
						who,
						ui.div { grow = 1 },
						branch_slot(),
						sync_node(),
						ui.div { id = "git-commit", shrink = 0, pl = "$space.1", button },
					},
				}
			end
			return ui.column {
				w_full = true,
				shrink = 0,
				pt = "$space.3",
				ui.div { w_full = true, shrink = 0, pt = "$space.3", border_sides = { 1, 0, 0, 0 }, border_color = "$zeron.settings.rule", inner },
			}
		end, { w_full = true, shrink = 0 })

		-- -- discarding, asked first ------------------------------------------

		local discard_dialog = Dialog {
			open = discard_open,
			tone = "danger",
			title = function(): string
				local paths = discard_paths:get()
				if #paths == 1 then
					return zeron.s("discard.title_one", { name = string.match(paths[1], "([^/]+)$") or paths[1] })
				end
				return zeron.s("discard.title_many", { n = #paths })
			end,
			body = zeron.t("discard.body"),
			confirm = {
				label = zeron.s("discard.confirm"),
				on_click = function()
					local paths = discard_paths:peek()
					discard_open:set(false)
					m.act("discard", "discard", { paths = paths })
				end,
			},
		}

		-- (No gap of the column's own: a part that says nothing takes no
		-- room, so the list starts right under the tab's band. Each part
		-- that shows brings its own room.)
		return ui.column {
			id = "git-changes",
			w_full = true,
			grow = 1,
			min_h = 0,
			gap = 0,
			state_notice,
			act_notice,
			body,
			dock,
			discard_dialog,
		}
	end)
end
