-- How much of an agent's plan its live account has used, as the composer's
-- model chip shows it (filled that far) and its card says it (Zeron's
-- crates/ui/src/account_usage.rs, and the Android app's wave,
-- apps/android ui/AccountUsage.kt): each window the provider reports (the
-- five hours, the week; Cursor's pools), the chip reading the most used.
--
--   plan_usage.fraction(p)  0 to 1: the most used window, or nil (none)
--   plan_usage.percent(f)   "67%"
--   Wave { fraction = fn }  Android's wave: a violet wavefront filling its
--                           box from the left as far as `fraction` (amber
--                           from 75%, red from 90%), eased there as it
--                           changes (600 ms)
--   Card { plan = fn }      the card (ds/hover_card's): the agent's usage
--                           and how much at its head, who on what plan,
--                           then each window: its name, a slim bar, how
--                           much is used and when it starts over

local locale = require("@self/locale")
local HoverCard = require("@self/hover_card")
local progress = require("@self/progress")
local ink = require("@self/visual_ink")
local motion = require("@self/motion")

-- A window: its name as the provider gives it ("5h", "Weekly"), how much
-- of it is used (0 to 1), and when it starts over (seconds since the
-- epoch), if said.
export type Window = { label: string, used: number, resets: number? }
-- An agent's live account: the agent's name, who, on what plan, and its
-- windows.
export type Plan = { name: string?, email: string?, plan: string?, windows: { Window } }

local M = {}

function M.fraction(p: Plan?): number?
	local most: number? = nil
	for _, w in (if p then p.windows else {}) :: { Window } do
		most = math.max(most or 0, math.clamp(w.used, 0, 1))
	end
	return most
end

-- The Android card's thresholds: amber from three quarters, red from nine
-- tenths.
local function tone(f: number): string
	return if f >= 0.9 then "danger" elseif f >= 0.75 then "warning" else "accent"
end

local function percent(f: number): string
	return locale.s("ds.composer.quota.percent", { n = math.floor(math.clamp(f, 0, 1) * 100 + 0.5) })
end
M.percent = percent

-- The wave's colour at `f`, at `alpha`: Android's violet (lighter on a
-- dark theme, deeper on a light one, read off the text's own ink so a
-- template's theme decides too) while there's room, then the theme's amber
-- from three quarters and its red from nine tenths, the card's thresholds.
local VIOLET_DARK, VIOLET_LIGHT = "#b2a5ff", "#6e50d7"
local function channels(hex: string): (number, number, number)
	return tonumber(hex:sub(2, 3), 16) or 255, tonumber(hex:sub(4, 5), 16) or 255, tonumber(hex:sub(6, 7), 16) or 255
end
local function wave_color(f: number, alpha: number): string
	local base
	local t = tone(f)
	if t == "accent" then
		local r, g, b = channels(ink.hex("text.primary"))
		base = if (r + g + b) / 3 > 128 then VIOLET_DARK else VIOLET_LIGHT
	else
		base = ink.hex(`zeron.tone.{t}.ink`)
	end
	local r, g, b = channels(base)
	return string.format("#%02x%02x%02x%02x", r, g, b, math.floor(alpha * 255 + 0.5))
end

-- The wavefront at `front` (0 to 1 of `w`), its edge bent by `bend`: flat
-- along the top to just past the front, then down in two curves, the
-- leading edge rising a little (Android's `wave`, the same control points).
local function wave(w: number, h: number, front: number, bend: number): any
	local x = w * front
	return ui.path()
		:move(0, 0)
		:line(x + bend, 0)
		:cubic(x + bend * 1.5, h * 0.22, x - bend * 1.8, h * 0.43, x, h * 0.58)
		:cubic(x + bend * 1.3, h * 0.76, x - bend, h * 0.91, x - bend, h)
		:line(0, h)
		:close()
end

-- The wave: Android's `accountUsageFill`, drawn behind what holds it. The
-- bend is greatest mid-way and calms near empty and full; a fainter crest
-- runs a step ahead; the fill deepens toward its front (13% to 30%).
function M.Wave(props: { fraction: () -> number?, [string]: any }): Node
	local target = props.fraction
	local shown = ui.use_state(math.clamp(target() or 0, 0, 1))
	-- Eased to a new reading over 600 ms (Android's tween), ticking only
	-- while it moves.
	ui.effect(function()
		local to = math.clamp(target() or 0, 0, 1)
		local from = shown:peek()
		if math.abs(to - from) < 0.001 then
			return
		end
		local ms = motion.ms(600)
		if ms <= 0 then
			shown:set(to)
			return
		end
		local start = os.clock()
		ui.every(16, function(): boolean
			local t = math.min((os.clock() - start) * 1000 / ms, 1)
			shown:set(from + (to - from) * (1 - (1 - t) ^ 3))
			return t < 1
		end)
	end)
	local canvas = table.clone(props)
	canvas.fraction = nil
	canvas.draw = function(c, w, h)
		local p = shown:get()
		if p <= 0 then
			return
		end
		local bend = h * 0.13 * math.clamp(math.min(p, 1 - p) * 10, 0, 1)
		-- (The colour reads the reading itself, not the eased fill.)
		local f = math.clamp(target() or 0, 0, 1)
		c:fill(wave(w, h, math.min(p + 0.025, 1), bend * 0.7), wave_color(f, 0.10))
		c:fill(wave(w, h, p, bend), ui.linear(0, 0, w, 0, { wave_color(f, 0.13), wave_color(f, 0.30) }))
	end
	return ui.canvas(canvas)
end

-- When a window starts over, as Settings says it: a clock time within 22
-- hours, a weekday within a week, else the day.
function M.resets(at: number?, now: number?): string?
	if not at then
		return nil
	end
	local left = at - (now or os.time())
	local when = if left < 22 * 3600
		then locale.clock(at)
		elseif left < 7 * 86400 then locale.weekday(at)
		else locale.month_day(at)
	return locale.s("ds.composer.quota.resets", { when = when })
end

-- One window: its name, the bar, how much is used, when it starts over,
-- on one line, the columns held so the windows line up.
local COLUMNS = { label = 56, bar = 88, percent = 34 }
local function row(w: State<Window>): Node
	local function used(): number
		return math.clamp(w:get().used, 0, 1)
	end
	return ui.row {
		class = "composer-plan-window",
		gap = "$space.2",
		items = "center",
		h = "$zeron.menu.heading",
		ui.text {
			function(): string
				return w:get().label
			end,
			w = COLUMNS.label,
			shrink = 0,
			truncate = true,
			text_size = "$font.size.xs",
			line_height = "$font.line.xs",
			color = "$text.secondary",
		},
		ui.div {
			w = COLUMNS.bar,
			shrink = 0,
			progress.ProgressBar {
				value = used,
				size = "thin",
				w_full = true,
				tone = function(): any
					return tone(used())
				end,
				label = function(): string
					return w:get().label
				end,
			},
		},
		ui.text {
			function(): string
				return percent(used())
			end,
			w = COLUMNS.percent,
			shrink = 0,
			align = "right",
			tabular = true,
			text_size = "$font.size.xs",
			line_height = "$font.line.xs",
			color = function(): string
				local t = tone(used())
				return if t == "accent" then "$text.secondary" else `$zeron.tone.{t}.ink`
			end,
		},
		ui.text {
			function(): string
				return M.resets(w:get().resets) or ""
			end,
			grow = 1,
			align = "right",
			tabular = true,
			text_size = "$font.size.xs",
			line_height = "$font.line.xs",
			color = "$text.tertiary",
		},
	}
end

-- The card: a hover card (ds/hover_card: the menus' glass, framed once),
-- the agent's usage and the most used window's percentage at its head, who
-- on what plan under it, then each window.
-- The card's props: its frame's measures and what it holds (for a hover
-- card, or a popover's panel); `id` names it.
local function card(plan: () -> Plan?, id: string): { [any]: any }
	local function fraction(): number
		return M.fraction(plan()) or 0
	end
	local function who(): string?
		local p = plan()
		if not p then
			return nil
		end
		local parts = {}
		if p.email and p.email ~= "" then
			table.insert(parts, p.email)
		end
		if p.plan and p.plan ~= "" then
			table.insert(parts, p.plan)
		end
		return if #parts > 0 then table.concat(parts, " · ") else nil
	end
	local windows = ui.computed(function(): { Window }
		local p = plan()
		return if p then p.windows else {}
	end)
	return {
		id = id,
		w = 328,
		p = "$space.1",
		gap = 0,
		ui.row {
			h = "$zeron.menu.heading",
			px = "$space.2",
			gap = "$space.2",
			items = "center",
			ui.text {
				function(): string
					local p = plan()
					return if p and p.name
						then locale.s("ds.composer.quota.heading_of", { name = p.name })
						else locale.s("ds.composer.quota.heading")
				end,
				grow = 1,
				text_size = "$font.size.xs",
				weight = "$font.weight.medium",
				color = "$text.tertiary",
			},
			ui.text {
				function(): string
					return percent(fraction())
				end,
				id = "composer-plan-percent",
				tabular = true,
				text_size = "$font.size.xs",
				weight = "$font.weight.medium",
				color = function(): string
					local t = tone(fraction())
					return if t == "accent" then "$text.secondary" else `$zeron.tone.{t}.ink`
				end,
			},
		},
		ui.column {
			px = "$space.2",
			pb = "$space.1",
			ui.show(function(): boolean
				return who() ~= nil
			end, ui.text {
				function(): string
					return who() or ""
				end,
				id = "composer-plan-who",
				truncate = true,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				weight = "$font.weight.medium",
				color = "$text.primary",
				pb = "$space.1",
			}),
			ui.each(windows, function(_w: Window, _i: number, w: State<Window>): Node
				return row(w)
			end, {
				key = function(w: Window): string
					return w.label
				end,
			}),
		},
	}
end

function M.Card(props: { plan: () -> Plan? }): Node
	return HoverCard.Card(card(props.plan, "composer-plan-card"))
end

return M
