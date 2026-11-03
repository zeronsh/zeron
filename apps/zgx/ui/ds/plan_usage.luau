-- How much of an agent's plan its live account has used, as the composer's
-- plan ring shows it beside the context ring and its card says it (Zeron's
-- crates/ui/src/account_usage.rs): each window the provider reports (the
-- five hours, the week; Cursor's pools), the ring reading the most used, in
-- the accent until it nears its limit (amber from four fifths, red from 95
-- in a hundred, as Settings → Providers' meters read).
--
--   plan_usage.fraction(p)  0 to 1: the most used window, or nil (none)
--   plan_usage.ink(f)       the ring's ink at `f`
--   Card { plan = fn }      the card (ds/hover_card's): the agent's usage
--                           and how much at its head, who on what plan,
--                           then each window: its name, a slim bar, how
--                           much is used and when it starts over

local locale = require("@self/locale")
local HoverCard = require("@self/hover_card")
local progress = require("@self/progress")

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

-- Settings → Providers' thresholds (ds/provider_card's `level`).
local function tone(f: number): string
	return if f >= 0.95 then "danger" elseif f >= 0.8 then "warning" else "accent"
end

function M.ink(f: number): string
	return `$zeron.tone.{tone(f)}.ink`
end

local function percent(f: number): string
	return locale.s("ds.composer.quota.percent", { n = math.floor(math.clamp(f, 0, 1) * 100 + 0.5) })
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
function M.Card(props: { plan: () -> Plan? }): Node
	local plan = props.plan
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
	return HoverCard.Card {
		id = "composer-plan-card",
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

return M
