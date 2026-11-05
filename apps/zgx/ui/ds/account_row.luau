-- You and settings, at a sidebar's foot (the settings window's, where the
-- main sidebar's own foot isn't): your avatar and name, your profile, shown
-- chosen while it's open; and beside you settings' own button, a toggle on
-- while they're open, a click again closing them. As the main sidebar's
-- foot draws them (ds/sidebar): a flat row, its wash, the same corner.
--
--   AccountRow { name = fn, id = fn, avatar = fn, on_user = fn, user_open = fn,
--                on_settings = fn, settings_open = fn }

local Surface = require("@self/surface")
local Avatar = require("@self/avatar")
local ToggleButton = require("@self/toggle_button")
local FadeText = require("@self/fade_text")
local locale = require("@self/locale")

export type Props = {
	name: () -> string,
	id: (() -> string?)?,
	avatar: (() -> string?)?,
	on_user: () -> (),
	user_open: () -> boolean,
	on_settings: () -> (),
	settings_open: () -> boolean,
}

-- A sidebar row's height (ds/sidebar's SLOT).
local SLOT = 30

return function(props: Props): Node
	local on = ui.use_state(props.settings_open())
	ui.effect(function()
		local v = props.settings_open()
		if on:peek() ~= v then
			on:set(v)
		end
	end)
	return ui.row {
		w_full = true,
		gap = "$zeron.list.gap",
		items = "center",
		Surface {
			depth = "flat",
			selected = props.user_open,
			on_click = props.on_user,
			label = props.name,
			grow = 1,
			min_w = 0,
			h = SLOT,
			border = 0,
			items = "center",
			direction = "row",
			-- (Laid out as the main sidebar's foot, so nothing moves between
			-- them: the avatar centred in a slot as tall as the row, the name
			-- straight after it.)
			ui.div {
				w = SLOT,
				h = SLOT,
				shrink = 0,
				items = "center",
				justify = "center",
				ui.presence(function(): string
					return (if props.id then props.id() else nil) or props.name()
				end, function(seed: string): Node
					return Avatar {
						seed = seed,
						size = 20,
						label = props.name(),
						src = function(): string?
							return if props.avatar then props.avatar() else nil
						end,
					}
				end),
			},
			FadeText {
				props.name,
				grow = 1,
				min_w = 0,
				mr = "$space.2",
				text_size = "$font.size.md",
				weight = "$font.weight.medium",
				color = "$text.primary",
			},
		},
		ToggleButton {
			on = on,
			-- (ToggleButton's label is its tooltip: a string.)
			label = locale.s("ds.account_row.settings"),
			icon = "settings",
			tone = "quiet",
			w = SLOT,
			h = SLOT,
			rounded = "$radius.lg",
			shrink = 0,
			on_change = function()
				props.on_settings()
			end,
		},
	}
end
