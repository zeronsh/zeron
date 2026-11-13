-- Machines: each device's load as it moves (a CPU line kept here from the
-- samples as they come, while something shows them), its memory, the
-- threads it runs, and the tests and builds running on it now.
--
--   Machines {
--     devices = function() return sessions.now's devices end,
--     loads = function() return orchestration.loads:get() end,
--     chats = function() return chats by id end,   -- ds/orch_data.chats
--     here = function() return this device's id end,
--     hue_of = function(chat_id) return hue? end,
--     on_open_chat = function(id) end,
--   }

local Icon = require("@self/icon")
local status = require("@self/status")
local locale = require("@self/locale")
local hue = require("@self/orch_hue")
local data = require("@self/orch_data")

export type Props = {
	devices: () -> { any },
	loads: () -> { [string]: any },
	chats: () -> { [string]: any },
	here: () -> string?,
	hue_of: (chat_id: string) -> number?,
	on_open_chat: (id: string) -> (),
}

-- Samples kept for each device's line: the last few minutes at one every
-- five seconds.
local KEEP = 48
local history: { [string]: { number } } = {}
local last_at: { [string]: number } = {}

local LOUD: { [string]: boolean } = { testing = true, building = true, installing = true, merging = true, pushing = true }

local function gb(bytes: number?): string
	if not bytes then
		return "–"
	end
	local g = bytes / 1e9
	return if g >= 10 then string.format("%.0f", g) else string.format("%.1f", g)
end

