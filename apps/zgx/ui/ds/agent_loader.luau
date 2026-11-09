-- NOT WIRED YET (Wing, Nov 8: the old loaders for now). Nothing requires
-- this; the working ring (ds/status `StatusIcon { kind = "working" }`) is
-- still every loader. To wire it (all of it on branch zeron/agent-loaders,
-- with tests/agent_loader.test.luau):
--   - the reply's Working line (ds/working): `{ glyph = "agent", size = 16 }`,
--     the big glyph, filling its square;
--   - running tool rows and a tool group's summary while live (ds/activity):
--     `{ glyph = "subagent", size = 16 }`, the small one at cap height;
--   - subagent and side-chat rows (ds/session_lanes) and the Subagents tray's
--     agent rows (ds/composer_dock): `{ glyph = "subagent", size = 14 }`;
--     the tray's plan step in progress `{ glyph = "agent", size = 14 }`;
--   - export it from ds/init as `AgentLoader`.
-- Cost, measured headless (one loader on an 800x600 window, 120 frames at
-- 60 Hz): no layout or script a frame, one node (its square) repainted 30
-- times a second; the working ring repaints the same way but rests 0.9 s in
-- every 4.8 s (34 of 120 frames against 60).

-- Zeron's agent loaders: the tiles of a glyph turn over one after another,
-- from the top left to the bottom right, diagonal by diagonal. Each tile
-- turns up to its lit face almost at once (9% of the cycle), turns back
-- slowly (46%) and rests; the old activity glyph's rhythm, drawn with the
-- mark's tiles (Wing's loaders.luau, its drawing exactly).
--
--   AgentLoader { glyph = "agent", size = 16 }      the main agent at work
--   AgentLoader { glyph = "subagent", size = 14 }   a subagent's or side chat's
--
-- `agent` is the mark (5 × 3 tiles, a 1.05 s cycle), `subagent` its smaller
-- one (3 × 3, 0.75 s), a tool's or a subagent's. `size` is the square it
-- stands in, as the working ring it replaces; the agent's mark fills it, the
-- small one draws its tiles at a row's cap height (`drawn`, its larger side,
-- 9.5 points: a 13-point line's capitals), centred, so it sits in its row as
-- the icons beside it do. `ink` a token (secondary text by
-- default): the lit face shows it at 0.62, the resting one at 0.2, so it
-- reads in light and dark, frosted and opaque.
--
-- Cost: all of it on the GPU, from the time (a shader, as the working
-- ring's), at 30 frames a second; no script or layout a frame, only its own
-- square drawn again, and nothing at all while it can't be seen or motion is
-- reduced (then it rests, every tile on its resting face).

local motion = require("@self/motion")

export type Glyph = "agent" | "subagent"
export type Props = { glyph: Glyph?, size: number?, drawn: number?, ink: string? }

-- The small glyph's larger side, in points: a 13-point line's cap height.
local SMALL = 9.5

local GAP = 1 / ((1 + math.sqrt(5)) / 2) ^ 4 -- the space between tiles, in tiles: 1/φ⁴
local GLYPHS = {
	agent = { rows = { "###.#", "##.##", "#.###" }, cycle = 1.05 },
	subagent = { rows = { "##.", "#.#", ".##" }, cycle = 0.75 },
}
local LIT, REST = 0.62, 0.2
local FPS = 30

-- A tile's place in its cycle: the diagonal it's on, a little apart from
-- its neighbours (the drawing's own jitter).
local function hash(r: number, c: number): number
	local x = math.sin(r * 127.1 + c * 311.7) * 43758.5453
	return x - math.floor(x)
end

local function num(x: number): string
	return string.format("%.6f", x)
end

-- The glyph's shader: its tiles (centre on a grid of `k`-wide tiles, and
-- phase) written in, so a frame reads nothing but the time and the ink.
local function shader(name: Glyph): any
	local g = GLYPHS[name]
	local rows = g.rows
	local R, N = #rows, #(rows[1])
	local D = N - 1 + R - 1
	-- In tile units: the glyph's width and height, each tile's centre.
	local w, h = N + (N - 1) * GAP, R + (R - 1) * GAP
	local tiles: { string } = {}
	for r = 1, R do
		for i = 1, N do
			if string.sub(rows[r], i, i) == "#" then
				local phase = (i - 1 + r - 1) / (D + 1) + (hash(r + 6, i + 12) - 0.5) * 0.03
				local x = (i - 1) * (1 + GAP) + 0.5 - w / 2
				local y = (r - 1) * (1 + GAP) + 0.5 - h / 2
				table.insert(tiles, ` a=max(a,zr_tile(q,vec2({num(x)},{num(y)}),{num(phase)},t,s,k,&tone));`)
			end
		end
	end
	return ui.shader(string.format([[
// A tile's angle (radians) at x of its cycle: up to its lit face at once,
// round to its rest slowly, then at rest.
fn zr_angle(x:f32)->f32 {
 let A=.09; let RL=.46;
 if x<A { let u=x/A; return (1.-pow(1.-u,3.))*3.14159265; }
 if x<A+RL {
  let u=(x-A)/RL;
  var e=0.; if u<.5 { e=4.*u*u*u; } else { e=1.-pow(-2.*u+2.,3.)/2.; }
  return 3.14159265*(1.+e);
 }
 return 0.;
}
// Coverage of one tile (a squircle, |x|⁴+|y|⁴=1, flipping about its upright
// axis) at q (points from the glyph's centre); its face's tone into tone.
fn zr_tile(q:vec2<f32>,c:vec2<f32>,phase:f32,t:f32,s:f32,k:f32,tone:ptr<function,f32>)->f32 {
 let x=select(fract(fract(t-phase)+1.),.99,t<0.);
 let cs=cos(zr_angle(x));
 let edge=1.-abs(cs);
 let hw=.5*k*max(.04,abs(cs));
 let hh=.5*k*(1.+.06*edge);
 let d=abs(q-c*k);
 let f=pow(pow(d.x/hw,4.)+pow(d.y/hh,4.),.25);
 // (Distance, roughly, in pixels: the radius along the narrower side.)
 let cov=clamp(.5-(f-1.)*min(hw,hh)*s,0.,1.);
 if cov>0. { *tone=select(%s,%s,cs>=0.)*(1.-.55*edge); }
 return cov;
}
fn fill(input:FillInput)->vec4<f32> {
 let ink=input.params[0];
 let side=min(input.size.x,input.size.y);
 // Its tiles' larger side: as asked (params[1].y), else the square's.
 let want=input.params[1].y;
 let k=select(side,min(want,side),want>0.)/%s;
 let q=input.p-input.size*.5;
 let s=input.scale;

 // Still (motion reduced): every tile on its resting face.
 let still=input.params[1].x>.5;
 let t=select(input.time/%s,-1.,still);
 var tone=0.;
 var a=0.;
%s
 return vec4(ink.rgb,ink.a*a*tone);
}
]], num(LIT), num(REST), num(math.max(w, h)), num(g.cycle), table.concat(tiles, "\n")))
end

-- Made once each, as the app first shows one.
local shaders: { [Glyph]: any } = {}
local function shader_of(name: Glyph): any
	local made = shaders[name]
	if not made then
		made = shader(name)
		shaders[name] = made
	end
	return made
end

-- Its loop put where one clock has it (ds/status `in_phase`): every loader
-- of a glyph stands at the same point of its cycle whenever it shows.
local function in_phase(anim: any, period: number)
	anim.currentTime = ui.now() % period
end

return function(props: Props): Node
	local name: Glyph = props.glyph or "agent"
	local size = props.size or 16
	local drawn = props.drawn or (if name == "subagent" then SMALL else 0)
	local cycle = GLYPHS[name].cycle
	local period = cycle * 1000
	-- It plays only while it can be seen (ds/status's ring, `looping`):
	-- out of sight it stops, and starts again in step as it shows. (Whether
	-- it plays, not the loop: a handle held here would keep its element.)
	local playing = false
	return ui.div {
		w = size,
		h = size,
		shrink = 0,
		aria_hidden = true,
		-- (What a test finds it by: `.agent-loader-agent`, `.agent-loader-subagent`.)
		class = `agent-loader-{name}`,
		bg_shader = shader_of(name),
		-- (A token: its colour follows the theme.)
		-- (Still when motion is reduced: its tiles at rest, `params[1].x`.)
		shader_params = { props.ink or "$text.secondary", { if motion.reduced_now() then 1 else 0, drawn, 0, 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 } },
		on_visible = function(visible: boolean, el: Element)
			if visible then
				if not playing and not motion.reduced_now() then
					playing = true
					in_phase(
						anime.animate(el, {
							shaderTime = { 0, cycle },
							duration = period,
							loop = true,
							ease = "linear",
							frameRate = FPS,
						}),
						period
					)
				end
			elseif playing then
				playing = false
				anime.utils.remove(el)
			end
		end,
	}
end
