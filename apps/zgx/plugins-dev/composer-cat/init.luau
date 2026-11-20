-- Composer cat: a pixel-art cat sitting on the message box's top edge
-- ("ui.add"), placed in the composer (before its footer, #composer-toolbar)
-- and laid absolute against the composer's body, so it sits on the pill
-- wherever the composer is and however wide. It takes no pointer:
-- everything under it works as before.
--
-- The cat is a sprite: a 28 by 24 grid (art/cat-*.png, drawn by
-- art-src/sprite.py), each pixel 2 points, sampled nearest so every pixel
-- stays square and hard-edged at any scale. Its life is frames: now and then
-- it blinks, flicks its tail, or twitches an ear, one at a time. Between
-- those nothing changes, so nothing is drawn. With Reduce motion it sits
-- still.

local zeron = require("./zeron")("composer-cat")

-- The sprite's grid, a pixel's size (points), and where it sits: its
-- outline's last row over the pill's top edge, 84 in from the composer's
-- right edge (clear of the draft's first line).
local GRID_W, GRID_H, PIXEL = 28, 24, 2
local FOOT = 22 * PIXEL
local RIGHT = 84

-- Each motion: its frames and how long each shows (ms), back to idle after.
local BLINK = { { "blink", 140 } }
local FLICK = { { "tail-out", 130 }, { "idle", 90 }, { "tail-over", 130 }, { "idle", 90 }, { "tail-out", 110 } }
local TWITCH = { { "ear", 120 }, { "idle", 80 }, { "ear", 120 } }
-- How often each comes (seconds, a range).
local EVERY = {
	{ motion = BLINK, range = { 3.5, 7 } },
	{ motion = FLICK, range = { 7, 12 } },
	{ motion = TWITCH, range = { 10, 18 } },
}

local function reduced(): boolean
	local p = (ui :: any).prefers
	return p ~= nil and p.reduced_motion ~= nil and p.reduced_motion:peek() == true
end

local function within(range: { number }): number
	return (range[1] + math.random() * (range[2] - range[1])) * 1000
end

-- At most this many timers are ever pending (checked as each motion comes
-- round): one per motion's next turn, and the playing motion's frames and
-- its end. Fired timers are let go, so a composer left open all day holds
-- no more than at its first minute.
local LONGEST = 0
for _, each in EVERY do
	LONGEST = math.max(LONGEST, #each.motion)
end
local MOST = #EVERY + LONGEST + 1

local function Cat(): Node
	local frame = ui.use_state("idle")
	-- Each motion's next turn (its one pending timer, replaced as it fires),
	-- and the playing motion's frame timers (let go when it ends).
	local turns: { [number]: (() -> ())? } = {}
	local steps: { () -> () } = {}
	local busy = false
	local gone = false
	local function pending(): number
		local n = #steps
		for i = 1, #EVERY do
			if turns[i] then
				n += 1
			end
		end
		return n
	end
	ui.on_cleanup(function()
		gone = true
		for i = 1, #EVERY do
			local cancel = turns[i]
			if cancel then
				cancel()
			end
		end
		for _, cancel in steps do
			cancel()
		end
	end)
	-- A motion's frames one after another, then idle. (One at a time: one
	-- that comes while another plays waits for its next turn.)
	local function play(motion: { { any } })
		if gone or busy then
			return
		end
		busy = true
		steps = {}
		local at = 0
		for _, step in motion do
			local name, ms = step[1], step[2]
			table.insert(steps, ui.after(at, function()
				frame:set(name)
			end))
			at += ms
		end
		table.insert(steps, ui.after(at, function()
			frame:set("idle")
			busy = false
			-- (All its timers have fired: none to keep.)
			steps = {}
		end))
	end
	if not reduced() then
		for i, each in EVERY do
			local function turn()
				turns[i] = nil
				play(each.motion)
				turns[i] = ui.after(within(each.range), turn)
				assert(pending() <= MOST, "composer-cat: timers left pending")
			end
			turns[i] = ui.after(within(each.range), turn)
		end
	end
	return ui.image {
		id = "composer-cat",
		src = function(): string
			return `art/cat-{frame:get()}.png`
		end,
		image_rendering = "pixelated",
		absolute = true,
		right = RIGHT,
		top = -FOOT,
		w = GRID_W * PIXEL,
		h = GRID_H * PIXEL,
		pointer_events = "none",
		aria_hidden = true,
	}
end

zeron.place { at = "#composer-toolbar", before = Cat }
