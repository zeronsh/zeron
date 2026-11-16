-- What the force Graph (ds/orch_force) shows, from the Map's world: the
-- Brain dump in the middle, the orchestrators round it, their chats round
-- them; strong links for who leads whom, weak ones for who talks to whom.
-- Pure: a world (and the comms) in, plain tables out, in a steady order.
--
--   fm.build(world, comms?, now?)   Model (below)
--
-- Only the chats the Map shows (ds/orch_map_visibility, through the
-- Map's model): a fading one with its strength in tenths (`fade`).

local model = require("@ds/orch_map_model")

local M = {}

-- How long a message stays a weak link, and how long it's "live" (in
-- flight: a dashed arc).
M.TALK_S = 30 * 60
M.LIVE_S = 90

export type Node = {
	id: string,
	kind: string, -- "brain" | "orchestrator" | "chat"
	title: string,
	device: string?,
	hue: number?, -- its orchestrator's (a chat's lead's)
	lead: string?, -- a chat's orchestrator (or the Brain dump), by id
	parent: number, -- index of its strong parent (0 for the Brain dump)
	own: boolean, -- the person's own thread (no lead)
	fade: number, -- 10 full
	-- A chat's todo: done of total.
	done: number,
	total: number,
}
export type Link = { a: number, b: number, strong: boolean, hub: boolean }
export type Arc = { a: number, b: number, at: number }
export type Model = {
	nodes: { Node },
	index: { [string]: number },
	links: { Link },
	arcs: { Arc },
	-- The machines, in their order (for colour by machine): id, name.
	machines: { { id: string, name: string } },
	-- What changes the physics: nodes and links.
	sig: string,
}

function M.build(world: any, comms: { any }?, now: number?): Model
	local fleet = model.fleet(world)
	local chats = world.chats or {}
	local t = now or os.time()
	local out: Model = { nodes = {}, index = {}, links = {}, arcs = {}, machines = {}, sig = "" }
	local sig = {}
	local function add(n: Node)
		table.insert(out.nodes, n)
		out.index[n.id] = #out.nodes
		table.insert(sig, `{n.kind}:{n.id}<{n.parent}`)
	end
	local function todo(id: string): (number, number)
		local td = chats[id] and chats[id].todo
		if type(td) == "table" then
			return td.done or 0, td.total or 0
		end
		return 0, 0
	end
	local function tenths(th: any): number
		return math.clamp(math.floor((th.opacity or 1) * 10 + 0.5), 1, 10)
	end
	-- The Brain dump: the middle (one; else no middle, orchestrators free).
	local brain: string? = nil
	for id, o in fleet.orchestrators do
		if o.kind == "brain_dump" and fleet.threads[id] then
			brain = id
		end
	end
	-- (A steady order: orchestrators by their order, chats by the fleet's.)
	local leads: { any } = {}
	local rest: { any } = {}
	for _, m in fleet.machines do
		table.insert(out.machines, { id = m.id, name = m.name })
		for _, g in m.groups do
			for _, th in g.threads do
				if th.kind == "orchestrator" then
					table.insert(leads, th)
				else
					table.insert(rest, th)
				end
			end
		end
	end
	table.sort(leads, function(a: any, b: any): boolean
		if (a.id == brain) ~= (b.id == brain) then
			return a.id == brain
		end
		if a.order ~= b.order then
			return (a.order or 0) < (b.order or 0)
		end
		return a.id < b.id
	end)
	for _, th in leads do
		local is_brain = th.id == brain
		add({
			id = th.id,
			kind = if is_brain then "brain" else "orchestrator",
			title = th.title,
			device = th.device,
			hue = th.hue,
			lead = if is_brain then nil else brain,
			parent = if is_brain or not brain then 0 else (out.index[brain :: string] or 0),
			own = false,
			fade = 10,
			done = 0,
			total = 0,
		})
	end
	for _, th in rest do
		local lead = th.orchestrator
		local p = (lead and out.index[lead]) or (brain and out.index[brain]) or 0
		local d, tot = todo(th.id)
		add({
			id = th.id,
			kind = "chat",
			title = th.title,
			device = th.device,
			hue = th.hue,
			lead = lead,
			parent = p,
			own = lead == nil,
			fade = tenths(th),
			done = d,
			total = tot,
		})
	end
	-- Strong: each to its parent (an orchestrator to the Brain dump: a hub
	-- link, longer).
	for i, n in out.nodes do
		if n.parent > 0 then
			table.insert(out.links, { a = n.parent, b = i, strong = true, hub = n.kind == "orchestrator" })
		end
	end
	-- Weak: who talked to whom lately (comms), an open ask to its
	-- orchestrator, a part the Brain dump handed over; once a pair.
	local seen: { [string]: boolean } = {}
	local function weak(a: number?, b: number?)
		if not a or not b or a == b then
			return
		end
		local n_a, n_b = out.nodes[a], out.nodes[b]
		if n_b.parent == a or n_a.parent == b then
			return
		end
		local key = if a < b then `{a}-{b}` else `{b}-{a}`
		if seen[key] then
			return
		end
		seen[key] = true
		table.insert(out.links, { a = a, b = b, strong = false, hub = false })
		table.insert(sig, `w:{key}`)
	end
	for _, e in comms or {} do
		local at = e.at or 0
		if at >= t - M.TALK_S then
			local a, b = out.index[e.from or ""], out.index[e.to or ""]
			weak(a, b)
			if a and b and at >= t - M.LIVE_S then
				table.insert(out.arcs, { a = a, b = b, at = at })
			end
		end
	end
	for _, ask in (world.now or {}).asks or {} do
		if ask.state == "open" and ask.from_chat and ask.orchestrator_id then
			weak(out.index[ask.orchestrator_id], out.index[ask.from_chat])
		end
	end
	local prov = model.provenance(world)
	for _, r in prov.routed do
		weak(out.index[r.from], out.index[r.to])
	end
	out.sig = table.concat(sig, "|")
	return out
end

return M
