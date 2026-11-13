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
	-- The models a composer offers (the host's catalog), and a pick made the
	-- chat's (`chat.configure`), noted `configure:<id>:<model>`.
	local store = require("@app/screens/chat/store") :: any
	store.catalog_on = function() end
	store.load_catalog = function() end
	store.catalog:set({
		state = "ready",
		complete = true,
		harnesses = { { id = "claude-code", label = "Claude", mark = "claude" } },
		models = {
			{ value = "claude-code/claude-opus-5-5", label = "Opus 5.5", harness = "claude-code" },
			{ value = "claude-code/claude-haiku-5-5", label = "Haiku 5.5", harness = "claude-code" },
		},
	})
	store.configure = function(id: string, patch: any)
		note(`configure:{id}:{patch.model or ""}`)
	end
	lanes.watch = function(slot: string, id: string?)
		if not id then
			return
		end
		watched:set_key({ slot }, id)
		local view = lanes.view(slot)
		for _, c in sample.chats do
			if c.id == id then
				view.session:set({ id = id, state = "ready", device = c.device, harness = "claude-code", model = `claude-code/{c.model}`, running = false })
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
			if op.picks then
				note(`dump-model:{op.picks.model}`)
			end
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
		elseif kind == "startItem" or (kind == "upsertItem" and op.item.state == "active" and op.item.id) then
			-- (The person starting a card with no thread, as the engine
			-- would: its thread made, the card in Now.)
			local id = op.id or op.item.id
			local card: any = nil
			for _, it in orchestration.now:peek().items or {} do
				if it.id == id then
					card = it
				end
			end
			if kind == "startItem" or (card and #(card.threads or {}) == 0) then
				note(`start:{id}:{if op.item then op.item.order else ""}`)
				republish(function(n)
					local items = {}
					for _, it in n.items do
						if it.id == id then
							it = table.clone(it)
							it.state = "active"
							it.order = if op.item then op.item.order else it.order
							if #(it.threads or {}) == 0 then
								it.threads = { `th-start-{id}` }
							end
						end
						table.insert(items, it)
					end
					n.items = items
				end)
				return settled({ id = id, threadId = `th-start-{id}` })
			end
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
	-- A host that writes no progress (an older engine): no rows at all.
	ui.command { id = "fixture.progress_none", title = "Fixture: no progress rows", run = function()
		orchestration.progress:set({})
	end }
	-- A proxy's row (the engine's, for a host that writes none).
	ui.command { id = "fixture.progress_proxy", title = "Fixture: a proxy row", run = function()
		local rows = table.clone(orchestration.progress:peek())
		rows["th-engine-state"] = {
			chat_id = "th-engine-state", device_id = "dev-desk", state = "working", started = os.time() - 900,
			last_step = os.time() - 60, note = "Proxy: running the engine tests.", note_at = os.time() - 30,
			stalled = false, refreshing = false, updated = os.time() - 30, proxy = true,
		}
		orchestration.progress:set(rows)
	end }
	-- Every chat at rest.
	ui.command { id = "fixture.all_idle", title = "Fixture: nothing working", run = function()
		local n: any = table.clone(sessions.now:peek())
		local function rest(list: { any }): { any }
			local out = {}
			for _, c in list do
				c = table.clone(c)
				c.status = nil
				c.activity = nil
				table.insert(out, c)
			end
			return out
		end
		n.chats = rest(n.chats or {})
		local kids = {}
		for parent, list in n.children or {} do
			kids[parent] = rest(list)
		end
		n.children = kids
		sessions.now:set(n)
	end }
	-- A crew thread that ended its turn with a plain question for the person
	-- (the engine raises it as an ask, `origin = "turn_end"`), idle since; its
	-- card on the board. (The real case.)
	ui.command { id = "fixture.turn_end_ask", title = "Fixture: a thread asks at its turn's end", run = function()
		local t = os.time()
		local n: any = table.clone(sessions.now:peek())
		local kids: any = table.clone(n.children or {})
		local list = table.clone(kids["orc-orch"] or {})
		table.insert(list, {
			id = "th-codex-slash", title = "Codex Slash Commands Missing", device = "dev-desk", parent = "orc-orch",
			child_kind = "crew", updated = t - 720, created = t - 3000, agent = "openai", model = "gpt-5.5",
		})
		kids["orc-orch"] = list
		n.children = kids
		sessions.now:set(n)
		republish(function(o)
			local crew = table.clone(o.crew)
			table.insert(crew, { chat_id = "th-codex-slash", orchestrator_id = "orc-orch", item_id = "it-codex-slash", role = "slash commands", released = false, joined = t - 3000 })
			o.crew = crew
			local items = table.clone(o.items)
			table.insert(items, {
				id = "it-codex-slash", orchestrator_id = "orc-orch", title = "Codex slash commands missing", detail = "", state = "active",
				priority = "normal", threads = { "th-codex-slash" }, links = {}, order = 9, created = t - 3000, updated = t - 720,
			})
			o.items = items
			local asks = table.clone(o.asks)
			table.insert(asks, {
				id = "ak-turn", from_chat = "th-codex-slash", orchestrator_id = "orc-orch", item_id = "it-codex-slash", kind = "question",
				question = "Should I commit and open a PR against `zeron/zgx`, or merge it into `zeron/orchestrators`?",
				options = { { label = "Commit and open a PR against zeron/zgx" }, { label = "Merge into zeron/orchestrators" } },
				urgent = false, state = "open", created = t - 150, origin = "turn_end", turn_id = "msg-turn-1",
			})
			o.asks = asks
		end)
	end }
	-- That ask gone stale unanswered: escalated, urgent.
	ui.command { id = "fixture.turn_end_escalate", title = "Fixture: the thread's ask goes stale", run = function()
		republish(function(o)
			local asks = {}
			for _, a in o.asks do
				if a.id == "ak-turn" then
					a = table.clone(a)
					a.escalated = os.time()
					a.urgent = true
				end
				table.insert(asks, a)
			end
			o.asks = asks
		end)
	end }
	-- The Scratchpad (account/scratchpad): the sample's, each edit noted in
	-- `calls` (`set_text:<text>`, `set_shapes:<n>`, `restore:<id>`) and
	-- answered at once with its rev, the watch's frame a moment later (as the
	-- engine's comes: the editor "Syncing" meanwhile).
	local scratchpad = require("@account/scratchpad")
	scratchpad.now:set(sample.scratchpad)
	-- Each revision's text, for a restore (the sample lists only previews).
	local texts: { [string]: string } = {}
	for i, r in sample.scratchpad.history do
		texts[r.id] = if i == 1 then sample.scratchpad.text else `# {r.preview}\n`
	end
	local revs = 41
	local function edited(text: string?, shapes: { any }?, by: string, restored_from: string?): string
		revs += 1
		local rev = `r-{revs}`
		local now = os.time()
		local pad = table.clone(scratchpad.now:peek())
		local history = table.clone(pad.history or {})
		local head = history[1]
		local new_text = text or pad.text
		local new_shapes = shapes or pad.shapes
		local info = { id = `rv-n{revs}`, at = now, edited_at = now, by = by, restored_from = restored_from, chars = utf8.len(new_text) or #new_text, shape_count = #new_shapes, preview = string.match(new_text, "^#*%s*([^\n]*)") or "" }
		-- (Edits by the same editor within a minute fold into its snapshot.)
		if head and head.by == by and not restored_from and now - (head.edited_at or 0) < 60 then
			info.id, info.at = head.id, head.at
			history[1] = info
		else
			table.insert(history, 1, info)
		end
		texts[info.id] = new_text
		ui.after(300, function()
			local p = table.clone(scratchpad.now:peek())
			p.text, p.shapes, p.rev, p.updated_at, p.updated_by, p.history = new_text, new_shapes, rev, now, by, history
			scratchpad.now:set(p)
		end)
		return rev
	end
	local pad_stub = scratchpad :: any
	pad_stub.set_text = function(text: string): any
		note(`set_text:{text}`)
		return settled(edited(text, nil, "person"))
	end
	local sent = ui.state("scratchpad_sent", {} :: { any })
	pad_stub.set_shapes = function(shapes: { any }): any
		note(`set_shapes:{#shapes}`)
		sent:set(shapes)
		return settled(edited(nil, shapes, "person"))
	end
	pad_stub.restore = function(id: string): any
		note(`restore:{id}`)
		return settled(edited(texts[id] or "", nil, "person", id))
	end
	pad_stub.revision = function(id: string): any
		note(`revision:{id}`)
		return settled({ id = id, text = texts[id] or "", shapes = {} })
	end
	-- The Brain dump writing to it (another device's edit comes the same way).
	ui.command { id = "fixture.scratchpad_remote", title = "Fixture: the Brain dump adds to the Scratchpad", run = function()
		local p = scratchpad.now:peek()
		edited(`{p.text}\n\n## From the Brain dump\n\nAsk Wing about the relay window.\n`, nil, "brain_dump")
	end }
	-- Another device adding to the drawing.
	ui.command { id = "fixture.scratchpad_remote_shapes", title = "Fixture: another device draws on the Scratchpad", run = function()
		local list = table.clone(scratchpad.now:peek().shapes or {})
		table.insert(list, { id = "sh-remote", kind = "ellipse", x = 20, y = 20, w = 60, h = 40, points = {}, text = "far", color = "blue", z = 99 })
		edited(nil, list, "other")
	end }
	ui.command { id = "fixture.scratchpad_empty", title = "Fixture: an empty Scratchpad", run = function()
		scratchpad.now:set({ loaded = true, text = "", rev = "", updated_by = "person", shapes = {}, history = {} })
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
