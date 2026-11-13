-- The Map's Machines: a block a machine, each its own unit. Its head (its
-- glyph, name and platform; its CPU now, its memory) over its CPU's last
-- few minutes; then what it runs, by project and orchestrator: each
-- thread's state (Zeron's glyph, the working ring), title, what it's
-- doing (rolling to the next as it changes), its todo in cells and for how
-- long. An orchestrator's hue marks its own (its dot, a rule beside its
-- crew), never a state. Blocks fill columns as wide as the Map allows, the
-- shortest column taking the next.
--
--   Machines { world, chat, on_open_chat, on_open_orchestrator }

local Icon = require("@self/icon")
local status = require("@self/status")
local locale = require("@self/locale")
local material = require("@self/material")
local RollText = require("@self/roll_text")
local hue = require("@self/orch_hue")
local kit = require("@self/orch_map_kit")
local Cells = require("@self/orch_cells")
local Counter = require("@self/orch_counter")
local Spark = require("@self/orch_spark")
local model = require("@self/orch_map_model")

local M = {}

local MIN_W = 300
local GAP = 16
local ROW_H = 28

-- A CPU line's range to draw it in: its own, padded, a fifth at least.
local function range_of(l: any): (number, number)
	local low, high = 1, 0
	for _, s in (l and l.history) or {} do
		low = math.min(low, s.cpu)
		high = math.max(high, s.cpu)
	end
	if high < low then
		return 0, 1
	end
	local lo = math.max(0, low - 0.08)
	return lo, math.min(1, math.max(high + 0.08, lo + 0.2))
end

local function gb(bytes: number?): string
	if not bytes then
		return "–"
	end
	local g = bytes / 1e9
	return if g >= 10 then string.format("%.0f", g) else string.format("%.1f", g)
end

-- A machine's lines: per project, per orchestrator (none last), its rows.
export type Line = { id: string, kind: string, thread: model.Thread?, name: string?, hue: number?, here: boolean?, crew: boolean? }
local function lines_of(m: model.Machine, orchestrators: { [string]: any }): { Line }
	local out: { Line } = {}
	for _, g in m.groups do
		if g.project then
			table.insert(out, { id = `p:{g.key}`, kind = "project", name = g.project })
		end
		local current: string? = nil
		for _, t in g.threads do
			if t.orchestrator ~= current then
				current = t.orchestrator
				if t.orchestrator and t.kind ~= "orchestrator" then
					-- Crew of one that lives elsewhere: its name, quieter.
					local o = orchestrators[t.orchestrator] or {}
					table.insert(out, { id = `o:{g.key}/{t.orchestrator}`, kind = "away", name = o.name, hue = o.hue })
				end
			end
			table.insert(out, { id = t.id, kind = t.kind, thread = t, hue = t.hue, crew = t.kind == "crew" })
		end
	end
	return out
end

