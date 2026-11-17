-- The world the Map (ds/orch_map) shows, sorted for its views: each
-- machine, the projects on it, and on them the orchestrators and the
-- threads at work (crew, and the person's own while they work or wait).
-- Pure: a world in, plain tables out.
--
--   model.fleet(world)  { machines = { Machine }, threads = { [id]: Thread },
--                         orchestrators = { [id]: any } }
--
-- `world` is the Map's: { now, chats, devices, projects?, loads, here,
-- show?, clock? }: `show` the Map's "Show chats" (ds/orch_map_visibility;
-- the person's choice when none), `clock` the time it's judged at (now).
-- A thread hidden by it is left out of its machine (and so out of every
-- view's lines and dots); one fading carries its `opacity`.

local Name = require("@ds/orch_name")
local Visibility = require("@ds/orch_map_visibility")

local M = {}

export type Thread = {
	id: string,
	title: string,
	device: string?,
	project: string?, -- a project's name
	project_id: string?,
	orchestrator: string?, -- whose (an orchestrator's own: its id)
	hue: number?,
	kind: string, -- "orchestrator" | "crew" | "thread"
	-- Crew of the Brain dump: a thread it started (none of an orchestrator's
	-- crew), hanging under it as crew do.
	started: boolean?,
	order: number,
	paused: boolean,
	-- How strongly it shows (1; less while it fades out, ds/orch_map_visibility).
	opacity: number?,
}
export type Group = { key: string, project: string?, threads: { Thread } }
export type Machine = {
	id: string,
	name: string,
	platform: string?,
	cloud: boolean,
	here: boolean,
	online: boolean,
	device: any,
	groups: { Group },
	count: number,
}
export type Fleet = { machines: { Machine }, threads: { [string]: Thread }, orchestrators: { [string]: any } }


-- Machines in their order: this one first, a Cloud machine last, the rest
-- by name.
function M.device_order(devices: { any }, here: string?): { any }
	local list = table.clone(devices or {})
	table.sort(list, function(a: any, b: any): boolean
		local ah, bh = a.id == here or a.here == true, b.id == here or b.here == true
		if ah ~= bh then
			return ah
		end
		local ac, bc = a.cloud == true or a.platform == "cloud", b.cloud == true or b.platform == "cloud"
		if ac ~= bc then
			return bc
		end
		return tostring(a.name or "") < tostring(b.name or "")
	end)
	return list
end

-- One machine per real one. A machine's engine registered again (a
-- reinstall, a new data dir) leaves its old device rows behind under the
-- same name: rows with the same name and platform are one machine, under
-- the row in use (this device, else one online, else the one seen last),
-- online if any is, seen when last any was. Cloud sessions are each their
-- own, as are differently named engines on one host (honeypot-1 beside
-- honeypot). Nothing is dropped: `alias` maps every row's id to its
-- machine's.
--   M.machines_of(devices, here) -> ({ device }, alias: { [id]: id })
function M.machines_of(devices: { any }, here: string?): ({ any }, { [string]: string })
	local alias: { [string]: string } = {}
	local groups: { [string]: { any } } = {}
	local order: { string } = {}
	for _, d in devices or {} do
		local cloud = d.cloud == true or d.platform == "cloud"
		local key = if cloud or d.name == nil or d.name == "" then `id:{d.id}` else `{string.lower(d.name)}/{d.platform or ""}`
		if not groups[key] then
			groups[key] = {}
			table.insert(order, key)
		end
		table.insert(groups[key], d)
	end
	local function rank(d: any): number
		if d.id == here or d.here == true then
			return 3
		end
		return if d.online ~= false then 2 else 1
	end
	local out: { any } = {}
	for _, key in order do
		local g = groups[key]
		local best = g[1]
		for i = 2, #g do
			local d = g[i]
			local rb, rd = rank(best), rank(d)
			if rd > rb or (rd == rb and (tonumber(d.seen) or 0) > (tonumber(best.seen) or 0)) then
				best = d
			end
		end
		local merged = best
		if #g > 1 then
			merged = table.clone(best)
			for _, d in g do
				alias[d.id] = best.id
				if d.online ~= false then
					merged.online = true
				end
				if tonumber(d.seen) and (tonumber(merged.seen) or 0) < d.seen then
					merged.seen = d.seen
				end
			end
		end
		alias[best.id] = best.id
		table.insert(out, merged)
	end
	return out, alias
end

-- Whether a thread of the person's own may show at all: never one
-- archived, nor a chat's subagent (whether it does is the Map's "Show
-- chats", ds/orch_map_visibility).
function M.shown(c: any): boolean
	return c ~= nil and not c.archived and c.child_kind ~= "subagent"
end

-- The Map's visibility rule for `world`: its context, and the time.
function M.visibility(world: any): (Visibility.Ctx, number)
	return Visibility.context(world.now, world.show, nil, world.read_at, world.dismissed), world.clock or os.time()
end

function M.fleet(world: any): Fleet
	local now = world.now or {}
	local chats = world.chats or {}
	local here = world.here
	local projects: { [string]: any } = {}
	for _, p in world.projects or {} do
		projects[p.id] = p
	end
	local items: { [string]: any } = {}
	for _, it in now.items or {} do
		items[it.id] = it
	end

	local threads: { [string]: any } = {}
	local orchestrators: { [string]: any } = {}
	-- (Each thread's name: ds/orch_name's rule, as every view has it.)
	local named = Name.index(now)
	local function project_of(c: any, fallback: string?): (string?, string?)
		local id = (c and c.project) or fallback
		local p = id and projects[id]
		return if p then p.name else nil, id
	end
	local order = 0
	-- (Nothing archived shows: an orchestrator, nor a thread, whose row or
	-- chat is.)
	local function archived(id: string?): boolean
		local c = id and chats[id]
		return c ~= nil and c.archived == true
	end
	-- (An archived orchestrator's crew, and what it started: gone with it,
	-- unless under way or asking for the person.)
	local ctx, clock = M.visibility(world)
	local orphaned: { [string]: boolean } = {}
	local gone: { [string]: boolean } = {}
	for _, o in now.orchestrators or {} do
		if o.archived or archived(o.id) then
			gone[o.id] = true
		end
	end
	for _, m in now.crew or {} do
		if not m.released and gone[m.orchestrator_id] then
			orphaned[m.chat_id] = true
		end
	end
	for _, o in now.orchestrators or {} do
		if not o.archived and not archived(o.id) then
			orchestrators[o.id] = o
			local c = chats[o.id]
			local name, pid = project_of(c, o.project)
			order += 1
			threads[o.id] = {
				id = o.id,
				title = Name.from(named, chats, o.id),
				device = (c and c.device) or here,
				project = name,
				project_id = pid,
				orchestrator = o.id,
				hue = o.hue,
				kind = "orchestrator",
				order = order,
				paused = o.paused == true,
			}
		end
	end
	for _, m in now.crew or {} do
		local o = orchestrators[m.orchestrator_id]
		if not m.released and o and not archived(m.chat_id) then
			local c = chats[m.chat_id]
			local home = threads[o.id]
			-- (A crew thread's project, else its orchestrator's where they
			-- share a machine.)
			local device = (c and c.device) or home.device
			local name, pid = project_of(c, if device == home.device then home.project_id else nil)
			local it = items[m.item_id or ""]
			threads[m.chat_id] = {
				id = m.chat_id,
				-- (Named from its crew row and item while its chat isn't
				-- heard, or has no title of its own yet.)
				title = Name.thread(c, m, it, m.chat_id),
				device = device,
				project = name,
				project_id = pid,
				orchestrator = o.id,
				hue = o.hue,
				kind = "crew",
				order = (it and it.order) or 0,
				paused = o.paused == true,
				joined = m.joined,
			}
		end
	end
	-- What the Brain dump started: under it, wherever they run.
	local prov = M.provenance(world)
	for id, by in prov.started do
		if gone[by] then
			orphaned[id] = true
		end
		local c = chats[id]
		local o = orchestrators[by]
		if c and o and not threads[id] and not c.archived then
			local name, pid = project_of(c, nil)
			threads[id] = ({
				id = id,
				title = Name.thread(c, nil, nil, id),
				device = c.device or here,
				project = name,
				project_id = pid,
				orchestrator = by,
				hue = o.hue,
				kind = "crew",
				order = 0,
				paused = o.paused == true,
				started = true,
			} :: any)
		end
	end
	-- The person's own (in a project or none): at work, or finished lately.
	for id, c in chats do
		if not threads[id] and M.shown(c) and not orchestrators[id] and not gone[id] and (not orphaned[id] or Visibility.live(c, ctx)) then
			local name, pid = project_of(c, nil)
			threads[id] = ({
				id = id,
				title = Name.thread(c, nil, nil, id),
				device = c.device or here,
				project = name,
				project_id = pid,
				orchestrator = nil,
				hue = nil,
				kind = "thread",
				order = 0,
				paused = false,
			} :: any)
		end
	end

	-- Each thread on its machine (a device row of one, M.machines_of).
	local devices, alias = M.machines_of(world.devices or {}, here)
	for _, t in threads do
		if t.device and alias[t.device] then
			t.device = alias[t.device]
		end
	end
	-- The Map's "Show chats": a thread hidden by it leaves the Map (an
	-- orchestrator's own always shows); one fading carries its opacity.
	for id, t in threads do
		if t.kind ~= "orchestrator" then
			local verdict, opacity = Visibility.visible(chats[id], clock, ctx)
			-- (A crew thread with no chat row: only while it's new.)
			if chats[id] == nil and Visibility.missing(t.joined, clock, ctx) == "hide" then
				verdict = "hide"
			end
			if verdict == "hide" then
				threads[id] = nil
			else
				t.opacity = opacity
			end
		end
	end
	-- Machines: those online, and any other a thread still shown is on (one
	-- whose threads are all hidden leaves with them: no empty "Nothing
	-- running" for a machine gone quiet weeks ago).
	local used: { [string]: boolean } = {}
	for _, t in threads do
		if t.device then
			used[t.device] = true
		end
	end
	local machines: { Machine } = {}
	for _, d in M.device_order(devices, here) do
		local is_here = d.id == here or d.here == true
		local online = d.online ~= false or is_here
		if online or used[d.id] then
			table.insert(machines, {
				id = d.id,
				name = d.name or "",
				platform = d.platform,
				cloud = d.cloud == true or d.platform == "cloud",
				here = is_here,
				online = online,
				device = d,
				groups = {},
				count = 0,
			})
		end
	end

	-- On each machine: its projects (by name; none last), and in each the
	-- orchestrators in their order, each followed by its crew, then the
	-- person's own threads.
	local by_machine: { [string]: Machine } = {}
	for _, m in machines do
		by_machine[m.id] = m
	end
	local list: { Thread } = {}
	for _, t in threads do
		table.insert(list, t)
	end
	local function rank(t: Thread): (number, number)
		if t.kind == "thread" then
			return 1e9, 0
		end
		local o = threads[t.orchestrator or ""]
		return (o and o.order) or 0, if t.kind == "orchestrator" then -1 else t.order
	end
	table.sort(list, function(a: Thread, b: Thread): boolean
		local pa, pb = a.project or "\u{10FFFF}", b.project or "\u{10FFFF}"
		if pa ~= pb then
			return pa < pb
		end
		local oa, ia = rank(a)
		local ob, ib = rank(b)
		if oa ~= ob then
			return oa < ob
		end
		if ia ~= ib then
			return ia < ib
		end
		return a.id < b.id
	end)
	for _, t in list do
		local m = by_machine[t.device or ""]
		if m then
			local key = t.project or ""
			local g = m.groups[#m.groups]
			if not g or g.key ~= key then
				g = { key = key, project = t.project, threads = {} }
				table.insert(m.groups, g)
			end
			table.insert(g.threads, t)
			m.count += 1
		end
	end
	return { machines = machines, threads = threads, orchestrators = orchestrators }
end

export type Provenance = {
	-- Each thread the Brain dump (or an orchestrator) started: by whom.
	started: { [string]: string },
	-- What a dump's parts were handed to: { from, to } chat ids.
	routed: { { from: string, to: string } },
}

-- Who started what, and what the Brain dump handed where: a chat's own
-- `started_by` where the host says it (the Brain dump, or an orchestrator
-- that spawned it), else the dumps' routes (a part that started a thread
-- or an orchestrator; a part delivered to one).
function M.provenance(world: any): Provenance
	local now = world.now or {}
	local out: Provenance = { started = {}, routed = {} }
	local brain: string? = nil
	for _, o in now.orchestrators or {} do
		if o.kind == "brain_dump" and not o.archived then
			brain = o.id
		end
	end
	-- (The host's word first: `started_by = { chat_id, dump_id?, route_id? }`,
	-- or the chat's id alone.)
	for id, c in world.chats or {} do
		local by = c.started_by
		local who = if type(by) == "string" then by elseif type(by) == "table" then (by.chat_id or by.chatId) else nil
		if type(who) == "string" and who ~= "" and who ~= id then
			out.started[id] = who
		end
	end
	if not brain then
		return out
	end
	local seen: { [string]: boolean } = {}
	for _, d in now.dumps or {} do
		for _, r in d.routes or {} do
			local t = r.target or {}
			local id = t.id
			if type(id) == "string" and id ~= "" and id ~= brain then
				if (t.kind == "new_thread" or t.kind == "new_orchestrator") and not out.started[id] then
					out.started[id] = brain :: string
				elseif (t.kind == "chat" or t.kind == "orchestrator") and not seen[id] then
					seen[id] = true
					table.insert(out.routed, { from = brain :: string, to = id })
				end
			end
		end
	end
	return out
end

-- A thread's state word, as its chat has it ("idle" for none); stopped by
-- its usage limit and waiting for the reset, "limited" (one running on
-- another account meanwhile stands as it works).
function M.status_of(chat: any): string
	local l = chat and chat.limited
	if type(l) == "table" and type(l.switched_to) ~= "table" then
		return "limited"
	end
	local s = chat and chat.status
	if s == "working" or s == "waiting" or s == "input" or s == "approval" or s == "failed" or s == "done" then
		return s
	end
	return "idle"
end

return M
