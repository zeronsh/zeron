-- The screen after an account is made, before the organization: your
-- member card, the one gift in setup. The card is the hero: whole, in full
-- light, with your name on it, dealt onto the screen as the page opens
-- (member_card's `arrive`: up from below, its foil catching the light
-- once), tilting and shining under the pointer as a card does. The offer
-- stands with it, one quiet control: Share on X, Copy link, Save image,
-- each with its glyph, in one surface; none of it in the way of going on,
-- which is Continue alone at the bottom right, the one filled button.
--
-- Three compositions, on the page's Direction switch:
--   Stage: the card centred and large on a glow of its own sky, a line
--     above it, the control under it. The eye lands on the card; the
--     feeling is a spotlight.
--   Tilt: the card larger still, set down at a tilt and running off the
--     window's top right corner, the words and the control at the left.
--     The eye lands on the card; the feeling is a card just dealt.
--   Deal: the card alone at the centre, bigger than either; the words at
--     the left and the control at the right arrive after it has. The eye
--     has only the card; the feeling is a quiet unboxing.
--
-- Each action settles into its done word in place (Shared, Copied: the
-- control keeps its shape), and a failure says so on a line kept under the
-- control, so nothing moves.
--
-- What a real one needs that isn't here: a public page for a card
-- (zeron.sh/<something>/1378) with its picture as the post's preview, the
-- X post intent that opens with that link, the clipboard, and a way to
-- write the card to a PNG.

local ds = require("@ds")
local kit = require("@demos/example_kit")
-- The share control, the one the Profile page shows beside the card too.
local CardActions = require("@ds/card_actions")
local prefers = require("@ds/prefers")

local shown = ui.state("ex_card_state", "ready")
local direction = ui.state("ex_card_direction", "stage")

-- Your card: the next number (there were 1,373 members on Oct 28, 2026),
-- made today, no badge yet. A made-up name.
local SKY = "assets/cards/blossom/sky.webp"
local YOU = {
	number = 1378,
	name = "Mara",
	-- The username chosen a step before: the card's address, what sharing
	-- shares.
	username = "mara",
	since = "Oct 30, 2026",
	layers = {
		{ src = SKY, depth = 0 },
		{ src = "assets/cards/blossom/far.webp", depth = 1 },
		{ src = "assets/cards/blossom/near.webp", depth = 2 },
		{ src = "assets/cards/blossom/detail.webp", depth = 3 },
	},
	holo = "glyph",
}

-- A card's own size (member_card's), and setup's window's (the stage's).
local W, H = 300, 420
local WIN_W, WIN_H = 680, 500

local STATES = {
	{ value = "ready", label = "Ready" },
	{ value = "shared", label = "Shared" },
	{ value = "copied", label = "Copied" },
	{ value = "failed", label = "Failed" },
}
local DIRECTIONS = {
	{ value = "stage", label = "Stage" },
	{ value = "tilt", label = "Tilt" },
	{ value = "deal", label = "Deal" },
}

local function reduced(): boolean
	return prefers.reduced_motion:get()
end

local function status(): string
	return if shown:get() == "failed" then "Couldn't save the picture. Try again." else ""
end

-- The control: the three ways to share, as the design system has them
-- (ds/card_actions): a row of them under a card, a column beside one.
local function control(vertical: boolean?, w: number?): Node
	return CardActions { state = shown, vertical = vertical, w = w }
end

-- The failure's line: kept at its height while there is none (`lines` of
-- them, where the column is narrow enough to wrap it).
local function failure(align: TextAlign?, lines: number?): Node
	local line = ui.theme.get("font.line.md") :: number
	return ui.text {
		status,
		align = align,
		h = line * (lines or 1),
		text_size = "$font.size.md",
		line_height = "$font.line.md",
		color = "$zeron.tone.danger.ink",
		wrap = true,
	}
end

-- The card, at `z` of its size, arriving.
local function card(z: number): Node
	return ui.div {
		zoom = z,
		shrink = 0,
		ds.MemberCard {
			number = YOU.number,
			name = YOU.name,
			since = YOU.since,
			layers = YOU.layers,
			holo = YOU.holo,
			arrive = true,
		},
	}
end

-- The card's own sky behind it, blurred large and faint, dissolving into
-- the window: the room lit by the card. (A blur and an opacity make it one
-- cached layer, about the size given: small.) A glow has no shape: each of
-- its clip's fade bands runs all the way to the box's middle, so from the
-- card outward it is one eased fall-off on every side with no end to find,
-- and the only crease, where the bands from either side meet, lies under
-- the card. (A band shorter than that ends in a knee, and the blurred art
-- read as a slab either side of the card.)
local GLOW_BLUR = 36
local function glow(box: { left: number, right: number, top: number, bottom: number }): Node
	local w = WIN_W - box.left - box.right
	local h = WIN_H - box.top - box.bottom
	return ui.div {
		absolute = true,
		left = box.left,
		right = box.right,
		top = box.top,
		bottom = box.bottom,
		overflow = "hidden",
		fade_sides = { h / 2, w / 2, h / 2, w / 2 },
		pointer_events = "none",
		ui.div {
			w_full = true,
			h_full = true,
			blur = GLOW_BLUR,
			opacity = "$zeron.card.glow",
			ui.image { src = SKY, w_full = true, h_full = true, fit = "cover" },
		},
	}
