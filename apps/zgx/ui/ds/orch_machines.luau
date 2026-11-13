-- Machines: each device as it works. Its head (its platform's mark, its
-- name, "this one", its CPU now as a rolling figure); its CPU over the last
-- six minutes as a live line (the bridge keeps the samples, `history`),
-- marked where a thread on it started tests, a build, a merge or a push, or
-- failed; its memory as a slim meter; and the threads running there, a
-- compact row each: how it stands, what it's doing, how far through its
-- todo.
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
local Spark = require("@self/orch_spark")
local Counter = require("@self/orch_counter")
local Cells = require("@self/orch_cells")
local hold = require("@self/orch_hold")

export type Props = {
	devices: () -> { any },
	loads: () -> { [string]: any },
	chats: () -> { [string]: any },
	here: () -> string?,
	hue_of: (chat_id: string) -> number?,
	on_open_chat: (id: string) -> (),
}

-- The seconds the line shows.
local SPAN = 360

-- What a thread starting marks on its machine's line, and in what hue.
local MARKED: { [string]: string } = { testing = "working", building = "working", installing = "working", merging = "success", pushing = "success" }
-- The states a thread is listed under its machine in.
local LISTED: { [string]: boolean } = { working = true, connecting = true, waiting = true, input = true, approval = true }

-- Each device's marks so far, by what made them (a thread's activity
-- starting, a thread failing): kept for the window, so one stays on the
-- line after its thread moves on.
local marks: { [string]: { [string]: Spark.Event } } = {}

local function gb(bytes: number?): string
	if not bytes then
		return "–"
	end
	local g = bytes / 1e9
	return if g >= 10 then string.format("%.0f", g) else string.format("%.1f", g)
end

local function platform_icon(p: string?): Icon.Name
	return if p == "linux" then "remote-server" elseif p == "ios" or p == "android" then "smartphone" else "laptop"
end

