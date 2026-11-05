-- Progress: how far along something is. A bar (a push uploading, an
-- index building) and a ring (a check running, a clone in a row's
-- corner). Given a value from 0 to 1 they fill to it, easing as it moves;
-- given none they say work of unknown size is going on: the bar's stretch
-- sweeps across, the ring's arc runs round, growing and shrinking. Both
-- draw on the switch's translucent groove, filled in a tone's ink (the
-- accent's by default).
--
-- `ProgressBar { value = fraction }`, `ProgressBar {}` (indeterminate),
-- `ProgressRing { value = fraction, size = 16 }`, `ProgressRing {}`.

local motion = require("@self/motion")
local control = require("@self/control")
local locale = require("@self/locale")

-- What it says to assistive technology: its name (a message too) and how
-- far along, or that it's working ("Progress, 42%"). Read in its label's
-- function, so it follows the locale.
local function said(label: Label?, v: number?): string
	local name = control.text(label) or locale.s("ds.progress.name")
	if v then
		return locale.s("ds.progress.value", { name = name, percent = math.floor(v * 100 + 0.5) })
	end
	return locale.s("ds.progress.working", { name = name })
end

local M = {}

export type Tone = "accent" | "info" | "success" | "warning" | "danger" | "neutral"
export type Value = (State<number?> | () -> number?)?

local function reader(v: Value): () -> number?
	if v == nil then
		return function()
			return nil
		end
	elseif type(v) == "function" then
		return v
	end
	return function()
		return (v :: State<number?>):get()
	end
end

-- A tone, or a function of one (a usage meter's, warning then danger as it
-- fills): the bar follows a function as it changes; the ring reads it once.
export type ToneValue = Tone | () -> Tone

local function ink(tone: ToneValue?): string
	local t: Tone? = if type(tone) == "function" then (tone :: any)() else tone :: any
	if t == nil or t == "accent" then
		return "$zeron.switch.on"
	elseif t == "neutral" then
		return "$text.secondary"
	end
	return `$zeron.tone.{t}.ink`
end

export type BarProps = {
	value: Value,
	tone: ToneValue?,
	-- Its thickness: thin (a list row's, 3) or regular (4) or thick (6).
	size: ("thin" | "regular" | "thick")?,
	label: Label?,
	w: number?,
	w_full: boolean?,
	grow: number?,
}

local THICK = { thin = 3, regular = 4, thick = 6 }

M.ProgressBar = ui.component(function(props: BarProps): Node
	local h = THICK[props.size or "regular"]
	local get = reader(props.value)
	local track = ui.ref()
	local indeterminate = props.value == nil
	local fill: Node
	if indeterminate then
		-- The stretch: two-fifths of the track, sweeping from beyond its
		-- start to beyond its end, eased so it lingers at neither.
		local sweep: any = nil
		fill = ui.div {
			absolute = true,
			left = 0,
			top = 0,
			h = h,
			w = "40%",
			rounded = "$radius.full",
			bg = if type(props.tone) == "function"
				then function()
					return ink(props.tone)
				end
				else ink(props.tone),
			on_resize = function(_size, el)
				local w = track.width or 0
				if sweep then
					sweep:cancel()
				end
				-- (Still, with motion reduced: it says it's busy without moving.)
				if motion.reduced_now() then
					return
				end
				sweep = anime.animate(el, { x = { -w * 0.4, w }, duration = 1400, ease = "inOut(2)", loop = true })
			end,
		}
		ui.on_cleanup(function()
			if sweep then
				sweep:cancel()
			end
		end)
	else
		fill = ui.div {
			absolute = true,
			left = 0,
			top = 0,
			h = h,
			rounded = "$radius.full",
			bg = if type(props.tone) == "function"
				then function()
					return ink(props.tone)
				end
				else ink(props.tone),
			w = motion.tween(ui.computed(function(): number
				return math.clamp(get() or 0, 0, 1) * (track.width or 0)
			end), { duration = 300, ease = "out(3)" }),
		}
	end
	return ui.div {
		ref = track,
		role = "status",
		label = function()
			local v = get()
			return said(props.label, v)
		end,
		relative = true,
		overflow = "hidden",
		h = h,
		w = if props.w_full or props.grow then nil else (props.w or 240),
		w_full = props.w_full,
		grow = props.grow,
		rounded = "$radius.full",
		bg = "$zeron.switch.track",
		fill,
	}
end)

-- The ring, drawn per pixel: a faint full circle (the groove) and an arc
-- of the ink over it with round ends, from the top, clockwise.
-- params[0]: the ink (zgx resolves a theme token only in a shader's first
-- parameter); params[1]: the fraction (or, spinning, nothing: the time
-- moves it), whether it spins, the stroke (points), the groove's strength.
local ARC = [[
fn zp_arc(d:vec2<f32>,R:f32,c:f32,ha:f32,hw:f32)->f32 {
 var th=degrees(atan2(d.x,-d.y));
 var da=th-c; da=da-360.*round(da/360.);
 if abs(da)<=ha { return abs(length(d)-R)-hw; }
 let e=radians(c+sign(da)*ha);
 return length(d-vec2(sin(e),-cos(e))*R)-hw;
}
fn fill(input:FillInput)->vec4<f32> {
 let ink=input.params[0];
 let q=input.params[1];
 let hw=q.z*.5;
 let d=input.p-input.size*.5;
 let R=min(input.size.x,input.size.y)*.5-hw;
 let groove=clamp(.5-(abs(length(d)-R)-hw)*input.scale,0.,1.)*q.w;
 var c=0.; var ha=0.;
 if q.y>.5 {
  let t=input.time;
  let len=30.+240.*(.5-.5*cos(t*2.4));
  c=t*300.+len*.5;
  ha=len*.5;
 } else {
  let v=clamp(q.x,0.,1.);
  ha=v*180.; c=ha;
 }
 var arc=0.;
 if ha>.01 { arc=clamp(.5-zp_arc(d,R,c,ha,hw)*input.scale,0.,1.); }
 return vec4(ink.rgb,ink.a*max(arc,groove));
}
]]
local STILL = ui.shader(ARC)
local SPIN = ui.shader(ARC, { fps = 60 })

export type RingProps = { value: Value, tone: ToneValue?, size: number?, label: Label? }

M.ProgressRing = ui.component(function(props: RingProps): Node
	local size = props.size or 16
	local get = reader(props.value)
	local spin = props.value == nil
	-- The stroke: an eighth of the ring, never under a point and a half.
	local stroke = math.max(1.5, size / 8)
	-- How strong the groove shows: the ink at this.
	local GROOVE = 0.18
	local ring = ui.ref()
	-- The arc eases to each new value (its fraction is shaderParam4:
	-- params[1].x).
	if not spin then
		ui.effect(function()
			local v = get() or 0
			local el = ring:get()
			if el then
				anime.animate(el, { shaderParam4 = v, duration = 300, ease = "out(3)" })
			end
		end)
	end
	return ui.div {
		w = size,
		h = size,
		shrink = 0,
		role = "status",
		label = function()
			local v = get()
			return said(props.label, v)
		end,
		bg_shader = if spin then SPIN else STILL,
		shader_params = { ink(props.tone), { get() or 0, if spin then 1 else 0, stroke, GROOVE } },
		ref = ring,
	}
end)

return M
