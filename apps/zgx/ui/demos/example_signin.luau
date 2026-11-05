-- Setup's first screen for someone signed out, as the app shows it
-- (screens/onboarding/signin: the words, the footer, the one-card column),
-- with the walls it was chosen from beside it and the states sign-in can be
-- in. Every wall is ds.CardWall; this page only says how each is laid.
--
-- Three directions, to compare on the page's switch: One card, the app's,
-- a single column of large cards at the window's edge, one in view at a
-- time; Columns, two columns of cards beside the words; Wall, the cards
-- running the window's whole width behind the words, which stand in a
-- scrim over them. Each is its own composition; the words, the footer and
-- the states are the screen's.
--
-- Reduce motion: nothing drifts, nothing rises in; the wall stands.

local ds = require("@ds")
local kit = require("@demos/example_kit")
local signin = require("@screens/onboarding/signin")

local shown = ui.state("ex_si_state", "ready")
local direction = ui.state("ex_si_direction", "one")

-- A card's own height (member_card's), and the gap between cards in a
-- column (space.3, in points for the sums).
local H = 420
local GAP = 12
-- Setup's window (the stage's).
local WIN_W, WIN_H = 680, 500

local DIRECTIONS = {
	{ value = "one", label = "One card" },
	{ value = "columns", label = "Columns" },
	{ value = "wall", label = "Wall" },
}
local STATES = {
	{ value = "ready", label = "Ready" },
	{ value = "waiting", label = "Waiting" },
	{ value = "failed", label = "Failed" },
}

local M = signin.MEMBERS

-- What shows of a wall inside the window's inset, from its right.
local function intrudes(z: number, columns: number, over: number): number
	return ds.CardWall.width(z, columns) - over - (ui.theme.get("space.6") :: number)
end

-- Two columns of smaller still cards beside the words.
local function columns_direction(): signin.Wall
	local z, over = 0.56, 36
	return {
		build = function()
			return ds.CardWall.Wall {
				z = z,
				over = over,
				still = true,
				columns = {
					{ members = { M.graham, M.ada, M.lin }, cycle = 46000, start = -40 },
					{ members = { M.theo, M.priya, M.jonas }, cycle = 58000, start = -(H * z + GAP) / 2 - 40 },
				},
			}
		end,
		intrudes = intrudes(z, 2, over),
	}
end

-- Wallpaper: five columns of small cards across the window and past both
-- its sides, each under the veil; the words stand on the field (none of it
-- keeps them back), and the pointer finds one card at a time.
local function wall_direction(): signin.Wall
	local z, over = 0.46, 29
	local step = H * z + GAP
	return {
		build = function()
			return ds.CardWall.Wall {
				z = z,
				over = over,
				still = true,
				veiled = true,
				columns = {
					{ members = { M.noor, M.graham, M.mateo }, cycle = 52000, start = -step * 0.6 },
					{ members = { M.ada, M.yuki, M.lin }, cycle = 44000, start = -step * 0.15 },
					{ members = { M.theo, M.elif, M.priya }, cycle = 61000, start = -step * 0.4 },
					{ members = { M.kofi, M.jonas, M.sasha }, cycle = 48000, start = -step * 0.85 },
					{ members = { M.graham, M.sasha, M.ada }, cycle = 56000, start = -step * 0.3 },
				},
			}
		end,
		intrudes = 0,
	}
end

return {
	id = "ex_signin",
	title = "Sign in",
	about = "Setup's first screen for someone signed out, as the app draws it: two words, three lines on what an account gives, and the way to one or on without. Beside the words, the members who already have one: one large card at a time, drifting up the window's right edge, held under the pointer and dimmed to the room's light until the pointer finds it. Two other walls to compare: two columns beside the words, or a wall behind them. Then the sign-in waiting on the browser, and one that didn't finish.",
	demo = function()
		return kit.stage {
			states = STATES,
			state = shown,
			-- The direction, beside the state: which wall the words stand by.
			also = ui.row {
				gap = "$space.2",
				items = "center",
				ds.Text { "Direction", style = "caption", tone = "tertiary" },
				ds.Segmented { value = direction, options = DIRECTIONS, label = "Direction" },
			},
			w = WIN_W,
			h = WIN_H,
			signin.screen {
				status = shown,
				direction = direction,
				walls = { one = signin.wall, columns = columns_direction, wall = wall_direction },
				on_create = function()
					shown:set(if shown:peek() == "ready" then "waiting" else "ready")
				end,
			},
		}
	end,
}