-- A thread's row under its machine.
local function thread_row(props: Props, _c: any, _j: number, crow: State<any>): Node
	local kind = hold.value(function(): status.Kind
		local st = crow:get().status
		return if st and status.KINDS[st] then st else "working"
	end)
	local activity = hold.value(function(): any
		return crow:get().activity
	end)
	local function todo(): any
		local t = crow:get().todo
		return if data.todo_fraction(t) ~= nil then t else nil
	end
	return ui.button {
		class = "orch-machine-job",
		label = function(): string
			return crow:get().title or ""
		end,
		w_full = true,
		direction = "column",
		items = "stretch",
		px = "$space.2",
		py = "$zeron.space.tight",
		gap = "$zeron.space.hair",
		bg = "#00000000",
		rounded = "$radius.md",
		hover = { bg = "$hover" },
		on_click = function()
			props.on_open_chat(crow:peek().id)
		end,
		ui.row {
			w_full = true,
			gap = "$space.2",
			items = "center",
			status.StatusIcon { kind = kind, size = 14 },
			ui.text {
				function(): string
					return crow:get().title or ""
				end,
				grow = 1,
				min_w = 0,
				truncate = true,
				text_size = "$font.size.sm",
				color = "$text.primary",
			},
			hue.Dot { hue = function(): number?
				return props.hue_of(crow:get().id)
			end, size = 6 },
			ui.show(function(): boolean
				return todo() ~= nil
			end, Cells {
				done = function(): number
					local t = todo()
					return if t then t.done or 0 else 0
				end,
				total = function(): number
					local t = todo()
					return if t then t.total or 0 else 0
				end,
				current = function(): boolean
					return kind() == "working"
				end,
				cell = 4,
				h = 10,
			}),
		},
		ui.show(function(): boolean
			return activity() ~= nil
		end, ui.row {
			w_full = true,
			pl = ui.raw(22, "under the title, past the status"),
			gap = "$space.1",
			items = "center",
			ui.div {
				color = "$text.tertiary",
				shrink = 0,
				Icon {
					name = function(): Icon.Name
						local a = activity()
						return (a and (data.ACTIVITY :: any)[a.kind]) or "terminal"
					end,
					size = 12,
				},
			},
			ui.text {
				function(): string
					local a = activity()
					if not a then
						return ""
					end
					return if a.label and a.label ~= "" then a.label else locale.s(`ds.orch.activity.{a.kind}`)
				end,
				class = "orch-machine-activity",
				grow = 1,
				min_w = 0,
				truncate = true,
				font = "$font.mono",
				text_size = "$font.size.xs",
				color = "$text.secondary",
			},
			ui.text {
				function(): string
					local a = activity()
					return if a and a.since then status.elapsed(status.since(a.since)) else ""
				end,
				tabular = true,
				shrink = 0,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
		}),
	}
end

local function device_row(props: Props, _d: any, _i: number, row: State<any>): Node
	local function id(): string
		return row:get().id
	end
	local function load(): any
		return (props.loads() or {})[id()]
	end
	local function online(): boolean
		local l = load()
		return row:get().online ~= false and (l == nil or l.online ~= false)
	end
	local running = ui.computed(function(): { any }
		local out = {}
		for _, c in props.chats() or {} do
			if c.device == id() and c.status and LISTED[c.status] and not c.archived then
				table.insert(out, c)
			end
		end
		table.sort(out, function(a: any, b: any): boolean
			local sa = (a.activity and a.activity.since) or a.updated or 0
			local sb = (b.activity and b.activity.since) or b.updated or 0
			if sa ~= sb then
				return sa < sb
			end
			return a.id < b.id
		end)
		return out
	end)
	-- Its marks: each loud activity's start, each failure, as they're seen.
	local events = ui.computed(function(): { Spark.Event }
		local dev = id()
		local mine = marks[dev] or {}
		marks[dev] = mine
		for _, c in props.chats() or {} do
			if c.device == dev then
				local a = c.activity
				if type(a) == "table" and MARKED[a.kind] and type(a.since) == "number" and LISTED[c.status or ""] then
					mine[`{c.id}:{a.kind}:{a.since}`] = { at = a.since, tone = MARKED[a.kind] }
				end
				if c.status == "failed" and type(c.updated) == "number" then
					mine[`{c.id}:failed:{c.updated}`] = { at = c.updated, tone = "danger" }
				end
			end
		end
		local l = load()
		local now = (l and l.at) or os.time()
		local out: { Spark.Event } = {}
		for key, e in mine do
			if now - e.at > SPAN then
				mine[key] = nil
			else
				table.insert(out, e)
			end
		end
		table.sort(out, function(a: Spark.Event, b: Spark.Event): boolean
			return a.at < b.at
		end)
		return out
	end)
	local samples = ui.computed(function(): { Spark.Sample }
		local out: { Spark.Sample } = {}
		local l = load()
		for _, s in (l and l.history) or {} do
			if type(s.cpu) == "number" and type(s.at) == "number" then
				table.insert(out, { at = s.at, v = s.cpu })
			end
		end
		-- (No line kept yet: where it stands now.)
		if #out == 0 and l and l.cpu and l.at then
			table.insert(out, { at = l.at, v = l.cpu })
		end
		return out
	end)
	local function cpu(): number?
		local l = load()
		return if l and l.cpu then l.cpu * 100 else nil
	end

	local head = ui.row {
		w_full = true,
		gap = "$space.2",
		items = "center",
		ui.div {
			w = 24,
			h = 24,
			shrink = 0,
			rounded = "$radius.md",
			bg = "$hover",
			items = "center",
			justify = "center",
			color = "$text.secondary",
			Icon {
				name = function(): Icon.Name
					return platform_icon(row:get().platform)
				end,
				size = 14,
			},
		},
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
		ui.show(function(): boolean
			return id() == props.here()
		end, ui.text { locale.t("ds.orch.map.here"), shrink = 0, text_size = "$font.size.xs", color = "$text.tertiary" }),
		ui.div { grow = 1 },
		ui.switch(function(): string
			return if online() then "on" else "off"
		end, {
			on = ui.row {
				gap = "$space.1",
				items = "baseline",
				shrink = 0,
				ui.text { locale.t("ds.orch.machines.cpu"), text_size = "$font.size.xs", color = "$text.tertiary" },
				Counter {
					value = cpu,
					suffix = "%",
					anchor = "end",
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
				},
			},
			off = ui.text { locale.t("ds.orch.map.offline"), text_size = "$font.size.xs", color = "$text.tertiary" },
		}),
	}

	local line = Spark.Spark {
		class = "orch-machine-line",
		samples = function(): { Spark.Sample }
			return samples:get()
		end,
		now = function(): number
			local l = load()
			local s = samples:get()
			return (l and l.at) or (if #s > 0 then s[#s].at else os.time())
		end,
		span = SPAN,
		events = function(): { Spark.Event }
			return events:get()
		end,
		height = 40,
		describe = function(): string
			local v = cpu()
			return if v then locale.s("ds.orch.map.cpu", { percent = math.floor(v + 0.5) }) else ""
		end,
	}

	local memory = ui.row {
		class = "orch-machine-memory",
		w_full = true,
		gap = "$space.2",
		items = "center",
		ui.text { locale.t("ds.orch.machines.memory_label"), shrink = 0, text_size = "$font.size.xs", color = "$text.tertiary" },
		ui.div {
			grow = 1,
			h = 3,
			relative = true,
			rounded = "$radius.full",
			bg = "$zeron.slider.track",
			overflow = "hidden",
			ui.div {
				absolute = true,
				left = 0,
				top = 0,
				bottom = 0,
				rounded = "$radius.full",
				bg = "$text.secondary",
				w = function(): string
					local l = load()
					local f = if l and l.mem_total and l.mem_used and l.mem_total > 0 then math.clamp(l.mem_used / l.mem_total, 0, 1) else 0
					return `{math.floor(f * 1000 + 0.5) / 10}%`
				end,
			},
		},
		ui.text {
			function(): string
				local l = load()
				return if l and l.mem_total then locale.s("ds.orch.machines.memory", { used = gb(l.mem_used), total = gb(l.mem_total) }) else "–"
			end,
			tabular = true,
			shrink = 0,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		},
	}

	return ui.column {
		class = "orch-machine",
		w_full = true,
		gap = "$space.3",
		head,
		ui.show(online, ui.column {
			w_full = true,
			gap = "$space.2",
			line,
			ui.show(function(): boolean
				local l = load()
				return l ~= nil and l.mem_total ~= nil
			end, memory),
		}),
		ui.show(function(): boolean
			return #running:get() > 0
		end, ui.column {
			w_full = true,
			ui.each(running, function(c: any, j: number, crow: State<any>): Node
				return thread_row(props, c, j, crow)
			end, { key = function(c: any): string
				return c.id
			end, w_full = true }),
		}),
	}
end

return ui.component(function(props: Props): Node
	local devices = ui.computed(function(): { any }
		local here = props.here()
		local out = {}
		for _, d in props.devices() or {} do
			if d.online ~= false or d.id == here then
				table.insert(out, d)
			end
		end
		table.sort(out, function(a: any, b: any): boolean
			if (a.id == here) ~= (b.id == here) then
				return a.id == here
			end
			return (a.name or "") < (b.name or "")
		end)
		return out
	end)

	return ui.column {
		class = "orch-machines",
		w_full = true,
		ui.each(devices, function(d: any, i: number, row: State<any>): Node
			return ui.column {
				w_full = true,
				ui.show(function(): boolean
					local first = devices:get()[1]
					return first ~= nil and first.id ~= row:get().id
				end, ui.div { w_full = true, h = 1, my = "$space.4", bg = "$border.subtle" }),
				device_row(props, d, i, row),
			}
		end, { key = function(d: any): string
			return d.id
		end, w_full = true }),
	}
end)
