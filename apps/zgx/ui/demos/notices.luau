-- Callouts, banners and toasts.

local banner = require("@ds/banner")
local toast = require("@ds/toast")
local Button = require("@ds/button")
local kit = require("@demos/kit")

local shown = ui.state("nt_shown", true)

return {
	id = "notices",
	title = "Callouts, banners, toasts",
	about = "A tone says how much it matters. Callouts sit in the flow, banners across a page, toasts in its corner.",
	-- It fills the window, so the toasts' stack sits in its corner.
	fill = true,
	demo = function()
		return ui.overlay { w_full = true, h_full = true, min_w = 0, ui.scroll { w_full = true, h_full = true, min_w = 0, fade = { 16, 16 }, fade_overflow = true, kit.page(
			kit.block(
				"BANNERS",
				ui.column {
					gap = "$space.2",
					w_full = true,
					-- Across a window or panel, edge to edge (`edge`): square, a
					-- hairline under it.
					banner.Banner { edge = true, tone = "danger", title = "Can't reach GitHub.", body = "Pull requests and checks are from 4 minutes ago.", action = { label = "Retry" } },
					banner.Banner { edge = true, tone = "warning", title = "This branch is 12 commits behind main.", action = { label = "Update branch" }, on_dismiss = function() end },
					banner.Banner { edge = true, tone = "info", title = "Zeron 0.3 is ready.", body = "Restart to update.", actions = { { label = "Restart", primary = true }, { label = "Later" } } },
					-- Inside a page, among its cards: the same line, rounded.
					banner.Banner { tone = "danger", title = "Tests failed", body = "2 of 318. Fix them on the branch and the check runs again.", action = { label = "See what failed" } },
				}
			),
			kit.block(
				"CALLOUTS",
				ui.column {
					gap = "$space.3",
					w = 560,
					banner.Callout { tone = "info", title = "Agents run in a worktree of their own.", body = "Your checkout stays as you left it; changes come back as a branch you review." },
					banner.Callout {
						tone = "warning",
						title = "Merge conflicts in 2 files",
						body = "ds/tokens.luau and ds/themes/zeron-dark.json changed on main too.",
						actions = { { label = "Resolve with agent", primary = true }, { label = "Open in editor" } },
					},
					banner.Callout { tone = "success", title = "Signed in as ada@example.net", action = { label = "Switch" } },
					ui.show(
						function()
							return shown:get()
						end,
						banner.Callout {
							tone = "danger",
							title = "The pre-commit hook failed.",
							body = "luau-lsp: 3 type errors in ds/badge.luau.",
							action = { label = "Show output" },
							on_dismiss = function()
								shown:set(false)
							end,
						}
					),
					banner.Callout { tone = "neutral", title = "Settings sync across your devices.", body = "Except keys and paths, which belong to this Mac." },
				}
			),
			kit.block(
				"TOASTS",
				kit.row(
					Button {
						"Success",
						tone = "secondary",
						on_click = function()
							toast.show { title = "Pushed to origin", body = "zeron/primitives, 3 commits.", tone = "success" }
						end,
					},
					Button {
						"With an action",
						tone = "secondary",
						on_click = function()
							toast.show { title = "Branch deleted", body = "zeron/old-composer", action = { label = "Undo", run = function() end } }
						end,
					},
					Button {
						"Warning",
						tone = "secondary",
						on_click = function()
							toast.show { title = "Checks are slow today", body = "GitHub Actions is queueing jobs.", tone = "warning" }
						end,
					},
					Button {
						"Danger (stays)",
						tone = "secondary",
						on_click = function()
							toast.show { title = "Commit failed", body = "pre-commit hook exited with 1.", tone = "danger", action = { label = "Show output", run = function() end } }
						end,
					},
					Button {
						"Plain",
						tone = "ghost",
						on_click = function()
							toast.show { title = "Copied to the clipboard" }
						end,
					}
				)
			)
		) }, toast.Host {} }
	end,
}