return ui.component(function(props: Props): Node
	local tick = ui.use_state(0)
	-- Each new sample onto its device's line.
	ui.effect(function()
		local loads = props.loads() or {}
		local moved = false
		for id, l in loads do
			if l.cpu and l.at and last_at[id] ~= l.at then
				last_at[id] = l.at
				local h = history[id] or {}
				table.insert(h, l.cpu)
				while #h > KEEP do
					table.remove(h, 1)
				end
				history[id] = h
				moved = true
			end
		end
		if moved then
			tick:set(tick:peek() + 1)
		end
	end)

	local devices = ui.computed(function(): { any }
		local here = props.here()
		local out = {}
		for _, d in props.devices() or {} do
			if d.online ~= false or d.id == here then
				table.insert(out, d)
			end
		end
		table.sort(out, function(a, b)
			if (a.id == here) ~= (b.id == here) then
				return a.id == here
			end
			return (a.name or "") < (b.name or "")
		end)
		return out
	end)

	local function device_row(_d: any, _i: number, row: State<any>): Node
		local function id(): string
			return row:get().id
		end
		local function load(): any
			return (props.loads() or {})[id()]
		end
		local function running(): { any }
			local out = {}
			for _, c in props.chats() or {} do
				if c.device == id() and c.status == "working" then
					table.insert(out, c)
				end
			end
			table.sort(out, function(a, b)
				local sa = (a.activity and a.activity.since) or 0
				local sb = (b.activity and b.activity.since) or 0
				if sa ~= sb then
					return sa < sb
				end
				return a.id < b.id
			end)
			return out
		end
		local loud = ui.computed(function(): { any }
			local out = {}
			for _, c in running() do
				if c.activity and LOUD[c.activity.kind] then
					table.insert(out, c)
				end
			end
			return out
		end)
		return ui.column {
			class = "orch-machine",
			w_full = true,
			gap = "$space.2",
			py = "$space.3",
			ui.row {
				w_full = true,
				gap = "$space.2",
				items = "baseline",
				ui.text {
					function(): string
						return row:get().name or ""
					end,
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
					color = "$text.primary",
					truncate = true,
					min_w = 0,
				},
				ui.text {
					function(): string
						local n = #running()
						return if n > 0 then locale.s("ds.orch.machines.running", { n = n }) else ""
					end,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
				ui.div { grow = 1 },
				ui.text {
					function(): string
						local l = load()
						return if l and l.cpu then `{math.floor(l.cpu * 100 + 0.5)}%` else "–"
					end,
					tabular = true,
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
					color = "$text.primary",
				},
				ui.text {
					function(): string
						local l = load()
						return if l and l.mem_total then locale.s("ds.orch.machines.memory", { used = gb(l.mem_used), total = gb(l.mem_total) }) else ""
					end,
					tabular = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			},
			-- Its CPU, as it moved.
			ui.canvas {
				class = "orch-machine-line",
				w_full = true,
				h = 26,
				label = function(): string
					local l = load()
					return if l and l.cpu then locale.s("ds.orch.map.cpu", { percent = math.floor(l.cpu * 100 + 0.5) }) else ""
				end,
				draw = function(c: any, w: number, h: number)
					tick:get()
					local samples = history[id()] or {}
					local l = load()
					c:rect(0, h - 1, w, 1, "$border.subtle")
					-- Memory, as a quiet band along the foot.
					if l and l.mem_total and l.mem_used and l.mem_total > 0 then
						c:rect(0, h - 3, w * math.clamp(l.mem_used / l.mem_total, 0, 1), 2, "$border.strong", 1)
					end
					local n = #samples
					if n == 0 then
						return
					end
					local step = w / math.max(1, KEEP - 1)
					local x0 = w - (n - 1) * step
					local top = 2
					local span = h - 6
					local function py(v: number): number
						return top + span * (1 - math.clamp(v, 0, 1))
					end
					if n == 1 then
						-- (One sample yet: where it stands, level.)
						c:rect(0, py(samples[1]) - 0.5, w, 1, "$border.strong")
						c:circle(w - 2, py(samples[1]), 2, "$text.primary")
						return
					end
					local line = ui.path():move(x0, py(samples[1]))
					local area = ui.path():move(x0, h - 4):line(x0, py(samples[1]))
					for i = 2, n do
						local x = x0 + (i - 1) * step
						line = line:line(x, py(samples[i]))
						area = area:line(x, py(samples[i]))
					end
					area = area:line(w, h - 4):close()
					c:fill(area, "$hover")
					c:stroke(line, "$text.secondary", 1.25, { join = "round", cap = "round" })
					c:circle(w - 1.5, py(samples[n]), 2, "$text.primary")
				end,
			},
			-- Tests and builds running on it now.
			ui.each(loud, function(_c: any, _j: number, crow: State<any>): Node
				return ui.button {
					class = "orch-machine-job",
					label = function(): string
						return crow:get().title or ""
					end,
					w_full = true,
					h = 24,
					gap = "$space.2",
					items = "center",
					bg = "#00000000",
					rounded = "$radius.md",
					hover = { bg = "$hover" },
					on_click = function()
						props.on_open_chat(crow:peek().id)
					end,
					hue.Dot { hue = function(): number?
						return props.hue_of(crow:get().id)
					end, size = 6 },
					ui.div {
						color = "$text.secondary",
						Icon {
							name = function(): Icon.Name
								return (data.ACTIVITY :: any)[crow:get().activity.kind] or "terminal"
							end,
							size = 12,
						},
					},
					ui.text {
						function(): string
							local a = crow:get().activity
							return if a then a.label or "" else ""
						end,
						grow = 1,
						min_w = 0,
						truncate = true,
						font = "$font.mono",
						text_size = "$font.size.xs",
						color = "$text.secondary",
					},
					ui.text {
						function(): string
							local a = crow:get().activity
							return if a and a.since then status.elapsed(status.since(a.since)) else ""
						end,
						tabular = true,
						shrink = 0,
						text_size = "$font.size.xs",
						color = "$text.tertiary",
					},
				}
			end, { key = function(c: any): string
				return c.id
			end, w_full = true }),
		}
	end

	return ui.column {
		class = "orch-machines",
		w_full = true,
		ui.each(devices, function(d: any, i: number, row: State<any>): Node
			return ui.column {
				w_full = true,
				ui.show(function(): boolean
					local first = devices:get()[1]
					return first ~= nil and first.id ~= row:get().id
				end, ui.div { w_full = true, h = 1, bg = "$border.subtle" }),
				device_row(d, i, row),
			}
		end, { key = function(d: any): string
			return d.id
		end, w_full = true }),
	}
end)
