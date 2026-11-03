-- People and time: an avatar stack (reviewers, who is here), a person's
-- line (their avatar and name), and a timestamp that says how long ago
-- and, under the pointer, exactly when.
--
--   AvatarStack { people = { { id = "u1", name = "Ada" }, … }, max = 3 }
--   Person { id = "u1", name = "Ada Lovelace" }
--   Timestamp { at = os.time() - 300 }            "5 minutes ago"
--   Timestamp { at = t, compact = true }           "5m"

local Avatar = require("@self/avatar")
local status = require("@self/status")
local FadeText = require("@self/fade_text")
local skeleton = require("@self/skeleton")
local locale = require("@self/locale")

local M = {}

-- `src`: their picture; or a function of one that arrives later (a
-- GitHub face fetched for them), their gradient showing until it does.
-- `unknown`: someone not known yet (you, before the service you're
-- writing to has said who you are there): their face and name as
-- skeletons, never a stand-in name or a placeholder face.
export type Who = { id: string, name: string, src: (string | () -> string?)?, unknown: boolean? }

-- A picture's cut (a gradient face cuts itself: ds.Avatar's `cut`): what
-- it holds, gone inside a circle (params[0]: its centre and radius, as
-- fractions of its width), edged over half a point either side.
local CUT = ui.layer_shader([[
fn layer(input:LayerInput)->vec4<f32> {
 let w=input.size.x;
 let d=distance(input.p,vec2(input.params[0].x*w,.5*input.size.y))-input.params[0].y*w;
 return vec4(input.color.rgb,input.color.a*smoothstep(-.5,.5,d));
}
]])

-- Someone's face: their picture, or their gradient (ds.Avatar). `cut`: a
-- circle taken out of it, `{ x, r }` in points (the face over it in a
-- stack, and the gap round that face).
local function face(who: Who, size: number, cut: { x: number, r: number }?): Node
	if who.unknown then
		return skeleton.Bar { w = size, h = size, rounded = "$radius.full" }
	end
	if type(who.src) == "function" then
		-- The gradient, and the picture over it once there is one.
		local later = who.src :: () -> string?
		local held: { [any]: any } = {
			w = size,
			h = size,
			shrink = 0,
			relative = true,
			rounded = "$radius.full",
			overflow = "hidden",
			label = who.name,
			Avatar { seed = who.id, size = size, label = who.name },
			ui.image {
				absolute = true,
				left = 0,
				top = 0,
				w = size,
				h = size,
				fit = "cover",
				-- (A face from the web that didn't come is asked for again by
				-- zgx while it shows.)
				src = function(): string
					return later() or ""
				end,
			},
		}
		if cut then
			held.layer_shader = CUT
			held.layer_params = { { cut.x / size, cut.r / size, 0, 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 } }
		end
		return ui.div(held)
	end
	if who.src then
		local picture: { [any]: any } = {
			w = size,
			h = size,
			shrink = 0,
			rounded = "$radius.full",
			overflow = "hidden",
			label = who.name,
			relative = true,
			ui.image { src = who.src :: string, w_full = true, h_full = true, fit = "cover" },
		}
		if cut then
			picture.layer_shader = CUT
			picture.layer_params = { { cut.x / size, cut.r / size, 0, 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 }, { 0, 0, 0, 0 } }
		end
		return ui.div(picture)
	end
	return Avatar { seed = who.id, size = size, label = who.name, cut = cut }
end
M.face = face

