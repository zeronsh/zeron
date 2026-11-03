-- A wall of member cards beside a screen's words: columns of cards drifting
-- up, each at its own pace, held still under the pointer. The wall runs the
-- window's full height, under the title bar's strip, and ends above the
-- footer's buttons, dissolving into the window as it goes, so it is cut by
-- nothing. Each card stands under a scrim of the window's colour
-- (`zeron.wall.scrim`: in the dark, a printed card is dimmed to the room's
-- light and no brighter than the way on; in light the paper is near the
-- window's own white and the scrim is clear), lifted from the one card under
-- the pointer, which then tilts and shows its foil as a card does.
--
--   CardWall {
--   	z = 0.78, over = 36,
--   	columns = { { members = { … }, cycle = 104000, start = 40 } },
--   }
--
-- It lays itself over its nearest positioned box (setup's Frame, the
-- gallery's stage), taking no room in the column it is put in. Its last
-- column runs past that box's right edge by `over`, cropped as a wall that
-- goes on would be. `CardWall.width(z, columns)` is how wide it is.
--
-- Reduce motion: nothing drifts, nothing rises in; the wall stands.

local motion = require("@self/motion")
local MemberCard = require("@self/member_card")
local prefers = require("@self/prefers")

export type Member = {
	number: number,
	name: string,
	since: string,
	layers: { MemberCard.Layer },
	holo: MemberCard.Holo,
	badges: { MemberCard.Badge }?,
}

-- A column: its cards (shown twice over, so its loop never shows a join),
-- the ms it takes to rise by one set of them, and where it starts (points
-- from the wall's top; negative, up into the window's edge, so no two
-- columns line up).
export type Column = { members: { Member }, cycle: number, start: number }

export type Props = {
	columns: { Column },
	-- The cards' size, of their own.
	z: number,
	-- How far the wall runs past its box's right edge.
	over: number,
	-- The cards' art holds still (a wall of many small cards).
	still: boolean?,
	-- Wallpaper: each card under a thicker veil, kept whole down the card
	-- (the words stand on it).
	veiled: boolean?,
	-- A wall that must show the same frame as another copy of it (the
	-- intro's stand-in for setup's window, and that window): its columns
	-- stand at their starts until this gives a time (`ui.now()`'s), then
	-- drift as reckoned from it, so every copy given the same time stands
	-- where the others do, whenever each mounted, with nothing held or
	-- started later. It doesn't rise in.
	epoch: (() -> number?)?,
	-- The first column's first card catches the light as this turns true.
	glint: (() -> boolean)?,
}

-- A card's own size (member_card's), and the gap between cards in a column
-- (space.3, in points for the loop's sum).
local W, H = 300, 420
local GAP = 12

local function reduced(): boolean
	return motion.reduced()
end

-- A card in the wall, at `z` of its size, under its scrim (or the thicker
-- veil of a wall that is wallpaper). The scrim lies on the art and thins out
-- down the card (its own clip fades it over the card's lower part), so the
-- sky steps back into the room's light while the paper keeps near its own
-- cream: dimmed paper read as grey, and the name on it as mud. It lifts
-- while the pointer is on the card (the card's own tilt and foil follow
-- it), and settles back a little more slowly than it lifted.
--
-- The wall holds still only while the pointer is on a card (`anims`, the
-- columns' drifts): over a gap, between the columns or between two cards of
-- one, it drifts on, and a card that drifts under a resting pointer stops
-- there, under it, as a card met by the hand does; off the card, it goes on
-- at once. (Nothing can flutter: a held card doesn't move, so it can't leave
-- the pointer by itself, and a moving one stops within a frame of meeting
-- it.)
local ART_FADE = 260
-- How the scrim goes as the pointer comes (ms), and comes back as it goes.
local LIFT, SETTLE = 160, 360
type CardProps = {
	m: Member,
	z: number,
	still: boolean,
	veiled: boolean?,
	anims: { any },
	standing: () -> boolean,
	glint: (() -> boolean)?,
}
local Card = ui.component(function(p: CardProps): Node
	-- The scrim's fade, a tween each way: a tween has one duration, so the
	-- scrim nests two, `lift` (inside) and `settle` (outside), their
	-- opacities multiplying. Lifted, the pair re-arm out of sight (`settle`
	-- down, then `lift` back up), so a leave is `settle`'s alone. (A leave
	-- within half a second eases both back at once.) Per hover, not an
	-- animation started on each: a pointer crossing a card again and again
	-- had each one replace the last before it finished.
	local lift = ui.use_state(1)
	local settle = ui.use_state(1)
	-- Which hover the timers belong to: a later one disowns them.
	local turn = 0
	-- The card's own hover (the one that tilts it): the scrim lifts and the
	-- wall holds; off it, the scrim settles and the wall goes on.
	local function hover(inside: boolean)
		turn += 1
		local mine = turn
		if inside then
			lift:set(0)
			ui.after(LIFT, function()
				if turn == mine then
					settle:set(0)
				end
			end)
			ui.after(LIFT + SETTLE, function()
				if turn == mine then
					lift:set(1)
				end
			end)
		else
			lift:set(1)
			settle:set(1)
		end
		for _, a in p.anims do
			if inside then
				a:pause()
			elseif not p.standing() then
				a:play()
			end
		end
	end
	return ui.div {
		zoom = p.z,
		relative = true,
		shrink = 0,
		MemberCard {
			number = p.m.number,
			name = p.m.name,
			since = p.m.since,
			layers = p.m.layers,
			holo = p.m.holo,
			badges = p.m.badges,
			still = p.still,
			shadow = "$zeron.wall.shadow",
			on_hover = hover,
			glint = p.glint,
		},
		ui.div {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			rounded = "$zeron.card.radius",
			overflow = "hidden",
			-- (Wallpaper keeps its veil whole: the words stand on it.)
			fade = if p.veiled then nil else { 0, ART_FADE },
			pointer_events = "none",
			opacity = ui.tween(settle, { duration = SETTLE, ease = "outQuad" }),
			ui.div {
				w_full = true,
				h_full = true,
				bg = if p.veiled then "$zeron.wall.veil" else "$zeron.wall.scrim",
				opacity = ui.tween(lift, { duration = LIFT, ease = "outQuad" }),
			},
		},
	}
end)

-- One column of cards, drifting up for ever: the cards twice over, moved by
-- one set's height each cycle so the join never shows. Given an epoch, it
-- stands at its start until there is one, then drifts as reckoned from it
-- (its time into the cycle, `(now - epoch) % cycle`, the engine's from
-- there). Its start is a margin, not a move: standing, it is no layer.
type ColumnProps = {
	c: Column,
	z: number,
	still: boolean,
	veiled: boolean?,
	anims: { any },
	epoch: (() -> number?)?,
	standing: () -> boolean,
	glint: (() -> boolean)?,
}
local ColumnOf = ui.component(function(p: ColumnProps): Node
	local start = p.c.start
	local step = H * p.z + GAP
	local loop = #p.c.members * step
	local kids: { Node } = {}
	for _ = 1, 2 do
		for i, m in p.c.members do
			table.insert(kids, Card {
				m = m,
				z = p.z,
				still = p.still,
				veiled = p.veiled,
				anims = p.anims,
				standing = p.standing,
				glint = if i == 1 and #kids == 0 then p.glint else nil,
			})
		end
	end
	local drift: any = nil
	-- Given an epoch, the drift goes where it reckons and on from there.
	local function reckon(epoch: number?)
		if epoch and drift then
			drift.currentTime = (ui.now() - epoch) % p.c.cycle
			drift:play()
		end
	end
	ui.effect(function()
		reckon(if p.epoch then p.epoch() else nil)
	end)
	return ui.column {
		gap = "$space.3",
		items = "center",
		shrink = 0,
		w = W * p.z,
		mt = ui.raw(start, "where the column starts: its drift's origin"),
		on_mount = function(el)
			if reduced() then
				return
			end
			drift = anime.animate(el, {
				y = { 0, -loop },
				duration = p.c.cycle,
				ease = "linear",
				loop = true,
				autoplay = p.epoch == nil,
			})
			table.insert(p.anims, drift)
			reckon(if p.epoch then p.epoch() else nil)
		end,
		table.unpack(kids),
	}
end)

-- The light across the wall: from above. The cards run to the window's top
-- edge (a long, gradual fade over the top, so a card rising out of the
-- window melts away rather than meeting an edge),
-- stand in full light through the window's upper half, and darken on the
-- way down into the footer, gone before its buttons, so the way on sits on
-- the window itself. A card drifting up rises out of the dark into the
-- light. (One long fall-off, not a band at each end: a short band dissolves
-- a card's paper into grey mid-card; this reads as a room lit from above.)
-- The fade is the clip's own, so it is the window's colour in either theme.
-- (The fall-off is short on purpose: a cream card over a dark window must
-- pass through grey as it goes, so a card is in that grey for as little of
-- its drift as the eased edge allows, and the two cards above it stand in
-- full light.)
local BREATH, FALL = 44, 130
-- Where the wall ends: above the footer's buttons (the footer's 72 less its
-- buttons' 32, halved, is where they begin: 52 up from the window's foot;
-- the wall stops a little above, so nothing of it stands under them).
local BUTTONS = 56

local M = {}

-- How wide a wall of `columns` columns of cards at `z` is.
function M.width(z: number, columns: number): number
	return W * z * columns + GAP * (columns - 1)
end

-- The wall: its columns, laid over its box's full height, clipped to it and
-- lit from above (each card holds it while the pointer is on it). It rises
-- into place as it opens, unless it's a copy given an epoch. (The fade is
-- the clip's; the rise is the box around it, so it doesn't fade.)
function M.Wall(props: Props): Node
	local anims: { any } = {}
	ui.on_cleanup(function()
		for _, a in anims do
			a:cancel()
		end
	end)
	local epoch = props.epoch
	-- Standing at its start: a copy given an epoch, with none yet.
	local function standing(): boolean
		return epoch ~= nil and epoch() == nil
	end
	local still = props.still == true
	local columns: { Node } = {}
	for i, c in props.columns do
		table.insert(columns, ColumnOf {
			c = c,
			z = props.z,
			still = still,
			veiled = props.veiled,
			anims = anims,
			epoch = epoch,
			standing = standing,
			glint = if i == 1 then props.glint else nil,
		})
	end
	local rises = not reduced() and epoch == nil
	return ui.div {
		id = "wall",
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		opacity = if rises then 0 else 1,
		on_mount = function(el)
			if rises then
				anime.animate(el, { opacity = { 0, 1 }, y = { 28, 0 }, duration = 900, ease = "out(3)" })
			end
		end,
		ui.div {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = BUTTONS,
			overflow = "hidden",
			fade = { BREATH, FALL },
			ui.row {
				absolute = true,
				top = 0,
				right = -props.over,
				gap = "$space.3",
				items = "start",
				table.unpack(columns),
			},
		},
	}
end

return M
