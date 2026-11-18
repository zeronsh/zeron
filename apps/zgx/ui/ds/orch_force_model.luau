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

-- A chat's provider, by its agent: "claude" | "openai" (Codex, GPT too) |
-- "gemini" | "cursor" | "grok" | "copilot" | "other".
local KNOWN = { { "claude", "claude" }, { "codex", "openai" }, { "openai", "openai" }, { "gpt", "openai" }, { "gemini", "gemini" }, { "cursor", "cursor" }, { "grok", "grok" }, { "copilot", "copilot" } }
local function provider(chat: any): string
	local a = string.lower(tostring((chat and (chat.agent or chat.harness)) or ""))
	for _, k in KNOWN do
		if string.find(a, k[1], 1, true) then
			return k[2]
		end
	end
	return "other"
end

local M = {}

M.provider = provider

-- How long a message stays a weak link, and how long it's "live" (in
-- flight: a dashed arc).
M.TALK_S = 30 * 60
M.LIVE_S = 90

export type Node = {
	id: string,
	kind: string, -- "brain" | "orchestrator" | "machine" | "chat"
	-- (Machine pivot: a thread that's an orchestrator's own.)
	lead_of: boolean?,
	title: string,
	device: string?,
	hue: number?, -- its orchestrator's (a chat's lead's)
	lead: string?, -- a chat's orchestrator (or the Brain dump), by id
	parent: number, -- index of its strong parent (0 for the Brain dump)
	own: boolean, -- the person's own thread (no lead)
	fade: number, -- 10 full
	-- Its provider (a chat's, an orchestrator's own): see provider() above.
	provider: string,
	-- A chat's todo: done of total.
	done: number,
	total: number,
}
-- A pair's one link: strong (who leads whom), or weak (who talks to whom:
-- `count` messages either way lately, the last at `at`; `ab`/`ba` how many
-- went each way). Never both for a pair, never two of one.
export type Link = { a: number, b: number, strong: boolean, hub: boolean, count: number?, at: number?, ab: number?, ba: number? }
-- A message in flight, one a pair (the latest), on its weak link; a pair
-- with a strong link shows its messages on that line instead (no arc).
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

