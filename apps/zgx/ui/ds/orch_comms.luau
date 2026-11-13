-- Comms: what passes between agents, and between them and the person, as
-- it happens: reports, messages, asks, answers, routes and spawns, newest
-- on top, each a compact row (who to whom, in their hues; what kind; how
-- long ago) with its one line under it. A new one slides in at the top.
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

export type Props = {
	comms: () -> { any },
	name_of: (id: string) -> string?,
	hue_of: (id: string) -> number?,
	on_open: (id: string) -> (),
	limit: number?,
}

local KIND_ICON: { [string]: Icon.Name } = {
	report = "checklist",
	message = "chat-round-line",
	ask = "status-input",
	answer = "check-circle",
	route = "map-arrow-right",
	spawn = "plus",
}

return ui.component(function(props: Props): Node
	local limit = props.limit or 8
	local rows = ui.computed(function(): { any }
		local list = props.comms() or {}
		local out = {}
		for i = #list, math.max(1, #list - limit + 1), -1 do
			table.insert(out, list[i])
		end
		return out
	end)

	local function party(id: () -> string): Node
		return ui.row {
			gap = "$zeron.space.tight",
			items = "center",
			min_w = 0,
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
					return props.name_of(i) or ""
				end,
				truncate = true,
				min_w = 0,
				text_size = "$font.size.sm",
				weight = "$font.weight.medium",
				color = "$text.primary",
			},
		}
	end

	local function row(_e: any, _i: number, item: State<any>): Node
		return ui.button {
			class = "orch-comm",
			label = function(): string
				local e = item:get()
				return e.summary or ""
			end,
			w_full = true,
			py = "$space.2",
			gap = "$zeron.space.hair",
			direction = "column",
			items = "start",
			bg = "#00000000",
			layout = "position",
			layout_duration = 220,
			on_click = function()
				local e = item:peek()
				local target = if e.from ~= "you" then e.from else e.to
				if target ~= "you" then
					props.on_open(target)
				end
			end,
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "center",
				party(function(): string
					return item:get().from
				end),
				ui.div { color = "$text.tertiary", shrink = 0, Icon { name = "arrow-right", size = 12 } },
				party(function(): string
					return item:get().to
				end),
				ui.div { grow = 1 },
				ui.div {
					color = "$text.tertiary",
					shrink = 0,
					Icon {
						name = function(): Icon.Name
							return KIND_ICON[item:get().kind] or "chat-round-line"
						end,
						size = 12,
					},
				},
				ui.text {
					function(): string
						return status.ago(status.since(item:get().at or 0))
					end,
					tabular = true,
					shrink = 0,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			},
			ui.text {
				function(): string
					return item:get().summary or ""
				end,
				literal = true,
				w_full = true,
				truncate = true,
				text_size = "$font.size.sm",
				color = "$text.secondary",
			},
		}
	end

	return ui.column {
		class = "orch-comms",
		w_full = true,
		ui.show(function(): boolean
			return #rows:get() == 0
		end, ui.text { locale.t("ds.orch.comms.none"), py = "$space.3", text_size = "$font.size.sm", color = "$text.tertiary" }),
		ui.each(rows, row, { key = function(e: any): string
			return e.id
		end, w_full = true }),
	}
end)
