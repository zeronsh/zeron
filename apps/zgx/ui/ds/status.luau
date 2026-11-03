-- What an agent is doing, at a glance: a state's glyph in its hue with its
-- name (in a chat's header; working, it counts the time), or a list row's
-- dot (pulsing while it works), its name a tooltip. A first pass, to iterate
-- from.
--
-- `Status { kind = "working", since = os.time() }`, `StatusDot { kind =
-- "input" }`, `StatusIcon { kind = "done" }`.

local motion = require("@self/motion")
local Icon = require("@self/icon")
local prefers = require("@self/prefers")
local locale = require("@self/locale")

local M = {}

export type Kind = "working" | "connecting" | "waiting" | "input" | "approval" | "plan" | "done" | "failed" | "limited"

-- Each state's glyph (none: its name alone) and its names: short, in a
-- header; long, on a list's dot. The names are read as strings in the
-- locale shown (`k.label`, `k.long`: a tooltip takes a string), each read
-- through its message as it's asked for.
type KindInfo = { glyph: Icon.Name?, label: string, long: string, pulse: boolean? }
local NAMES: { [Kind]: { string } } = {
	working = { "ds.status.working", "ds.status.working" },
	connecting = { "ds.status.connecting", "ds.status.connecting" },
	waiting = { "ds.status.waiting", "ds.status.waiting" },
	input = { "ds.status.input", "ds.status.input_long" },
	approval = { "ds.status.approval", "ds.status.approval_long" },
	plan = { "ds.status.plan", "ds.status.plan_long" },
	done = { "ds.status.done", "ds.status.done_long" },
	failed = { "ds.status.failed", "ds.status.failed" },
	limited = { "ds.status.limited", "ds.status.limited" },
}
local function kind(name: Kind, glyph: Icon.Name?, pulse: boolean?): KindInfo
	local names = NAMES[name]
	return setmetatable({ glyph = glyph, pulse = pulse }, {
		__index = function(_, field: string): string?
			if field == "label" then
				return locale.s(names[1])
			elseif field == "long" then
				return locale.s(names[2])
			end
			return nil
		end,
	}) :: any
end
local KINDS: { [Kind]: KindInfo } = {
	working = kind("working", "status-working", true),
	connecting = kind("connecting", "status-working", true),
	waiting = kind("waiting"),
	input = kind("input", "status-input"),
	approval = kind("approval", "status-approval"),
	plan = kind("plan"),
	done = kind("done", "status-done"),
	failed = kind("failed", "status-failed"),
	limited = kind("limited", "status-failed"),
}
M.KINDS = KINDS

-- How long it has worked ("42s", "5m", "1h 5m"), and how long ago in a
-- corner's room ("now", "5m", "3d"), in the locale's words (ds/locale).
local elapsed = locale.elapsed
M.elapsed = elapsed
local ago = locale.ago_short
M.ago = ago

-- Working: the dashed ring (the glyph's eight dashes, on its 24-unit grid:
-- radius 10, stroke 2, round ends), drawn along. An unseen lead runs the
-- ring twice in one eased sweep (slow away, quick round, slow home); the
-- dashes near it stretch and lean toward it, most just behind it (its pull
-- trails it the faster it goes), so neighbours reach into one another and
-- let go as it passes. Home again it lets them settle to eight even dashes
-- (the glyph exactly), and the ring rests before it runs again. The dashes
-- keep their shape throughout: only their lengths and places move, and a
-- stretched dash grows stronger with its stretch (lighter on dark, darker on
-- light). All from
-- the time, on the GPU, at 30 frames a second (smooth at an icon's size, a
-- quarter of a 120 Hz display's), and none while it rests.
-- params[0]: ink. A stretched dash leans away from the middle grey its ink
-- sits nearest: a light ink (a dark theme's) toward white, a dark one
-- toward black. (Worked out here: zgx resolves a theme token only in a
-- shader's first parameter.)
local RING = ui.shader([[
// Distance to a round-ended arc on the ring (radius R) centred at angle c
// (degrees, clockwise from the top), `ha` degrees either way, hw thick.
fn zr_arc(d:vec2<f32>,R:f32,c:f32,ha:f32,hw:f32)->f32 {
 var th=degrees(atan2(d.x,-d.y));
 var da=th-c; da=da-360.*round(da/360.);
 if abs(da)<=ha { return abs(length(d)-R)-hw; }
 let e=radians(c+sign(da)*ha);
 return length(d-vec2(sin(e),-cos(e))*R)-hw;
}
fn fill(input:FillInput)->vec4<f32> {
 let ink=input.params[0];
 let u=min(input.size.x,input.size.y)/24.;
 let d=input.p-input.size*.5;
 let R=10.*u; let hw=u;
 // One run: the lead sets off, runs two laps, comes home (its rest is the
 // animation's pause between runs, at the run's end: even dashes).
 let RUN=3.9;
 let ph=clamp(input.time,0.,RUN);
 let amp=smoothstep(0.,.55,ph)*(1.-smoothstep(RUN-.75,RUN,ph));
 let tau=clamp(ph/RUN,0.,1.);
 let lead=720.*(.5-.5*cos(3.14159265*tau));
 let speed=sin(3.14159265*tau);
 // Where it pulls from: behind the lead, the more so the faster.
 let pull=lead-14.*speed;
 // Each dash's coverage here, and how stretched the most stretched one
 // covering it is (it lightens as it stretches).
 var a=0.;
 var lit=0.;
 for (var i=0;i<8;i++) {
  let c=45.*f32(i);
  var dl=pull-c; dl=dl-360.*round(dl/360.);
  // Ahead of the pull it reaches a little; behind it, further.
  let sig=select(40.,26.,dl<0.);
  let g=amp*exp(-(dl*dl)/(sig*sig));
  let cov=clamp(.5-zr_arc(d,R,c+sign(dl)*9.*g,10.95*(1.+1.3*g),hw)*input.scale,0.,1.);
  a=max(a,cov);
  lit=max(lit,g*cov);
 }
 // Stretched, stronger: lighter if its ink is light, darker if dark, by as
 // much as 60% at the full stretch.
 let t=.6*lit/max(a,.001);
 let toward=select(vec3(0.),vec3(1.),dot(ink.rgb,vec3(.2126,.7152,.0722))>.5);
 return vec4(mix(ink.rgb,toward,t),ink.a*a);
}
]])

-- The ring's run (seconds, as the shader has it), its rest between runs
-- (milliseconds) and its frame rate; the dots pulse at the same rate, so
-- the frames that draw one draw them all.
local RUN, REST, FPS = 3.9, 900, 30

-- A node whose loop (`start(el)`'s animation) plays only while it can be
-- seen: out of sight (in a collapsed sidebar, a closed panel) it stops,
-- and starts again as it shows.
-- Left looping there, it would be all the animation engine had to play,
-- and the engine, seeing only unseen loops, looks just four times a second:
-- an animation starting then would wait for that look, and leap. Still
-- when motion is reduced.
local function looping(props: ContainerProps, start: (el: Element) -> Animation): Node
	-- The loop, held weakly: the engine holds it while it plays. Held here
	-- strongly, it would hold its element, the element this node, and this
	-- node this function: a cycle through Rust that no collector sees, and
	-- every row with a loop would stay alive after it leaves.
	local held = setmetatable({}, { __mode = "v" }) :: { anim: Animation? }
	props.on_visible = function(visible: boolean, el: Element)
		if visible then
			if not held.anim and not motion.reduced_now() then
				held.anim = start(el)
			end
		elseif held.anim then
			held.anim:cancel()
			held.anim = nil
		end
	end
	return ui.div(props)
end

-- The working ring, `size` points, in `ink` (a token).
local function ring(ink: string, size: number): Node
	return looping({
		w = size,
		h = size,
		shrink = 0,
		bg_shader = RING,
		-- (A token: its colour follows the theme.)
		shader_params = { ink, { 0, 0, 0, 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 } },
	}, function(el)
		-- A run, then a rest that draws nothing (the loop's pause), and again.
		return anime.animate(el, {
			shaderTime = { 0, RUN },
			duration = RUN * 1000,
			loop = true,
			loopDelay = REST,
			ease = "linear",
			frameRate = FPS,
		})
	end)
end

-- The working ring alone, as a spinner: `size` points, in `ink` (a token;
-- a button's busy state draws it in the button's own text colour).
function M.Spinner(props: { size: number?, ink: string? }): Node
	return ring(props.ink or "$zeron.status.working", props.size or 14)
end

-- A state's glyph, `size` points: working and connecting their ring.
local function glyph(kind: Kind, size: number): Node?
	local k = KINDS[kind]
	if kind == "working" or kind == "connecting" then
		return ring(`$zeron.status.{kind}`, size)
	end
	return if k.glyph then Icon { name = k.glyph, size = size } else nil
end

-- A state's glyph alone, in its hue. `kind` may be a function: the glyph
-- and its hue follow it in place, and only a move between the working ring
-- and a glyph (or between the ring's two kinds) swaps what's drawn.
function M.StatusIcon(props: { kind: Kind | () -> Kind, size: number? }): Node
	local size = props.size or 16
	local given = props.kind
	if type(given) ~= "function" then
		local kind = given :: Kind
		return ui.div { shrink = 0, color = `$zeron.status.{kind}`, glyph(kind, size) }
	end
	local kind = given :: () -> Kind
	local function drawn(): string
		local k = kind()
		return if k == "working" or k == "connecting" then k else "glyph"
	end
	return ui.div {
		shrink = 0,
		color = function(): string
			return `$zeron.status.{kind()}`
		end,
		ui.switch(drawn, {
			working = ring("$zeron.status.working", size),
			connecting = ring("$zeron.status.connecting", size),
			glyph = ui.show(function(): boolean
				return KINDS[kind()].glyph ~= nil
			end, Icon {
				name = function(): Icon.Name
					return KINDS[kind()].glyph or "status-done"
				end,
				size = size,
			}),
		}),
	}
end

-- A state's glyph and its name, in its hue; working, the time it has taken
-- (from `since`, in `os.time()` seconds) after it.
M.Status = ui.component(function(props: { kind: Kind, label: string?, since: number? }): Node
	local k = KINDS[props.kind]
	local now = ui.use_state(os.time())
	if props.since and props.kind == "working" then
		ui.every(1000, function()
			now:set(os.time())
		end)
	end
	return ui.row {
		gap = "$space.1",
		items = "center",
		color = `$zeron.status.{props.kind}`,
		text_size = "$font.size.sm",
		weight = "$font.weight.medium",
		glyph(props.kind, 16),
		ui.text { props.label or locale.t(NAMES[props.kind][1]) },
		if props.since and props.kind == "working"
			then ui.text {
				function(): string
					return elapsed(now:get() - (props.since :: number))
				end,
			}
			else nil,
	}
end)

-- A list row's dot: 9 points, its state's hue, its long name a tooltip;
-- working or connecting, it pulses (dimming to half and back every two
-- seconds, in six steps each way).
function M.StatusDot(props: { kind: Kind, size: number? }): Node
	local k = KINDS[props.kind]
	local size = props.size or 9
	local dot: ContainerProps = {
		w = size,
		h = size,
		shrink = 0,
		rounded = "$radius.full",
		bg = `$zeron.status.dot.{props.kind}`,
		tooltip = k.long,
		label = k.long,
	}
	if not k.pulse then
		return ui.div(dot)
	end
	return looping(dot, function(el)
		-- The dimming and the brightening alone, each held for the loop's
		-- pause (which draws nothing) before the next.
		return anime.animate(el, {
			opacity = { 1, 0.5 },
			duration = 200,
			ease = "steps(6)",
			loop = true,
			alternate = true,
			loopDelay = 800,
			frameRate = FPS,
		})
	end)
end

-- A state's dot and its long name beside it, small, in its hue (a list
-- row with room for it).
function M.StatusTag(props: { kind: Kind }): Node
	local k = KINDS[props.kind]
	return ui.row {
		gap = "$space.1",
		items = "center",
		color = `$zeron.status.{props.kind}`,
		M.StatusDot { kind = props.kind, size = 6 },
		ui.text { locale.t(NAMES[props.kind][2]), text_size = "$font.size.xs" },
	}
end

return M