-- `pivot`: "orchestrator" (the hubs are the orchestrators, colour says
-- the machine) or "machine" (the hubs are the machines, each a "machine"
-- node `machine:<device>`, the Brain dump linked to them; every thread,
-- an orchestrator's own too, hangs off its machine, coloured by its
-- orchestrator's hue).
M.MACHINE = "machine:"
-- The web last built and what from: the same fleet (model.fleet keeps it
-- through a push that moved nothing it shows), messages and pivot give it
-- back, while no node's todo or agent says otherwise.
local last_build: any = nil
-- (How many times a web was built, not given back: for tests.)
M.builds = 0

function M.build(world: any, comms: { any }?, now: number?, pivot: string?): Model
	local fleet = model.fleet(world)
	local chats = world.chats or {}
	local t = now or os.time()
	local lb = last_build
	if lb and rawequal(lb.fleet, fleet) and rawequal(lb.comms, comms) and lb.pivot == pivot and lb.t == t then
		local same = true
		for _, n in lb.model.nodes do
			if n.kind == "chat" or n.kind == "orchestrator" then
				local c = chats[n.id]
				if provider(c) ~= n.provider then
					same = false
					break
				end
				-- (A chat's todo on its node; an orchestrator's node has none.)
				if n.kind == "chat" then
					local td = c and c.todo
					local d, tot = 0, 0
					if type(td) == "table" then
						d, tot = td.done or 0, td.total or 0
					end
					if d ~= n.done or tot ~= n.total then
						same = false
						break
					end
				end
			end
		end
		if same then
			return lb.model
		end
	end
	M.builds += 1
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
	if pivot == "machine" then
		-- The Brain dump in the middle; a hub a machine (in the machines'
		-- order); every thread off its machine, an orchestrator's own as a
		-- thread wearing its hue.
		local brain_th: any = nil
		for _, th in leads do
			if th.id == brain then
				brain_th = th
			end
		end
		if brain_th then
			add({ id = brain_th.id, kind = "brain", title = brain_th.title, device = brain_th.device, hue = brain_th.hue, parent = 0, own = false, provider = "other", fade = 10, done = 0, total = 0 })
		end
		local hub_of: { [string]: number } = {}
		for _, m in fleet.machines do
			local any_here = false
			for _, g in m.groups do
				for _, th in g.threads do
					if th.id ~= brain then
						any_here = true
					end
				end
			end
			if any_here then
				add({ id = M.MACHINE .. m.id, kind = "machine", title = m.name or m.id, device = m.id, parent = if brain then 1 else 0, own = false, provider = "other", fade = 10, done = 0, total = 0 })
				hub_of[m.id] = #out.nodes
			end
		end
		for _, list in { leads, rest } do
			for _, th in list do
				if th.id ~= brain then
					local d, tot = todo(th.id)
					local lead = if th.kind == "orchestrator" then th.id else th.orchestrator
					add({
						id = th.id,
						kind = "chat",
						title = th.title,
						device = th.device,
						hue = th.hue,
						lead = lead,
						parent = hub_of[th.device or ""] or (if brain then 1 else 0),
						own = lead == nil,
						lead_of = th.kind == "orchestrator",
						provider = provider(chats[th.id]),
						fade = if th.kind == "orchestrator" then 10 else tenths(th),
						done = d,
						total = tot,
					} :: any)
				end
			end
		end
		leads, rest = {}, {}
	end
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
			provider = if is_brain then "other" else provider(chats[th.id]),
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
			provider = provider(chats[th.id]),
			fade = tenths(th),
			done = d,
			total = tot,
		})
	end
	-- Strong: each to its parent (an orchestrator to the Brain dump: a hub
	-- link, longer).
	for i, n in out.nodes do
		if n.parent > 0 then
			table.insert(out.links, { a = n.parent, b = i, strong = true, hub = n.kind == "orchestrator" or n.kind == "machine" })
		end
	end
	-- Weak: who talked to whom lately (comms), an open ask to its
	-- orchestrator, a part the Brain dump handed over. One link a pair,
	-- whichever way (A to B and B to A the same), its messages counted into
	-- it; none where the pair has a strong link (what passes between them
	-- shows on that line).
	local function pair_key(a: number, b: number): string
		return if a < b then `{a}-{b}` else `{b}-{a}`
	end
	local strong_pairs: { [string]: boolean } = {}
	for _, l in out.links do
		strong_pairs[pair_key(l.a, l.b)] = true
	end
	local weak_of: { [string]: Link } = {}
	local function weak(a: number?, b: number?, at: number?): Link?
		if not a or not b or a == b then
			return nil
		end
		local key = pair_key(a, b)
		if strong_pairs[key] then
			return nil
		end
		local l = weak_of[key]
		if not l then
			l = ({ a = math.min(a, b), b = math.max(a, b), strong = false, hub = false, count = 0, at = 0, ab = 0, ba = 0 } :: any) :: Link
			weak_of[key] = l
			table.insert(out.links, l)
			table.insert(sig, `w:{key}`)
		end
		if at then
			local w = l :: any
			w.count += 1
			w.at = math.max(w.at, at)
			if a < b then
				w.ab += 1
			else
				w.ba += 1
			end
		end
		return l
	end
	local arc_of: { [string]: Arc } = {}
	for _, e in comms or {} do
		local at = e.at or 0
		if at >= t - M.TALK_S then
			local a, b = out.index[e.from or ""], out.index[e.to or ""]
			local l = weak(a, b, at)
			-- (In flight: one arc a pair, its latest, and only on a weak link.)
			if l and a and b and at >= t - M.LIVE_S then
				local key = pair_key(a, b)
				local cur = arc_of[key]
				if not cur then
					cur = { a = a, b = b, at = at }
					arc_of[key] = cur
					table.insert(out.arcs, cur)
				elseif at > cur.at then
					cur.a, cur.b, cur.at = a, b, at
				end
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
	last_build = { fleet = fleet, comms = comms, pivot = pivot, t = t, model = out }
	return out
end

return M
