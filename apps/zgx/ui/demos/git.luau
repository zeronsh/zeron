-- The git pages' pieces, put together: a changes panel (staging with a
-- tri-state folder, file status, counts, the diff's toolbar, a commit
-- message with its subject count, a split commit), a pull request list,
-- and a review thread with its reply. Every piece is a general primitive.

local Checkbox = require("@ds/checkbox")
local TextArea = require("@ds/text_area")
local SplitButton = require("@ds/split_button")
local Segmented = require("@ds/segmented")
local ToggleButton = require("@ds/toggle_button")
local Tabs = require("@ds/tabs")
local SearchField = require("@ds/search_field")
local FadeText = require("@ds/fade_text")
local Icon = require("@ds/icon")
local badge = require("@ds/badge")
local people = require("@ds/people")
local comment = require("@ds/comment")
local toolbar = require("@ds/toolbar")
local banner = require("@ds/banner")
local kit = require("@demos/kit")

local staged = ui.state("gt_staged", { true, true, false, false })
local message = ui.state("gt_message", "Add the design system's primitives")
local view = ui.state("gt_view", "unified")
local wrap = ui.state("gt_wrap", false)
local list = ui.state("gt_list", "open")
local query = ui.state("gt_query", "")
local draft = ui.state("gt_draft", "")
local resolved = ui.state("gt_resolved", false)

local FILES = {
	{ path = "ds/badge.luau", status = "added", add = 312, del = 0 },
	{ path = "ds/tokens.luau", status = "modified", add = 64, del = 2 },
	{ path = "ds/sidebar.luau", status = "modified", add = 4, del = 18 },
	{ path = "ds/old_tag.luau", status = "deleted", add = 0, del = 41 },
}

local ME: people.Who = { id = "mara", name = "Mara" }
local ADA: people.Who = { id = "ada", name = "Ada Lovelace" }
local GRACE: people.Who = { id = "grace", name = "Grace Hopper" }
local ALAN: people.Who = { id = "alan", name = "Alan Turing" }

-- A staging row: its box, status letter, name (fading), counts.
local function file_row(i: number, f: any): Node
	return ui.row {
		h = 28,
		pl = "$space.6",
		pr = "$space.2",
		gap = "$space.2",
		items = "center",
		rounded = "$radius.md",
		hover = { bg = "$hover" },
		Checkbox {
			value = function()
				return staged:get()[i]
			end,
			on_change = function(on)
				staged:set_key(i, on)
			end,
			name = f.path,
		},
		badge.FileStatus { status = f.status },
		FadeText { f.path, grow = 1, text_size = "$font.size.md", color = if f.status == "deleted" then "$text.tertiary" else "$text.primary" },
		badge.DiffStat { added = f.add, removed = f.del },
	}
end

local function changes(): Node
	local rows: { Node } = {}
	for i, f in FILES do
		table.insert(rows, file_row(i, f))
	end
	local function all(): boolean
		for _, on in staged:get() do
			if not on then
				return false
			end
		end
		return true
	end
	local function some(): boolean
		local any, every = false, true
		for _, on in staged:get() do
			any = any or on
			every = every and on
		end
		return any and not every
	end
	return ui.column {
		w = 420,
		gap = "$space.3",
		ui.row {
			h = 28,
			px = "$space.2",
			gap = "$space.2",
			items = "center",
			Checkbox {
				value = all,
				mixed = some,
				on_change = function(on)
					staged:set { on, on, on, on }
				end,
				name = "Stage all",
			},
			ui.div { color = "$text.tertiary", Icon { name = "folder", size = 14 } },
			ui.text { "ds", grow = 1, text_size = "$font.size.md", weight = "$font.weight.medium", color = "$text.primary" },
			badge.Count {
				n = function()
					local n = 0
					for _, on in staged:get() do
						n += if on then 1 else 0
					end
					return n
				end,
			},
		},
		ui.column { gap = "$zeron.list.gap", table.unpack(rows) },
		TextArea { value = message, placeholder = "Message", name = "Commit message", subject = { soft = 50, max = 72 }, rows = 2, max_rows = 8, w_full = true },
		ui.row {
			gap = "$space.2",
			items = "center",
			people.Person { id = "mara", name = "Mara", size = 18, detail = "on zeron/primitives" },
			ui.div { grow = 1 },
			SplitButton {
				"Commit",
				size = "sm",
				disabled = function()
					for _, on in staged:get() do
						if on then
							return message:get() == ""
						end
					end
					return true
				end,
				items = {
					{ label = "Commit and push", icon = "arrow-up", shortcut = "cmd+shift+enter" },
					{ label = "Commit and open pull request", icon = "pull-request" },
					{ label = "Amend last commit", icon = "pen" },
				},
			},
		},
	}
