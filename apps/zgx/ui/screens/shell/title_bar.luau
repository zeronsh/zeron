-- The main window's title bar: the design system's (ds.TitleBar: the
-- window's buttons, the sidebar's toggle, back and forward, at Zeron's
-- places), over the window's top, its island showing while the sidebar is
-- collapsed over a new chat's wallpaper. At its other end, the page's own
-- controls: on a session, its panel's toggle. Plugins place theirs by
-- `#titlebar-actions`.
--
-- `TitleBar { collapsed = state, files = state, art = fn }`

local ds = require("@ds")
local route = require("@app/screens/shell/route")
local fit = require("@app/screens/shell/fit")
local boundary = require("@app/screens/shell/boundary")
local locale = require("@ds/locale")

-- The actions' last width, kept across a reload of the scripts.
local actions_width = ui.state("shell_titlebar_actions_w", 0)

export type Props = {
	collapsed: State<boolean>,
	files: State<boolean>,
	-- Whether the bar stands over a wallpaper (its island shows, collapsed).
	art: () -> boolean,
}

-- Production's "corner card" glyph (icons.rs `corner_card_glyph`): the
-- sidebar glyph's frame, a card in its top right.
local function corner_card(size: number): Node
	local s = size / 24
	local stroke = 1.75
	local function raw(v: number): any
		return ui.raw(v, "the glyph's own geometry, scaled from its 24-unit source")
	end
	return ui.div {
		w = size,
		h = size,
		shrink = 0,
		ui.div {
			absolute = true,
			left = (3 - stroke / 2) * s,
			top = (4 - stroke / 2) * s,
			w = (18 + stroke) * s,
			h = (16 + stroke) * s,
			rounded = raw((4 + stroke / 2) * s),
			border = "$zeron.glyph.stroke",
			border_color = "$text.secondary",
		},
		ui.div {
			absolute = true,
			right = 6.5 * s,
			top = 7.5 * s,
			w = 5.5 * s,
			h = 4.5 * s,
			rounded = raw(0.875 * s),
			bg = "$text.secondary",
		},
	}
end

return function(props: Props): Node
	local size = ds.island.metrics().control
	-- The session's lanes (side chats, subagents: screens/chat/lanes), where
	-- that screen is there.
	local function lanes(): any
		return boundary.load("@app/screens/chat/lanes")
	end
	local function badge(): number
		local l = lanes()
		return if l and l.badge then (l.badge :: any)() else 0
	end
	local actions = ui.row {
		id = "titlebar-actions",
		items = "center",
		gap = "$zeron.island.gap",
		-- The session's details (its lanes: side chats and subagents), as
		-- Zeron's titlebar toggle: its count while any run, open or not.
		ui.show(
			function(): boolean
				local l = lanes()
				-- (Off while the card is: `lanes.on`.)
				return route.current:get().name == "session" and l ~= nil and l.on ~= nil and (l.on :: any)() == true
			end,
			ui.div {
				relative = true,
				ds.IconButton {
					id = "titlebar-session-details",
					label = function(): string
						local l = lanes()
						local open = l and l.shown and (l.shown :: any)()
						return locale.s(if open then "shell.title_bar.hide_details" else "shell.title_bar.show_details")
					end,
					tooltip_side = "bottom",
					w = size,
					h = size,
					lit = function(): boolean
						local l = lanes()
						return l ~= nil and l.shown ~= nil and (l.shown :: any)() == true
					end,
					on_click = function()
						local l = lanes()
						if l and l.toggle then
							(l.toggle :: any)()
						end
					end,
					corner_card(16),
				},
				ui.show(
					function(): boolean
						return badge() > 0
					end,
					ui.row {
						absolute = true,
						right = -2,
						top = -2,
						h = 13,
						min_w = 13,
						px = ui.raw(3, "production's titlebar count badge (running_pill.rs): its digits 3 in from its round ends"),
						items = "center",
						justify = "center",
						rounded = "$radius.full",
						bg = "$zeron.status.working",
						pointer_events = "none",
						ui.text {
							function(): string
								local n = badge()
								return if n > 99 then "99+" else tostring(n)
							end,
							text_size = "$font.size.xs",
							weight = "$font.weight.medium",
							color = "$background",
							tabular = true,
						},
					}
				),
			}
		),
		ui.show(
			function(): boolean
				-- (A plugin's page has a panel of its own too.)
				local name = route.current:get().name
				return name == "session" or name == "plugin"
			end,
			-- The session's panel: the sidebar's glyph mirrored (its panel at
			-- the right), narrowing as the panel closes, as the left one does.
			ds.IconButton {
				label = ui.t("shell.title_bar.panel"),
				tooltip = locale.s("shell.title_bar.panel_tooltip", { key = ui.shortcut_text("cmd+r") }),
				tooltip_side = "bottom",
				w = size,
				h = size,
				on_click = function()
					fit.toggle_panel()
				end,
				ds.SidebarIcon { open = props.files, side = "right", size = 16 },
			}
		),
	}
	return ui.div {
		id = "titlebar",
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		-- (Over the sidebar's top and the page's: the pointer goes through
		-- to them, but for the bar's own controls.)
		pointer_events = "none",
		ds.TitleBar {
			collapsed = props.collapsed,
			on_toggle = fit.toggle_sidebar,
			island = function(): boolean
				return props.collapsed:get() and props.art()
			end,
			can_back = route.can_back,
			can_forward = route.can_forward,
			on_back = route.back,
			on_forward = route.forward,
			actions = actions,
			-- (Over a new chat's wallpaper, what's in it (a plugin's) stands on
			-- the island's glass, the sidebar open or not.)
			actions_island = props.art,
			actions_width = actions_width,
		},
	}
end
