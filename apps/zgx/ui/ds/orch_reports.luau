--!strict
-- Crew updates: what an orchestrator's crew reported, as its transcript
-- shows the engine's `crewReports` message (never a bubble of the
-- person's). A quiet head, then a row a report: its thread, a chip for
-- how it stands, its one line, and when. Past three, the rest fold under
-- "Show N more". A row opens its thread.
--
--   OrchReports {
--     reports = function() return { Report } end,  -- in the order filed
--     title_of = function(chat_id) return title end, -- the thread's name
--     on_open = function(chat_id) end,
--     hue = function() return 196 end,             -- its orchestrator's, if known
--   }

local Icon = require("@self/icon")
local Badge = require("@self/badge")
local status = require("@self/status")
local control = require("@self/control")
local locale = require("@self/locale")
local hue = require("@self/orch_hue")

export type Report = {
	id: string,
	from_chat: string,
	item_id: string?,
	-- "progress" | "done" | "blocked" | "failed" | "message" | "other"
	state: string,
	summary: string,
	detail: string?,
	progress: number?,
	-- Unix seconds.
	at: number?,
}
export type Props = {
	reports: control.Live<{ Report }>,
	title_of: ((chat_id: string) -> string?)?,
	on_open: ((chat_id: string) -> ())?,
	hue: control.Live<number?>?,
}

-- How many rows show folded.
local FIRST = 3
-- A row's height, and its corner (a list row's, standing on the surface).
local ROW_H = 30
local ROW_RADIUS = "$radius.md"

-- Each state's chip: its words and its tone.
local CHIP: { [string]: { key: string, tone: Badge.Tone } } = {
	done = { key = "orch.reports.state.done", tone = "success" },
	blocked = { key = "orch.reports.state.blocked", tone = "warning" },
	failed = { key = "orch.reports.state.failed", tone = "danger" },
	progress = { key = "orch.reports.state.progress", tone = "info" },
	message = { key = "orch.reports.state.message", tone = "neutral" },
}
local function chip_of(state: string?): { key: string, tone: Badge.Tone }
	return CHIP[state or ""] or { key = "orch.reports.state.progress", tone = "neutral" }
end

return ui.component(function(props: Props): Node
	local open = ui.use_state(false)
	local function all(): { Report }
		return (control.value(props.reports) :: { Report }?) or {}
	end
	local shown = ui.computed(function(): { Report }
		local list = all()
		if open:get() or #list <= FIRST then
			return list
		end
		return { list[1], list[2], list[3] }
	end)
	local function title(chat: string): string
		local t = if props.title_of then props.title_of(chat) else nil
		return if t and t ~= "" then t else locale.s("orch.reports.thread")
	end
	-- The orchestrator's hue down the card's side, a thread back to it;
	-- the quiet rule's ink while it isn't known.
	local function stripe(): string
		local h = if props.hue then control.value(props.hue) :: number? else nil
		return if h then hue.paint(h) else "$border"
	end

	local function row(_r: Report, _i: number, now: State<Report>): Node
		local hovered = ui.use_state(false)
		local function chip(): { key: string, tone: Badge.Tone }
			return chip_of(now:get().state)
		end
		return ui.button {
			class = "orch-report",
			label = function(): string
				local r = now:get()
				return locale.s("orch.reports.open", { title = title(r.from_chat), summary = r.summary })
			end,
			w_full = true,
			h = ROW_H,
			px = "$space.2",
			gap = "$space.2",
			items = "center",
			rounded = ROW_RADIUS,
			bg = function(): string
				return if hovered:get() then "$hover" else "#00000000"
			end,
			hover = {},
			active = { bg = "$pressed" },
			on_hover = function(inside: boolean)
				hovered:set(inside)
			end,
			on_click = function()
				if props.on_open then
					props.on_open(now:peek().from_chat)
				end
			end,
			ui.text {
				function(): string
					return title(now:get().from_chat)
				end,
				shrink = 1,
				min_w = 0,
				max_w = 200,
				truncate = true,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.primary",
			},
			-- (Its tone is fixed as a badge is made: a state that moves
			-- swaps its chip.)
			ui.presence(function(): string
				return now:get().state or ""
			end, function(_state: string): Node
				local c = chip()
				return Badge.Badge {
					function(): string
						return locale.s(chip().key)
					end,
					tone = c.tone,
					size = "sm",
					class = "orch-report-state",
				}
			end, { shrink = 0 }),
			ui.text {
				function(): string
					return now:get().summary or ""
				end,
				grow = 1,
				min_w = 0,
				truncate = true,
				text_size = "$font.size.sm",
				color = "$text.secondary",
			},
			ui.text {
				function(): string
					local at = now:get().at
					return if at then status.ago(status.since(at)) else ""
				end,
				shrink = 0,
				tabular = true,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
		}
	end

	local function more(): number
		return #all() - FIRST
	end

	return ui.row {
		class = "orch-reports",
		w_full = true,
		gap = "$space.3",
		items = "stretch",
		ui.div { w = 2, shrink = 0, rounded = "$radius.full", bg = stripe },
		ui.column {
			grow = 1,
			min_w = 0,
			gap = "$zeron.list.gap",
			ui.row {
				w_full = true,
				h = 24,
				px = "$space.2",
				gap = "$space.2",
				items = "center",
				color = "$text.tertiary",
				Icon { name = "users-group-rounded", size = 14 },
				ui.text {
					locale.t("orch.reports.title"),
					text_size = "$font.size.xs",
					weight = "$font.weight.medium",
					color = "$text.tertiary",
				},
				ui.text {
					function(): string
						return tostring(#all())
					end,
					tabular = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			},
			ui.column { w_full = true, gap = "$zeron.list.gap", ui.each(shown, row, { key = function(r: Report): string
				return r.id
			end, w_full = true }) },
			ui.show(function(): boolean
				return more() > 0
			end, ui.button {
				class = "orch-reports-more",
				label = function(): string
					return if open:get() then locale.s("orch.reports.show_less") else locale.s("orch.reports.show_more", { n = more() })
				end,
				expanded = function(): boolean
					return open:get()
				end,
				w_full = true,
				h = 28,
				px = "$space.2",
				gap = "$space.2",
				items = "center",
				rounded = ROW_RADIUS,
				bg = "#00000000",
				hover = { bg = "$hover" },
				on_click = function()
					open:set(not open:peek())
				end,
				ui.div {
					color = "$text.tertiary",
					rotate = ui.spring(open, { from = 0, to = 90, stiffness = 380, damping = 30 }),
					Icon { name = "alt-arrow-right", size = 14 },
				},
				ui.text {
					function(): string
						return if open:get() then locale.s("orch.reports.show_less") else locale.s("orch.reports.show_more", { n = more() })
					end,
					text_size = "$font.size.sm",
					color = "$text.secondary",
				},
			}),
		},
	}
end)
