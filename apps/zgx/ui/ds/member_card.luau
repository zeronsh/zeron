-- A member card: a printed card, a sky across all of it (in the intro's
-- characters, its cloud layers drifting over and shifting by depth as the
-- card tilts) and paper rising from below under the member's name, badges
-- and the day they joined. Its foil (one of five finishes) catches the light
-- as it turns toward the pointer.
--
-- `MemberCard { number = 182, name = "Graham Hale", since = "Oct 4, 2023",
-- layers = { { src = "…/sky.jpg", depth = 0 }, … } }`, and as much more as
-- there is: `tagline` under the name, `badges` over it, and the art's
-- `art_number`, `art_title` and `finish` as a caption under the card. A
-- member has a number, a day they joined and whatever badges they were
-- given, so a card with only those is complete.
--
-- It is the same in either theme (a printed object): its colours, type and
-- shadow are `zeron.card.*` tokens.

-- A badge: what a member has done or been, as a pill in the card's own
-- language (mono, as its number and date), led by one of the intro's
-- characters. `style`: ink (a dark pill, as the number's), outline (a
-- hairline), foil (an iridescent pill, the holo's).
export type BadgeStyle = "ink" | "outline" | "foil"
export type Badge = { label: string, glyph: string?, style: BadgeStyle? }

export type Layer = { src: string, depth: number }

-- The foil's finish, after trading cards' own (each made ours):
-- aurora: spectral pillars over the art sliding against the tilt (a
--   Pokémon holo's); reverse: glitter on the paper, the art left clean (a
--   reverse holo); secret: a fine cross-hatch whose colours turn with the
--   angle (a Yu-Gi-Oh secret rare); starlight: sparse four-point stars
--   that glint as it turns (a starlight rare); glyph: our characters lighting
--   up in colour along a band through the pointer.
export type Holo = "aurora" | "reverse" | "secret" | "starlight" | "glyph"
local HOLOS: { Holo } = { "aurora", "reverse", "secret", "starlight", "glyph" }

export type Props = {
	number: number,
	-- The member's name; a function follows it as it is typed (a profile's
	-- card, a preview of the name in its field).
	name: string | () -> string,
	tagline: string?,
	since: string,
	-- The art, back to front: a sky (depth 0), then cut-outs at increasing
	-- depth; or `art`, one picture; or neither, and a `seed` (the member's
	-- id) picks the scene, the foil and the character field's start
	-- (ds/card_seed), among the `scenes` stored with the member (today's
	-- count by default). What is given explicitly wins over the seed.
	layers: { Layer }?,
	art: string?,
	seed: string?,
	scenes: number?,
	-- Or the face as the server gives it (a card from `/cards/:username`,
	-- which never shows the id): the scene's index (from 0) and the sky
	-- (where the character field starts), with `holo`. Each wins over the
	-- seed's.
	scene: number?,
	sky: number?,
	-- The art's place in the set, its name and its finish ("02 — HORIZON ·
	-- HALFTONE MIST" under the card), when the card is shown as a print.
	art_number: number?,
	art_title: string?,
	finish: string?,
	badges: { Badge }?,
	-- The foil's finish; by default one per card, from its number.
	holo: Holo?,
	-- The art holds still: its layers neither drift on their own nor shift
	-- with the pointer. For a card among many (a wall of them, small), where
	-- the drift is too small to see and each card repainting every frame
	-- adds up; and the system's Reduce motion asks for it on every card.
	-- The card itself still tilts toward the pointer.
	still: boolean?,
	-- Its shadow, in place of the card's own (`zeron.card.shadow`): a wall of
	-- cards on a dark window gives each none, where the shadows of many
	-- pooled into a dark halo round every column.
	shadow: string?,
	-- Told when the pointer comes onto the card and leaves it: the card's
	-- own hover, the one that tilts it, so whatever a page does with it (a
	-- wall holding its drift) has exactly the card for its hit area. (A
	-- handler on a zoomed box round the card never fires; this one does.)
	on_hover: ((inside: boolean) -> ())?,
	-- It arrives: dealt onto the screen as it mounts, rising a little as it
	-- comes in, its foil catching the light once across it as it settles
	-- (under a second, and nothing waits on it). No tilt on the way. With
	-- the system's Reduce motion it simply appears.
	arrive: boolean?,
	-- Its foil catches the light once, as `arrive`'s does, each time this
	-- turns true (not as it mounts true): a moment that lands on the card
	-- (setup's window taking over from the intro).
	glint: (() -> boolean)?,
}

local motion = require("@self/motion")
local ds_decode = require("@self/decode")
local card_seed = require("@self/card_seed")
local card_scenes = require("@self/card_scenes")
local prefers = require("@self/prefers")

local W, H = 300, 420
-- How far it tilts toward the pointer, in degrees at its edge.
local TILT = 12

-- The foil's shine: one of the finishes above, laid over the card. (Plain
-- transparency, not a blend mode: colour-dodge blew the edges of the ink
-- out into coloured specks, and reading what lies beneath every frame made
-- every edge on the card shimmer.) `params[0]`:
-- pointer x, y (0 to 1 across the card), strength, finish (0 to 4).
local shine = ui.shader(ds_decode.GLYPHS .. [[
fn spectrum(t: f32) -> vec3<f32> {
	return vec3<f32>(0.5) + 0.5 * cos(6.2831853 * (vec3<f32>(fract(t)) + vec3<f32>(0.0, 0.33, 0.67)));
}
fn hash2(c: vec2<f32>) -> f32 {
	return fract(sin(dot(c, vec2<f32>(127.1, 311.7))) * 43758.5453);
}
fn fill(input: FillInput) -> vec4<f32> {
	let uv = input.p / input.size;
	let at = input.params[0].xy;
	let k = input.params[0].z;
	let finish = u32(input.params[0].w + 0.5);
	if (k <= 0.001) { return vec4<f32>(0.0); }
	let tilt = at - vec2<f32>(0.5);
	// The paper's part of the card, under the words: every finish runs the
	// card's full height but steps back there, so the words stay legible.
	let paper = smoothstep(0.5, 0.62, uv.y);
	let quiet = mix(1.0, 0.4, paper);
	let across = (uv.x - at.x) + (uv.y - at.y);
	var rgb = vec3<f32>(1.0);
	var a = 0.0;
	switch finish {
		case 1u: {
			// Reverse: glitter across the card, each fleck catching the light
			// at its own angle.
			let cell = floor(input.p / 2.5);
			let h = hash2(cell);
			let glint = pow(max(sin(h * 40.0 + tilt.x * 9.0 + tilt.y * 7.0), 0.0), 14.0) * step(0.5, h);
			rgb = mix(spectrum(h * 2.0 + tilt.x), vec3<f32>(1.0), 0.35);
			a = (glint * 0.7 + 0.05) * mix(1.0, 0.35, paper);
		}
		case 2u: {
			// Secret: a fine cross-hatch, its colours turning with the angle,
			// brightest along a band through the pointer.
			let d1 = fract((input.p.x + input.p.y) / 4.0);
			let d2 = fract((input.p.x - input.p.y) / 4.0);
			let lines = max(1.0 - smoothstep(0.0, 0.2, abs(d1 - 0.5)), 1.0 - smoothstep(0.0, 0.2, abs(d2 - 0.5)));
			rgb = spectrum((uv.x - uv.y) * 1.3 + tilt.x * 1.6 - tilt.y);
			a = lines * (0.1 + 0.26 * exp(-across * across * 3.0)) * quiet;
		}
		case 3u: {
			// Starlight: four-point stars, sparse, glinting as it turns.
			let size = 16.0;
			let cell = floor(input.p / size);
			let h = hash2(cell);
			let centre = (cell + vec2<f32>(0.3 + 0.4 * hash2(cell + 7.0), 0.3 + 0.4 * hash2(cell + 13.0))) * size;
			let d = abs(input.p - centre);
			let star = exp(-d.x * 0.7 - d.y * 4.0) + exp(-d.y * 0.7 - d.x * 4.0) + exp(-dot(d, d) * 0.5);
			let glint = pow(max(sin(h * 30.0 + tilt.x * 8.0 - tilt.y * 6.0), 0.0), 6.0) * step(0.62, h);
			rgb = mix(vec3<f32>(1.0), spectrum(h + uv.y), 0.3);
			a = (min(star * glint, 1.0) * 0.95 + 0.05 * exp(-across * across * 4.0)) * quiet;
		}
		case 4u: {
			// Glyph: our characters lighting up in colour along a band through
			// the pointer.
			let px = 1.5;
			let cell_size = vec2<f32>(6.0, 8.0) * px;
			let cell = floor(input.p / cell_size);
			let c = vec2<u32>(max(cell, vec2<f32>(0.0)));
			let g = glyph(1u + u32(cell_hash(c) * 9.999));
			let fp = vec2<u32>(floor((input.p - cell * cell_size) / px));
			var lit = 0.0;
			if (fp.x < 5u && fp.y < 7u) {
				lit = f32((select(g.y, g.x >> (fp.y * 5u), fp.y < 6u) >> (4u - fp.x)) & 1u);
			}
			rgb = spectrum(uv.y * 1.5 + uv.x * 0.7 + tilt.x);
			a = lit * exp(-across * across * 7.0) * 0.6 * mix(1.0, 0.3, paper);
		}
		default: {
			// Aurora: broad, soft spectral pillars down the card, sliding
			// against the tilt, pastel, easing in and out along their length.
			let x = uv.x * 2.2 - tilt.x * 2.0 + uv.y * 0.35;
			let pillar = smoothstep(0.0, 1.0, 0.5 + 0.5 * cos(x * 6.2831853));
			rgb = mix(spectrum(uv.x * 0.6 - tilt.x * 1.1 + uv.y * 0.25), vec3<f32>(1.0), 0.35);
			let along = smoothstep(0.0, 0.25, uv.y) * (1.0 - smoothstep(0.75, 1.0, uv.y));
			a = pillar * 0.24 * along * quiet;
		}
	}
	return vec4<f32>(rgb, a * k);
}]])

