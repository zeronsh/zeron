-- A part of a dump, as its hover card says it: where it went (its hue,
-- name and kind of place), what it is and how it stands (Zeron's status
-- glyph), the message the Brain dump sent, the answer if one came, and the
-- way in. The receipt (ds/orch_receipt) shows it over a part's words and
-- its reply line.
--
--   PartCard {
--     route = function() return route end,       -- a dump's route, as it stands
--     target = function() return target end,     -- ds/orch_data.target of it
--     on_open = function(target) end,            -- { kind, id }
--   }

local Icon = require("@self/icon")
local status = require("@self/status")
local locale = require("@self/locale")
local hue = require("@self/orch_hue")
local data = require("@self/orch_data")
local HoverCard = require("@self/hover_card")
local Markdown = require("@self/markdown")

export type Props = {
	route: () -> any,
	target: () -> data.Target,
	on_open: (target: { kind: string, id: string? }) -> (),
}

-- Each kind of place's glyph.
local TARGET_ICON: { [string]: Icon.Name } = {
	orchestrator = "users-group-rounded",
	new_orchestrator = "users-group-rounded",
	chat = "chat-round-line",
	new_thread = "chat-round-line",
	item = "inbox",
	inbox = "inbox",
	note = "document",
	unsure = "status-input",
}

local INTENT_ICON: { [string]: Icon.Name } = {
	task = "checklist",
	info = "info-circle",
	question = "chat-round-line",
	priority = "flag",
	cancel = "close-circle",
	note = "document",
}

-- The card's width, and the most its reply shows before it scrolls.
local WIDTH = 480
local REPLY_MAX = 280

-- How a route's state reads as Zeron's status.
local STATUS: { [string]: string } = { pending = "waiting", delivered = "done", answered = "done", failed = "failed", unsure = "input", undone = "waiting" }

return ui.component(function(props: Props): Node
	local r = props.route
	local t = props.target
	local function state(): string
		local rr = r()
		if rr.target and rr.target.kind == "unsure" then
			return "unsure"
		end
		return rr.state or "pending"
	end
	-- The reply's height as drawn: its scroll stands that tall, up to its
	-- room, so the card (and the popup round it) is exactly as tall as what
	-- it holds. (A scroll told only a most-height is measured at its whole
	-- content's: the popup was sized to that, its panel and shadow running
	-- on past the card's end.)
	local reply_h = ui.use_state(0)
	return HoverCard.Card {
		class = "orch-part-card",
		w = WIDTH,
		p = "$space.3",
		gap = "$space.2",
		ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			hue.Dot { hue = function(): number?
				return t().hue
			end, size = 7 },
			ui.text {
				function(): string
					return t().label
				end,
				grow = 1,
				min_w = 0,
				truncate = true,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.primary",
			},
			ui.div {
				color = "$text.tertiary",
				shrink = 0,
				Icon {
					name = function(): Icon.Name
						return TARGET_ICON[t().kind] or "map-arrow-right"
					end,
					size = 13,
				},
			},
		},
		-- What it is and how it stands, quiet.
		ui.row {
			class = "orch-part-meta",
			gap = "$space.3",
			items = "center",
			ui.row {
				gap = "$space.1",
				items = "center",
				ui.div {
					color = "$text.tertiary",
					Icon {
						name = function(): Icon.Name
							return INTENT_ICON[r().intent or "task"] or "checklist"
						end,
						size = 12,
					},
				},
				ui.text {
					function(): string
						return locale.s(`ds.orch.intent.{r().intent or "task"}`)
					end,
					text_size = "$font.size.xs",
					color = "$text.secondary",
				},
			},
			ui.row {
				gap = "$space.1",
				items = "center",
				status.StatusIcon {
				kind = function(): any
					return STATUS[state()] or "waiting"
				end,
					size = 12,
				},
				ui.text {
					function(): string
						return locale.s(`ds.orch.route_state.{state()}`)
					end,
					text_size = "$font.size.xs",
					color = "$text.secondary",
				},
			},
		},
		ui.show(function(): boolean
			local m = r().message
			return type(m) == "string" and m ~= ""
		end, ui.text {
			function(): string
				return r().message or ""
			end,
			class = "orch-part-message",
			literal = true,
			wrap = true,
			-- (Two lines, cut short: the whole of it is in the thread.)
			lines = 2,
			truncate = true,
			w_full = true,
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = "$text.secondary",
		}),
		-- The reply, whole, as the thread has it (rendered), scrolling past
		-- its room.
		ui.show(function(): boolean
			local a = r().answer_full or r().answer
			return type(a) == "string" and a ~= ""
		end, ui.column {
			w_full = true,
			gap = "$space.2",
			ui.div { w_full = true, h = 1, bg = "$border.subtle" },
			ui.scroll {
				class = "orch-part-answer",
				w_full = true,
				h = function(): number
					return math.min(reply_h:get(), REPLY_MAX)
				end,
				fade = { 8, 8 },
				fade_overflow = true,
				ui.column {
					w_full = true,
					on_resize = function(size: { w: number, h: number })
						if math.abs(size.h - reply_h:peek()) >= 0.5 then
							reply_h:set(size.h)
						end
					end,
					-- (A reply's own markdown, as the transcript sets it.)
					Markdown {
						text = function(): string
							local rr = r()
							return rr.answer_full or rr.answer or ""
						end,
					},
				},
			},
		}),
		ui.show(function(): boolean
			local tg = t()
			return tg.id ~= nil and tg.kind ~= "unsure"
		end, ui.row {
			w_full = true,
			ui.div { grow = 1 },
			ui.button {
				class = "orch-part-open",
				label = function(): string
					return locale.s("ds.orch.receipt.open", { name = t().label })
				end,
				h = 22,
				px = "$space.2",
				gap = "$zeron.space.hair",
				items = "center",
				rounded = "$radius.md",
				bg = "#00000000",
				hover = { bg = "$hover" },
				on_click = function()
					local tg = t()
					props.on_open({ kind = tg.kind, id = tg.id })
				end,
				ui.text { locale.t("ds.orch.receipt.open_short"), text_size = "$font.size.xs", color = "$text.secondary" },
				ui.div { color = "$text.tertiary", Icon { name = "arrow-up-right", size = 11 } },
			},
		}),
	}
end)
