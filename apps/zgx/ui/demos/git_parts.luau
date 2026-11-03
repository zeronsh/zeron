-- The git journey's pieces, shared by its pages: one pull request from
-- its commit to its merge (#1142, "Add the design system's primitives",
-- zeron/primitives into main), the people around it, its commits and
-- files, a diff of one file (with a comment on any line of it), its checks
-- (as a block that folds to one line), what happened to it in order (the
-- activity), and the header every page of it opens with. The vocabulary
-- is Zeron's (pull_requests.rs row_status: Draft, Open, Approved, Changes
-- requested, Merge conflicts; Checks · n).
--
-- Every page has two widths: the window's, and a side pane's. Zeron's
-- right pane is 520 by default and 360 at its narrowest (settings.rs
-- RIGHT_PANE_MIN); its pull request page tightens its gutters to 16 below
-- 480 (pull_request_detail.rs NARROW_PAGE, NARROW_PAD_X). The narrow
-- layout here is drawn for the narrowest pane, so it holds at any width
-- the pane can take: its header stacks, its tabs' badges drop under them,
-- its diff keeps one column of line numbers, its buttons stack.
--
-- The pieces themselves are the design system's (ds/git), which the GitHub
-- plugin draws from too: this file is the journey's own facts (the pull
-- request, its people, its commits and files, the diff of badge.luau) and
-- the window the pages stand in, with the pieces taken over as they were.

local ds = require("@ds")
local Git = require("@ds/git")
local Comment = require("@ds/comment")
local people = require("@ds/people")
local kit = require("@demos/example_kit")

local M = {}

export type Line = Git.Line
export type BlockCtx = Git.BlockCtx
export type Block = Git.Block
export type Diff = Git.Diff
export type Check = Git.Check
export type Tabs = Git.TabsProps

M.RADIUS = Git.RADIUS
M.EDGE = Git.EDGE

-- The window the pages sit in: a pane's worth, wide enough for a list
-- beside a diff; and the side pane at its narrowest.
M.W, M.H = 960, 640
M.NARROW = 360

