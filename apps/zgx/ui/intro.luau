-- Onboarding's intro, from zgx's `agents_intro` example: an art piece of its
-- own (its values are the scene's, not the design system's). At launch, the
-- host makes setup's window, hidden (`prepare`); taking the way in, it shows
-- under the scene (`reveal`), the scene closes onto it and sign-in decodes
-- out of it, and the intro hands over (`done`).
--
-- "Your agents, anywhere": an intro played over the desktop, the desktop
-- itself its frame. A sea at night decodes out of characters round the moon; it
-- spreads as the night turns to blue hour and to sunrise, clouds drifting in
-- at their own speeds; the agents rise from the horizon; then the words, and
-- a way in.
--
-- The plates and clouds are painted (art/), drawn as characters (`characters`:
-- a glyph a cell by the brightness there, over the picture, on the GPU as it
-- draws). Their shared shader parameters reveal them: a front moving out from
-- a point, cells ahead of it empty, those in its band a glyph alone and
-- scrambling, those behind it the picture, its edge ragged with characters.
-- Everything moves on the compositor or in those parameters: once started, no
-- script and no layout a frame.

local PIXEL = 2
local HORIZON = 0.62
-- The reveal (`shader_params` of every layer): front, band, ragged; centre.
local BAND, RAGGED = 0.36, 0.14
-- The agents Zeron runs (their marks from zeron.sh), floating: scattered
-- loosely about the words, no two alike in place, size or tilt, flying in
-- from outside (`fx`, `fy`) and drifting each on its own period. `x` is a fraction of the width from the middle, `y`
-- of the height from the top.
type Agent = {
	mark: string,
	x: number,
	y: number,
	s: number,
	rot: number,
	fx: number,
	fy: number,
	d: number,
	dur: number,
	dx: number,
	dy: number,
	wob: number,
}
local AGENTS: { Agent } = {
	{
		mark = "claude-mark",
		x = -0.33,
		y = 0.2,
		s = 96,
		rot = -8,
		fx = -160,
		fy = -60,
		d = 0.38,
		dur = 10.5,
		dx = 4,
		dy = -12,
		wob = -2,
	},
	{
		mark = "openai-mark",
		x = 0.28,
		y = 0.12,
		s = 84,
		rot = 7,
		fx = 140,
		fy = -90,
		d = 0.46,
		dur = 9.2,
		dx = -4,
		dy = -10,
		wob = 2,
	},
	{
		mark = "opencode-mark-dark",
		x = -0.08,
		y = 0.1,
		s = 48,
		rot = 6,
		fx = -30,
		fy = -100,
		d = 0.62,
		dur = 8.5,
		dx = 2,
		dy = -8,
		wob = 3,
	},
	{
		mark = "antigravity-mark",
		x = 0.11,
		y = 0.21,
		s = 56,
		rot = -5,
		fx = 50,
		fy = -80,
		d = 0.58,
		dur = 9.8,
		dx = -2,
		dy = -9,
		wob = -3,
	},
	{
		mark = "devin-mark",
		x = -0.44,
		y = 0.46,
		s = 64,
		rot = 9,
		fx = -200,
		fy = 10,
		d = 0.66,
		dur = 10.1,
		dx = 3,
		dy = -8,
		wob = 2,
	},
	{
		mark = "pi-mark",
		x = 0.21,
		y = 0.71,
		s = 60,
		rot = -7,
		fx = 80,
		fy = 120,
		d = 0.5,
		dur = 8.8,
		dx = -3,
		dy = -10,
		wob = -2,
	},
	{
		mark = "cursor-mark",
		x = 0.41,
		y = 0.4,
		s = 76,
		rot = -5,
		fx = 190,
		fy = -20,
		d = 0.7,
		dur = 7.8,
		dx = -3,
		dy = -11,
		wob = 3,
	},
	{
		mark = "grok-mark",
		x = -0.17,
		y = 0.67,
		s = 68,
		rot = 4,
		fx = -90,
		fy = 110,
		d = 0.54,
		dur = 11.5,
		dx = 3,
		dy = -9,
		wob = -3,
	},
	{
		mark = "hermes-mark",
		x = 0.39,
		y = 0.6,
		s = 52,
		rot = 3,
		fx = 170,
		fy = 90,
		d = 0.74,
		dur = 9.5,
		dx = 2,
		dy = -7,
		wob = 2,
	},
	{
		mark = "muse-mark",
		x = -0.36,
		y = 0.82,
		s = 80,
		rot = -4,
		fx = -140,
		fy = 140,
		d = 0.78,
		dur = 10.8,
		dx = -2,
		dy = -8,
		wob = -2,
	},
}