-- Someone's name as a line shows it: theirs, or, not known yet, a skeleton
-- of a name's length at the line's height (`size`, its text's).
function M.name(who: Who, text: string?, weight: string?): Node
	if who.unknown then
		return skeleton.Bar { w = 72, h = 10, rounded = "$radius.sm" }
	end
	return ui.text { who.name, literal = true, shrink = 0, text_size = text, weight = weight or "$font.weight.medium", color = "$text.primary" }
end

-- Avatars overlapping, each cut where the next one sits (and a gap round
-- it), so the one behind reads as behind: a real cut, whatever is behind
-- shows through it, glass or not (a ring in the window's colour went
-- opaque on glass). Past `max`, a "+N" disc. The names are its tooltip.
export type StackProps = { people: { Who }, size: number?, max: number?, label: string? }

function M.AvatarStack(props: StackProps): Node
	local size = props.size or 20
	local max = props.max or 4
	local people = props.people
	local shown = math.min(#people, max)
	-- When one more would only replace the "+1", show it instead.
	if #people == max + 1 then
		shown = #people
	end
	local overlap = math.floor(size * 0.3)
	-- The gap round each face, cut from the one behind it.
	local GAP = 1.5
	-- Where the next face sits on this one (its centre, along the middle),
	-- and how much of this one it takes.
	local next_cut = { x = size - overlap + size / 2, r = size / 2 + GAP }
	local names: { string } = {}
	for _, p in people do
		table.insert(names, p.name)
	end
	local rest = #people - shown
	local items: { Node } = {}
	for i = 1, shown do
		-- (Cut by the next face, or by the "+N" disc after the last.)
		local covered = i < shown or rest > 0
		table.insert(items, ui.div {
			shrink = 0,
			ml = if i == 1 then 0 else ui.raw(-overlap, "faces overlap by a third"),
			face(people[i], size, if covered then next_cut else nil),
		})
	end
	if rest > 0 then
		table.insert(items, ui.div {
			shrink = 0,
			ml = ui.raw(-overlap, "faces overlap by a third"),
			ui.row {
				w = size,
				h = size,
				rounded = "$radius.full",
				bg = "$zeron.secondary.hover",
				items = "center",
				justify = "center",
				ui.text {
					`+{rest}`,
					text_size = if size >= 24 then "$font.size.xs" else ui.raw(9, "a count inside a face under 24 points"),
					weight = "$font.weight.medium",
					tabular = true,
					color = "$text.secondary",
				},
			},
		})
	end
	return ui.row {
		shrink = 0,
		items = "center",
		label = props.label or table.concat(names, ", "),
		tooltip = table.concat(names, ", "),
		table.unpack(items),
	}
end

-- A person: their face and name, and something after (their role, a
-- time) in the tertiary ink.
export type PersonProps = Who & { size: number?, detail: (string | Node)? }

function M.Person(props: PersonProps): Node
	local size = props.size or 20
	local detail = props.detail
	return ui.row {
		gap = "$space.2",
		items = "center",
		min_w = 0,
		face(props, size),
		FadeText {
			props.name,
			text_size = if size >= 24 then "$font.size.md" else "$font.size.sm",
			weight = "$font.weight.medium",
			color = "$text.primary",
		},
		if type(detail) == "string"
			then ui.text { detail, text_size = "$font.size.sm", color = "$text.tertiary", shrink = 0 }
			else detail,
	}
end

-- How long ago, in words: "just now", then minutes, hours, days, weeks,
-- months, years ("3 hours ago", never "3 hrs"), in the locale's words
-- (ds/locale).
local long = locale.ago
M.long = long

-- Exactly when: "Oct 29, 2026 at 14:05".
local exact = locale.exact
M.exact = exact

-- A timestamp: how long ago (or compact, a corner's: "5m"), its exact
-- time a tooltip; it moves on by itself, a minute at a time.
export type TimestampProps = { at: number, compact: boolean?, size: ("xs" | "sm" | "md")?, tone: ("secondary" | "tertiary")? }

M.Timestamp = ui.component(function(props: TimestampProps): Node
	local now = ui.use_state(os.time())
	-- A minute's tick: the words move on while it shows.
	local timer = anime.createTimer {
		duration = 60000,
		loop = true,
		onLoop = function()
			now:set(os.time())
		end,
	}
	ui.on_cleanup(function()
		timer:cancel()
	end)
	local size = props.size or "sm"
	return ui.text {
		function()
			local ago = now:get() - props.at
			return if props.compact then status.ago(ago) else long(ago)
		end,
		shrink = 0,
		tabular = true,
		text_size = `$font.size.{size}`,
		color = if props.tone == "secondary" then "$text.secondary" else "$text.tertiary",
		tooltip = exact(props.at),
	}
end)

return M