end

-- Words and a control that come after the card has arrived (`after` ms),
-- rising a little; at once with Reduce motion.
local function later(after: number, node: Node): Node
	return ui.div {
		opacity = if reduced() then 1 else 0,
		on_mount = function(el)
			if reduced() then
				return
			end
			ui.after(after, function()
				anime.animate(el, { opacity = { 0, 1 }, y = { 10, 0 }, duration = 420, ease = "out(3)" })
			end)
		end,
		node,
	}
end

-- The card's address, the username chosen a step before: a quiet line by
-- the card, what the ways to share it share.
local function address(align: TextAlign?): Node
	return ui.text {
		`zeron.sh/c/{YOU.username}`,
		align = align,
		text_size = "$font.size.sm",
		line_height = "$font.line.sm",
		color = "$text.tertiary",
	}
end

-- A line of words at display size's little sister: the one line above or
-- beside the card.
local function headline(text: string, align: TextAlign?): Node
	return ui.text {
		text,
		align = align,
		text_size = "$font.size.xl",
		line_height = "$font.line.xl",
		weight = "$font.weight.semibold",
		color = "$text.primary",
		wrap = true,
	}
end

-- Stage: centred. The line above the card, the card on its glow, the
-- control under it, the failure's line under that, all on the window's
-- middle; the way on stays alone at the right.
local function stage_direction(inset: number): Node
	-- (The stack, 48 down from the top: the line, the card, the control
	-- and the failure's line end 19 above the footer's buttons.)
	local z = 0.64
	return ui.div {
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		glow { left = 40, right = 40, top = -40, bottom = 40 },
		ui.column {
			absolute = true,
			left = 0,
			right = 0,
			top = 48,
			items = "center",
			gap = "$space.3",
			headline("Share your card", "center"),
			card(z),
			address(),
			control(),
			ui.div { mt = ui.raw(-4, "the failure's line a touch nearer the control than the control is to the card"), failure("center") },
		},
	}
end

-- Tilt: the card set down large at a tilt, its top right corner past the
-- window's, the words and the control at the left where every step's words
-- stand, the way on under the card's foot with room to spare.
local function tilt_direction(inset: number, pad: number): Node
	local z = 0.9
	return ui.div {
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		glow { left = 240, right = -80, top = -80, bottom = 80 },
		ui.div {
			absolute = true,
			left = 372,
			top = -46,
			rotate = 8,
			card(z),
		},
		ui.column {
			absolute = true,
			left = inset + pad,
			top = 126,
			w = 300,
			gap = "$space.5",
			ds.Text { "Share\nyour card", style = "display", heading = 1, wrap = true },
			ui.column {
				items = "start",
				gap = "$space.3",
				address(),
				control(true, 168),
				failure(),
			},
		},
	}
end

-- Deal: the card alone at the centre, biggest of the three; the words at
-- the left and the control at the right come once it has landed, the
-- control's edge on the inset where the way on ends.
local function deal_direction(inset: number, pad: number): Node
	local z = 0.8
	local side = 152
	return ui.div {
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		ui.row {
			absolute = true,
			left = 0,
			right = 0,
			top = 76,
			justify = "center",
			card(z),
		},
		later(700, ui.column {
			absolute = true,
			left = inset + pad,
			top = 190,
			w = side,
			gap = "$space.2",
			ds.Text { "Share\nyour card", style = "title", heading = 1, wrap = true },
			ui.text { "Yours from today.", text_size = "$font.size.md", line_height = "$font.line.md", color = "$text.secondary", wrap = true },
			address(),
		}),
		later(820, ui.column {
			absolute = true,
			right = inset,
			top = 190,
			w = side,
			gap = "$space.3",
			control(true, side),
			failure(nil, 2),
		}),
	}
end

return {
	id = "ex_card",
	title = "Your card",
	about = "Right after an account is made: your member card, whole and in the light, dealt onto the screen with its foil catching the light once, tilting under the pointer. Share it on X, copy its link or keep the picture, from one quiet control beside it; or just continue. Three compositions to compare: centred on a glow, tilted off the corner, or alone at the centre with the rest arriving after. Each action settles in place; a failure says so without moving anything.",
	demo = function()
		local inset = ui.theme.get("space.6") :: number
		local pad = ui.theme.get("space.4") :: number
		local builders: { [string]: () -> Node } = {
			stage = function()
				return stage_direction(inset)
			end,
			tilt = function()
				return tilt_direction(inset, pad)
			end,
			deal = function()
				return deal_direction(inset, pad)
			end,
		}
		return kit.stage {
			states = STATES,
			state = shown,
			also = ui.row {
				gap = "$space.2",
				items = "center",
				ds.Text { "Direction", style = "caption", tone = "tertiary" },
				ds.Segmented { value = direction, options = DIRECTIONS, label = "Direction" },
			},
			w = WIN_W,
			h = WIN_H,
			kit.step {
				continue = { label = "Continue" },
				-- Laid over the window (the stage is what it positions
				-- against): the footer paints after it, so the way on stands
				-- on top of the glow.
				ui.presence(direction, function(d: string): Node
					return builders[d]()
				end, { enter = { opacity = { 0, 1 }, duration = 240 }, exit = { opacity = 0, duration = 160 } }),
			},
		}
	end,
}
