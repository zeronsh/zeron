-- Badges, tags, pills, counts, dots, and the git pages' badges.

local badge = require("@ds/badge")
local people = require("@ds/people")
local kit = require("@demos/kit")

local filters = ui.state("bd_filters", { true, false, false })

local TEAM: { people.Who } = {
	{ id = "ada", name = "Ada Lovelace" },
	{ id = "grace", name = "Grace Hopper" },
	{ id = "alan", name = "Alan Turing" },
	{ id = "katherine", name = "Katherine Johnson" },
	{ id = "edsger", name = "Edsger Dijkstra" },
	{ id = "barbara", name = "Barbara Liskov" },
}

return {
	id = "badges",
	title = "Badges and tags",
	about = "Small labels on one look: a colour's ink on a faint wash of it, so a tone, a pull request's state or a file's status each make one. Tags are chips that can be chosen or removed.",
	demo = function()
		local tones = {}
		for _, t in { "neutral", "accent", "info", "success", "warning", "danger" } do
			table.insert(tones, badge.Badge { string.upper(string.sub(t, 1, 1)) .. string.sub(t, 2), tone = t :: any })
		end
		local tones_md = {}
		for _, t in { "neutral", "accent", "info", "success", "warning", "danger" } do
			table.insert(tones_md, badge.Badge { string.upper(string.sub(t, 1, 1)) .. string.sub(t, 2), tone = t :: any, size = "md", dot = true })
		end
		local filter_tags = {}
		for i, name in { "Mine", "Needs review", "Has conflicts" } do
			table.insert(filter_tags, badge.Tag {
				name,
				selected = function()
					return filters:get()[i]
				end,
				on_click = function()
					filters:set_key(i, not filters:peek()[i])
				end,
			})
		end
		local now = os.time()
		return kit.page(
			kit.block("TONES", kit.row(table.unpack(tones)), kit.row(table.unpack(tones_md))),
			kit.block(
				"LOOKS AND SHAPES",
				kit.row(
					badge.Badge { "Soft", tone = "accent", size = "md" },
					badge.Badge { "Solid", tone = "accent", size = "md", look = "solid" },
					badge.Badge { "Outline", tone = "accent", size = "md", look = "outline" },
					badge.Badge { "Pill", tone = "success", size = "md", shape = "pill" },
					badge.Badge { "New", tone = "danger", look = "solid", shape = "pill" },
					badge.Badge { "v0.2.129", mono = true },
					badge.Badge { icon = "star-bold", tone = "warning", label = "Starred" }
				)
			),
			kit.block(
				"COUNTS AND DOTS",
				kit.row(
					badge.Count { n = 3 },
					badge.Count { n = 12, tone = "accent" },
					badge.Count { n = 4, tone = "danger", solid = true },
					badge.Count { n = 240, solid = true, tone = "accent" },
					badge.Dot { tone = "success", label = "Online" },
					badge.Dot { tone = "warning", pulse = true, label = "Working" },
					badge.Dot { tone = "danger", label = "Failed" },
					badge.Dot { color = "$text.tertiary", label = "Offline" }
				)
			),
			kit.block(
				"TAGS",
				kit.row(
					badge.Tag { "frontend", hue = "violet", on_remove = function() end },
					badge.Tag { "design-system", hue = "emerald", on_remove = function() end },
					badge.Tag { "bug", hue = "red" },
					badge.Tag { "zeron/primitives", icon = "git-branch" },
					badge.Tag { "small", size = "sm", on_remove = function() end }
				),
				kit.row(table.unpack(filter_tags))
			),
			kit.block(
				"PULL REQUESTS",
				kit.row(
					badge.PullRequest { state = "open", number = 1142, title = "Primitives" },
					badge.PullRequest { state = "merged", number = 1089 },
					badge.PullRequest { state = "closed", number = 1050 },
					badge.PullRequest { state = "draft", number = 1151 }
				),
				kit.row(
					badge.PullRequest { state = "open", size = "md" },
					badge.PullRequest { state = "merged", size = "md" },
					badge.PullRequest { state = "closed", size = "md" },
					badge.PullRequest { state = "draft", size = "md" }
				)
			),
			kit.block(
				"REVIEWS AND CHECKS",
				kit.row(
					badge.Review { state = "approved" },
					badge.Review { state = "changes" },
					badge.Review { state = "commented" },
					badge.Review { state = "pending" }
				),
				kit.row(
					badge.Checks { state = "passing", passed = 12, total = 12 },
					badge.Checks { state = "failing", passed = 9, total = 12 },
					badge.Checks { state = "running", passed = 4, total = 12 },
					badge.Checks { state = "passing" }
				)
			),
			kit.block(
				"FILES AND DIFFS",
				kit.row(
					badge.FileStatus { status = "added" },
					badge.FileStatus { status = "modified" },
					badge.FileStatus { status = "deleted" },
					badge.FileStatus { status = "renamed" },
					badge.FileStatus { status = "untracked" },
					badge.FileStatus { status = "conflict" },
					badge.FileStatus { status = "added", look = "badge" },
					badge.FileStatus { status = "deleted", look = "badge" }
				),
				kit.row(
					badge.DiffStat { added = 12, removed = 3 },
					badge.DiffStat { added = 248, removed = 191, bar = true },
					badge.DiffStat { added = 1240, removed = 12, bar = true, size = "md" },
					badge.DiffStat { added = 0, removed = 87, bar = true }
				)
			),
			kit.block(
				"PEOPLE AND TIME",
				kit.row(
					people.AvatarStack { people = TEAM, max = 3 },
					people.AvatarStack { people = { TEAM[1], TEAM[2] }, size = 24 },
					people.AvatarStack { people = TEAM, size = 16, max = 4 },
					people.Person { id = "ada", name = "Ada Lovelace", detail = "author" },
					people.Timestamp { at = now - 20 },
					people.Timestamp { at = now - 60 * 7 },
					people.Timestamp { at = now - 3600 * 5 },
					people.Timestamp { at = now - 86400 * 9 },
					people.Timestamp { at = now - 3600 * 5, compact = true }
				)
			)
		)
	end,
}
