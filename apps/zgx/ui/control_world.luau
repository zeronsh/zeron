-- The world the Control room's fixtures draw: account/orchestration_sample
-- as the host would publish it (its time shifted to now), its chats and
-- devices as `sessions.now` (crew threads under their orchestrator, as
-- the host lists them), and the host's calls stood in for: each op noted
-- in the `calls` state and answered at once, changing the world as the
-- engine would. `ui.command`s play what the engine does on its own.
--
--   require("@app/control_world")()   once, before the screen is built

local sessions = require("@account/sessions")
local orchestration = require("@account/orchestration")
local sample_of = require("@account/orchestration_sample")

return function()
	local sample = sample_of.at(os.time())
	local calls = ui.state("calls", "")
	local function note(what: string)
		calls:set(if calls:peek() == "" then what else `{calls:peek()},{what}`)
	end
	local function settled(value: any): any
		return {
			after = function(_self: any, done: ((any) -> ())?, _failed: any)
				if done then
					done(value)
				end
			end,
		}
	end

	-- Sessions: top-level chats, and crew threads as their parent's children.
	local top, children = {}, {}
	for _, c0 in sample.chats do
		-- (The host names a harness's mark as ds.Mark does: Codex's is OpenAI's.)
		local c = table.clone(c0)
		if c.agent == "codex" then
			c.agent = "openai"
		end
		if c.parent then
			children[c.parent] = children[c.parent] or {}
			table.insert(children[c.parent], c)
		else
			table.insert(top, c)
		end
	end
	sessions.now:set({
		state = "ready",
		here = "dev-desk",
		devices = sample.devices,
		projects = { { id = "p-zeron", name = "zeron", device = "dev-desk", path = "/home/you/zeron", git = true } },
		chats = top,
		archived = {},
		children = children,
		places = { pinned = {}, sections = {} },
		places_ready = true,
	} :: any)

	local now = table.clone(sample.now)
	orchestration.now:set(now)

	-- A chat watched in a page's own composer (Capture's, the Brain dump's):
	-- its session as the host would give it, noted by slot in `watched`.
	local lanes = require("@app/screens/chat/lanes") :: any
	local watched = ui.state("watched", {} :: { [string]: string })
	lanes.watch = function(slot: string, id: string?)
		if not id then
			return
		end
		watched:set_key({ slot }, id)
		local view = lanes.view(slot)
		for _, c in sample.chats do
			if c.id == id then
				view.session:set({ id = id, state = "ready", device = c.device, harness = c.agent, model = c.model, running = false })
			end
		end
	end
	orchestration.loads:set(sample.loads)
	orchestration.comms:set(sample.comms)
	orchestration.progress:set(sample.progress)

	local function republish(f: (any) -> ())
		local n = orchestration.now:peek()
		local copy = table.clone(n)
		f(copy)
		orchestration.now:set(copy)
	end

	local stub = orchestration :: any
	stub.loads_wanted = function(on: boolean)
		note(`loads_wanted:{if on then "on" else "off"}`)
	end
	-- "What's up?": the row back at once, its fresh note on its way.
	stub.check = function(chat: string): any
		note(`check:{chat}`)
		local rows = table.clone(orchestration.progress:peek())
		if rows[chat] then
			local r = table.clone(rows[chat])
			r.refreshing = true
			rows[chat] = r
			orchestration.progress:set(rows)
		end
		-- (The host's fresh note, a moment later.)
		ui.after(1500, function()
			local now = table.clone(orchestration.progress:peek())
			if now[chat] then
				local r = table.clone(now[chat])
				r.refreshing = false
				r.note = `Fresh: {r.note or ""}`
				r.note_at = os.time()
				now[chat] = r
				orchestration.progress:set(now)
			end
		end)
		return settled(rows[chat])
	end
	stub.op = function(op: any): any
		local kind = op.op
		if kind == "dump" then
			note(`dump:{op.text}`)
			for _, f in op.attachments or {} do
				note(`dump-file:{f.kind}:{f.name or f.path}`)
			end
			local id = `dp-new-{#calls:peek()}`
			republish(function(n)
				local dumps = table.clone(n.dumps)
				table.insert(dumps, 1, { id = id, text = op.text, state = "routing", routes = {}, device = "dev-desk", created = os.time() })
				n.dumps = dumps
			end)
			return settled({ id = id })
		elseif kind == "answerAsk" then
			note(`answer:{op.id}:{op.answer}`)
			ui.after(400, function()
				republish(function(n)
					local asks = {}
					for _, a in n.asks do
						if a.id == op.id then
							a = table.clone(a)
							a.state = "answered"
							a.answer = op.answer
						end
						table.insert(asks, a)
					end
					n.asks = asks
				end)
			end)
			return settled(nil)
		elseif kind == "dismissAsk" then
			note(`dismiss:{op.id}`)
			return settled(nil)
		elseif kind == "reroute" then
			note(`reroute:{op.dumpId}:{op.routeId}:{op.target.kind}:{op.target.id or ""}`)
			republish(function(n)
				local dumps = {}
				for _, d in n.dumps do
					if d.id == op.dumpId then
						d = table.clone(d)
						local routes = {}
						for _, r in d.routes do
							if r.id == op.routeId then
								r = table.clone(r)
								r.target = { kind = if op.target.kind == "newThread" then "new_thread" else op.target.kind, id = op.target.id }
								r.state = "delivered"
								r.candidates = {}
							end
							table.insert(routes, r)
						end
						d.routes = routes
						d.state = "routed"
					end
					table.insert(dumps, d)
				end
				n.dumps = dumps
			end)
			return settled(nil)
		end
		note(`{kind}`)
		return settled(nil)
	end

	-- The Brain dump placing the newest dump's parts, one after another.
	ui.command { id = "fixture.route_newest", title = "Fixture: route the newest dump", run = function()
		republish(function(n)
			local d = table.clone(n.dumps[1])
			local text = d.text
			local half = math.floor(#text / 2)
			d.routes = {
				{ id = "rt-a", excerpt = string.sub(text, 1, half), span = { from = 0, to = half }, target = { kind = "orchestrator", id = "orc-orch" }, intent = "task", message = "", state = "delivered", candidates = {} },
				{ id = "rt-b", excerpt = string.sub(text, half + 2), span = { from = half + 1, to = #text }, target = { kind = "orchestrator", id = "orc-vpn" }, intent = "question", message = "", state = "pending", candidates = {} },
			}
			d.state = "routed"
			local dumps = table.clone(n.dumps)
			dumps[1] = d
			n.dumps = dumps
		end)
	end }
	-- A report coming in (the Map's pulse, a Comms row).
	ui.command { id = "fixture.report", title = "Fixture: a crew report", run = function()
		local n = #orchestration.comms:peek()
		orchestration.comms:push({ id = `cm-x{n}`, kind = "report", from = "th-engine-state", to = "orc-orch", summary = "WatchOrchestration streams", at = os.time() })
	end }
	-- A thread's activity moving (the pill alone follows it).
	ui.command { id = "fixture.activity", title = "Fixture: a thread starts merging", run = function()
		local n = sessions.now:peek()
		local copy: any = table.clone(n)
		local kids: any = table.clone(copy.children or {})
		local list = {}
		for _, c in kids["orc-orch"] do
			if c.id == "th-engine-flow" then
				c = table.clone(c)
				c.activity = { kind = "merging", label = "git merge orch/engine-state", since = os.time() }
			end
			table.insert(list, c)
		end
		kids["orc-orch"] = list
		copy.children = kids
		sessions.now:set(copy)
	end }
	-- Nothing working anywhere (the Progress widget's quiet line).
	ui.command { id = "fixture.progress_none", title = "Fixture: nothing working", run = function()
		orchestration.progress:set({})
	end }
	ui.command { id = "theme_light", title = "Light theme", run = function()
		ui.theme.follow_system(false)
		ui.theme.current:set("zeron-light")
	end }
	ui.command { id = "theme_dark", title = "Dark theme", run = function()
		ui.theme.follow_system(false)
		ui.theme.current:set("zeron-dark")
	end }
end