-- When the agents come, in ms from the start.
local AGENTS_AT = 4200

local started = false
local leaving = false

-- Setup's first screen, drawn here too (the same module, in the same look)
-- so the intro can decode it out of characters over setup's window.
require("@ds").setup()
local onboarding = require("@screens/onboarding")
local onboarding_state = require("@screens/onboarding/state")
-- Sign-in, setup's first screen: where its card stands.
local signin = require("@screens/onboarding/signin")
require("@screens/onboarding/actions").appearance(onboarding_state.appearance:peek())
-- Setup's window: its size (the host's), its corner radius (once it has
-- opened), and the overlay's size it is centred in.
local onboarding_w = ui.shared("onboarding.width", 680)
local onboarding_h = ui.shared("onboarding.height", 500)
local onboarding_radius = ui.shared("onboarding.radius", 0)
-- Whether the host makes setup's window hidden (its first frame drawn, not
-- shown) so it can be asked for at launch. A host that doesn't say so shows
-- a new window for a few frames first: there it's asked for under the
-- scene, as the way in comes (`prepare`).
local prepares_hidden = ui.shared("onboarding.prepare_hidden", false):peek() == true
-- When the way in was taken: what the copy's card wall and the window's
-- reckon their drift from (screens/onboarding/signin), so they drift as one.
local onboarding_epoch = ui.shared("onboarding.epoch", 0)
local vw, vh = 0, 0
local scenery = ui.ref()
-- A picture of the scenery, taken as the way in is taken ("" before).
local frozen = ui.state("frozen", "")
frozen:set("")
-- Setup's screen, built while staging (`stage`), before the scene starts.
local replica = ui.state("replica", false)
replica:set(false)
-- The way in taken: the screen drawn over setup's window stops being inert
-- to the eye (its buttons' disabled look) and starts swallowing the pointer.
local handing = ui.state("handing", false)
handing:set(false)
-- What the scene meets later, drawn once, unseen, while staging (`stage`).
local warm = ui.state("warm", false)
warm:set(false)

local decode = require("@ds").decode


type Rect = { x: number, y: number, w: number, h: number }

-- Where setup's window stands: centred across and a little above the
-- middle, as macOS centres a window; whole points, so the card and the
-- window share their edges.
local function target(): Rect
	local w = onboarding_w:peek() :: number
	local h = onboarding_h:peek() :: number
	return { x = math.floor((vw - w) / 2), y = math.floor((vh - h) * 0.42), w = w, h = h }
end

local function cover(src: string, id: string?, opacity: number?): Node
	return ui.image {
		id = id,
		class = "scene",
		src = src,
		w_full = true,
		h_full = true,
		fit = "cover",
		characters = PIXEL,
		-- Nothing shown yet: the front at the moon, on the horizon.
		shader_params = { { 0, BAND, RAGGED, 0 }, { 0, HORIZON - 0.5, 0, 0 } },
		opacity = opacity,
	}
end

-- A layer of the scene, moved as one (the camera, a bank of cloud).
local function layer(id: string, opacity: number?, ...: Node): Node
	return ui.overlay {
		id = id,
		opacity = opacity,
		absolute = true,
		left = "-6%",
		right = "-6%",
		top = 0,
		bottom = 0,
		...,
	}
end

-- An agent's card: white, its mark in its own colours at 56%; the
-- outer box flies in, the inner drifts.
local function agent(i: number, a: Agent): Node
	return ui.div {
		id = `agent{i}`,
		class = "agent",
		absolute = true,
		left = `{50 + a.x * 100}%`,
		top = `{a.y * 100}%`,
		w = a.s,
		h = a.s,
		ml = -a.s / 2,
		mt = -a.s / 2,
		opacity = 0,
		ui.column {
			id = `drift{i}`,
			w = a.s,
			h = a.s,
			rounded = a.s * 0.25,
			items = "center",
			justify = "center",
			gradient = { angle = 135, stops = { "#ffffff", "#f6f6f8" } },
			border = 1,
			border_color = "#ffffffcc",
			shadow = { y = 20, blur = 48, color = "#1a0f3a73" },
			inset_shadow = { y = -2, color = "#0000000d" },
			ui.svg {
				src = `assets/icons/{a.mark}.svg`,
				w = a.s * 0.56,
				h = a.s * 0.56,
			},
		},
	}
end

local agents: { Node } = {}
for i, a in AGENTS do
	table.insert(agents, agent(i, a))
end

-- Everything the intro plays until the way in is taken, so that the click
-- can hold it all still: the scene must stay what its picture shows until
-- the picture stands in for it.
local playing: { Animation } = {}
local function play(targets: ElementTargets, params: ElementParams): Animation
	local animation = anime.animate(targets, params)
	table.insert(playing, animation)
	return animation
end

-- The scene decodes out of characters round the moon, then spreads to fill
-- most of the window, its centre gliding up to the middle.
local function open(_w: number, _h: number)
	-- (Only a few characters round the moon by then.)
	play("#scenery", { opacity = { 0, 1 }, delay = 120, duration = 300, ease = "outQuad" })
	play(".scene", { shaderParam0 = { 0, 0.5 }, duration = 1800, ease = "outCubic" })
	play(".scene", {
		shaderParam0 = 1.3,
		shaderParam5 = 0,
		delay = 1800,
		duration = 2800,
		ease = "inOutCubic",
	})
	-- Then its edge stays alive: the characters round the screen's edges
	-- keep scrambling as the band's raggedness breathes, the front itself
	-- standing where the reveal ended. (Moving the front out drew the picture
	-- in over those characters: the bright sunrise filled the edges and they
	-- lightened for no reason the eye could see.)
	play(".scene", {
		shaderParam2 = { RAGGED, RAGGED * 2 },
		delay = 4600,
		duration = 6500,
		alternate = true,
		loop = true,
		ease = "inOutSine",
	})
end

-- Night to blue hour to sunrise, the camera drifting all the while.
local function timelapse(w: number)
	play("#world", { x = { w * 0.03, -w * 0.03 }, duration = 11000, ease = "outSine" })
	play("#bluehour", { opacity = { 0, 1 }, delay = 1700, duration = 1700, ease = "inOutSine" })
	play("#sunrise", { opacity = { 0, 1 }, delay = 3500, duration = 2000, ease = "inOutSine" })
	-- Nearer, faster: the banks come in from the sides as the light does.
	play("#clouds-left", {
		x = { -w * 0.16, 0 },
		opacity = { 0, 1 },
		delay = 2400,
		duration = 4200,
		ease = "outCubic",
	})
	play("#clouds-right", {
		x = { w * 0.16, 0 },
		opacity = { 0, 1 },
		delay = 2400,
		duration = 4200,
		ease = "outCubic",
	})
	play("#clouds-mid", {
		x = { w * 0.05, -w * 0.02 },
		opacity = { 0, 1 },
		delay = 3000,
		duration = 8000,
		ease = "outSine",
	})
	-- Then it never quite settles: the camera and each bank drift on, each
	-- on its own period, and the picture moving under the characters' grid
	-- keeps their glyphs changing.
	local function drift(id: string, from: number, to: number, at: number, period: number)
		play(id, {
			x = { from, to },
			delay = at,
			duration = period,
			alternate = true,
			loop = true,
			ease = "inOutSine",
		})
	end
	drift("#world", -w * 0.03, -w * 0.008, 11000, 15000)
	drift("#clouds-mid", -w * 0.02, w * 0.006, 11000, 12500)
	drift("#clouds-left", 0, w * 0.016, 6600, 9000)
	drift("#clouds-right", 0, -w * 0.013, 6600, 11000)
end

-- The agents fly in from outside their places, turning and growing into them,
-- each in its turn; then each drifts on its own period.
local function rise(_w: number, _h: number)
	for i, a in AGENTS do
		local at = AGENTS_AT + a.d * 1000
		play(`#agent{i}`, {
			opacity = { 0, 1 },
			x = { a.fx, 0 },
			y = { a.fy, 0 },
			rotate = { a.rot + 24, a.rot },
			scale = { 0.6, 1 },
			delay = at,
			duration = 1000,
			ease = "out(4)",
		})
		play(`#drift{i}`, {
			x = { 0, a.dx },
			y = { 0, a.dy },
			rotate = { 0, a.wob },
			delay = at + 1000,
			duration = a.dur * 500,
			alternate = true,
			loop = true,
			ease = "inOutSine",
		})
	end
end

-- Everything the intro and its hand-off do for the first time, done at
-- launch, while the window shows nothing yet: setup's window made, hidden
-- (its screen built in it and its first frame drawn, but not shown: the
-- host's `prepare`); setup's screen built here too and painted at nothing
-- shown (`layerParam0` short of its middle) where that window stands; and
-- what the scene meets later made once, unseen (the collapse's shader, the
-- way in's glass, the words' blur and the agents' shadows). Each first time
-- is a long frame (setup's screen, built the first time, takes two of about
-- 70 ms), so they go one a frame, and the scene starts once they're done.
-- Done as the way in came, they stalled the scene; done on the click, the
-- transition.
local STAGE_GAP = 20
-- Setup's window, asked for once.
local prepared = false
local function prepare()
	if not prepared then
		prepared = true
		ui.invoke("prepare", target())
	end
end
local STAGES: { (to: Rect) -> () } = {
	function()
		if prepares_hidden then
			prepare()
		end
	end,
	function()
		replica:set(true)
	end,
	function(to)
		anime.animate("#sheet", { left = to.x, top = to.y, opacity = 1, layerParam0 = -0.5, duration = 0 })
	end,
	function()
		warm:set(true)
	end,
}
local staged = 0
local function stage_next(): boolean
	if staged >= #STAGES then
		return false
	end
	staged += 1
	STAGES[staged](target())
	return true
end
-- One a frame, then `after`.
local function stage(after: () -> ())
	if stage_next() then
		ui.after(STAGE_GAP, function()
			stage(after)
		end)
	else
		after()
	end
end
-- What's left of it, at once (the way in taken before it was done).
local function stage_now()
	while stage_next() do
	end
end

-- The words, then the way in (and the pointer, for it).
local function words()
	play(".word", {
		opacity = { 0, 1 },
		y = { 18, 0 },
		blur = { 12, 0 },
		delay = anime.stagger(140, { start = 6300 }),
		duration = 1100,
		ease = "outExpo",
	})
	play("#cta", {
		opacity = { 0, 1 },
		y = { 10, 0 },
		delay = 7400,
		duration = 900,
		ease = "outExpo",
	})
	-- (A timer, not the animation's `onBegin`, which runs as it is made,
	-- delay or not: the pointer would be taken from the desktop at once.)
	ui.after(7400, function()
		ui.invoke("interactive")
	end)
	-- (A host that can't make it hidden: under the scene, as only the drift
	-- moves, a long frame of its own.)
	if not prepares_hidden then
		ui.after(8600, prepare)
	end
end

local function start(w: number, h: number)
	stage(function()
		-- (Not if the way in was taken while staging: Escape.)
		if leaving then
			return
		end
		open(w, h)
		timelapse(w)
		rise(w, h)
		words()
	end)
end

-- Out, as one moment carried into sign-in, built round its card:
--
-- A picture of the scene is taken at once, the click's own moment (a
-- frame's stall, felt as the press, before anything moves), and stands in
-- for the scenery. It closes onto setup's window through characters, from
-- the screen's edges in, so the sunrise's light ends at the window's edge.
-- Inside, sign-in decodes out of the same characters from its member card
-- out (the card's sky is drawn in them: the sea's glyphs settle into the
-- card first, and the screen spreads from it). The agents, the words and
-- the way in, in the picture too, go with it. The screen, now drawn as the window draws it,
-- fades onto the window (held to the same frame until then: setup's
-- `onboarding.arrived`), which takes over.
local SCREEN_MS = 1700
local COLLAPSE_AT, COLLAPSE_MS = 80, 1500

local function leave()
	if leaving then
		return
	end
	leaving = true
	for _, animation in playing do
		animation:pause()
	end
	stage_now()
	prepare()
	handing:set(true)
	local to = target()
	-- Where the characters open: the middle of what shows of the card
	-- (it runs past the window's right edge), from the screen's middle.
	local card = signin.card(to.w)
	local ox = (card.x + to.w) / 2 - to.w / 2
	local oy = card.y + card.h / 2 - to.h / 2
	local full = decode.full(ox, oy, to.w, to.h)
	local function radius(): number
		local r = onboarding_radius:peek() :: number
		return if r > 0 then r else 16
	end
	local reach = decode.reach(to, vw, vh)
	local live = scenery:peek()
	local picture = live and live:snapshot(4096)
	-- Setup's window shows now, under the scene and its picture (its first
	-- frame and macOS's opening in this same stall, out of sight), its card
	-- wall drifting from this moment, as the copy's here does.
	local epoch = ui.now()
	onboarding_epoch:set(epoch)
	ui.invoke("reveal", epoch)
	if picture then
		frozen:set(picture)
		-- Mounted unseen: uploaded and its rect set before it stands in.
		ui.after(16, function()
			anime.animate("#frozen", {
				layerParam0 = reach,
				layerParam4 = to.x,
				layerParam5 = to.y,
				layerParam6 = to.w,
				layerParam7 = to.h,
				layerParam8 = radius(),
				duration = 0,
			})
		end)
	end
	ui.after(50, function()
		if picture then
			anime.animate("#frozen", { opacity = 1, duration = 0 })
			anime.animate("#scenery", { opacity = 0, duration = 0 })
			anime.animate("#frozen", { layerParam0 = 0, delay = COLLAPSE_AT, duration = COLLAPSE_MS, ease = "inOutCubic" })
			-- What's left of it lies under the screen: gone before the
			-- screen goes.
			anime.animate("#frozen", { opacity = 0, delay = COLLAPSE_AT + COLLAPSE_MS, duration = 250, ease = "outQuad" })
		else
			anime.animate("#title, #cta", { opacity = 0, duration = 180, ease = "inQuad" })
			anime.animate("#scenery, #agents", { opacity = 0, duration = 900, ease = "inQuad" })
		end
		anime.animate("#sheet", { layerParam4 = ox, layerParam5 = oy, duration = 0 })
		-- Moving at once (it answers the click), easing in to the edge.
		anime.animate("#sheet", { layerParam0 = { 0, full }, duration = SCREEN_MS, ease = "outQuad" })
		-- The agents, the words and the way in are in the picture too (it is
		-- the window as last shown), where they stand: the live ones stop
		-- drawing as it stands in, and they go as the picture does, with the
		-- closing front or under the screen decoding over them. (Kept live,
		-- moving or fading, they left the picture's copies doubled behind
		-- them.)
		if picture then
			anime.animate("#agents, #title, #cta", { opacity = 0, duration = 0 })
		end
		anime.animate("#sheet", { opacity = 0, delay = SCREEN_MS + 150, duration = 260, ease = "outQuad" })
		ui.after(SCREEN_MS + 450, function()
			ui.invoke("done")
		end)
	end)
end

-- Text over the bright sea: a dusk-coloured blur of itself behind it, as a
-- text shadow (drawn once into its own layer, then only composited).
local function shadowed(props: { [any]: any }): Node
	local shadow = table.clone(props)
	shadow.color = "#2a1250"
	shadow.blur = 10
	shadow.opacity = 0.7
	shadow.absolute = true
	shadow.left = 0
	shadow.top = 2
	return ui.overlay { class = "word", opacity = 0, ui.text(shadow), ui.text(props) }
end

local function word(text: Label, italic: boolean?): Node
	return shadowed {
		text,
		font = "Instrument Serif",
		italic = italic,
		text_size = 80,
		letter_spacing = -1,
		color = "#ffffff",
	}
end

-- Setup's first screen as its window draws it (size, tint, corners), placed
-- over that window by `stage` and decoded by `leave` through `decode`.
local sheet = ui.column {
	id = "sheet",
	absolute = true,
	-- Off the screen until `stage` places it. Never in the pointer's way:
	-- disabled until the way in is taken (its buttons sit where the intro's
	-- does), then under a shield (a click or a hover would change it, and it
	-- would no longer match setup's window).
	left = -100000,
	top = 0,
	pointer_events = "none",
	disabled = function()
		return not handing:get()
	end,
	w = onboarding_w:peek() :: number,
	h = onboarding_h:peek() :: number,
	opacity = 0,
	-- Setup's own background, solid as its window's: what the picture holds
	-- under it (the words) never shows through, and the last fade, onto the
	-- window, changes no pixel.
	bg = "$zeron.window",
	rounded = function()
		local r = onboarding_radius:get() :: number
		return if r > 0 then r else 16
	end,
	overflow = "hidden",
	layer_shader = decode.shader,
	layer_params = decode.params,
	ui.presence(function()
		return if replica:get() then "screen" else nil
	end, function()
		return onboarding()
	end, { w_full = true, h_full = true }),
	ui.div {
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		pointer_events = function()
			return if handing:get() then "auto" else "none"
		end,
		on_press = function() end,
		on_hover = function(_inside: boolean) end,
	},
}

-- The way in: setup's own forward pill, in frosted glass for the scene
-- (ds.Onboarding.WayIn), the object that becomes sign-in's Create an
-- account as the intro hands over. Unseen until the words are in.
local button = ui.div {
	id = "cta",
	opacity = 0,
	require("@ds").Onboarding.WayIn { label = ui.t("intro.get_started") :: any, size = "lg", on_click = leave },
}

return ui.overlay {
	id = "intro",
	w_full = true,
	h_full = true,
	on_key = function(e)
		if e.key == "Escape" then
			leave()
			return true
		end
		return false
	end,
	-- The sequence starts once the window's size is known.
	on_resize = function(s)
		vw, vh = s.w, s.h
		if not started and s.w > 0 and s.h > 0 then
			started = true
			start(s.w, s.h)
		end
	end,
	-- The scene, revealed by its own characters.
	ui.overlay {
		id = "portal",
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		bottom = 0,
		ui.overlay {
			id = "scenery",
			ref = scenery,
			-- Hidden until the reveal is under way: an image's first frames
			-- can draw before its `shader_params` reach it, and with no
			-- band zgx's characters show the whole picture (a flash of the
			-- night sea over the desktop).
			opacity = 0,
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			layer(
				"world",
				nil,
				cover("assets/art/plate-1-night.jpg"),
				cover("assets/art/plate-2-bluehour.jpg", "bluehour", 0),
				cover("assets/art/plate-3-sunrise.jpg", "sunrise", 0)
			),
			-- The banks start hidden, as their animations do: shown for the
			-- first frame, they glinted round the moon.
			layer("clouds-mid", 0, cover("assets/art/clouds-mid.png")),
			layer("clouds-left", 0, cover("assets/art/clouds-left.png")),
			layer("clouds-right", 0, cover("assets/art/clouds-right.png")),
		},
		-- The scenery as it was when the way in was taken, closing onto
		-- setup's window (`leave`). Only then is it here: a layer shader over
		-- the live scene would draw it through a layer every frame, its
		-- characters resampled off their grid.
		ui.show(
			function()
				return frozen:get() ~= ""
			end,
			ui.overlay {
				id = "frozen",
				opacity = 0.01,
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				layer_shader = decode.collapse,
				layer_params = decode.collapse_params,
				-- In the click's stall, not a frame after it: it stands in 50
				-- ms on, the scenery gone that same frame.
				ui.image { src = frozen, w_full = true, h_full = true, fit = "fill", decode = "sync" },
			}
		),
		-- Setup's screen: over the scenery and its picture, under the agents,
		-- the words and the way in (which must take the click, not it).
		sheet,
		ui.overlay { id = "agents", absolute = true, left = 0, right = 0, top = 0, bottom = 0, table.unpack(agents) },
		-- The title in the deep blue high up, where white reads best.
		ui.row {
			absolute = true,
			left = 0,
			right = 0,
			top = "35%",
			justify = "center",
			ui.row {
				id = "title",
				gap = 16,
				word(ui.t("intro.headline.your")),
				word(ui.t("intro.headline.agents")),
				word(ui.t("intro.headline.anywhere"), true),
			},
		},
		-- The way in, on the sea under the sun.
		ui.column { absolute = true, left = 0, right = 0, top = "48%", items = "center", button },
	},
	-- What the scene meets later, made while staging, unseen (2 points, all
	-- but clear): the collapse's shader, the way in's glass (a backdrop
	-- blur), the words' blur and the agents' shadows.
	ui.show(warm, ui.overlay {
		absolute = true,
		left = 0,
		top = 0,
		w = 2,
		h = 2,
		pointer_events = "none",
		ui.overlay {
			w = 2,
			h = 2,
			layer_shader = decode.collapse,
			layer_params = decode.collapse_params,
			ui.div { w = 2, h = 2, bg = "#00000001" },
		},
		ui.div { absolute = true, w = 2, h = 2, bg = "#00000001", backdrop_blur = "$zeron.scene.pill.blur" },
		ui.div { absolute = true, w = 2, h = 2, blur = 2, opacity = 0.01, ui.div { w = 2, h = 2, bg = "#000000" } },
		ui.div { absolute = true, w = 2, h = 2, bg = "#00000001", shadow = { y = 2, blur = 4, color = "#00000002" } },
	}),
}
