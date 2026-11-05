-- Divider, link, breadcrumbs, key–value list; split button and toolbar.

local layout = require("@ds/layout")
local SplitButton = require("@ds/split_button")
local toolbar = require("@ds/toolbar")
local badge = require("@ds/badge")
local people = require("@ds/people")
local Segmented = require("@ds/segmented")
local ToggleButton = require("@ds/toggle_button")
local IconButton = require("@ds/icon_button").IconButton
local Button = require("@ds/button")
local Hint = require("@ds/hint")
local Rail = require("@ds/rail")
local kit = require("@demos/kit")

-- A scroll view's offset, how tall its content is and how tall it is, for
-- the rail over it.
local rail_at = ui.state("sx_rail_at", 0)
local rail_extent = ui.state("sx_rail_extent", 0)
local rail_view = ui.state("sx_rail_view", 0)

local view = ui.state("sx_view", "unified")
local wrap = ui.state("sx_wrap", true)
local blame = ui.state("sx_blame", false)
local said = ui.state("sx_said", "")

return {
	id = "structure",
	title = "Structure and actions",
	about = "The small pieces a page is put together with (dividers, links, breadcrumbs, key–value lists) and two action pieces: a split button (an action and its variants) and a toolbar (one Tab stop, groups a hairline apart).",
	demo = function()
		return kit.page(
			kit.block(
				"SPLIT BUTTON",
				kit.row(
					SplitButton {
						"Commit",
						on_click = function()
							said:set("Commit")
						end,
						items = {
							{ label = "Commit and push", icon = "arrow-up", shortcut = "cmd+shift+enter", on_click = function() said:set("Commit and push") end },
							{ label = "Commit and open pull request", icon = "pull-request", on_click = function() said:set("Commit and open PR") end },
							{ label = "Amend last commit", icon = "pen", description = "Folds these changes into “Add badges”.", on_click = function() said:set("Amend") end },
						},
					},
					SplitButton {
						"Merge",
						tone = "secondary",
						items = {
							{ label = "Squash and merge", icon = "git-merge" },
							{ label = "Rebase and merge", icon = "git-branch" },
						},
					},
					SplitButton { "Push", size = "sm", tone = "secondary", items = { { label = "Force push", danger = true } } },
					SplitButton { "Deploy", tone = "accent", items = { { label = "Deploy to staging" } } },
					SplitButton { "Commit", disabled = true, items = {} },
					Hint {
						function()
							return if said:get() == "" then "" else `Chose: {said:get()}`
						end,
					}
				)
			),
			kit.block(
				"TOOLBAR",
				ui.column {
					w = 620,
					gap = "$space.4",
					toolbar.Toolbar {
						label = "Diff",
						look = "bar",
						toolbar.Group {
							Segmented { value = view, label = "View", options = { { value = "unified", label = "Unified" }, { value = "split", label = "Split" } } },
						},
						toolbar.Group {
							ToggleButton { on = wrap, label = "Wrap lines", icon = "wrap-text" },
							ToggleButton { on = blame, label = "Blame", icon = "clock-circle" },
						},
						toolbar.Group {
							IconButton { icon = "collapse-arrows", label = "Collapse all" },
							IconButton { icon = "expand-arrows", label = "Expand all" },
						},
						toolbar.Spacer {},
						badge.DiffStat { added = 248, removed = 91 },
						Button { "Open file", size = "sm", tone = "ghost" },
					},
					ui.row {
						toolbar.Toolbar {
							label = "Find",
							look = "island",
							IconButton { icon = "arrow-up", label = "Previous" },
							IconButton { icon = "arrow-down", label = "Next" },
							IconButton { icon = "close", label = "Close" },
						},
					},
				}
			),
			kit.block(
				"BREADCRUMBS AND LINKS",
				layout.Breadcrumbs {
					items = {
						{ label = "comet-native", icon = "folder", on_click = function() end },
						{ label = "apps", on_click = function() end },
						{ label = "zgx", on_click = function() end },
						{ label = "ui", on_click = function() end },
						{ label = "ds", on_click = function() end },
						{ label = "badge.luau", icon = "file-code" },
					},
				},
				kit.row(
					layout.Link { "Learn more", on_click = function() end },
					layout.Link { "Open on GitHub", external = true, on_click = function() end },
					layout.Link { "Show all 21 files", tone = "quiet", on_click = function() end },
					layout.Link { "Privacy", size = "sm", tone = "quiet", on_click = function() end }
				)
			),
			kit.block(
				"DIVIDERS",
				ui.column {
					w = 420,
					gap = "$space.4",
					layout.Divider {},
					layout.Divider { label = "or" },
					layout.Divider { strong = true },
					ui.row { gap = "$space.3", items = "center", ui.text { "Left", color = "$text.secondary" }, layout.Divider { vertical = true }, ui.text { "Right", color = "$text.secondary" } },
				}
			),
			kit.block(
				"KEY–VALUE",
				ui.column {
					w = 440,
					layout.KeyValue {
						items = {
							{ key = "Branch", value = "zeron/primitives", mono = true, copy = true },
							{ key = "Base", value = "main", mono = true },
							{ key = "Author", value = people.Person { id = "ada", name = "Ada Lovelace", size = 18 } },
							{ key = "State", value = badge.PullRequest { state = "open", size = "md" } },
							{ key = "Checks", value = badge.Checks { state = "passing", passed = 12, total = 12 } },
							{ key = "Commit", value = "8b7afb30 Bump version to v0.2.129", mono = true, copy = true },
						},
					},
				}
			),
			kit.block(
				"SCROLLBAR",
				ui.column {
					w = 420,
					gap = "$space.3",
					Hint { "The app's rail over a scroll view: there while it scrolls and a moment after, or under the pointer; dragged, the view follows. Scroll the card." },
					-- A card of lines, taller than it is: the rail lays over the
					-- scroll in a `relative` box, its thumb 2 in from the card's
					-- edge and 4 from its ends, so it clears the corners.
					ui.div {
						id = "sx-rail-card",
						w = 320,
						h = 200,
						relative = true,
						rounded = "$radius.xl",
						bg = "$zeron.settings.card",
						border = 1,
						border_color = "$zeron.settings.edge",
						overflow = "hidden",
						on_resize = function(size)
							rail_view:set(size.h)
						end,
						ui.scroll {
							w_full = true,
							h_full = true,
							offset = rail_at,
							ui.column {
								w_full = true,
								px = "$space.3",
								py = "$space.2",
								on_resize = function(size)
									rail_extent:set(size.h)
								end,
								table.unpack((function(): { Node }
									local rows: { Node } = {}
									for n = 1, 40 do
										table.insert(rows, ui.text { `Line {n}`, h = 24, text_size = "$font.size.md", color = "$text.secondary" })
									end
									return rows
								end)()),
							},
						},
						Rail { offset = rail_at, extent = rail_extent, view = rail_view },
					},
				}
			)
		)
	end,
}
