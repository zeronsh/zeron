-- What the new Graph (ds/orch_graph2) lays out, from the Map's world: each
-- machine and its groups (an orchestrator with the crew it runs there; the
-- crew of one led from another machine, under a ghost of their lead; the
-- person's own threads), and the lines that aren't a comb's (what the
-- Brain dump started or handed a part to). Pure: a world in, the scene
-- ds/orch_graph2_layout takes out, in a steady order.
--
--   scene.of(world, now?)   { machines, links, next_change }
--                           (ds/orch_graph2_layout's scene; `next_change`:
--                           seconds until a chat's visibility changes on its
--                           own, or nil)
--   scene.provider(chat)    "claude" | "openai" | "gemini" | "other"
--
-- Only chats ds/orch_graph2_visibility shows are in it (a fading one with
-- its strength in tenths, `fade`, 10 for full); a ghost lead with none of
-- its crew left goes too. Orchestrators always show.

local model = require("@ds/orch_map_model")
local vis = require("@ds/orch_graph2_visibility")

local function provider(chat: any): string
	local a = string.lower(tostring((chat and (chat.agent or chat.harness)) or ""))
	if string.find(a, "claude", 1, true) then
		return "claude"
	elseif string.find(a, "openai", 1, true) or string.find(a, "codex", 1, true) or string.find(a, "gpt", 1, true) then
		return "openai"
	elseif string.find(a, "gemini", 1, true) then
		return "gemini"
	end
	return "other"
end

local M = {}

M.provider = provider

function M.of(world: any, now: number?): any
	local fleet = model.fleet(world)
	local threads = fleet.threads
	local chats = world.chats or {}
	local t_now = now or vis.now()
	local ctx = vis.context(world)
	local soonest: number? = nil
	-- A thread's say: "show", "fade" or "hide", and its strength in tenths
	-- (an orchestrator always shows).
	local function seen(t: any): (string, number)
		if t.kind == "orchestrator" then
			return "show", 10
		end
		local c = chats[t.id]
		local wait = vis.next_change(c, t_now, ctx)
		if wait and (soonest == nil or wait < soonest) then
			soonest = wait
		end
		local v, a = vis.visible(c, t_now, ctx)
		return v, math.clamp(math.floor(a * 10 + 0.5), 1, 10)
	end
	local function kind_of(t: any): string
		if t.kind == "orchestrator" then
			local o = fleet.orchestrators[t.id]
			return if o and o.kind == "brain_dump" then "brain" else "orchestrator"
		end
		return t.kind
	end
	local function brief(t: any, tenths: number?): any
		return { id = t.id, title = t.title, kind = kind_of(t), hue = t.hue, fade = tenths or 10, provider = provider(chats[t.id]) }
	end
	local machines = {}
	for _, m in fleet.machines do
		-- (The fleet's order: by project, each orchestrator then its crew,
		-- the person's own last.)
		local leads: { any } = {}
		local by_lead: { [string]: any } = {}
		local ghosts: { any } = {}
		local own = { id = `own@{m.id}`, members = {} }
		local function group_for(o: string): any
			local g = by_lead[o]
			if g then
				return g
			end
			local lead = threads[o]
			if lead and lead.device == m.id then
				g = { id = o, lead = brief(lead), members = {} }
				table.insert(leads, g)
			else
				g = { id = `{o}@{m.id}`, ghost = true, lead = if lead then brief(lead) else { id = o, title = "", kind = "orchestrator" }, members = {} }
				table.insert(ghosts, g)
			end
			by_lead[o] = g
			return g
		end
		for _, grp in m.groups do
			for _, t in grp.threads do
				local say, tenths = seen(t)
				if say == "hide" then
					continue
				end
				if t.kind == "orchestrator" then
					group_for(t.id)
				elseif t.kind == "crew" and t.orchestrator then
					table.insert(group_for(t.orchestrator).members, brief(t, tenths))
				else
					table.insert(own.members, brief(t, tenths))
				end
			end
		end
		local groups = {}
		for _, g in leads do
			table.insert(groups, g)
		end
		for _, g in ghosts do
			if #g.members > 0 then
				table.insert(groups, g)
			end
		end
		if #own.members > 0 then
			table.insert(groups, own)
		end
		table.insert(machines, { id = m.id, name = m.name, groups = groups, device = m.device, here = m.here })
	end
	local links = {}
	local prov = model.provenance(world)
	for id, by in prov.started do
		local t = threads[id]
		if t and t.kind == "orchestrator" and threads[by] then
			table.insert(links, { a = by, b = id, kind = "started", hue = threads[by].hue })
		end
	end
	table.sort(links, function(a: any, b: any): boolean
		return a.b < b.b
	end)
	for _, r in prov.routed do
		if threads[r.from] and threads[r.to] then
			table.insert(links, { a = r.from, b = r.to, kind = "routed", hue = threads[r.from].hue })
		end
	end
	return { machines = machines, links = links, next_change = soonest }
end

return M
