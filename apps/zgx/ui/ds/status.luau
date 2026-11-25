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
	-- Stopped by its provider's usage limit, waiting for the reset
	-- (ds/usage_limit).
	limited = { "ds.status.limited", "ds.limit.waiting" },
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
	-- Waiting on background work that will wake it: under way, drawn as
	-- working is (its ring, in working's hue), its name its own.
	waiting = kind("waiting", "status-working", true),
	input = kind("input", "status-input"),
	approval = kind("approval", "status-approval"),
	plan = kind("plan"),
	done = kind("done", "status-done"),
	failed = kind("failed", "status-failed"),
	limited = kind("limited", "status-limited"),
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
--
-- It ticks only as a label read from it would change: each age read says
-- when its label next steps (`ago`'s: "now" to "1m" at a minute, minutes
-- to "1h" at an hour, hours to days, days to weeks), and the clock moves at
-- the soonest of those, on a half-minute grid so that labels due within the
-- same half minute move together, as late as the half-minute tick that
-- once moved them all. Corners an hour old wake the window once an hour,
-- not twice a minute.
local clock = ui.state("ds_status_clock", os.time())
-- When it next moves (`os.time()` seconds; none: nothing read).
local due = ui.state("ds_status_clock_due", math.huge)
ui.effect(function()
	local at = due:get()
	if at == math.huge then
		return
	end
	-- (This run's: the next moves it, or cancels it.)
	ui.after(math.max(1, at - os.time()) * 1000, function()
		ui.batch(function()
			due:set(math.huge)
			-- (Readers run again and say when they next change.)
			clock:set(os.time())
		end)
	end)
end)
-- Seconds until an age's label (`ago`) next reads otherwise.
local function next_step(age: number): number
	local span = if age < 3600 then 60 elseif age < 86400 then 3600 elseif age < 7 * 86400 then 86400 else 7 * 86400
	return span - age % span
end
local GRID = 30
function M.since(t: number): number
	local now = math.max(clock:get(), os.time())
	local age = math.max(0, now - t)
	local at = math.ceil((now + next_step(age)) / GRID) * GRID
	if at < due:peek() then
		-- (A binding reading the clock doesn't write state: the next turn
		-- does. Should this run be replaced first, the next run asks again.)
		ui.after(0, function()
			if at < due:peek() then
				due:set(at)
			end
		end)
	end
	return age
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
-- (milliseconds) and its frame rate; the dots pulse at the same rate, so
-- the frames that draw one draw them all.
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

-- Off macOS, a 30 fps loop keeps zgx's event loop turning between its
-- frames (a frame that draws nothing asks for no frame callback, so the
-- next is handed out at once): one ring on screen held a core at half.
-- There the rings and dots step instead, all on one clock, STEP ms apart,
-- a timer that sleeps between steps and stops while none shows. On macOS
-- a loop is baked and played by Core Animation: no frames, no wakes.
local STEP = 100
local platform: string? = nil
local function stepped(): boolean
	if platform == nil then
		local ok, name = pcall(function(): any
			return ui.shared("platform", ""):peek()
		end)
		platform = if ok and type(name) == "string" then name else ""
	end
	return platform ~= "macos"
end
M.stepped = stepped
-- The steps' clock (ui.now() at the last step); what steps (each an
-- element and what sets it at a step, by a key of its own), and how many
-- steppers show (a dot steps by reading `step_clock`, nothing set).
local step_clock = ui.state("ds_status_step", 0)
type Step = { el: Element, set: (el: Element, now: number) -> () }
local steps: { [any]: Step } = {}
local showing = 0
local stepping = false
local step_timer: any = nil
local function ring_time(now: number): number
	return math.min((now % (RUN * 1000 + REST)) / 1000, RUN)
end
local ticked = 0
-- A second's look, the script's own: steppers showing and the clock
-- stopped, as when what started its timer went, it starts again. It looks
-- only while something steps: idle, nothing of the script runs (a frame
-- with nothing moving calls no script). Started from its own effect, so
-- that a start from a component's run doesn't tie it to that component.
local watch = ui.state("ds_status_watch", 0)
local watching = false
local step_start: () -> ()
ui.effect(function()
	if watch:get() == 0 then
		return
	end
	ui.every(1000, function(): boolean?
		if showing <= 0 then
			watching = false
			return false
		end
		step_start()
		return nil
	end)
end)
function step_start()
	if not watching then
		watching = true
		watch:set(watch:peek() + 1)
	end
	-- (Its timer is owned as an effect's is: started from one's run, it goes
	-- when that runs again. Not stepped for three steps: started again.)
	if stepping and ui.now() - ticked < 3 * STEP then
		return
	end
	stepping = true
	ticked = ui.now()
	local mine = {}
	step_timer = mine
	ui.every(STEP, function(): boolean?
		if step_timer ~= mine then
			return false
		end
		if showing <= 0 then
			showing = 0
			stepping = false
			return false
		end
		local now = ui.now()
		ticked = now
		step_clock:set(now)
		for key, st in steps do
			-- (An element gone without a word: it steps no more.)
			if not pcall(st.set, st.el, now) then
				M.unstep(key)
			end
		end
		return nil
	end)
end
-- Steps `el` (under `key`) on the shared clock, `set(el, now)` at each step
-- (and at once), until `M.unstep(key)`: for a loop off macOS that a script
-- starts and stops itself. Nothing when motion is reduced.
function M.step(key: any, el: Element, set: (el: Element, now: number) -> ())
	if motion.reduced_now() then
		return
	end
	if not steps[key] then
		showing += 1
	end
	steps[key] = { el = el, set = set }
	pcall(set, el, ui.now())
	step_start()
end
function M.unstep(key: any)
	if steps[key] then
		steps[key] = nil
		showing -= 1
	end
end
-- A node that steps while it can be seen (`set`: its element set at each
-- step; else its bound values read `step_clock`), and stops as it goes.
-- Still when motion is reduced.
local Stepper = ui.component(function(props: { node: ContainerProps, set: ((el: Element, now: number) -> ())? }): Node
	local key = {}
	local shown = false
	local function hide()
		if shown then
			shown = false
			if props.set then
				M.unstep(key)
			else
				showing -= 1
			end
		end
	end
	ui.on_cleanup(hide)
	local node = props.node
	node.on_visible = function(visible: boolean, el: Element)
		if visible and not shown then
			if motion.reduced_now() then
				return
			end
			shown = true
			if props.set then
				M.step(key, el, props.set)
			else
				showing += 1
				step_start()
			end
		elseif not visible then
			hide()
		end
	end
	return ui.div(node)
end)
-- `node`, stepped while it shows: `set(el, now)` at each step.
function M.Stepped(node: ContainerProps, set: ((el: Element, now: number) -> ())?): Node
	return Stepper { node = node, set = set }
end
local function set_ring(el: Element, now: number)
	anime.utils.set(el, { shaderTime = ring_time(now) })
end
local function stepper(node: ContainerProps, is_ring: boolean?): Node
	return Stepper { node = node, set = if is_ring then set_ring else nil }
end
-- A sine pulse between `from` and `to`, there and back every `2 * half` ms
-- (anime's inOutSine, alternating), at `now`.
function M.sine_at(now: number, from: number, to: number, half: number): number
	local p = (now % (2 * half)) / half
	if p > 1 then
		p = 2 - p
	end
	return from + (to - from) * (0.5 - 0.5 * math.cos(math.pi * p))
end
-- The time (ms) into a `period` ms cycle, by the steps' clock.
local function step_at(period: number): number
	return step_clock:get() % period
end
M.test_step = {
	clock = step_clock,
	showing = function(): number
		return showing
	end,
	stepping = function(): boolean
		return stepping
	end,
	STEP = STEP,
}

-- A glyph's size: points, or a function the glyph follows as it changes
-- (a map's zoom), drawn at the size it is each frame.
export type Size = number | () -> number

-- The working ring, `size` points, in `ink` (a token).
local function ring(ink: string, size: Size): Node
	if stepped() then
		-- Its run and rest by the steps' clock (`shaderTime` set at each).
		return stepper({
			w = size,
			h = size,
			shrink = 0,
			bg_shader = RING,
			shader_params = { ink, { 0, 0, 0, 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 } },
		}, true)
	end
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

-- A state's glyph, `size` points: working, waiting and connecting their
-- ring.
local function glyph(kind: Kind, size: Size): Node?
	local k = KINDS[kind]
	if kind == "working" or kind == "connecting" or kind == "waiting" then
		return ring(`$zeron.status.{hue(kind)}`, size)
	end
	return if k.glyph then Icon { name = k.glyph, size = size } else nil
end

-- A state's glyph alone, in its hue. `kind` may be a function: the glyph
-- and its hue follow it in place, and only a move between the working ring
-- and a glyph (or between the ring's two kinds) swaps what's drawn.
function M.StatusIcon(props: { kind: Kind | () -> Kind, size: Size? }): Node
	local size: Size = props.size or 16
	local given = props.kind
	if type(given) ~= "function" then
		local kind = given :: Kind
		return ui.div { shrink = 0, color = `$zeron.status.{hue(kind)}`, glyph(kind, size) }
	end
	local kind = given :: () -> Kind
	-- (Working and waiting share one ring: a move between them keeps it.)
	local function drawn(): string
		local k = hue(kind())
		return if k == "working" or k == "connecting" then k else "glyph"
	end
	return ui.div {
		shrink = 0,
		color = function(): string
			return `$zeron.status.{hue(kind())}`
		end,
		ui.switch(drawn, {
			working = ring("$zeron.status.working", size),
			connecting = ring("$zeron.status.connecting", size),
			-- (A kind with no entry, nil as a row goes or one a newer
			-- engine sends, draws no glyph.)
			glyph = ui.show(function(): boolean
				local k = KINDS[kind()]
				return k ~= nil and k.glyph ~= nil
			end, Icon {
				name = function(): Icon.Name
					local k = KINDS[kind()]
					return k and k.glyph or "status-done"
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
		color = `$zeron.status.{hue(props.kind)}`,
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
-- working, waiting or connecting, it pulses (dimming to half and back every
-- two seconds, in six steps each way).
function M.StatusDot(props: { kind: Kind, size: number? }): Node
	local k = KINDS[props.kind]
	local size = props.size or 9
	local dot: ContainerProps = {
		w = size,
		h = size,
		shrink = 0,
		rounded = "$radius.full",
		bg = `$zeron.status.dot.{hue(props.kind)}`,
		tooltip = k.long,
		label = k.long,
	}
	if not k.pulse then
		return ui.div(dot)
	end
	if stepped() then
		-- Dimming to half (200 ms), held (800), brightening (200), held:
		-- by the steps' clock.
		dot.opacity = function(): number
			local p = step_at(2 * (200 + 800))
			if p < 200 then
				return 1 - 0.5 * p / 200
			elseif p < 1000 then
				return 0.5
			elseif p < 1200 then
				return 0.5 + 0.5 * (p - 1000) / 200
			end
			return 1
		end
		return stepper(dot)
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
		-- (A dimming and a brightening, each with its pause.)
	end, 2 * (200 + 800))
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