-- The foil's glare: light on the card at the pointer.
-- `params[0]`: pointer x, y, strength.
local glare = ui.shader([[
fn fill(input: FillInput) -> vec4<f32> {
	let uv = input.p / input.size;
	let off = uv - input.params[0].xy;
	let k = input.params[0].z;
	if (k <= 0.001) { return vec4<f32>(0.0); }
	return vec4<f32>(vec3<f32>(1.0), exp(-dot(off, off) * 5.0) * 0.22 * k);
}]])

-- The sky in characters: each cell a glyph picked by the picture's
-- brightness there and lit in its colour at full value, over the picture a
-- little dimmed (the intro's recipe). Only the still sky goes through it:
-- anything moving under the cells (a drifting cloud, a ticking clock) turns
-- their glyphs every frame, a flicker. `layer_params[0].x`: a font pixel,
-- in points; `.y`, `.z`: where the field's grid starts, in points (a
-- seed's `sky` sets it, so two cards of one scene read differently).
local sky = ui.layer_shader(ds_decode.GLYPHS .. [[
fn layer(input: LayerInput) -> vec4<f32> {
	let px = max(input.params[0].x, 1.0);
	let cell_size = vec2<f32>(6.0, 8.0) * px;
	let origin = vec2<f32>(input.params[0].y, input.params[0].z);
	let p = input.p + origin;
	let cell = floor(p / cell_size);
	let at = zgx_sample((cell + 0.5) * cell_size - origin);
	let own = input.color;
	let index = u32(pow(clamp((dot(at.rgb, vec3<f32>(0.299, 0.587, 0.114)) * at.a - 0.3) / 0.65, 0.0, 1.0), 1.6) * 10.999);
	let g = glyph(index);
	let fp = vec2<u32>(floor((p - cell * cell_size) / px));
	var lit = 0.0;
	if (fp.x < 5u && fp.y < 7u) {
		lit = f32((select(g.y, g.x >> (fp.y * 5u), fp.y < 6u) >> (4u - fp.x)) & 1u);
	}
	let ink = at.rgb / max(max(at.r, max(at.g, at.b)), 0.25);
	let k = lit * 0.62;
	return vec4<f32>(ink * k + own.rgb * 0.88 * (1.0 - k), own.a);
}]])

-- "004·182": six digits, a centred dot between their halves.
local function numbered(n: number): string
	return `{string.format("%03d", (n // 1000) % 1000)}·{string.format("%03d", n % 1000)}`
end

local function mono(text: string, size: string, color: string, spacing: number?, weight: string?): Node
	return ui.text {
		text,
		font = "$zeron.card.mono",
		text_size = size,
		weight = weight,
		letter_spacing = spacing,
		color = color,
	}
end

-- A badge pill.
local function badge(b: Badge): Node
	local style = b.style or "ink"
	local ink = if style == "ink" then "$zeron.card.light" else "$zeron.card.ink"
	return ui.row {
		h = 20,
		px = "$space.2",
		gap = "$space.1",
		items = "center",
		rounded = "$radius.full",
		bg = if style == "ink" then "$zeron.card.pill" else nil,
		border = if style == "outline" then 1 else nil,
		border_color = if style == "outline" then "$zeron.card.rule" else nil,
		gradient = if style == "foil"
			then { angle = 120, stops = { "#c9d6ffcc", "#efd0f2cc", "#ffe0c6cc", "#cdf1e6cc" } }
			else nil,
		-- The glyph, centred in a square of its own (its line no taller than
		-- the label's, so it sits neither high nor low beside it).
		if b.glyph
			then ui.div {
				w = 10,
				h = 12,
				items = "center",
				justify = "center",
				ui.text {
					b.glyph,
					font = "$zeron.card.mono",
					text_size = "$zeron.card.size.badge",
					line_height = 12,
					weight = "$font.weight.medium",
					color = ink,
				},
			}
			else nil,
		ui.text {
			string.upper(b.label),
			font = "$zeron.card.mono",
			text_size = "$zeron.card.size.badge",
			line_height = 12,
			weight = "$font.weight.medium",
			letter_spacing = 1,
			color = ink,
		},
	}
end

return ui.component(function(props: Props): Node
	-- The tilt: where the pointer is on the card, -0.5 to 0.5 each way.
	-- The card itself, which tilts only while the pointer is on it: a tilt
	-- makes it a 3D layer, and zgx's scroll fade washes such layers out,
	-- so at rest it has none.
	local card_el = ui.ref()
	-- The same, 0 to 1, for the art's layers to shift by.
	local px = ui.use_state(0.5)
	local py = ui.use_state(0.5)
	local shine_el = ui.ref()
	local glare_el = ui.ref()
	-- The foil is there only while the pointer is (and a moment after, to
	-- fade): a card at rest is the card alone.
	local lit = ui.use_state(false)
	-- Whether any of the card is on screen: its sky (a layer of its own,
	-- about 2.6 MB at 2x) is there only then. zgx keeps a layer's texture
	-- for as long as its node is a layer, on screen or not: the gallery's 27
	-- cards scrolled end to end held 64 MiB with every sky mounted, 15.5 MiB
	-- at most this way.
	local on_screen = ui.use_state(true)
	-- How lit: the foil overlay's opacity, tweened (in over 140 ms as the
	-- pointer comes, out as it goes).
	local strength = ui.use_state(0)
	local inside_now = false
	-- From the seed, what isn't given: the scene, the foil, and where the
	-- character field starts (its grid shifted within one cell, by the
	-- sky's low bits).
	local seeded = if props.seed then card_seed.pick(props.seed, props.scenes or card_scenes.COUNT) else nil
	local holo = props.holo or (if seeded then seeded.holo else nil)
	local scene_index = props.scene or (if seeded then seeded.scene else nil)
	local sky_seed = props.sky or (if seeded then seeded.sky else nil)
	local sky_origin = { 0, 0 }
	if sky_seed then
		sky_origin = { sky_seed % 9, math.floor(sky_seed / 9) % 12 }
	end
	local finish = table.find(HOLOS, holo or HOLOS[props.number % #HOLOS + 1]) or 1
	local mode = finish - 1

	-- Still (asked for, or by the system's Reduce motion): the art neither
	-- drifts nor shifts with the pointer. The card still tilts.
	local still = props.still == true or motion.reduced()
	-- Flat (Reduce motion): no tilt toward the pointer either.
	local flat = motion.reduced()

	-- What follows the pointer, and how. Nothing is animated per move: an
	-- animation started on every pointer move replaces the last before it
	-- finishes, the value only ever chasing, and zgx warns of it twenty
	-- moves in (its animatables are plain animations underneath, so setting
	-- one warns the same). So: the pointer's place goes into `px` and `py`,
	-- and springs bound to them carry the tilt and the art's shift, retargeted
	-- in Rust; the foil's parameters are set outright, through one animatable
	-- a shader with no duration (a glare has no reason to lag a pointer),
	-- so no clip is ever replaced while it plays; and the foil comes and
	-- goes by its overlay's opacity, a tween on `strength`.
	local foils: { [any]: any } = {}
	local function foil_of(el): any
		local f = foils[el]
		if not f then
			f = anime.createAnimatable(el, {
				shaderParam0 = { duration = 0 },
				shaderParam1 = { duration = 0 },
			})
			foils[el] = f
		end
		return f
	end

	local function aim(x: number, y: number)
		px:set(x + 0.5)
		py:set(y + 0.5)
		for _, s in { shine_el:peek(), glare_el:peek() } do
			if s then
				local f = foil_of(s)
				f.shaderParam0(x + 0.5)
				f.shaderParam1(y + 0.5)
			end
		end
	end

	-- A layer of the art: shifted against the pointer by its depth
	-- (parallax), drifting on a period of its own if it has depth. Still
	-- (asked for, or by the system's Reduce motion), it does neither. The
	-- drift keeps the clock's time, not the card's: two copies of one card,
	-- mounted apart (in two windows), show the same frame.
	local function layer(l: Layer): Node
		local d = l.depth
		return ui.div {
			-- (Named for tools: it moves by the clock, not by the app.)
			id = "card-art",
			absolute = true,
			left = -24,
			right = -24,
			top = -16,
			bottom = -16,
			-- The art hangs past the card's edges (room to shift), clipped by
			-- the card; it takes no pointer, so the card's hover is the card's
			-- box and not a halo round it (a wall's gaps stayed "on a card").
			pointer_events = "none",
			x = if still then nil else ui.spring(px, { from = d * 8, to = -d * 8, stiffness = 160, damping = 22 }),
			y = if still then nil else ui.spring(py, { from = d * 5, to = -d * 5, stiffness = 160, damping = 22 }),
			ui.div {
				w_full = true,
				h_full = true,
				on_mount = function(el)
					if d > 0 and not still then
						local period = 16000 / d + (props.number % 5) * 900
						local drift = anime.animate(el, {
							x = { -d * 5, d * 5 },
							duration = period,
							alternate = true,
							loop = true,
							ease = "inOutSine",
							-- A few points over many seconds (3 a second at
							-- most): a tenth of a point a frame at 30, the
							-- same to the eye. At the display's rate (120 on
							-- ProMotion) the card drew the whole window 120
							-- times a second for as long as it showed: 5% CPU
							-- with nothing else moving, 2% at 30. (Lower
							-- saves nothing more: zgx's display link ticks at
							-- the display's rate while any animation plays.)
							frameRate = 30,
						})
						drift.currentTime = ui.now() % (period * 2)
					end
				end,
				ui.image { src = l.src, w_full = true, h_full = true, fit = "cover" },
			},
		}
	end
	local layers = props.layers
		or (if props.art then { { src = props.art, depth = 0 } } else nil)
		or (if scene_index then card_scenes.layers(scene_index) else nil)
		or { { src = "", depth = 0 } }
	local steady: { Node } = {}
	local drifting: { Node } = {}
	for _, l in layers do
		table.insert(if l.depth > 0 then drifting else steady, layer(l))
	end

	local badges: { Node } = {}
	for _, b in props.badges or {} do
		table.insert(badges, badge(b))
	end

	-- The arrival, as the card mounts: 36 points low and unseen, it comes up
	-- into place over 820 ms, easing out; its foil, lit for the moment,
	-- sweeps once from its left edge to its right and dies away. Then the
	-- card is at rest, a plain node again.
	local arriving = props.arrive == true and not motion.reduced()
	-- Unseen until it arrives (the style's own opacity, so that the clean-up
	-- after the animation finds it at one, not back at none).
	local arrived = ui.use_state(not arriving)
	-- The foil lit for a moment and swept once from the card's left edge to
	-- its right, dying away as the sweep ends.
	local function sweep()
		lit:set(true)
		ui.after(16, function()
			strength:set(1)
			for _, s in { shine_el:peek(), glare_el:peek() } do
				if s then
					anime.animate(s, {
						shaderParam0 = { -0.4, 1.4 },
						shaderParam1 = 0.45,
						duration = 900,
						delay = 120,
						ease = "inOutSine",
					})
				end
			end
		end)
		-- The light dies away as the sweep ends, and the foil goes once it
		-- has faded, unless the pointer is on the card by then.
		ui.after(1000, function()
			if not inside_now then
				strength:set(0)
			end
		end)
		ui.after(1200, function()
			if not inside_now then
				lit:set(false)
			end
		end)
	end
	local function arrive(el: Element)
		anime.animate(el, {
			y = { 36, 0 },
			opacity = { 0, 1 },
			duration = 820,
			ease = "out(3)",
			onComplete = function()
				arrived:set(true)
				anime.utils.cleanInlineStyles(el)
			end,
		})
		sweep()
	end
	-- `glint`: the sweep alone, as it turns true.
	local glint = props.glint
	if glint and not motion.reduced() then
		local was: boolean? = nil
		ui.effect(function()
			local now = glint()
			if was == false and now then
				sweep()
			end
			was = now
		end)
	end

	local card = ui.overlay {
		w = W,
		h = H,
		rounded = "$zeron.card.radius",
		overflow = "hidden",
		-- No fill of its own: the sky covers it and the paper rises over the
		-- sky, so each corner is drawn once (a paper fill under the sky, on the
		-- same curve, showed through its edge as a pale fringe).
		border = 1,
		border_color = "$zeron.card.edge",
		ref = card_el,
		-- Its tilt toward the pointer: springs on the pointer's place, each
		-- way up to TILT degrees at the card's edge; at rest exactly none,
		-- and a rotation of none is no transform at all, so the card is a
		-- plain node again, not a layer. A still card tilts too (its art is
		-- what holds still); only Reduce motion keeps it flat.
		perspective = if flat then nil else 900,
		rotate_x = if flat then nil else ui.spring(py, { from = TILT, to = -TILT, stiffness = 220, damping = 28 }),
		rotate_y = if flat then nil else ui.spring(px, { from = -TILT, to = TILT, stiffness = 220, damping = 28 }),
		-- The sky, in characters: the still sky through the characters, its
		-- drifting layers over it. On screen only (see `on_screen`).
		ui.show(on_screen, ui.overlay {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			ui.div {
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				layer_shader = sky,
				layer_params = { { 1.5, sky_origin[1], sky_origin[2], 0 } },
				table.unpack(steady),
			},
			table.unpack(drifting),
		}),
		-- The paper rising through it, under the words.
		ui.div {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			gradient = {
				angle = 180,
				stops = {
					{ 0.28, "#f1efe800" },
					{ 0.38, "#f1efe81a" },
					{ 0.46, "#f1efe84d" },
					{ 0.54, "#f1efe899" },
					{ 0.61, "#f1efe8d1" },
					{ 0.68, "#f1efe8f0" },
					{ 0.76, "#f1efe8ff" },
				},
			},
		},
		-- The words: the mark and number on the sky; badges, name and date on
		-- the paper. Laid out inside the card's padding, so nothing leaves it.
		ui.column {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			bottom = 0,
			p = "$space.4",
			ui.row {
				items = "center",
				ui.text {
					"zeron",
					font = "$font.sans",
					weight = "$font.weight.bold",
					text_size = "$zeron.card.size.mark",
					color = "$zeron.card.light",
					grow = 1,
				},
				ui.div {
					h = 22,
					px = "$space.2",
					rounded = "$radius.full",
					bg = "$zeron.card.pill",
					justify = "center",
					mono(numbered(props.number), "$zeron.card.size.number", "$zeron.card.light", 1),
				},
			},
			ui.div { grow = 1 },
			-- (Only with badges: an empty row would still hold its margin.)
			if #badges > 0 then ui.row { flex_wrap = true, gap = "$space.1", mb = "$space.3", table.unpack(badges) } else nil,
			ui.text {
				props.name,
				font = "$font.sans",
				weight = "$font.weight.semibold",
				text_size = "$zeron.card.size.name",
				letter_spacing = -1,
				color = "$zeron.card.ink",
			},
			if props.tagline
				then ui.div { mt = "$space.1", mono(string.upper(props.tagline), "$zeron.card.size.tagline", "$zeron.card.muted", 3) }
				else nil,
			-- The day they joined, in a pill as the number is.
			ui.div {
				self = "start",
				mt = "$space.3",
				h = 22,
				px = "$space.2",
				rounded = "$radius.full",
				bg = "$zeron.card.pill",
				justify = "center",
				mono(props.since, "$zeron.card.size.number", "$zeron.card.light", 1),
			},
		},
		-- The foil, over everything, while the pointer is on the card: lit
		-- by its opacity (the shaders at full strength beneath).
		ui.show(
			lit,
			ui.overlay {
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				pointer_events = "none",
				opacity = ui.tween(strength, { duration = 140, ease = "outQuad" }),
				ui.div {
					ref = shine_el,
					absolute = true,
					left = 0,
					right = 0,
					top = 0,
					bottom = 0,
					bg_shader = shine,
					shader_params = { { 0.5, 0.5, 1, mode } },
				},
				ui.div {
					ref = glare_el,
					absolute = true,
					left = 0,
					right = 0,
					top = 0,
					bottom = 0,
					bg_shader = glare,
					shader_params = { { 0.5, 0.5, 1, 0 } },
				},
			}
		),
	}

	return ui.column {
		items = "center",
		gap = "$space.4",
		-- The card, with the pointer read over it by a box that doesn't tilt.
		-- It arrives as this box, not the card: the card clips, and a
		-- clipping box sliding in reads to zgx's layout check as its content
		-- overflowing it by the slide (the card's own move is set aside, its
		-- children's isn't). The shadow comes in with it, too.
		ui.div {
			w = W,
			h = H,
			shadow = props.shadow or "$zeron.card.shadow",
			rounded = "$zeron.card.radius",
			relative = true,
			opacity = function(): number
				return if arrived:get() then 1 else 0
			end,
			on_mount = if arriving then arrive else nil,
			on_visible = function(visible: boolean)
				on_screen:set(visible)
			end,
			card,
			-- Over the card, unturned, so the card's tilt never moves where the
			-- pointer finds it: tilted inside a zoomed box, a card's own hit box
			-- is projected at the wrong scale (zgx e00d192), and a wall of them
			-- fluttered between cards under a resting pointer.
			ui.div {
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				on_pointer_move = function(e)
					aim(e.local_x / W - 0.5, e.local_y / H - 0.5)
				end,
				on_hover = function(inside: boolean)
					inside_now = inside
					if inside then
						lit:set(true)
						-- (Lit on the frame after it is there, so it fades in from
						-- none rather than appearing lit.)
						ui.after(16, function()
							if inside_now then
								strength:set(1)
							end
						end)
					else
						aim(0, 0)
						strength:set(0)
						-- Gone once it has faded, unless the pointer is back.
						ui.after(260, function()
							if not inside_now then
								lit:set(false)
							end
						end)
					end
					if props.on_hover then
						props.on_hover(inside)
					end
				end,
			},
		},
		-- The print's caption, when the card is shown as one.
		if props.art_number and props.art_title and props.finish
			then mono(
				`{string.format("%02d", props.art_number :: number)} — {props.art_title} · {props.finish}`,
				"$zeron.card.size.caption",
				"$zeron.card.caption",
				3
			)
			else nil,
	}
end)
