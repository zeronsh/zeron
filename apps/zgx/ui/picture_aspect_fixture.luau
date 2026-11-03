-- A wide picture (2400 by 600, a row of circles) in every small frame that
-- crops one: the composer's chip and its Appshot chip, a sent message's
-- thumbnail, an avatar, and a bare `ui.image` cropped with a `fallback` and
-- without (the dock's thumbnail's shape). Each is cropped, never squeezed:
-- a circle stays round. For src/picture_aspect_tests.rs.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local cargo = require("@ds/composer_cargo")
local UserMessage = require("@ds/user_message")
local Avatar = require("@ds/avatar")

local WIDE = "assets/pictures/circles.png"

local CARGO = ui.state("aspect_cargo", {
	{ id = "c1", kind = "image", name = "wide.png", src = WIDE },
	{ id = "c2", kind = "appshot", name = "Safari", src = WIDE, appshot = { app = "Safari" } },
} :: { cargo.Item })

-- A frame of `w` by `h` cropping the picture (`fit = "cover"`), with or
-- without a fallback.
local function bare(id: string, fallback: boolean): Node
	return ui.overlay {
		id = id,
		w = 56,
		h = 40,
		rounded = 8,
		overflow = "hidden",
		ui.image {
			src = WIDE,
			fit = "cover",
			w_full = true,
			h_full = true,
			fallback = if fallback then ui.div {} else nil,
		},
	}
end

return ui.column {
	w_full = true,
	h_full = true,
	p = 24,
	gap = 16,
	bg = "$zeron.window",
	ui.row {
		gap = 16,
		bare("aspect-cover", false),
		bare("aspect-cover-fallback", true),
		ui.div { id = "aspect-avatar", Avatar { seed = "wide", size = 40, src = WIDE, label = "Wide face" } },
	},
	ui.div { id = "aspect-cargo", cargo.Strip { items = CARGO } },
	ui.div {
		id = "aspect-sent",
		w = 600,
		UserMessage {
			id = "m1",
			text = "These two?",
			attachments = { { kind = "image", name = "wide.png", src = WIDE } },
		},
	},
}
