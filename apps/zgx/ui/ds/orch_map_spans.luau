-- What the Map's Timeline (ds/orch_map_timeline) draws, worked out: its
-- rows (machines, projects, a lane a thread) and each lane's spans and
-- ticks over a window, from what the host kept of each thread
-- (`orchestration.timeline`) and what passed between them (comms). Pure.
--
--   spans.build(input)            { rows, h, t0, t1 }
--   spans.spans_of(entries, chat, t0, now)   one lane's spans and ticks

local model = require("@ds/orch_map_model")

local M = {}

M.MACHINE_H = 32
M.PROJECT_H = 22
M.LANE_H = 26
local MACHINE_H, PROJECT_H, LANE_H = M.MACHINE_H, M.PROJECT_H, M.LANE_H

-- A span of a lane: from `t0` to `t1` (seconds), its state and what it
-- did.
export type Span = { t0: number, t1: number, status: string, kind: string? }
-- A tick: at `t`, what (`report`, `ask`, `failed`, `merged`, `done`).
export type Tick = { t: number, kind: string }
export type Row = {
	id: string,
	kind: string, -- "machine" | "project" | "lane"
	y: number,
	h: number,
	name: string?,
	device: any?,
	machine: string?,
	thread: model.Thread?,
	spans: { Span }?,
	ticks: { Tick }?,
}
export type Built = { rows: { Row }, h: number, t0: number, t1: number }


-- What a thread did from `t0` to `now`: its changes as the host kept them,
-- its state now added if it moved since.
local function spans_of(entries: { any }?, c: any, t0: number, now: number): ({ Span }, { Tick })
	local list = table.clone(entries or {})
	local st = model.status_of(c)
	local a = c and c.activity
	local kind = if st == "working" and a then a.kind else nil
	local last = list[#list]
	if not last or last.status ~= st or (last.kind or nil) ~= kind then
		local at = if st == "working" and a and a.since then a.since else now
		if last and at < last.at then
			at = last.at
		end
		if st ~= "idle" or last then
			table.insert(list, { at = math.min(at, now), status = st, kind = kind })
		end
	end
	local spans: { Span } = {}
	local ticks: { Tick } = {}
	for i, e in list do
		local s, f = e.at, if list[i + 1] then list[i + 1].at else now
		local prev = list[i - 1]
		if e.at >= t0 and e.at <= now then
			if e.status == "failed" then
				table.insert(ticks, { t = e.at, kind = "failed" })
			elseif e.status == "done" then
				table.insert(ticks, { t = e.at, kind = "done" })
			end
			if prev and prev.kind == "merging" and e.status ~= "failed" then
				table.insert(ticks, { t = e.at, kind = "merged" })
			end
		end
		-- (What it does now shows from now, even for no time yet.)
		if f > t0 and (s < now or i == #list) and s <= now then
			table.insert(spans, { t0 = math.max(s, t0), t1 = math.min(f, now), status = e.status, kind = e.kind })
		end
	end
	return spans, ticks
end
M.spans_of = spans_of

-- The rows (machines, projects, lanes) with each lane's spans and ticks,
-- over the `window` minutes up to `now`.
function M.build(input: { world: any, chat: (string) -> any, timeline: { [string]: { any } }, comms: { any }, now: number, window: number }): Built
	local fleet = model.fleet(input.world)
	local t1 = input.now
	local t0 = t1 - input.window * 60
	local rows: { Row } = {}
	local y = 0
	-- What each thread said and asked, in the window.
	local said: { [string]: { Tick } } = {}
	for _, e in input.comms or {} do
		if e.at and e.at >= t0 and e.at <= t1 and (e.kind == "report" or e.kind == "ask") then
			said[e.from] = said[e.from] or {}
			table.insert(said[e.from], { t = e.at, kind = e.kind })
		end
	end
	for _, m in fleet.machines do
		table.insert(rows, { id = `m:{m.id}`, kind = "machine", y = y, h = MACHINE_H, name = m.name, device = m.device, machine = m.id })
		y += MACHINE_H
		for _, g in m.groups do
			if g.project then
				table.insert(rows, { id = `p:{m.id}/{g.key}`, kind = "project", y = y, h = PROJECT_H, name = g.project, machine = m.id })
				y += PROJECT_H
			end
			for _, t in g.threads do
				local spans, ticks = spans_of(input.timeline[t.id], input.chat(t.id), t0, t1)
				for _, k in said[t.id] or {} do
					table.insert(ticks, k)
				end
				table.insert(rows, { id = t.id, kind = "lane", y = y, h = LANE_H, machine = m.id, thread = t, spans = spans, ticks = ticks })
				y += LANE_H
			end
		end
		y += 6
	end
	return { rows = rows, h = y, t0 = t0, t1 = t1 }
end


return M