M.View = ui.component(function(props: {
	world: () -> any,
	chat: (id: string) -> any,
	on_open_chat: (id: string) -> (),
	on_open_orchestrator: (id: string) -> (),
}): Node
	local width = ui.use_state(720)
	local fleet = ui.computed(function(): model.Fleet
		return model.fleet(props.world())
	end)
	-- The blocks, in columns: each to the shortest so far.
	local columns = ui.computed(function(): { { id: string, machines: { string } } }
		local w = width:get()
		local n = math.clamp(math.floor((w + GAP) / (MIN_W + GAP)), 1, 4)
		local f = fleet:get()
		n = math.min(n, math.max(1, #f.machines))
		local cols: { { id: string, machines: { string } } } = {}
		local heights = {}
		for i = 1, n do
			table.insert(cols, { id = `col{i}`, machines = {} :: { string } })
			heights[i] = 0
		end
		for _, m in f.machines do
			local best = 1
			for i = 2, n do
				if heights[i] < heights[best] - 1 then
					best = i
				end
			end
			table.insert(cols[best].machines, m.id)
			heights[best] += 110 + #lines_of(m, f.orchestrators) * ROW_H
		end
		return cols
	end)
	local function machine(id: string): model.Machine?
		for _, m in fleet:get().machines do
			if m.id == id then
				return m
			end
		end
		return nil
	end

	local function thread_row(line: State<any>): Node
		local function t(): any
			return line:get().thread or {}
		end
		local function c(): any
			return props.chat(line:get().id) or {}
		end
		-- (Its state and activity held 300 ms at least.)
		local st, activity = kit.held(c)
		local is_orch = line:peek().kind == "orchestrator"
		return ui.button {
			class = if line:peek().kind == "crew" then "orch-mc-row orch-pill" elseif is_orch then "orch-mc-row orch-node" else "orch-mc-row",
			label = function(): string
				local a = activity()
				local title = t().title or ""
				return if a then `{title}, {locale.s(`ds.orch.activity.{a.kind}`)}` else title
			end,
			w_full = true,
			h = ROW_H,
			pl = function(): number
				return if line:get().crew then 14 else 4
			end,
			pr = 4,
			gap = "$space.2",
			items = "center",
			rounded = "$radius.md",
			relative = true,
			bg = "#00000000",
			hover = { bg = "$hover" },
			on_click = function()
				if is_orch then
					props.on_open_orchestrator(line:peek().id)
				else
					props.on_open_chat(line:peek().id)
				end
			end,
			-- Its orchestrator's rule, beside its crew.
			ui.show(function(): boolean
				return line:get().crew == true
			end, ui.div {
				absolute = true,
				left = 4,
				top = 4,
				bottom = 4,
				w = 2,
				rounded = "$radius.full",
				bg = function(): any
					return hue.paint(line:get().hue, 0.55)
				end,
			}),
			ui.div {
				w = 14,
				h = 14,
				shrink = 0,
				items = "center",
				justify = "center",
				ui.switch(function(): string
					if is_orch and st() ~= "working" then
						return "orch"
					end
					return if st() == "idle" then "idle" else "state"
				end, {
					orch = hue.Dot { hue = function(): number?
						return line:get().hue
					end, size = 9 },
					idle = ui.div { w = 7, h = 7, rounded = "$radius.full", border = 1, border_color = "$text.tertiary" },
					state = status.StatusIcon { kind = function(): any
						return st()
					end, size = 14 },
				}),
			},
			ui.text {
				function(): string
					return t().title or ""
				end,
				min_w = 0,
				shrink = 1,
				truncate = true,
				text_size = "$font.size.sm",
				weight = if is_orch then "$font.weight.semibold" else nil,
				color = function(): string
					local s = st()
					return if (s == "done" or s == "idle") and not is_orch then "$text.secondary" else "$text.primary"
				end,
			},
			RollText {
				function(): string
					local a = activity()
					local s = st()
					if a then
						return locale.s(`ds.orch.activity.{a.kind}`)
					elseif s == "input" or s == "approval" or s == "failed" then
						return status.KINDS[s :: any].long
					end
					return ""
				end,
				text_size = "$font.size.xs",
				color = function(): string
					local s = st()
					if s == "input" or s == "approval" or s == "failed" then
						return `$zeron.status.{s}`
					end
					return "$text.secondary"
				end,
			},
			ui.div { grow = 1 },
			ui.show(function(): boolean
				local td = c().todo
				return td ~= nil and (td.total or 0) > 0
			end, Cells {
				done = function(): number
					return (c().todo or {}).done or 0
				end,
				total = function(): number
					return (c().todo or {}).total or 0
				end,
				failed = function(): number
					return if st() == "failed" then 1 else 0
				end,
				current = function(): boolean
					return st() == "working"
				end,
				cell = 3,
				h = 9,
			}),
			ui.text {
				function(): string
					local a = activity()
					if a and a.since then
						return status.elapsed(kit.since(a.since))
					end
					local u = c().updated
					return if u then status.ago(kit.since(u)) else ""
				end,
				shrink = 0,
				w = 30,
				align = "end",
				tabular = true,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
		}
	end

	local function line_row(_l: Line, _i: number, line: State<any>): Node
		local kind = _l.kind
		if kind == "project" then
			return ui.row {
				class = "orch-mc-project",
				w_full = true,
				h = 26,
				pl = 4,
				pt = 6,
				gap = "$space.1",
				items = "center",
				ui.div { color = "$text.tertiary", shrink = 0, Icon { name = "folder", size = 12 } },
				ui.text {
					function(): string
						return line:get().name or ""
					end,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			}
		elseif kind == "away" then
			return ui.row {
				class = "orch-mc-away",
				w_full = true,
				h = 22,
				pl = 4,
				gap = "$space.2",
				items = "center",
				hue.Dot { hue = function(): number?
					return line:get().hue
				end, size = 7 },
				ui.text {
					function(): string
						return line:get().name or ""
					end,
					text_size = "$font.size.xs",
					weight = "$font.weight.medium",
					color = "$text.secondary",
				},
			}
		end
		return thread_row(line)
	end

	local function block(id: string): Node
		local function m(): any
			return machine(id) or { groups = {}, count = 0 }
		end
		local function load(): any
			return ((props.world().loads or {}) :: any)[id]
		end
		local lines = ui.computed(function(): { Line }
			local mm = machine(id)
			return if mm then lines_of(mm, fleet:get().orchestrators) else {}
		end)
		return ui.column {
			class = "orch-mc-block",
			w_full = true,
			p = "$space.4",
			gap = "$space.3",
			rounded = "$radius.xl",
			bg = material.pick("$zeron.settings.tint", "$zeron.settings.card"),
			border = 1,
			border_color = "$zeron.settings.edge",
			opacity = function(): number
				return if m().online == false then 0.55 else 1
			end,
			-- Its head.
			ui.row {
				w_full = true,
				gap = "$space.3",
				items = "center",
				ui.div {
					w = 30,
					h = 30,
					shrink = 0,
					rounded = "$radius.md",
					bg = "$hover",
					items = "center",
					justify = "center",
					color = "$text.secondary",
					Icon { name = function(): any
						return kit.platform_icon(m().device)
					end, size = 16 },
				},
				ui.column {
					grow = 1,
					min_w = 0,
					gap = 1,
					ui.text {
						function(): string
							return m().name or ""
						end,
						truncate = true,
						text_size = "$font.size.sm",
						weight = "$font.weight.semibold",
						color = "$text.primary",
					},
					ui.text {
						function(): string
							local word = kit.platform_word(m().device)
							return if m().here then `{word} · {locale.s("ds.orch.map.here")}` else word
						end,
						truncate = true,
						text_size = "$font.size.xs",
						color = "$text.tertiary",
					},
				},
				ui.column {
					shrink = 0,
					items = "end",
					gap = 1,
					Counter {
						value = function(): number?
							local l = load()
							return if m().online ~= false and l and l.cpu then l.cpu * 100 else nil
						end,
						suffix = "%",
						text_size = "$font.size.md",
						weight = "$font.weight.medium",
						color = "$text.primary",
						anchor = "end",
					},
					ui.text {
						function(): string
							local l = load()
							if not l or not l.mem_total then
								return ""
							end
							return locale.s("ds.orch.machines.memory", { used = gb(l.mem_used), total = gb(l.mem_total) })
						end,
						tabular = true,
						text_size = "$font.size.xs",
						color = "$text.tertiary",
					},
				},
			},
			-- Its CPU over the last minutes, as the host keeps it.
			Spark.Spark {
				-- (In its own range, a little room round it: the figure above
				-- says how much, the line how it moves.)
				samples = function(): { Spark.Sample }
					local l = load()
					local lo, hi = range_of(l)
					local out = {}
					for _, s in (l or {}).history or {} do
						table.insert(out, { at = s.at, v = (s.cpu - lo) / (hi - lo) })
					end
					return out
				end,
				now = function(): number
					local l = load()
					return (l and l.at) or os.time()
				end,
				span = 300,
				height = 30,
			},
			ui.show(function(): boolean
				return (m().count or 0) == 0
			end, ui.text { locale.t("ds.orch.map.idle"), text_size = "$font.size.xs", color = "$text.tertiary", pl = 4 }),
			ui.column {
				w_full = true,
				ui.each(lines, line_row, { key = function(l: Line): string
					return l.id
				end, w_full = true }),
			},
		}
	end

	return ui.row {
		class = "orch-map-machines",
		w_full = true,
		gap = GAP,
		items = "start",
		on_resize = function(size: { w: number, h: number })
			if math.abs(size.w - width:peek()) >= 1 then
				width:set(size.w)
			end
		end,
		ui.each(columns, function(_c: any, _i: number, col: State<any>): Node
			return ui.column {
				grow = 1,
				basis = 0,
				min_w = 0,
				gap = GAP,
				ui.each(ui.computed(function(): { { id: string } }
					local out = {}
					for _, id in col:get().machines do
						table.insert(out, { id = id })
					end
					return out
				end), function(x: { id: string }): Node
					return block(x.id)
				end, { key = function(x: { id: string }): string
					return x.id
				end, w_full = true, gap = GAP }),
			}
		end, { key = function(c: any): string
			return c.id
		end, direction = "row", w_full = true, gap = GAP, items = "start" }),
	}
end)

return M
