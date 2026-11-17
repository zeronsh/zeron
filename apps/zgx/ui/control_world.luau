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
			-- (Let go: no longer noted, and its transcript gone, as the
			-- host drops it.)
			if watched:peek()[slot] then
				watched:set_key({ slot }, nil)
				-- (A frame on, as the host's answer comes: never under a
				-- transcript still going.)
				ui.after(0, function()
					if not watched:peek()[slot] then
						lanes.view(slot).messages:set({})
					end
				end)
			end
			return
		end
		watched:set_key({ slot }, id)
		local view = lanes.view(slot)
		for _, c in sample.chats do
			if c.id == id then
				view.session:set({ id = id, state = "ready", device = c.device, harness = "claude-code", model = `claude-code/{c.model}`, running = false })
			end
		end
		-- The Brain dump's transcript, as the Control room's Chat tab reads
		-- it: a dump, its routing turn (a tool call each part), a reply.
		if slot == "control-chat" then
			view.messages:set({
				{ id = "bd-u1", role = "user", text = "Moscow node is flaky again, and the board needs a Review column." },
				{
					id = "bd-a1",
					role = "assistant",
					text = "Routed two parts: the Moscow node to VPN rollout, the Review column to UI.",
					tools = {
						{ id = "bd-t1", kind = "tool", verb = "Routed", target = "orc-vpn" },
						{ id = "bd-t2", kind = "tool", verb = "Routed", target = "th-ui-orch" },
					},
				},
				{ id = "bd-u2", role = "user", text = "Also check the relay window with Wing." },
			})
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
	local function rejected(message: string): any
		return {
			after = function(_self: any, _done: any, failed: ((any) -> ())?)
				if failed then
					failed({ kind = "action", message = message })
				end
			end,
		}
	end
	-- An ask the engine marked answered, a beat later (as its state comes).
	local function mark_answered(id: string, answer: string?)
		ui.after(400, function()
			republish(function(n)
				local asks = {}
				for _, a in n.asks do
					if a.id == id then
						a = table.clone(a)
						a.state = "answered"
						a.answer = answer
						a.answered = os.time()
					end
					table.insert(asks, a)
				end
				n.asks = asks
			end)
		end)
	end
	-- Which host and engine answer (`answer_mode`, or the
	-- `fixture.answers_*` commands): "ok", this build's; "refuse", an
	-- engine that turns answers and defers down; "no_host", an app from
	-- before `orchestration.answer_ask`/`defer_ask` on an engine from before
	-- options and `deferAsk` (the ops alone); "old_engine", this app on that
	-- engine (it says an option with no words is empty).
	local mode = ui.state("answer_mode", "ok")
	-- The host's calls (account/orchestration's `host`): `answer_ask`
	-- noted `answer:<id>:<option>:<text>`, `defer_ask`
	-- `defer:<id>:<until in seconds>`, each as the mode has it.
	stub.host = function(name: string, args: any): any
		local m = mode:peek()
		if m == "no_host" then
			error(`no host action named \`{name}\``)
		end
		if name == "orchestration.answer_ask" then
			note(`answer:{args.id}:{args.option or ""}:{args.text or ""}`)
			if m == "refuse" then
				return rejected("That ask was already answered or dismissed")
			end
			if m == "old_engine" then
				local text = if args.text then string.match(args.text, "^%s*(.-)%s*$") else ""
				if text == "" then
					return rejected("An answer can't be empty")
				end
				mark_answered(args.id, text)
				return settled({ id = args.id })
			end
			local label = args.text
			if not label then
				for _, a in orchestration.now:peek().asks or {} do
					if a.id == args.id then
						label = ((a.options or {})[args.option or 0] or {}).label
					end
				end
			end
			mark_answered(args.id, label)
			return settled({ id = args.id })
		elseif name == "orchestration.defer_ask" then
			local secs = args["until"]
			note(`defer:{args.id}:{secs or ""}`)
			if m == "refuse" then
				return rejected("That ask was already answered or dismissed")
			elseif m == "old_engine" then
				return rejected("unknown variant `deferAsk`")
			end
			republish(function(n)
				local asks = {}
				for _, a in n.asks do
					if a.id == args.id then
						a = table.clone(a)
						a.deferred_until = secs
					end
					table.insert(asks, a)
				end
				n.asks = asks
			end)
			return settled({ id = args.id })
		end
		return settled(nil)
	end
	for _, m in { "ok", "refuse", "no_host", "old_engine" } do
		ui.command { id = `fixture.answers_{m}`, title = `Fixture: answers go to the {m} host`, run = function()
			mode:set(m)
		end }
	end
	-- A thread opened, noted `open:<chat>:<message>` (then opened).
	local route = require("@app/screens/shell/route") :: any
	local open_session = route.open_session
	route.open_session = function(id: string, opts: any?)
		note(`open:{id}:{if opts and opts.message then opts.message else ""}`)
		return open_session(id, opts)
	end
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
			-- (What it references, ds/orch_ref; the op as sent in `last_dump`.)
			for _, r in op.refs or {} do
				note(`dump-ref:{r.kind}:{r.id}`)
			end
			ui.state("last_dump", nil :: any):set(op)
			local id = `dp-new-{#calls:peek()}`
			republish(function(n)
				local dumps = table.clone(n.dumps)
				table.insert(dumps, 1, { id = id, text = op.text, state = "routing", routes = {}, device = "dev-desk", created = os.time() })
				n.dumps = dumps
			end)
			return settled({ id = id })
		elseif kind == "answerAsk" then
			-- (The op an older app sends, its answer plain words.)
			note(`answer-op:{op.id}:{op.answer}`)
			if mode:peek() == "refuse" then
				return rejected("That ask was already answered or dismissed")
			end
			mark_answered(op.id, op.answer)
			return settled(nil)
		elseif kind == "deferAsk" then
			note(`defer-op:{op.id}:{op["until"] or ""}`)
			-- (An engine from before it.)
			if mode:peek() == "no_host" or mode:peek() == "old_engine" then
				return rejected("unknown variant `deferAsk`, expected one of `createOrchestrator`, `answerAsk`, `dismissAsk`")
			end
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
	-- Two files carried on the Brain dump's draft (Capture's chips).
	ui.command { id = "fixture.carry", title = "Fixture: files on Capture's draft", run = function()
		require("@app/screens/chat/shared").attach_files({
			{ name = "notes.md", path = "/Users/dev/notes.md" },
			{ name = "plan.txt", path = "/Users/dev/plan.txt" },
		}, "orc-brain")
	end }
	-- The Control room's run counts (ds/orch_perf_trace): reset, and read
	-- into `perf_counts`; and ds/stage's background pieces still waiting,
	-- into `stage_idle_waiting`.
	ui.command { id = "fixture.perf_reset", title = "Fixture: reset the run counts", run = function()
		require("@ds/orch_perf_trace").reset()
	end }
	ui.command { id = "fixture.perf_read", title = "Fixture: read the run counts", run = function()
		ui.state("perf_counts", {} :: any):set(table.clone(require("@ds/orch_perf_trace").counts))
		ui.state("stage_idle_waiting", 0):set(require("@ds/stage").idle_waiting())
	end }
	-- Everything the room shows changing at once (the board's items, every
	-- chat's sessions, a report, the progress rows): a push of each.
	-- The orchestration sent whole again, every item a new table, one of
	-- them (`it-moscow`) retitled: as a push that changes one card.
	ui.command { id = "fixture.push_one", title = "Fixture: a push that changes one card", run = function()
		local n = table.clone(orchestration.now:peek())
		local items = {}
		for _, it in n.items or {} do
			local c = table.clone(it)
			if c.id == "it-moscow" then
				c.title = "Reboot Moscow node tonight"
			end
			table.insert(items, c)
		end
		n.items = items
		orchestration.now:set(n)
	end }
	ui.command { id = "fixture.push_all", title = "Fixture: a push of everything", run = function()
		local n = table.clone(orchestration.now:peek())
		local items = {}
		for _, it in n.items or {} do
			local c = table.clone(it)
			c.updated = (c.updated or 0) + 1
			table.insert(items, c)
		end
		n.items = items
		orchestration.now:set(n)
		local s: any = table.clone(sessions.now:peek())
		local chats = {}
		for _, c in s.chats or {} do
			local c2 = table.clone(c)
			c2.updated = (c2.updated or 0) + 1
			table.insert(chats, c2)
		end
		s.chats = chats
		sessions.now:set(s)
		local m = #orchestration.comms:peek()
		orchestration.comms:push({ id = `cm-p{m}`, kind = "report", from = "th-engine-state", to = "orc-orch", summary = "pushed", at = os.time() })
		orchestration.progress:set(table.clone(orchestration.progress:peek() or {}))
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
	-- (An engine without the Scratchpad: every op refused, as the host's
	-- call to it is: `fixture.scratchpad_no_engine`.)
	local no_engine = false
	local function refused(): any
		return {
			after = function(_self: any, _done: any, failed: ((any) -> ())?)
				if failed then
					failed("unknown method: Scratchpad")
				end
			end,
		}
	end
	pad_stub.set_text = function(text: string): any
		note(`set_text:{text}`)
		if no_engine then
			return refused()
		end
		return settled(edited(text, nil, "person"))
	end
	local sent = ui.state("scratchpad_sent", {} :: { any })
	pad_stub.set_shapes = function(shapes: { any }): any
		note(`set_shapes:{#shapes}`)
		if no_engine then
			return refused()
		end
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
	-- The engine without the Scratchpad (the host's watch refused), and an
	-- app without its host module (only the scripts' default); then the
	-- engine with it after all, its first copy coming.
	ui.command { id = "fixture.scratchpad_no_engine", title = "Fixture: an engine without the Scratchpad", run = function()
		no_engine = true
		scratchpad.now:set({ host = true, missing = "engine", loaded = false, text = "", rev = "", updated_by = "person", shapes = {}, history = {} })
	end }
	ui.command { id = "fixture.scratchpad_no_host", title = "Fixture: an app without the Scratchpad", run = function()
		no_engine = true
		scratchpad.now:set({ host = false, loaded = false, text = "", rev = "", updated_by = "person", shapes = {}, history = {} })
	end }
	ui.command { id = "fixture.scratchpad_engine_back", title = "Fixture: the engine has the Scratchpad again", run = function()
		no_engine = false
		local p = table.clone(sample.scratchpad)
		p.host = true
		scratchpad.now:set(p)
	end }
	ui.command { id = "fixture.scratchpad_empty", title = "Fixture: an empty Scratchpad", run = function()
		scratchpad.now:set({ loaded = true, text = "", rev = "", updated_by = "person", shapes = {}, history = {} })
	end }
	-- References dragged onto Capture (ds/orch_ref): a card, a thread and a
	-- machine as chips on the Brain dump's draft (`fixture.refs_chips`), and
	-- another on its way, over the composer: "Drop to reference"
	-- (`fixture.drag_refs`).
	local function ref_chips()
		local Shared = require("@app/screens/chat/shared") :: any
		local Ref = require("@ds/orch_ref")
		Shared.mention_ref(Ref.make("card", "it-moscow", "Reboot Moscow node", { state = "review", orchestrator = "orc-vpn" }), "orc-brain")
		Shared.mention_ref(Ref.make("chat", "th-vpn-merge", "Merge phase-1 fixes", { orchestrator = "orc-vpn", device = "dev-desk" }), "orc-brain")
		Shared.mention_ref(Ref.make("device", "dev-mac", "macbook", { os = "macos" }), "orc-brain")
	end
	ui.command { id = "fixture.refs_chips", title = "Fixture: references on Capture", run = ref_chips }
	ui.command { id = "fixture.drag_refs", title = "Fixture: a reference dragged over Capture", run = function()
		ref_chips()
		local Shared = require("@app/screens/chat/shared") :: any
		Shared.dropping_own:set_key({ "lane.control-capture" }, "ref")
	end }
	-- A thread asking the person to pick between two variants, with
	-- pictures, scores, a recommendation, context and links; its card on
	-- the board (the decision UI's whole case).
	-- The engine's state again, every ask and session a fresh copy (as its
	-- watch sends the whole of it), nothing changed.
	ui.command { id = "fixture.republish", title = "Fixture: the engine publishes its state again", run = function()
		republish(function(n)
			local asks = {}
			for _, a in n.asks or {} do
				table.insert(asks, table.clone(a))
			end
			n.asks = asks
			n.items = table.clone(n.items or {})
		end)
		sessions.now:set(table.clone(sessions.now:peek()))
	end }
	-- The engine's state for a moment without its asks (an engine coming
	-- up again publishes before it has read them back), then whole again.
	ui.command { id = "fixture.asks_blip", title = "Fixture: the asks gone a moment", run = function()
		local before = orchestration.now:peek()
		republish(function(n)
			n.asks = {}
		end)
		ui.after(2000, function()
			orchestration.now:set(table.clone(before))
		end)
	end }
	ui.command { id = "fixture.decision", title = "Fixture: a decision with pictures", run = function()
		for _, a in orchestration.now:peek().asks or {} do
			if a.id == "ak-topo" then
				return
			end
		end
		local t = os.time()
		local n: any = table.clone(sessions.now:peek())
		local kids: any = table.clone(n.children or {})
		local list = table.clone(kids["orc-orch"] or {})
		table.insert(list, {
			id = "th-topo", title = "Topology lines", device = "dev-desk", parent = "orc-orch",
			child_kind = "crew", updated = t - 29000, created = t - 40000, agent = "claude", model = "claude-opus-5-5",
		})
		kids["orc-orch"] = list
		n.children = kids
		sessions.now:set(n)
		republish(function(o)
			local crew = table.clone(o.crew)
			table.insert(crew, { chat_id = "th-topo", orchestrator_id = "orc-orch", item_id = "it-topo", role = "topology", released = false, joined = t - 40000 })
			o.crew = crew
			local items = table.clone(o.items)
			table.insert(items, {
				id = "it-topo", orchestrator_id = "orc-orch", title = "Topology lines: pick a variant", detail = "", state = "active",
				priority = "normal", threads = { "th-topo" }, links = {}, order = 0, created = t - 40000, updated = t - 29000,
			})
			o.items = items
			local asks = table.clone(o.asks)
			table.insert(asks, {
				id = "ak-topo", from_chat = "th-topo", orchestrator_id = "orc-orch", item_id = "it-topo", kind = "decision",
				question = "Topology lines: which variant ships, **solid lead lines** or **dashed crew lines**?",
				context = "Both are built on `orch/topo-variants` (c1e090c7e). Each draws the same honeypot, Windows and M2A machines.\n\n- **Solid** keeps one ink per orchestrator and reads at a glance.\n- **Dashed** separates crew from leads, but gets busy past five threads.",
				options = {
					{ label = "Solid lead lines", detail = "One stroke per orchestrator; crew threads hang off it.", images = { "assets/decision/topology-a.png" }, scores = { { name = "Clarity", value = 9 }, { name = "Density", value = 7 }, { name = "Effort", value = "S" } } },
					{ label = "Dashed crew lines", detail = "Crew threads dashed, leads solid; the Brain dump's own lane.", images = { "assets/decision/topology-b.png" }, scores = { { name = "Clarity", value = 7 }, { name = "Density", value = 9 }, { name = "Effort", value = "M" } } },
					{ label = "Keep both behind a switch", detail = "Ship solid, keep dashed under Map → View." },
				},
				recommendation = { option = 1, why = "Reads at a glance at every crew size, and it's the smaller change." },
				links = { { kind = "pr", label = "#1312", target = "https://github.com/zeronsh/zeron/pull/1312" }, { kind = "branch", label = "orch/topo-variants", target = "orch/topo-variants" } },
				images = {},
				urgent = false, state = "open", created = t - 29000, origin = "tool", turn_id = "msg-topo-ask",
			})
			o.asks = asks
		end)
	end }
	-- A question whose context carries an email to send, quoted, and a
	-- command (`fixture.decision_email`): what the person copies out.
	ui.command { id = "fixture.decision_email", title = "Fixture: a question with an email to send", run = function()
		for _, a in orchestration.now:peek().asks or {} do
			if a.id == "ak-mail" then
				return
			end
		end
		local t = os.time()
		republish(function(o)
			local asks = table.clone(o.asks)
			table.insert(asks, {
				id = "ak-mail", from_chat = "orc-vpn", orchestrator_id = "orc-vpn", kind = "question",
				question = "Could you send this support request to mhost.ee about the German server's outbound connections dropping every 20–30 minutes?",
				context = table.concat({
					"**Root cause of tonight's outages:** de-mhost loses the ability to open new **outbound** TCP connections for about 3 minutes, every 20–30 minutes. Text to send (from your mhost account):",
					"",
					"> Hello. Our VPS 103.227.84.140 periodically loses the ability to open **new outbound TCP connections** for about 2–3 minutes, roughly every 20–30 minutes.",
					">",
					"> 1. Do you apply any rate limit or anti-abuse rule to outbound TCP connections from VMs?",
					"> 2. If so, what are the thresholds, and can it be raised for this VM?",
					">",
					"> Thank you.",
					"",
					"To check from the server:",
					"",
					"```sh",
					"ss -s | head -3",
					"```",
				}, "\n"),
				options = {
					{ label = "Sent", detail = "I'll paste their answer here when it comes" },
					{ label = "Later", detail = "Keep it parked; failover work continues regardless" },
				},
				links = {}, images = {},
				urgent = false, state = "open", created = t - 30, origin = "tool", turn_id = "msg-mail-ask",
			})
			o.asks = asks
		end)
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