-- Which width the pages are shown at (the demo's switch), and whether the
-- change has a thousand files or four (the second switch).
M.width = ui.state("ex_git_width", "full")
function M.narrow(): boolean
	return M.width:get() == "narrow"
end
M.many = ui.state("ex_git_many", false)

-- Whether the checks block stands open: remembered, as the app would.
M.checks_open = ui.persist("ex_git_checks_open", false)

-- The people: you, and the others on the pull request.
local ME: people.Who = { id = "mara", name = "Mara Okafor" }
local THEO: people.Who = { id = "theo", name = "Theo Lindqvist" }
local PRIYA: people.Who = { id = "priya", name = "Priya Raman" }
local JONAS: people.Who = { id = "jonas", name = "Jonas Weber" }
M.ME, M.THEO, M.PRIYA, M.JONAS = ME, THEO, PRIYA, JONAS

M.PR = {
	number = 1142,
	title = "Add the design system's primitives",
	branch = "zeron/primitives",
	base = "main",
	added = 2480,
	removed = 61,
	commits = 3,
	opened = "2 hours ago",
	body = "Fields, choices, notices, overlays and the settings pieces, each a recipe over the tokens. Screens import only `@ds`.\n\n- `TextField`, `TextArea`, `TokenField`, `Select`, `Slider`\n- `Callout`, `Banner`, `toast`\n- `Dialog`, `Sheet`\n- `Settings.Page`, `Section`, `Group`, `Row`\n\nThe lint warns on any value that isn't a token.",
}

-- The branch's commits, oldest first.
M.COMMITS = {
	{ sha = "a41c0d2", subject = "Add the badge primitives" },
	{ sha = "7e9f31b", subject = "Token field, select and slider" },
	{ sha = "3f2c9a1", subject = "Settings pieces, and the token lint" },
}

-- The commit's files: Zeron's statuses, with the lines each adds and takes.
M.FILES = {
	{ path = "ui/ds/badge.luau", status = "added", add = 312, del = 0 },
	{ path = "ui/ds/tokens.luau", status = "modified", add = 64, del = 2 },
	{ path = "ui/ds/sidebar.luau", status = "modified", add = 4, del = 18 },
	{ path = "ui/ds/old_tag.luau", status = "deleted", add = 0, del = 41 },
}
-- The same tree, two files in conflict after main moved under them.
M.CONFLICTED = {
	{ path = "ui/ds/badge.luau", status = "added", add = 312, del = 0 },
	{ path = "ui/ds/tokens.luau", status = "conflict", add = 64, del = 2 },
	{ path = "ui/ds/sidebar.luau", status = "conflict", add = 4, del = 18 },
	{ path = "ui/ds/old_tag.luau", status = "deleted", add = 0, del = 41 },
}

-- A window with the strip, the state switch and the width switch, and the
-- page: `body` builds it for a width (true: the side pane's), and is built
-- again when the width switches. `fixed` says when the page keeps to the
-- window's height and scrolls in parts instead (the Code tab); the page is
-- built again as that changes too.
export type Window = {
	states: any,
	state: any,
	body: ((narrow: boolean, fixed: boolean) -> Node)?,
	fixed: (() -> boolean)?,
	[number]: Child,
}

function M.window(props: Window): Node
	local body: Node
	if props.body then
		local build = props.body
		local fixed = props.fixed or function()
			return false
		end
		body = ui.presence(function(): string
			return `{M.width:get()}|{fixed()}|{M.many:get()}`
		end, function(key: string): Node
			local narrow, is_fixed = M.width:peek() == "narrow", fixed()
			return kit.window { narrow = M.narrow, fixed = is_fixed, build(narrow, is_fixed) }
		end, { w_full = true, h_full = true, enter = { opacity = { 0, 1 }, duration = 120 }, exit = { opacity = 0, duration = 60 }, mode = "wait" })
	else
		body = kit.window { narrow = M.narrow, props[1] }
	end
	return kit.stage {
		states = props.states,
		state = props.state,
		also = ui.row {
			gap = "$space.3",
			items = "center",
			ds.Segmented {
				value = M.width,
				options = { { value = "full", label = "Full window" }, { value = "narrow", label = "Side pane" } },
				label = "Width",
			},
			ds.Segmented {
				value = M.many,
				options = { { value = false, label = "Few" }, { value = true, label = "Many" } },
				label = "Files",
			},
		},
		w = function(): number
			return if M.narrow() then M.NARROW else M.W
		end,
		h = M.H,
		body,
	}
end

-- The pieces, as the design system has them.
M.glyph = Git.glyph
M.tabs = Git.tabs
M.file_row = Git.file_row
M.card = Git.card
M.chevron = Git.chevron
M.selectable = Git.selectable
M.faded = Git.faded
M.files_list = Git.files_list
M.files_select = Git.files_select
M.line_key = Git.line_key
M.gutter = Git.gutter
M.DiffLine = Git.DiffLine
M.checks = Git.checks
M.checks_summary = Git.checks_summary
M.checks_block = Git.checks_block
M.log = Git.log

-- The pull request's header, as every page of it opens (ds/git.header),
-- for #1142 in `state`.
function M.header(state: string, narrow: boolean?): Node
	local pr = M.PR
	return Git.header({
		state = state,
		number = pr.number,
		title = pr.title,
		author = M.ME,
		opened = pr.opened,
		branch = pr.branch,
		base = pr.base,
		added = pr.added,
		removed = pr.removed,
	}, narrow)
end

-- The diff of badge.luau the pages show: two hunks, as the change really
-- reads.
local HUNK: { Line } = {
	{ kind = "hunk", text = "@@ -40,7 +40,9 @@ function M.Badge(props: Props): Node" },
	{ kind = " ", old = 40, new = 40, text = '	local size = SIZE[props.size or "sm"]' },
	{ kind = " ", old = 41, new = 41, text = '	local look: Look = props.look or "soft"' },
	{ kind = "-", old = 42, text = '	local ink = `$zeron.tone.{props.tone or "neutral"}.ink`' },
	{ kind = "+", new = 42, text = "	local ink = ink_of(props)" },
	{ kind = "+", new = 43, text = "	-- A colour given outright (a pull request's state) wins over a tone." },
	{ kind = " ", old = 43, new = 44, text = '	local radius = if props.shape == "pill" then "$radius.full" else size.radius' },
	{ kind = " ", old = 44, new = 45, text = "	local text = props[1]" },
	{ kind = "hunk", text = "@@ -118,6 +120,12 @@ function M.Badge(props: Props): Node" },
	{ kind = " ", old = 118, new = 120, text = "	return ui.row {" },
	{ kind = "+", new = 121, text = "		shrink = 0," },
	{ kind = "+", new = 122, text = "		relative = true," },
	{ kind = " ", old = 119, new = 123, text = "		h = size.h," },
}

M.HUNK = HUNK

-- A diff of one file of the change: a few lines of badge.luau, as the
-- change really reads (ds/git.diff).
function M.diff(path: string, status: string, add: number, del: number, opts: Diff?): Node
	return Git.diff({ path = path, status = status, add = add, del = del }, HUNK, opts)
end

-- A review thread on a line of the change, as Zeron shows one inline: the
-- design system's band under the line, its words on the code's text edge
-- (`inset`), joined to a block above it if there is one.
function M.thread(opts: { inset: number?, joined: boolean?, draft: State<string>, resolved: State<boolean>, changes: boolean? }): Node
	local comments: { Comment.Comment } = {
		{
			author = M.THEO,
			at = os.time() - 3600 * 2 + 200,
			body = "Should the soft wash follow the theme's strength here, or stay at 12% in both?",
			badge = if opts.changes then ds.ReviewBadge { state = "changes", size = "sm" } else ds.ReviewBadge { state = "commented", size = "sm" },
		},
		{
			author = M.ME,
			at = os.time() - 1800,
			body = "Each theme has its own: 14% in dark, 11% in light. A faint hue on charcoal all but vanishes.",
			mine = true,
		},
	}
	return ds.InlineThread {
		inset = opts.inset,
		joined = opts.joined,
		location = "ui/ds/badge.luau:42",
		resolved = opts.resolved,
		draft = opts.draft,
		me = M.ME,
		comments = comments,
	}
end

-- What happened to the pull request, in order: its commits (a run of them
-- as one event, so comments stand out), each comment, the checks' outcome,
-- each verdict, and the merge. Each with who and when.
export type Event = Git.Event

export type Verdict = { who: people.Who, verdict: string, at: number }

function M.events(o: { checks: { Check }, verdicts: { Verdict }?, merged: boolean? }): { Event }
	local now = os.time()
	local list: { Event } = {
		{ kind = "commits", who = M.ME, at = now - 7200, commits = M.COMMITS },
		{ kind = "comment", who = M.THEO, at = now - 7000, where = "ui/ds/badge.luau:42", body = "Should the soft wash follow the theme's strength here, or stay at 12% in both?" },
		{ kind = "checks", at = now - 3300, checks = o.checks, sha = M.COMMITS[#M.COMMITS].sha },
		{ kind = "comment", who = M.ME, at = now - 1800, where = "ui/ds/badge.luau:42", body = "Each theme has its own: 14% in dark, 11% in light. A faint hue on charcoal all but vanishes." },
	}
	for _, v in o.verdicts or {} do
		local review: Event = { kind = "review", who = v.who, at = v.at, verdict = v.verdict }
		table.insert(list, review)
	end
	if o.merged then
		local merged: Event = { kind = "merged", who = M.ME, at = now - 30 }
		table.insert(list, merged)
	end
	table.sort(list, function(a: Event, b: Event): boolean
		return a.at < b.at
	end)
	return list
end

function M.activity(events: { Event }): Node
	return Git.activity(events, { branch = M.PR.branch, base = M.PR.base })
end

return M