end

local function pr_row(number: number, state: badge.PrState, title: string, who: people.Who, reviewers: { people.Who }, checks: badge.ChecksState, review: badge.ReviewState, ago: number, add: number, del: number): Node
	return ui.row {
		w_full = true,
		h = 56,
		px = "$space.3",
		gap = "$space.3",
		items = "center",
		rounded = "$radius.lg",
		hover = { bg = "$hover" },
		cursor = "pointer",
		badge.PullRequest { state = state, number = number },
		ui.column {
			grow = 1,
			min_w = 0,
			gap = "$zeron.space.hair",
			FadeText { title, text_size = "$font.size.md", weight = "$font.weight.medium", color = "$text.primary" },
			ui.row {
				gap = "$space.2",
				items = "center",
				people.face(who, 14),
				ui.text { who.name, text_size = "$font.size.sm", color = "$text.secondary", shrink = 0 },
				people.Timestamp { at = os.time() - ago, compact = true },
				badge.DiffStat { added = add, removed = del },
			},
		},
		badge.Checks { state = checks, size = "sm" },
		badge.Review { state = review, size = "sm" },
		people.AvatarStack { people = reviewers, size = 18, max = 3 },
	}
end

return {
	id = "git",
	title = "Git pieces",
	about = "The primitives the changes panel, pull requests and code review are built from, put together: staging with a tri-state folder, file status and counts, the diff's toolbar, a commit message that counts its subject, a split commit, a pull request list, and a review thread.",
	demo = function()
		return kit.page(
			kit.block(
				"CHANGES",
				ui.row {
					gap = "$space.6",
					items = "start",
					flex_wrap = true,
					changes(),
					ui.column {
						w = 520,
						gap = "$space.3",
						toolbar.Toolbar {
							label = "Diff",
							look = "bar",
							toolbar.Group {
								Segmented { value = view, label = "View", options = { { value = "unified", label = "Unified" }, { value = "split", label = "Split" } } },
							},
							toolbar.Group { ToggleButton { on = wrap, label = "Wrap lines", icon = "wrap-text" } },
							toolbar.Spacer {},
							badge.DiffStat { added = 380, removed = 61, bar = true },
						},
						banner.Callout { tone = "warning", title = "This branch is 3 commits behind main.", action = { label = "Update" } },
					},
				}
			),
			kit.block(
				"PULL REQUESTS",
				ui.column {
					w = 760,
					gap = "$space.3",
					ui.row {
						gap = "$space.3",
						items = "center",
						Tabs {
							value = list,
							look = "pill",
							size = "sm",
							options = {
								{ value = "open", label = "Open", count = 3 },
								{ value = "review", label = "Needs your review", count = 1 },
								{ value = "closed", label = "Closed" },
							},
						},
						ui.div { grow = 1 },
						SearchField { value = query, size = "sm", placeholder = "Filter", w = 200, shortcut = "cmd+f" },
					},
					ui.column {
						gap = "$zeron.list.gap",
						pr_row(1142, "open", "Add the design system's primitives", ME, { ADA, GRACE }, "passing", "approved", 1800, 2480, 61),
						pr_row(1151, "draft", "Settings pages on the new primitives", ME, {}, "running", "pending", 600, 412, 90),
						pr_row(1139, "open", "Teach the sidebar to use Badge for its pull requests", ADA, { ME, GRACE, ALAN, { id = "k", name = "Katherine" } }, "failing", "changes", 86400, 18, 44),
						pr_row(1089, "merged", "Folded user bubbles fade out", GRACE, { ME }, "passing", "approved", 86400 * 3, 120, 30),
					},
				}
			),
			kit.block(
				"A REVIEW THREAD, INLINE",
				ui.column {
					w = 560,
					comment.Thread {
						card = true,
						location = "ds/badge.luau:42",
						resolved = resolved,
						draft = draft,
						me = ME,
						comments = {
							{ author = ADA, at = os.time() - 3600 * 2, body = "Should the soft wash follow the theme's strength here, or stay at 12% in both?", badge = badge.Review { state = "commented", size = "sm" } },
							{ author = ME, at = os.time() - 1800, body = "Each theme has its own: 14% in dark, 11% in light. A faint hue on charcoal all but vanishes.", mine = true, edited = true },
						},
					},
				}
			)
		)
	end,
}
