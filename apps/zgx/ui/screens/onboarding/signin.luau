-- Setup's first screen for someone signed out: the way to an account, or on
-- without one. A welcome opens it (Welcome to Zeron, two lines at display
-- size), three lines say what an account gives (crates/ui/src/onboarding.rs
-- render_sync, each promise cut to a line), and the footer is setup's: I
-- already have an account at the left; Skip for now, then the pill that
-- opens the browser, at the right. One left edge of ink for the title, the
-- lines and that label: the window's inset and a ghost pill's pad in.
--
-- Beside the words, the members who already have one: one large card at a
-- time, in a column at the window's right edge and a little past it,
-- drifting up, its art alive (ds/card_wall). It stands whole under the
-- title bar's strip as the screen arrives. While the intro hands over
-- (`onboarding.arrived`, the host's, false), the intro draws a copy of this
-- screen over the window, and both must show the same frame: both walls
-- reckon their drift from the moment the way in was taken
-- (`onboarding.epoch`, the host's), so they drift through the hand-off as
-- one, and nothing starts as the window takes over. Then the card catches
-- the light once.
--
-- The gallery's Sign in example draws this screen, with its other walls to
-- compare and the states sign-in can be in (waiting on the browser, failed).

local ds = require("@ds")
local state = require("./state")
local Onboarding = ds.Onboarding

local M = {}

-- A scene from assets/cards (manifest.json): its sky, then its layers
-- toward the viewer.
local function scene(slug: string, back: string, ...: string): { ds.CardLayer }
	local layers: { ds.CardLayer } = { { src = `assets/cards/{slug}/{back}.webp`, depth = 0 } }
	for i, name in { ... } do
		table.insert(layers, { src = `assets/cards/{slug}/{name}.webp`, depth = i })
	end
	return layers
end

-- Twelve members: a number (the directory counts from one; there were 1,373
-- members on Oct 28, 2026), the day they joined, and the one badge really
-- issued, Cloud Tester, on two of them. Names are made up; the cards carry
-- no tagline or caption, only what a member has. (Until the directory
-- serves real ones.)
local CLOUD_TESTER: ds.CardBadge = { label = "Cloud Tester", style = "outline" }
M.MEMBERS = {
	noor = { number = 8, name = "Noor", since = "Sep 14, 2023", layers = scene("kelp", "back", "mid", "near", "detail"), holo = "starlight" },
	graham = { number = 51, name = "Graham", since = "Oct 4, 2023", layers = scene("violet-dusk", "sky", "far", "near"), holo = "aurora" },
	lin = { number = 117, name = "Lin", since = "Jan 12, 2024", layers = scene("dunes", "sky", "detail", "far", "near"), holo = "starlight" },
	ada = { number = 182, name = "Ada", since = "Mar 3, 2024", layers = scene("aurora", "sky", "far", "near"), holo = "secret", badges = { CLOUD_TESTER } },
	mateo = { number = 233, name = "Mateo", since = "Apr 21, 2024", layers = scene("forest", "back", "mid", "near", "detail"), holo = "reverse" },
	theo = { number = 306, name = "Theo", since = "Jun 18, 2024", layers = scene("blossom", "sky", "far", "near", "detail"), holo = "reverse" },
	priya = { number = 418, name = "Priya", since = "Aug 2, 2024", layers = scene("lighthouse", "sky", "far", "near"), holo = "glyph", badges = { CLOUD_TESTER } },
	yuki = { number = 589, name = "Yuki", since = "Oct 7, 2024", layers = scene("bamboo", "back", "mid", "near", "detail"), holo = "aurora" },
	elif = { number = 744, name = "Elif", since = "Dec 1, 2024", layers = scene("ravine", "back", "mid", "near", "detail"), holo = "glyph" },
	kofi = { number = 965, name = "Kofi", since = "Mar 9, 2025", layers = scene("violet-dusk", "sky", "far", "near"), holo = "secret" },
	jonas = { number = 1290, name = "Jonas", since = "Sep 9, 2025", layers = scene("canyon", "back", "mid", "near", "detail"), holo = "aurora" },
	sasha = { number = 1361, name = "Sasha", since = "Jan 20, 2026", layers = scene("aurora", "sky", "far", "near"), holo = "reverse" },
} :: { [string]: ds.CardMember }
local MEMBERS = M.MEMBERS

-- The words: the welcome, and what an account gives, a line each. The
-- welcome is three words at display size, two lines in the column: broken
-- before "to", so the product's name keeps its preposition and neither line
-- ends on a small word.
-- (Messages: they follow the language chosen.)
local TITLE = ui.t("onboarding.signin.title")
local LINES = {
	ui.t("onboarding.signin.lines.phone"),
	ui.t("onboarding.signin.lines.machines"),
	ui.t("onboarding.signin.lines.cloud"),
}

-- The words' column, no narrower than the title's widest word at display
-- size ("Welcome" at 170): a word never breaks inside itself; and no wider
-- than a measure of its own.
local WORD, MEASURE = 184, 300

-- The wall: one column of large cards, at the window's right edge and 36
-- past it, one whole in the light at a time. It starts with its first card
-- standing under the title bar's strip, top to its bottom edge, so the
-- screen arrives on a whole card.
M.WALL = {
	z = 0.78,
	over = 36,
	column = {
		members = { MEMBERS.graham, MEMBERS.ada, MEMBERS.priya, MEMBERS.jonas },
		cycle = 104000,
		start = Onboarding.TITLEBAR,
	},
}

-- Where the wall's first card stands in a `w` wide window, as it starts:
-- its box, in the window's points. (The intro opens the screen from it.)
function M.card(w: number): { x: number, y: number, w: number, h: number }
	local cw, ch = 300 * M.WALL.z, 420 * M.WALL.z
	return { x = w + M.WALL.over - cw, y = M.WALL.column.start, w = cw, h = ch }
end

-- The window is the user's: the host says so once the intro has handed
-- over (false until then, in the intro's copy too); with no intro, it
-- always was.
local arrived = ui.shared("onboarding.arrived", true)
-- When the way in was taken (`ui.now()`'s; 0 until then): what the intro's
-- copy of the wall and the window's reckon their drift from.
local epoch = ui.shared("onboarding.epoch", 0)
-- When the wall first stood on its own (not handed over): a hot reload
-- builds it again from then, so it neither rises again nor starts its
-- drift over.
local born = ui.state("wall_born", 0)

export type Wall = {
	-- The wall, built.
	build: () -> Node,
	-- What shows of it inside the window's inset, from its right: the
	-- words' row ends a gap short of it.
	intrudes: number,
}

export type Props = {
	-- What the state says: "ready", "waiting" (on the browser) or "failed".
	status: State<string>?,
	-- Other walls in place of the column, by name, and which one shows
	-- (the gallery's directions; "one" is this screen's own).
	walls: { [string]: () -> Wall }?,
	direction: State<string>?,
	on_create: (() -> ())?,
	on_existing: (() -> ())?,
	-- Gives up waiting on the browser: the quiet left action reads Cancel
	-- while it waits.
	on_cancel: (() -> ())?,
	on_skip: (() -> ())?,
	-- Why the last sign-in didn't finish, as a line (failed only).
	says: (() -> string?)?,
}

-- The one-card wall, as the app shows it.
function M.wall(): Wall
	local inset = ui.theme.get("space.6") :: number
	return {
		build = function()
			-- (Handed over by the intro: the host's epoch, which outlives a
			-- reload too. Standing on its own: from when it first stood.)
			local handed = (epoch:peek() :: number) > 0
			local again = state.wall_again:peek()
			local first = arrived:peek() and (again or (not handed and born:peek() == 0))
			if first then
				born:set(ui.now())
				state.wall_again:set(false)
			end
			return ds.CardWall.Wall {
				z = M.WALL.z,
				over = M.WALL.over,
				columns = { M.WALL.column },
				-- Handed over to (the intro's copy, or the window under it):
				-- reckoned from the way in. Otherwise its own.
				epoch = if first
					then nil
					elseif arrived:peek() and (not handed or born:peek() > 0) then function(): number?
						return born:get()
					end
					else function(): number?
						local at = epoch:get() :: number
						return if at > 0 then at else nil
					end,
				glint = function()
					return arrived:get() :: boolean
				end,
			}
		end,
		intrudes = ds.CardWall.width(M.WALL.z, 1) - M.WALL.over - inset,
	}
end

function M.screen(props: Props?): Node
	local p: Props = props or {}
	local status = p.status or ui.state("signin_status", "ready")
	local walls = p.walls or { one = M.wall }
	local direction = p.direction or ui.state("signin_wall", "one")
	-- What the state says, under the lines: nothing while ready.
	local function said(): string
		local s = status:get()
		if s == "waiting" then
			return ui.t("onboarding.signin.waiting"):get()
		elseif s == "failed" then
			return (if p.says then p.says() else nil) or ui.t("onboarding.signin.failed"):get()
		end
		return ""
	end
	local lines: { Node } = {}
	for _, l in LINES do
		table.insert(lines, ui.text { l, text_size = "$font.size.lg", line_height = "$font.line.lg", color = "$text.secondary", wrap = true })
	end
	-- One left edge for everything on the page: the title's ink, each
	-- line's and the footer label's start on the same column. No optical
	-- offsets: the user's rule.
	-- The status line hangs under the lines, out of the flow: the block the
	-- room centres is the title and the lines, and the line comes and goes
	-- without moving them.
	local gap = ui.theme.get("space.5") :: number
	local line_h = ui.theme.get("font.line.md") :: number
	local words = ui.column {
		id = "copy",
		relative = true,
		grow = 1,
		min_w = WORD,
		max_w = MEASURE,
		ml = "$space.4",
		gap = "$space.5",
		ds.Text { TITLE, style = "display", heading = 1, wrap = true },
		ui.column { w_full = true, gap = "$space.2", table.unpack(lines) },
		ui.text {
			said,
			absolute = true,
			left = 0,
			bottom = -(gap + line_h),
			h = line_h,
			text_size = "$font.size.md",
			line_height = "$font.line.md",
			color = function()
				return if status:get() == "failed" then "$zeron.tone.danger.ink" else "$text.secondary"
			end,
		},
	}
	-- The words' own inset, past the window's: the footer's ghost pill
	-- stands on the window's inset and its words start a pad in, so the
	-- title and the lines start there too (one left edge of ink).
	local inset = ui.theme.get("space.6") :: number
	local pad = ui.theme.get("space.4") :: number
	-- What the wall takes of the room: the words' row ends there (and no
	-- wider than the words' measure), so it never lies over a card. A box
	-- over a card takes the pointer from it, pass-through or not, and the
	-- card's hover (its hold, its tilt) goes dead under it.
	local room = ui.ref()
	local function row_w(): number
		local kept = walls[direction:get()]().intrudes + pad
		return math.max(WORD, math.min(MEASURE + pad, (room.width or 680 - 2 * inset) - kept))
	end
	return Onboarding.Step {
		center = true,
		-- (Each ghost pill's words stand on the window's inset plus its pad,
		-- its wash hanging out past it: the lines' edge and the label's are
		-- one.)
		lead = Onboarding.Quiet {
			label = function()
				return if status:get() == "waiting" then ui.t("common.cancel"):get() else ui.t("onboarding.signin.have_account"):get()
			end,
			on_click = function()
				local go = if status:peek() == "waiting" then p.on_cancel else p.on_existing
				if go then
					go()
				end
			end,
		},
		aside = Onboarding.Quiet { label = ui.t("onboarding.skip_for_now") :: any, on_click = p.on_skip },
		continue = {
			label = function()
				local s = status:get()
				return if s == "waiting"
					then ui.t("onboarding.signin.waiting_browser"):get()
					elseif s == "failed" then ui.t("onboarding.try_again"):get()
					else ui.t("onboarding.signin.create"):get()
			end,
			disabled = function()
				return status:get() == "waiting"
			end,
			on_click = p.on_create,
		},
		-- The wall first, under the words: laid over the window in a box of
		-- its own that takes no room in the screen's column (so the words are
		-- all the column centres), the words over it.
		ui.div {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			ui.presence(direction, function(d: string): Node
				return walls[d]().build()
			end, { enter = { opacity = { 0, 1 }, duration = 320 }, exit = { opacity = 0, duration = 200 } }),
		},
		-- The words, centred in the screen's room (between the header's slot
		-- and the footer), the same whichever wall stands behind them: the
		-- column takes the room and centres its one row.
		ui.column {
			ref = room,
			w_full = true,
			grow = 1,
			min_h = 0,
			items = "start",
			justify = "center",
			-- (The words take no pointer: the wall under them is held and lit
			-- by it.)
			pointer_events = "none",
			ui.row {
				id = "words",
				w = row_w,
				items = "center",
				words,
			},
		},
	}
end

return M
