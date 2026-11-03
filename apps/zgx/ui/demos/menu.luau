-- The gallery's Menu page: the pieces every menu is built of (rows,
-- headings, rules, groups, the panel), drawn in place, and one opened from
-- an icon button.

local ds = require("@ds")
local Menu = ds.Menu

local function block(title: string, ...: Child): Node
	return ui.column { gap = "$space.3", ds.SectionTitle { title }, ... }
end

return {
	id = "menu",
	title = "Menu",
	about = "One row, heading, rule and group for every menu: the sidebar's, the dropdown's, the composer's, the context menu. Glyph or not, names line up.",
	demo = function(): Node
		local by = ui.state("menu_by", "project")
		local branch = ui.state("menu_branch", true)
		local pr = ui.state("menu_pr", false)
		local open = ui.state("menu_open", false)
		local function choice(value: string): () -> boolean
			return function(): boolean
				return by:get() == value
			end
		end
		local function panel(): Node
			return Menu.Panel {
				w = "$zeron.menu.width",
				shrink = 0,
				Menu.Group {
					heading = "Organize",
					Menu.Row { icon = "folder", label = "By project", checked = choice("project"), on_click = function() by:set("project") end },
					Menu.Row { icon = "laptop", label = "By device", checked = choice("device"), on_click = function() by:set("device") end },
					Menu.Row { icon = "list", label = "None", checked = choice("none"), on_click = function() by:set("none") end },
				},
				Menu.Rule {},
				Menu.Group {
					heading = "Show",
					Menu.Row { icon = "git-branch", label = "Branch", checked = branch, on_click = function() branch:toggle() end },
					Menu.Row { icon = "pull-request", label = "Pull request", checked = pr, on_click = function() pr:toggle() end },
				},
				Menu.Rule {},
				Menu.Group {
					Menu.Row { icon = "copy", label = "Copy link", shortcut = "cmd+shift+c" },
					Menu.Row { icon = "split-columns", label = "Open beside", submenu = true },
					Menu.Row { label = "No glyph: its name still lines up" },
					Menu.Row { icon = "archive-minimalistic", label = "Archive", disabled = true },
					Menu.Row { icon = "trash-bin-minimalistic", label = "Delete", danger = true },
				},
			}
		end
		local trigger = ds.IconButton {
			icon = "more-horizontal",
			label = "Open the menu",
			lit = open,
			on_click = function()
				open:set(true)
			end,
		}
		return ui.column {
			gap = "$space.6",
			block("IN PLACE", panel()),
			block(
				"WITH A DESCRIPTION",
				Menu.Panel {
					w = 280,
					shrink = 0,
					Menu.Group {
						Menu.Row { icon = "folder", label = "Local", description = "Works in the folder itself.", checked = function() return true end },
						Menu.Row { icon = "worktree", label = "Worktree", description = "An isolated copy, on a branch of its own." },
					},
				}
			),
			block(
				"OPENED",
				ui.row {
					-- (The popover draws its anchor, the trigger, in place.)
					ui.popover(Menu.panel {
						open = open,
						anchor = trigger,
						placement = { side = "bottom", align = "start", offset = 4 },
						w = "$zeron.menu.width",
						build = function()
							return Menu.Group {
								Menu.Row { icon = "pen", label = "Rename", on_click = function() open:set(false) end },
								Menu.Row { icon = "archive-minimalistic", label = "Archive all", on_click = function() open:set(false) end },
								Menu.Row { icon = "trash-bin-minimalistic", label = "Delete", danger = true, on_click = function() open:set(false) end },
							}
						end,
					}),
				}
			),
		}
	end,
}
