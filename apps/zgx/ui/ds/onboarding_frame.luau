-- Setup's window and the steps in it: what every onboarding screen stands
-- in, the app's and the gallery's alike.
--
--   Frame { Step { … } }      the window's content: its background, frosted
--                             or opaque, and the title bar's strip, which
--                             moves the window
--   Step { step = 2, count = 3, back = Back {}, continue = { label = "Continue", … }, Form { … } }
--   Forward { label, on_click }   the way on: the one filled pill
--   Quiet { label, on_click }     every other footer action: a ghost pill
--   Back { on_click }             a ghost pill with an arrow before its words
--   Form { title, body, … }       a step's words and what it asks, one column
--
-- Setup's window is 680 by 500 (src/main.rs `ONBOARDING_SIZE`) with an inset
-- title bar. The gallery's examples stage a Step in a drawing of that window
-- (demos/example_kit.luau); the app puts it in the real one, in a Frame.

local Button = require("@self/button")
local control = require("@self/control")
local Icon = require("@self/icon")
local Steps = require("@self/steps")
local Text = require("@self/text")
local FadeText = require("@self/fade_text")
local material = require("@self/material")
local look = require("@self/look")
local locale = require("@self/locale")

local M = {}

-- The window's own geometry, not spacing: the inset title bar's strip, and
-- setup's footer (72 tall, its buttons on the window's inset).
M.TITLEBAR = 40
local FOOTER = 72
-- The header slot's own geometry, the real frame's (onboarding.rs
-- render_page): 20 tall, 28 above the content.
local HEADER, HEADER_GAP = 20, 28
-- The room between the header slot's and a step's title: where every
-- step's title starts, scrolling or not.
local LEAD = 12
-- A step's title's top, from the window's: the title bar's strip, the
-- header slot and its room, and the lead (a step drawn by hand, Your
-- card's, stands its title here too).
M.TITLE_TOP = M.TITLEBAR + HEADER + HEADER_GAP + LEAD

-- The window's content. Frosted, the window's glass shows through the
-- theme's `background`; opaque, it is the window's own colour
-- (`zeron.window`). `opaque` holds it opaque whatever the setting (a stand-in
-- for the window drawn somewhere with no glass under it: the intro's), and
-- it eases to frosted once that lets go. Everything in it wears the app's
-- accent (ds.look). The title bar's strip lies over
-- everything, last, so whatever runs under it (a wall of cards) never takes
-- the drag from it. `steps` draws the dots once, in the header slot's
-- place, over whatever screen stands there: they stay put as the screens
-- change under them, and only their own spring moves (a screen's own dots
-- would come and go with it). `current` is nil on a step without dots.
export type FrameSteps = { count: () -> number, current: () -> number? }
export type FrameProps = { opaque: (() -> boolean)?, steps: FrameSteps?, [number]: Child }

-- The frame's dots: in the header slot's place, following their count in
-- place (a dot coming or going fades, the rest stay), fading out on a step
-- without them.
local function frame_steps(s: FrameSteps): Node
	local shown = ui.computed(function(): number
		return if s.current() then 1 else 0
	end)
	return ui.row {
		absolute = true,
		left = 0,
		right = 0,
		top = M.TITLEBAR,
		h = HEADER,
		items = "center",
		justify = "center",
		pointer_events = "none",
		opacity = ui.tween(shown, { from = 0, to = 1, duration = 200 }),
		Steps {
			count = s.count,
			current = function()
				return s.current() or 0
			end,
		},
	}
end

function M.Frame(props: FrameProps): Node
	local kids: { any } = {}
	for _, c in ipairs(props :: any) do
		table.insert(kids, c)
	end
	local opaque = props.opaque
	local frosted = ui.computed(function(): number
		local held = if opaque then opaque() else false
		return if material.see_through() and not held then 1 else 0
	end)
	return ui.column {
		id = "onboarding",
		w_full = true,
		h_full = true,
		relative = true,
		theme = function()
			return look.theme()
		end,
		bg = ui.tween(frosted, { from = "$zeron.window", to = "$background", duration = 320, ease = "outQuad" }),
		table.unpack(kids),
		ui.div {
			absolute = true,
			left = 0,
			right = 0,
			top = 0,
			h = M.TITLEBAR,
			window_drag = true,
		},
		if props.steps then frame_steps(props.steps) else nil,
	}
end

-- A step of setup: the title bar's strip; under it the header slot, where
-- the steps (the progress dots) stand, the same place on every step whatever
-- the page holds, centred over the content as every step's title is; the
-- screen; and the footer, as the user set it: the way on at the bottom right
-- as the filled pill with the arrow (`continue`), a Skip (`aside`)
-- immediately left of it, and at the bottom left a way back (`back`) or any
-- other quiet button that belongs there (`lead`: the first screen's "I
-- already have an account"). Both ends sit the same way: every button's box
-- on the window's inset, so box matches box and ink matches ink (a left
-- label and the right arrow both a pill's padding in from their edges). A
-- page whose text must line up with a footer label moves its text to the
-- label; the button doesn't move. Without `step`, a quiet `note` stands at
-- the bottom left (who is signed in). The screen stands at the top of the
-- room between the slot and the footer, its title on the line every step's
-- is on (`center` floats it in the middle instead, for a scene of its own);
-- `top` also lets it scroll (a list longer than the room).
-- (Words here may be messages, `ui.t`, drawn as they are; `choices` stay
-- strings: the pill is measured by them.)
export type Forward = {
	label: string | Message | () -> string,
	on_click: (() -> ())?,
	disabled: (boolean | () -> boolean)?,
	-- A way on whose words change with a choice ("Continue in Halcyon
	-- Labs"): every one it can say, and which it says now. The pill is as
	-- wide as the widest (no jump as the choice changes), at most `max_w`,
	-- where a longer one fades. The choices may come later (a function).
	choices: ({ string } | () -> { string })?,
	choice: (() -> number)?,
	max_w: number?,
}

export type StepProps = {
	step: number?,
	count: number?,
	back: Node?,
	lead: Node?,
	aside: Node?,
	continue: Forward?,
	note: Node?,
	-- The screen centred in its room, top to bottom (Sign in, a scene of
	-- its own), rather than its title at every step's place.
	center: boolean?,
	-- In the footer's middle, centred on the window whatever stands at its
	-- ends (Your card's ways to share it).
	middle: Node?,
	top: boolean?,
	[number]: Child,
}

-- A step's footer: the way back (or a quiet button) at the left, Skip and
-- the way on at the right.
local function footer(props: StepProps): Node
	local left = ui.row {
		gap = "$space.4",
		items = "center",
		props.back,
		props.lead,
		props.note,
	}
	local right = ui.row {
		gap = "$space.2",
		items = "center",
		props.aside,
		if props.continue then M.Forward(props.continue) else nil,
	}
	return ui.row {
		h = FOOTER,
		shrink = 0,
		px = "$space.6",
		items = "center",
		justify = "between",
		relative = true,
		left,
		if props.middle
			then ui.row {
				absolute = true,
				left = 0,
				right = 0,
				top = 0,
				bottom = 0,
				items = "center",
				justify = "center",
				pointer_events = "none",
				ui.div { pointer_events = "auto", props.middle },
			}
			else nil,
		right,
	}
end

function M.Step(props: StepProps): Node
	local kids: { any } = {}
	for _, c in ipairs(props :: any) do
		table.insert(kids, c)
	end
	local step = props.step
	-- The header slot, kept whether or not there are steps, so the screen
	-- under it stands in the same room on every step.
	local header = ui.row {
		id = "steps",
		w_full = true,
		h = HEADER,
		shrink = 0,
		mb = ui.raw(HEADER_GAP, "the real frame's room under its header slot"),
		items = "center",
		justify = "center",
		if step
			then Steps {
				count = props.count or 3,
				current = function()
					return step
				end,
			}
			else nil,
	}
	local footer = footer(props)
	local screen: Node
	if props.top then
		screen = ui.scroll {
			w_full = true,
			grow = 1,
			min_h = 0,
			fade = { 16, 16 },
			fade_overflow = true,
			ui.column {
				w_full = true,
				items = "center",
				px = "$space.6",
				pb = "$space.4",
				table.unpack(kids),
			},
		}
	else
		-- Its title where every step's stands (a scrolling one's too), what
		-- it asks under it: never floated to the middle, so moving on keeps
		-- the eye on one line.
		screen = ui.column {
			w_full = true,
			grow = 1,
			min_h = 0,
			items = "center",
			justify = if props.center then "center" else "start",
			pt = if props.center then nil else ui.raw(LEAD, "where every step's title starts"),
			px = "$space.6",
			table.unpack(kids),
		}
	end
	-- (Relative: what a step lays over itself, Your card's stage, stands
	-- on the step, not the frame, so it goes with the step as it leaves.)
	return ui.column {
		w_full = true,
		h_full = true,
		relative = true,
		ui.div { h = M.TITLEBAR, w_full = true, shrink = 0 },
		header,
		screen,
		footer,
	}
end

-- A step whose content can outgrow the window (Your agents, Choose an
-- organization): its words and its content scroll together, between the
-- top that holds still (the title bar's strip, the dots, and the room under
-- them every step leaves) and the footer, so the content has the window's
-- whole height. There's no bar: the top stands on the window's own surface,
-- and content going under it (or under the footer) melts away through a
-- soft band at the scroll's edge, as the gallery's and the sidebar's lists
-- do, never cut on a line, and never under the small title.
--
-- The big title isn't cut either: while the content runs past the window,
-- it is drawn over the window rather than in the scroll (it stays in the
-- scroll, undrawn, for its room and for whoever reads the page), and goes
-- up with the scroll, point for point, fading and drifting up a little as
-- it goes, into the room under the dots, where a small one (the body's
-- size, medium) comes in over its last stretch, as a large title gives way
-- to a navigation bar's. All of it follows the scroll itself (scrubbed by
-- its offset in the engine, `anime.onScroll`, no script a frame), so it
-- tracks it and turns back with it. Content that fits doesn't scroll: it
-- stands centred, as any step's does, its title its own.
--
--   ScrollStep { step = 4, count = 6, back = …, continue = { … }, form = { title = "Your agents", body = "…", measure = 560 }, …content }
-- `place`: the screen's own route ("providers"), so a reload of the scripts
-- puts its scroll titles back as they were (keyed by it and the step),
-- rather than drawing them done for a beat; without it, each mount is a
-- fresh entry.
export type ScrollStepProps = Merge<StepProps, { form: FormProps, place: string? }>

-- What a step's titles were when it went (`place` and step: its measures,
-- whether they had settled, when it went and in which load of this module).
-- A reload makes the step again within moments, in a new load (module
-- locals fresh, keyed state kept); a step left and come back to, in the
-- same load or long after, is a fresh entry and plays as one.
local LOAD = tostring({})
local kept_titles = ui.state("onboarding_frame_kept_titles", {} :: { [string]: any })
local RELOAD_WITHIN = 2000

-- The room above the content, and under it; the bands content melts
-- through at the top and the foot (taller than a line of text: they ramp on
-- the square, so a line dissolves rather than being cut).
local FOOT = 12
local TOP_FADE, FOOT_FADE = 32, 40
-- The scroll, in points from its top, over which the big title goes (up
-- from where it rests and through the small title's room, so the hand-off
-- spans several wheel ticks, not one), and how much further up it drifts
-- meanwhile. The small one comes as the big one's last trace goes, the two
-- crossing faint, and settles a little further on.
local TITLE_OUT, DRIFT = 32, 6
local SMALL_FROM, SMALL_IN = 28, 16

local ScrollStep = ui.component(function(props: ScrollStepProps): Node
	local kids: { any } = {}
	for _, c in ipairs(props :: any) do
		table.insert(kids, c)
	end
	local step = props.step
	-- Made again by a reload (see `kept_titles`): as it was, at once.
	local key = if props.place then `{props.place}:{step or 0}` else nil
	local was = if key then kept_titles:peek()[key] else nil
	local reloaded = was ~= nil and was.load ~= LOAD and ui.now() - was.left < RELOAD_WITHIN
	-- The scroll's height, for its content to be at least as tall (a scroll's
	-- content is as long as its children make it, so a percentage has nothing
	-- to be of), and the content's: kept by their own resizes, not a frame's
	-- work. More content than room scrolls, and its title is drawn over the
	-- window (`lifted`).
	local room = ui.use_state(if reloaded then was.room else 0)
	local tall = ui.use_state(if reloaded then was.tall else 0)
	local lifted = ui.computed(function(): boolean
		return tall:get() > room:get() + 0.5
	end)
	-- The titles' scroll-linked runs drawn where the scroll stands (see the
	-- seek below). Until then the scroll's own title stands, and the small
	-- one and the lifted one aren't drawn: armed, the engine first draws
	-- them done, which flashed the small title on the way in.
	local settled = ui.use_state(false)
	if key then
		ui.on_cleanup(function()
			local all = table.clone(kept_titles:peek())
			all[key :: string] = { room = room:peek(), tall = tall:peek(), settled = settled:peek(), load = LOAD, left = ui.now() }
			kept_titles:set(all)
		end)
	end
	local function lifted_now(): boolean
		return lifted:get() and settled:get()
	end
	-- What the scroll links: the scroll, the big title over the window, the
	-- small one, and the two runs of the scroll they follow (spans laid at
	-- the content's top, the length of each). Armed once all have mounted.
	local els: { [string]: Element? } = {}
	local armed = false
	-- Made again by a reload: the titles' runs, put at the scroll's top as
	-- soon as its first layout is done (`at_top`), and shown then.
	local runs: { any }? = nil
	-- (Once, on a layout after the runs are made: the scroll's first
	-- resize can come before them, so the small title's layout asks too.)
	local placed = false
	local function at_top()
		if not (reloaded and runs) or placed then
			return
		end
		placed = true
		for _, a in runs :: { any } do
			a:seek(0.01)
		end
		settled:set(was.settled == true)
	end
	local function mounted(key: string): (el: Element) -> ()
		return function(el: Element)
			els[key] = el
			local big_el, small_el, out_el, in_el = els.big, els.small, els.out, els["in"]
			if armed or not (els.scroll and big_el and small_el and out_el and in_el) then
				return
			end
			armed = true
			local function over(run: Element): ScrollObserver
				return anime.onScroll { target = run, enter = "start start", leave = "start end", sync = true }
			end
			-- Up with the content, point for point (and a little more), its
			-- fade gentle over the whole way.
			local big = anime.animate(big_el, {
				y = { from = 0, to = -(TITLE_OUT + DRIFT), ease = "linear" },
				opacity = { from = 1, to = 0, ease = "inOutSine" },
				scale = { from = 1, to = 0.98, ease = "linear" },
				duration = 1000,
				autoplay = over(out_el),
			})
			local small = anime.animate(small_el, {
				opacity = { 0, 1 },
				y = { 4, 0 },
				duration = 1000,
				ease = "inOutSine",
				autoplay = over(in_el),
			})
			runs = { big, small }
			-- A step's scroll opens at its top. (Armed before its layout
			-- settles, the engine takes them as scrolled past and draws them
			-- done until the scroll first moves; and a seek to exactly
			-- nothing doesn't draw, so a hair past it.)
			ui.after(60, function()
				for _, a in { big, small } do
					a:seek(0.01)
				end
				ui.after(0, function()
					settled:set(true)
				end)
			end)
		end
	end
	local form: { [any]: any } = table.clone(props.form :: any)
	form.title_hidden = lifted_now
	for _, k in kids do
		table.insert(form, k)
	end
	local measure = props.form.measure
		or ui.theme.get(if props.form.wide then "zeron.form.list" else "zeron.form.measure") :: number?
	-- The top: the dots, and the room under them, where the small title
	-- comes.
	local top = ui.column {
		w_full = true,
		shrink = 0,
		ui.row {
			id = "steps",
			w_full = true,
			h = HEADER,
			shrink = 0,
			items = "center",
			justify = "center",
			if step
				then Steps {
					count = props.count or 3,
					current = function()
						return step
					end,
				}
				else nil,
		},
		ui.row {
			w_full = true,
			h = ui.raw(HEADER_GAP, "the real frame's room under its header slot"),
			shrink = 0,
			px = "$space.6",
			items = "center",
			justify = "center",
			ui.div {
				id = "scroll_title",
				on_mount = mounted("small"),
				-- (Made again by a reload: its run put at the top once laid out.)
				on_resize = function()
					at_top()
				end,
				opacity = 0,
				invisible = function(): boolean
					return not settled:get()
				end,
				aria_hidden = true,
				min_w = 0,
				ui.text {
					props.form.title,
					text_size = "$font.size.md",
					line_height = "$font.line.md",
					weight = "$font.weight.medium",
					color = "$text.primary",
				},
			},
		},
	}
	local screen = ui.scroll {
		id = "scroll_step",
		on_mount = mounted("scroll"),
		on_resize = function(size: { w: number, h: number })
			room:set(size.h)
			at_top()
		end,
		w_full = true,
		grow = 1,
		min_h = 0,
		fade = { TOP_FADE, FOOT_FADE },
		fade_overflow = true,
		-- At least the room's height, its content at its top: content that
		-- fits stands as any step's, and only more than fits scrolls.
		ui.column {
			w_full = true,
			min_h = function(): number
				return room:get()
			end,
			relative = true,
			items = "center",
			justify = "start",
			px = "$space.6",
			-- (Its own height, apart from the room it fills: one stack of
			-- what it holds, measured.)
			ui.column {
				w_full = true,
				items = "center",
				on_resize = function(size: { w: number, h: number })
					tall:set(size.h)
				end,
				-- The runs the titles follow, laid from the content's top.
				ui.div { on_mount = mounted("out"), absolute = true, top = 0, left = 0, w = 1, h = TITLE_OUT, aria_hidden = true },
				ui.div { on_mount = mounted("in"), absolute = true, top = SMALL_FROM, left = 0, w = 1, h = SMALL_IN, aria_hidden = true },
				ui.div { w = 1, h = LEAD, shrink = 0 },
				M.Form(form :: any),
				ui.div { w = 1, h = FOOT, shrink = 0 },
			},
		},
	}
	-- The big title over the window, where the scroll's own rests at its
	-- top: under the dots' room and `LEAD`, as wide as the form.
	local big = ui.row {
		absolute = true,
		top = ui.raw(HEADER + HEADER_GAP + LEAD, "the dots, their room and the lead: where the title rests"),
		left = 0,
		right = 0,
		px = "$space.6",
		justify = "center",
		pointer_events = "none",
		aria_hidden = true,
		invisible = function(): boolean
			return not lifted_now()
		end,
		ui.column {
			w_full = true,
			max_w = measure,
			items = "center",
			ui.div {
				on_mount = mounted("big"),
				Text { props.form.title, style = "title", align = "center", wrap = true },
			},
		},
	}
	return ui.column {
		w_full = true,
		h_full = true,
		ui.div { h = M.TITLEBAR, w_full = true, shrink = 0 },
		ui.column {
			w_full = true,
			grow = 1,
			min_h = 0,
			relative = true,
			top,
			screen,
			big,
		},
		footer(props),
	}
end)

function M.ScrollStep(props: ScrollStepProps): Node
	return ScrollStep(props)
end

-- Setup's footer buttons, one family of pills (Button's own pill shape), as
-- the user chose for onboarding: the way on (`Forward`) is the one filled
-- pill, the secondary tone with an arrow after its words; every other action
-- (Back, with an arrow before its words; Skip, Sign out, I already have an
-- account) is a ghost pill, no fill until touched. Scoped to setup: Button
-- itself keeps its corners. `label` may be a function.
function M.Forward(props: Forward): Node
	local given, choice = props.choices, props.choice
	if given and choice then
		local function choices(): { string }
			return if type(given) == "function" then given() else given
		end
		-- Every choice's words, one over another in one grid cell, the chosen
		-- one shown: the stack is as wide as the widest. Each choice's words
		-- are its place's, following its text as the choices change.
		local places = ui.computed(function(): { number }
			local out: { number } = {}
			for i = 1, #choices() do
				table.insert(out, i)
			end
			return out
		end)
		local stack = ui.each(places, function(i: number): Node
			return ui.div {
				col_start = 1,
				row_start = 1,
				min_w = 0,
				opacity = function()
					return if choice() == i then 1 else 0
				end,
				aria_hidden = true,
				FadeText {
					function(): string
						return choices()[i] or ""
					end,
					text_size = "$font.size.md",
					line_height = "$font.line.md",
					weight = "$font.weight.medium",
				},
			}
		end, {
			key = function(i: number): number
				return i
			end,
			grid = true,
			cols = 1,
			-- (Its one row on the words' line, as the overlay it replaced.)
			content = "center",
			min_w = 0,
			shrink = 1,
		})
		return Button {
			label = function()
				return choices()[choice()] or ""
			end,
			tone = "secondary",
			shape = "pill",
			px = "$space.4",
			max_w = props.max_w,
			disabled = props.disabled,
			on_click = props.on_click,
			stack,
			Icon { name = "arrow-right", size = 16 },
		}
	end
	return Button {
		props.label,
		tone = "secondary",
		shape = "pill",
		px = "$space.4",
		disabled = props.disabled,
		on_click = props.on_click,
		Icon { name = "arrow-right", size = 16 },
	}
end

-- The way in, on the intro's scene: the forward pill itself (its shape,
-- its words and arrow), solid white with dark words so it stands on a
-- night sky or a sunrise alike, a touch greyer as it is touched; no shadow
-- of its own. It
-- is the object that becomes sign-in's way on as the intro hands over, so
-- it is the same pill, only dressed for the scene. `size` "lg" stands it a
-- step larger on a whole display. A press gives a little.
export type WayIn = Forward & { id: string?, size: ("md" | "lg")? }

-- The focus ring, outside the hairline (which it keeps).
local FOCUS = table.clone(control.ring)
FOCUS.border = nil

local WayInPill = ui.component(function(props: WayIn): Node
	local size: control.Size = props.size or "md"
	local pressed = ui.use_state(0)
	return Button {
		props.label,
		id = props.id,
		tone = "secondary",
		size = size,
		shape = "pill",
		px = if size == "lg" then "$space.5" else "$space.4",
		bg = "$zeron.scene.pill.tint",
		color = "$zeron.scene.pill.ink",
		hover = { bg = "$zeron.scene.pill.hover" },
		active = { bg = "$zeron.scene.pill.active" },
		border = 1,
		border_color = "$zeron.scene.pill.edge",
		focus = FOCUS,
		scale = ui.spring(pressed, { from = 1, to = 0.98, stiffness = 520, damping = 30 }),
		disabled = props.disabled,
		on_click = props.on_click,
		on_press = function()
			pressed:set(1)
			ui.after(140, function()
				pressed:set(0)
			end)
		end,
		Icon { name = "arrow-right", size = control.ICON[size] },
	}
end)

function M.WayIn(props: WayIn): Node
	return WayInPill(props)
end

export type Quiet = {
	label: string | Message | () -> string,
	on_click: (() -> ())?,
	disabled: (boolean | () -> boolean)?,
}

function M.Quiet(props: Quiet): Node
	return Button {
		props.label,
		tone = "ghost",
		shape = "pill",
		px = "$space.4",
		disabled = props.disabled,
		on_click = props.on_click,
	}
end

function M.Back(props: { on_click: (() -> ())? }): Node
	return Button {
		Icon { name = "arrow-left", size = 16 },
		locale.t("common.back"),
		tone = "ghost",
		shape = "pill",
		px = "$space.4",
		on_click = props.on_click,
	}
end

-- A step that lists cards (Your agents, Choose an organization): its
-- measure, the real setup's column (onboarding.rs COLUMN_WIDTH).
M.LIST_MEASURE = 560

-- A line of setup's summary (You're all set): what it is at the left,
-- quiet; what was chosen at the right (a settings row's insets, one line).
function M.Fact(props: { label: string | Message, value: string | Message | () -> string }): Node
	return ui.row {
		w_full = true,
		min_h = "$zeron.settings.row",
		px = "$space.4",
		py = "$space.3",
		gap = "$space.4",
		items = "center",
		justify = "between",
		ui.text { props.label, shrink = 0, text_size = "$font.size.md", line_height = "$font.line.md", color = "$text.secondary" },
		FadeText { props.value, text_size = "$font.size.md", line_height = "$font.line.md", color = "$text.primary" },
	}
end

-- A setup screen's words and what it asks, one column: the title and the
-- line under it, centred, then its field and the way on, all at the form's
-- measure, so a field and the button under it are one shape. `wide` for a
-- step that lists instead. `body` may be a function (a line that says what
-- went wrong, in its own `tone`).
export type FormProps = {
	-- (A function for words that change: an organization's name.)
	title: string | Message | () -> string,
	body: (string | Message | () -> string)?,
	tone: (string | () -> string)?,
	wide: boolean?,
	-- A measure of its own, in points, past `wide` (a step whose rows carry
	-- more than a list's: an agent's accounts and their meters).
	measure: number?,
	-- The title keeps its room but isn't drawn (a scrolling step draws it
	-- over the window instead).
	title_hidden: (() -> boolean)?,
	[number]: Child,
}

function M.Form(props: FormProps): Node
	local kids: { any } = {}
	for _, c in ipairs(props :: any) do
		table.insert(kids, c)
	end
	return ui.column {
		w_full = true,
		-- (A theme number, read as it is built: max_w takes points.)
		max_w = props.measure or ui.theme.get(if props.wide then "zeron.form.list" else "zeron.form.measure") :: number?,
		gap = "$space.6",
		ui.column {
			w_full = true,
			items = "center",
			gap = "$space.2",
			Text { props.title, style = "title", align = "center", wrap = true, heading = 1, invisible = props.title_hidden },
			if props.body
				then ui.text {
					props.body,
					align = "center",
					wrap = true,
					text_size = "$font.size.md",
					line_height = "$font.line.md",
					color = props.tone or "$text.secondary",
				}
				else nil,
		},
		ui.column { w_full = true, gap = "$space.4", table.unpack(kids) },
	}
end

return M
