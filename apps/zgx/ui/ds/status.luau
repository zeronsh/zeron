-- What an agent is doing, at a glance: a state's glyph in its hue with its
-- name (in a chat's header; working, it counts the time), or a list row's
-- dot (pulsing while it works), its name a tooltip. An agent at work is its
-- light sweep (ds/sweep): the mark, or a sub-agent's dot glyph; the ring is
-- for anything else loading (`Spinner`).
--
-- `Status { kind = "working", since = os.time() }`, `StatusDot { kind =
-- "input" }`, `StatusIcon { kind = "done" }`, `StatusIcon { kind =
-- "working", who = "subagent" }`.

local motion = require("@self/motion")
local sweep = require("@self/sweep")
local Icon = require("@self/icon")
local prefers = require("@self/prefers")
local locale = require("@self/locale")

local M = {}

export type Kind = "working" | "connecting" | "waiting" | "input" | "approval" | "plan" | "done" | "failed" | "limited" | "woke"

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
	-- Put to sleep, and woken (its time came, or it asked): not looked at since.
	woke = { "ds.status.woke", "ds.status.woke_long" },
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
	-- Waiting on background work that will wake it: present, at rest (its
	-- sweep standing still, in working's hue), its name its own.
	waiting = kind("waiting", "status-working", true),
	input = kind("input", "status-input"),
	approval = kind("approval", "status-approval"),
	plan = kind("plan"),
	done = kind("done", "status-done"),
	failed = kind("failed", "status-failed"),
	limited = kind("limited", "status-failed"),
	woke = kind("woke", "status-woke"),
}
M.KINDS = KINDS

-- The hue a state is drawn in: waiting wears working's.
local function hue(kind: Kind): string
	return if kind == "waiting" then "working" else kind
end

-- How long it has worked ("42s", "5m", "1h 5m"), and how long ago in a
-- corner's room ("now", "5m", "3d"), in the locale's words (ds/locale).
local elapsed = locale.elapsed
M.elapsed = elapsed
local ago = locale.ago_short
M.ago = ago

-- Seconds since `t` (a time, `os.time()` seconds), followed: what reads it
-- in a binding moves on as the clock ticks, so "now" becomes "1m" without
-- anything else changing. One timer for every reader, sleeping between
-- ticks. (The clock is never behind the wall's: a test can set it ahead.)
local clock = ui.state("ds_status_clock", os.time())
ui.every(30000, function()
	clock:set(os.time())
end)
function M.since(t: number): number
	return math.max(0, math.max(clock:get(), os.time()) - t)
end

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
-- (milliseconds) and its frame rate.
local RUN, REST, FPS = 3.9, 900, 30

-- Its loop put where one clock has it: a loop of `period` ms that started
-- when that clock read 0. Every ring (and every dot) stands at the same
-- point of its cycle whenever it shows, so one coming back into sight picks
-- up in step with the rest, not from its start. (Set as the loop is made,
-- before the renderer ever takes it over, which it still may.)
function M.in_phase(anim: any, period: number)
	anim.currentTime = ui.now() % period
end

-- A node whose loop (`start(el)`'s animation, `period` ms a cycle) plays
-- only while it can be seen: out of sight (in a collapsed sidebar, a
-- closed panel, scrolled away) it stops, and starts again as it shows, in
-- step with the clock (`in_phase`).
-- Left looping there, it would be all the animation engine had to play,
-- and the engine, seeing only unseen loops, looks just four times a second:
-- an animation starting then would wait for that look, and leap. Still
-- when motion is reduced.
local function looping(props: ContainerProps, start: (el: Element) -> Animation, period: number): Node
	-- Whether it plays, and not the loop itself: a handle held here strongly
	-- would hold its element, the element this node, and this node this
	-- function, a cycle through Rust that no collector sees; held weakly, the
	-- collector takes it while the loop plays, and nothing could stop it.
	local playing = false
	props.on_visible = function(visible: boolean, el: Element)
		if visible then
			if not playing and not motion.reduced_now() then
				playing = true
				M.in_phase(start(el), period)
			end
		elseif playing then
			playing = false
			anime.utils.remove(el)
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
	end, RUN * 1000 + REST)
end

-- The working ring alone, as a spinner: `size` points, in `ink` (a token;
-- a button's busy state draws it in the button's own text colour).
function M.Spinner(props: { size: number?, ink: string? }): Node
	return ring(props.ink or "$zeron.status.working", props.size or 14)
end

-- An agent's presence: one sweep where you look, still marks elsewhere.
-- A glyph's `role`: "primary" sweeps while it shows (the open chat's foot,
-- its running sub-agents, `presence.primaries` of those at most); "quiet"
-- (the default: rows, tabs, the palette, badges) sweeps for `presence.
-- start` as it's drawn (its agent started or came back) and then holds the
-- mark still, lit through its gap, in its hue. A "needs you" glyph drawn
-- quietly flashes for a moment. Nothing moves while the window isn't the
-- active one or motion is reduced.
export type Who = "agent" | "subagent"
export type Role = "primary" | "quiet"
local P = motion.presence

-- The beat: what moves (`frame(k)`: the properties of its k-th frame,
-- every `every` beats), all set at once by one timer on the beat's own
-- boundaries, from `ui.now()`, so everything on screen shows the same frame
-- whenever it came into sight. No animation plays for it: the engine stays
-- idle (a loop capped to a low frame rate keeps any other that starts
-- meanwhile waiting for its next frame). The timer runs only while
-- something moves, the window is active and motion isn't reduced; what's
-- gone from the window drops out. A quiet one ends (`done`: set to `last`)
-- when its time is up.
type Beat = {
	every: number,
	frame: (k: number) -> { [string]: any },
	last: { [string]: any }?,
	done: boolean?,
	primary: boolean?,
	el: Element?,
	shown: number?,
}
local beating: { [Beat]: boolean } = {}
local next_beat: (() -> ())? = nil
local watching = false
local function beat()
	next_beat = nil
	if ui.window.active:peek() == false then
		return
	end
	local now = ui.now()
	for b in beating do
		local el = b.el :: Element
		if not (el :: any).mounted then
			beating[b] = nil
			continue
		end
		local k = math.floor(now / (P.beat * b.every) + 0.001)
		if k ~= b.shown then
			b.shown = k
			anime.utils.set(el, b.frame(k))
		end
	end
	if next(beating) ~= nil then
		next_beat = ui.after(P.beat - now % P.beat, beat)
	end
end
-- How many sub-agents sweep now (primaries past `presence.primaries` hold
-- still), and how many glyphs move at all.
local subagents = 0
function M.moving(): number
	local n = 0
	for _ in beating do
		n += 1
	end
	return n
end
local function stop(b: Beat)
	if beating[b] then
		beating[b] = nil
		if b.primary then
			subagents -= 1
		end
	end
end
local function on_beat(props: ContainerProps, b: Beat, quiet: boolean): Node
	if quiet then
		ui.after(P.start, function()
			b.done = true
			if beating[b] and b.last then
				anime.utils.set(b.el :: Element, b.last)
			end
			stop(b)
		end)
	end
	props.on_visible = function(visible: boolean, el: Element)
		b.el = el
		if not visible or motion.reduced_now() or b.done or (b.primary and subagents >= P.primaries) then
			if b.last and (b.done or visible) then
				anime.utils.set(el, b.last)
			end
			stop(b)
			return
		end
		if not beating[b] then
			beating[b] = true
			b.shown = nil
			if b.primary then
				subagents += 1
			end
		end
		if not watching then
			-- (The window coming back: what moves picks up.)
			watching = true
			ui.effect(function()
				if ui.window.active:get() ~= false and not next_beat and next(beating) ~= nil then
					ui.after(0, beat)
				end
			end)
		end
		if not next_beat then
			beat()
		end
	end
	return ui.div(props)
end

-- An agent's light sweep (ds/sweep), as its role has it: still when it
-- waits or motion is reduced.
function M.Sweep(props: sweep.Props & { role: Role? }): Node
	local given = table.clone(props)
	given.reduced = motion.reduced_now()
	given.rest = P.rest
	local node, every, glyph = sweep.node(given)
	if not every then
		return ui.div(node)
	end
	local primary = props.role == "primary"
	return on_beat(node, {
		every = every,
		frame = function(k: number)
			return { shaderTime = sweep.band(glyph, k) }
		end,
		last = { shaderTime = sweep.GLYPHS[glyph].still },
		-- (A sub-agent's: one of the few that sweep.)
		primary = primary and props.who == "subagent",
	}, not primary)
end
local function present(kind: Kind, size: number, who: Who?, role: Role?, ink: string?): Node
	local hue = if kind == "connecting" then "connecting" else "working"
	return M.Sweep {
		who = who,
		size = size,
		role = role,
		pace = if kind == "waiting" then "still" else hue :: any,
		ink = ink or `$zeron.status.{hue}`,
	}
end

-- "Needs you" (input, approval) drawn quietly: a flash as it's drawn, a
-- dimming and back twice, then still.
local FLASH = { 0.3, 1 }
local function needs(kind: Kind): boolean
	return kind == "input" or kind == "approval"
end
local function flash(child: Node): Node
	return on_beat({ shrink = 0, child }, {
		every = 2,
		frame = function(k: number)
			return { opacity = FLASH[k % 2 + 1] }
		end,
		last = { opacity = 1 },
	}, true)
end

-- A state's glyph, `size` points: working, waiting and connecting an
-- agent's sweep.
local function glyph(kind: Kind, size: number, who: Who?, role: Role?): Node?
	local k = KINDS[kind]
	if kind == "working" or kind == "connecting" or kind == "waiting" then
		return present(kind, size, who, role)
	end
	if not k.glyph then
		return nil
	end
	local icon = Icon { name = k.glyph, size = size }
	return if needs(kind) and role ~= "primary" then flash(icon) else icon
end

-- A state's glyph alone, in its hue: at work, the agent's sweep (`who`:
-- "agent", the mark, by default; "subagent", the dot glyph), as its `role`
-- has it (quiet by default). `kind` may be a function: the glyph and its
-- hue follow it in place, and only a move between the sweep's kinds, a
-- "needs you" and a glyph swaps what's drawn.
function M.StatusIcon(props: { kind: Kind | () -> Kind, size: number?, who: Who?, role: Role? }): Node
	local size = props.size or 16
	local given = props.kind
	if type(given) ~= "function" then
		local kind = given :: Kind
		return ui.div { shrink = 0, color = `$zeron.status.{hue(kind)}`, glyph(kind, size, props.who, props.role) }
	end
	local kind = given :: () -> Kind
	local function drawn(): string
		local k = kind()
		return if k == "working" or k == "connecting" or k == "waiting"
			then k
			elseif needs(k) then k
			else "glyph"
	end
	return ui.div {
		shrink = 0,
		color = function(): string
			return `$zeron.status.{hue(kind())}`
		end,
		ui.switch(drawn, {
			working = present("working", size, props.who, props.role),
			connecting = present("connecting", size, props.who, props.role),
			waiting = present("waiting", size, props.who, props.role),
			input = glyph("input", size, props.who, props.role),
			approval = glyph("approval", size, props.who, props.role),
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
M.Status = ui.component(function(props: { kind: Kind, label: string?, since: number?, who: Who? }): Node
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
		color = `$zeron.status.{hue(props.kind)}`,
		text_size = "$font.size.sm",
		weight = "$font.weight.medium",
		glyph(props.kind, 16, props.who),
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
-- working, waiting or connecting, the agent's micro mark instead (quiet:
-- see `StatusIcon`).
function M.StatusDot(props: { kind: Kind, size: number? }): Node
	local k = KINDS[props.kind]
	local size = props.size or 9
	if props.kind == "working" or props.kind == "connecting" or props.kind == "waiting" then
		return ui.div {
			shrink = 0,
			tooltip = k.long,
			label = k.long,
			present(props.kind, size, nil, "quiet", `$zeron.status.dot.{hue(props.kind)}`),
		}
	end
	return ui.div {
		w = size,
		h = size,
		shrink = 0,
		rounded = "$radius.full",
		bg = `$zeron.status.dot.{hue(props.kind)}`,
		tooltip = k.long,
		label = k.long,
	}
end

-- A state's dot and its long name beside it, small, in its hue (a list
-- row with room for it).
function M.StatusTag(props: { kind: Kind }): Node
	local k = KINDS[props.kind]
	return ui.row {
		gap = "$space.1",
		items = "center",
		color = `$zeron.status.{hue(props.kind)}`,
		M.StatusDot { kind = props.kind, size = 6 },
		ui.text { locale.t(NAMES[props.kind][2]), text_size = "$font.size.xs" },
	}
end

return M
