-- Comms: what passes between agents, and between them and the person, as
-- it happens: reports, messages, asks, answers, routes and spawns, newest
-- on top. Each a row: what kind (its glyph), who to whom in their hues, its
-- one line under it. Those that came close together are one burst: their
-- time said once, at its head, a thread down their side holding them
-- together; a quiet gap between one burst and the next. A new one slides
-- in at the top.
--
--   Comms {
--     comms = function() return orchestration.comms:get() end,
--     name_of = function(id) return "Orchestrators" end,  -- "you" too
--     hue_of = function(id) return hue? end,
--     on_open = function(id) end,                          -- a chat's id
--     limit = 8,
--   }

local Icon = require("@self/icon")
local status = require("@self/status")
local locale = require("@self/locale")
local hue = require("@self/orch_hue")
local Block = require("@self/orch_block")
local Ref = require("@self/orch_ref")

export type Props = {
	comms: () -> { any },
	name_of: (id: string) -> string?,
	hue_of: (id: string) -> number?,
	on_open: (id: string) -> (),
	limit: number?,
}

-- Two events this close (seconds) are one burst.
local BURST = 90

local KIND_ICON: { [string]: Icon.Name } = {
	report = "checklist",
	message = "chat-round-line",
	ask = "status-input",
	answer = "check-circle",
	route = "map-arrow-right",
	spawn = "plus",
}
-- A kind's glyph ink: an ask is the one that wants the person.
local KIND_INK: { [string]: string } = {
	ask = "$zeron.status.input",
	answer = "$zeron.status.done",
}

-- A row as drawn: the event, and where it stands in its burst.
type Row = { id: string, e: any, first: boolean, last: boolean, top: boolean }

return ui.component(function(props: Props): Node
	local limit = props.limit or 8
	local rows = ui.computed(function(): { Row }
		local list = props.comms() or {}
		local out: { Row } = {}
		for i = #list, math.max(1, #list - limit + 1), -1 do
			local e = list[i]
			local prev = out[#out]
			local first = prev == nil or ((prev.e.at or 0) - (e.at or 0)) > BURST
			if prev and first then
				prev.last = true
			end
			table.insert(out, { id = e.id, e = e, first = first, last = false, top = prev == nil })
		end
		if #out > 0 then
			out[#out].last = true
		end
		return out
	end)

	-- Who, by name always: a chat not (yet) known by its id, or none yet
	-- (a route to a thread still being made), by what it is.
	local function party(id: () -> string, kind: () -> string?): Node
		return ui.row {
			gap = "$zeron.space.tight",
			items = "center",
			min_w = 0,
			shrink = 1,
			ui.show(function(): boolean
				return id() ~= "you"
			end, hue.Dot { hue = function(): number?
				return props.hue_of(id())
			end, size = 6 }),
			ui.text {
				function(): string
					local i = id()
					if i == "you" then
						return locale.s("ds.orch.comms.you")
					end
					local name = props.name_of(i)
					if name and name ~= "" then
						return name
					end
					local k = kind()
					return locale.s(if k == "route" or k == "spawn" then "ds.orch.target.new_thread" else "ds.orch.target.chat")
				end,
				truncate = true,
				min_w = 0,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = function(): string
					return if id() == "you" then "$text.secondary" else "$text.primary"
				end,
			},
		}
	end

	local function row(_r: Row, _i: number, item: State<Row>): Node
		local function e(): any
			return item:get().e
		end
		-- Dragged: a reference to it (ds/orch_ref), named by its first words.
		local drag = Ref.source(function(): any
			local ev = item:peek().e
			local text = ev.summary or ev.text or ""
			local words = string.match(string.gsub(text, "%s+", " "), "^%s*(.-)%s*$") or ""
			local first = Ref.cut(words, 40)
			return Ref.make("comm", ev.id, first, { from = ev.from, to = ev.to, text = text })
		end)
		return ui.button {
			class = "orch-comm",
			on_drag = drag.on_drag,
			drag_preview = drag.drag_preview,
			drag_preview_anchor = drag.drag_preview_anchor,
			drag_preview_offset = drag.drag_preview_offset,
			label = function(): string
				return e().summary or ""
			end,
			w_full = true,
			pt = function(): any
				local r = item:get()
				return if r.first and not r.top then "$space.3" else 0
			end,
			gap = "$space.2",
			items = "stretch",
			bg = "#00000000",
			layout = "position",
			layout_duration = 220,
			on_click = function()
				local ev = item:peek().e
				local target = if ev.from ~= "you" then ev.from else ev.to
				if target ~= "you" then
					props.on_open(target)
				end
			end,
			-- Its kind, and the thread down a burst.
			ui.column {
				w = 16,
				shrink = 0,
				items = "center",
				ui.div {
					h = 20,
					items = "center",
					justify = "center",
					color = function(): string
						return KIND_INK[e().kind] or "$text.tertiary"
					end,
					Icon {
						name = function(): Icon.Name
							return KIND_ICON[e().kind] or "chat-round-line"
						end,
						size = 13,
					},
				},
				ui.div {
					class = "orch-comm-thread",
					w = 1,
					grow = 1,
					bg = function(): string
						return if item:get().last then "#00000000" else "$border"
					end,
				},
			},
			ui.column {
				grow = 1,
				min_w = 0,
				pb = "$space.2",
				gap = "$zeron.space.hair",
				ui.row {
					w_full = true,
					h = 20,
					gap = "$zeron.space.tight",
					items = "center",
					party(function(): string
						return e().from or ""
					end, function(): string?
						return nil
					end),
					ui.div { color = "$text.tertiary", shrink = 0, Icon { name = "arrow-right", size = 11 } },
					party(function(): string
						return e().to or ""
					end, function(): string?
						return e().kind
					end),
					ui.div { grow = 1 },
					ui.text {
						function(): string
							return if item:get().first then status.ago(status.since(e().at or 0)) else ""
						end,
						class = "orch-comm-time",
						y = Block.baseline("$font.size.sm", "$font.size.xs"),
						tabular = true,
						shrink = 0,
						text_size = "$font.size.xs",
						color = "$text.tertiary",
					},
				},
				ui.text {
					function(): string
						return e().summary or ""
					end,
					literal = true,
					w_full = true,
					truncate = true,
					text_size = "$font.size.sm",
					color = "$text.secondary",
				},
			},
		}
	end

	return ui.column {
		class = "orch-comms",
		w_full = true,
		ui.show(function(): boolean
			return #rows:get() == 0
		end, ui.text { locale.t("ds.orch.comms.none"), py = "$space.2", text_size = "$font.size.sm", color = "$text.tertiary" }),
		ui.each(rows, row, { key = function(r: Row): string
			return r.id
		end, w_full = true }),
	}
end)
