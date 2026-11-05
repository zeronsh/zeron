-- The chat page on its own, with a made-up session and no engine: what
-- tests/chat_*.test.luau drive. The host's calls (screens/chat/store) are
-- stubbed to change this data as the engine would; `ui.command`s stand in
-- for what the engine does on its own (a reply streaming, a turn ending,
-- an approval asked). Here, beside app.luau, as a mounted script loads
-- modules from its own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local store = require("@app/screens/chat/store")
local route = require("@app/screens/shell/route")
local chat = require("@app/screens/chat")
local sessions = require("@account/sessions")
local files = require("@app/screens/files/api")
-- Cloud for the new chat's Work in (none until `cloud_stub.offer`).
require("./cloud_stub")

local EFFORT = {
	{ value = "low", label = "Low" },
	{ value = "medium", label = "Medium" },
	{ value = "high", label = "High" },
}
store.catalog:set {
	state = "ready",
	harnesses = { { id = "claude-code", label = "Claude", mark = "claude" } },
	models = {
		{ value = "opus", label = "Opus", harness = "claude-code", efforts = EFFORT, effort = "high" },
		{ value = "sonnet", label = "Sonnet", harness = "claude-code", efforts = EFFORT, effort = "medium" },
		-- A model configured in place (Devin Fusion's shape): its lead and
		-- sidekick are options.
		{
			value = "fusion",
			label = "Fusion",
			harness = "claude-code",
			efforts = EFFORT,
			effort = "high",
			fast = true,
			options = {
				{ id = "lead", label = "Lead", default = "fable", choices = { { id = "fable", label = "Fable 5.1" }, { id = "sol", label = "GPT-6 Sol" } } },
				{ id = "sidekick", label = "Sidekick", default = "swe-medium", choices = { { id = "swe-medium", label = "SWE-2 Medium" }, { id = "swe-high", label = "SWE-2 High" } } },
			},
		},
	},
}

-- Two sessions' transcripts: a long one and a short one.
local function long(n: number): { any }
	local out = {}
	for i = 1, n do
		table.insert(out, { id = `u{i}`, role = "user", text = `Question number {i}: how does part {i} work?` })
		table.insert(out, {
			id = `a{i}`,
			role = "assistant",
			text = `Part {i} works like this.\n\nIt reads its input, then it writes **its answer**, then it stops. That's the whole of part {i}.`,
			model = "Opus",
			-- (Each its own minute, so a lane can be told from another.)
			at = `Sep 8, 4:{10 + i} PM`,
			copy = `Part {i} works like this.`,
			tools = { { id = `t{i}`, kind = "read", verb = "Read", target = `part{i}.rs` } },
		})
	end
	return out
end
local TRANSCRIPTS: { [string]: { any } } = {
	s1 = long(30),
	s2 = { { id = "x1", role = "user", text = "Hello there" }, { id = "x2", role = "assistant", text = "Hi! What shall we work on?", model = "Opus" } },
}
local META: { [string]: any } = {
	s1 = { title = "Long chat", project = "atlas", model = "opus", effort = "high", access = "full", plan = false },
	s2 = { title = "Short chat", project = "atlas", model = "sonnet", effort = "medium", access = "edits", plan = false },
}

-- What the page asked of the host, in order (`calls`).
local said: { string } = {}
local calls = ui.state("calls", "")
local function note(what: string)
	table.insert(said, what)
	calls:set(table.concat(said, ","))
end

local function publish(id: string, patch: { [string]: any }?)
	local s = table.clone(META[id] or {})
	s.id = id
	s.state = if META[id] then "ready" else "missing"
	for k, v in patch or {} do
		s[k] = v
	end
	META[id] = s
	store.session:set(s)
end

local stub = store :: any
-- The open session with a dev server running in the background.
ui.command {
	id = "task_running",
	title = "A background process runs",
	run = function()
		local s = store.session:peek()
		if s and s.id then
			publish(s.id, { tasks = { { id = "bg1", kind = "process", name = "npm run dev", detail = ":5173", since = os.time() } } })
		end
	end,
}
-- The open session, as if it ran on the box (a remote device that's up),
-- or on the old laptop (one gone quiet).
for _, d in { "box", "old" } do
	ui.command {
		id = `session_on_{d}`,
		title = `The open session runs on {d}`,
		run = function()
			local s = store.session:peek()
			if s and s.id then
				publish(s.id, { device = d })
			end
		end,
	}
end
-- The chat open, kept across a reload as the host keeps it (the fixture
-- runs again on one; the host's transcript doesn't start over).
local kept_open = ui.state("fixture_opened", nil :: string?)
local opened: string? = kept_open:peek()
stub.open = function(id: string?)
	-- (As the host: the chat open already, nothing changes.)
	if id == opened then
		return
	end
	opened = id
	kept_open:set(id)
	note(`open:{id or "none"}`)
	if not id then
		return
	end
	store.messages:set(TRANSCRIPTS[id] or {})
	store.draft:set { id = "", text = "" }
	publish(id)
end
-- A promise settled at once.
local function settled(value: any): any
	return {
		after = function(self: any, ok: (any) -> (), _err: any)
			ok(value)
		end,
	}
end
stub.send = function(req: any)
	note(`send:{req.session or "new"}:{req.text}`)
	for _, a in req.attachments or {} do
		note(`att:{a.path}|{if a.on_host then "host" else "upload"}`)
	end
	-- (An Appshot's: what the host writes after the words.)
	for _, a in req.appshots or {} do
		note(`appshot:{a.app}|{a.title or "-"}|{a.path}|{if a.text then "text" else "-"}`)
	end
	-- Sent with a pick (another agent, model or effort): the run takes it,
	-- and the session with it, as the host does once the run is sent.
	if req.session and not req.fresh and (req.model or req.effort or req.fast ~= nil) then
		note(`took:{req.session}:{req.harness or "-"}|{req.model or "-"}|{req.effort or "-"}`)
		local patch: { [string]: any } = {}
		for _, k in { "harness", "model", "effort", "fast" } do
			if req[k] ~= nil then
				patch[k] = req[k]
			end
		end
		publish(req.session, patch)
	end
	if req.fresh then
		note(`run:{req.cwd or "-"}|{if req.worktree then "worktree" else "local"}|{req.branch or "-"}`)
	end
	if req.fresh then
		TRANSCRIPTS[req.session] = { { id = req.message_id or `{req.session}u`, role = "user", text = req.text } }
		-- (As the host: a prompt to the chat open shows at once.)
		if opened == req.session then
			store.messages:set(TRANSCRIPTS[req.session])
		end
	end
	return settled(nil)
end
stub.retry = function(id: string, message: string)
	note(`retry:{id}:{message}`)
end
-- Each project's branches, as git where it lives would list them (the
-- box's asked of the box).
local REFS: { [string]: { any } } = {
	["/tmp/atlas"] = {
		{ name = "main", current = true },
		{ name = "fix/render-loop", current = false, worktree = "/tmp/atlas-wt/render-loop" },
		{ name = "release/2.4", current = false },
	},
	["/srv/borealis"] = {
		{ name = "trunk", current = true },
		{ name = "feature/sync", current = false },
	},
}
-- The clipboard's picture, written out (noted): a picture of the app's own
-- here, or none after `clipboard_empty`.
local CLIPBOARD_IMAGE = ui.state("fixture_clipboard_image", true)
stub.paste_image = function()
	note("paste_image")
	return settled(if CLIPBOARD_IMAGE:peek() then "assets/favicons/zgx.png" else nil)
end
ui.command {
	id = "clipboard_empty",
	title = "The clipboard holds no picture",
	run = function()
		CLIPBOARD_IMAGE:set(false)
	end,
}
-- The open session working in a worktree of Atlas, where the agent has
-- since switched to a branch of its own (the chat still names main).
REFS["/tmp/atlas-wt/pr"] = {
	{ name = "main", current = false },
	{ name = "zeron/readme-pr", current = true, worktree = "/tmp/atlas-wt/pr" },
}
ui.command {
	id = "session_in_worktree",
	title = "The open session works in a worktree, on its own branch",
	run = function()
		local s = store.session:peek()
		if s and s.id then
			publish(s.id, { cwd = "/tmp/atlas-wt/pr", branch = "main" })
		end
	end,
}
stub.refs = function(path: string, device: string?)
	note(`refs:{path}:{device or "here"}`)
	return settled(REFS[path] or {})
end
ui.command {
	id = "undelivered",
	title = "A send wasn't delivered",
	run = function()
		store.messages:push { id = "lost1", role = "user", text = "Lost on the way", waiting = "failed" }
	end,
}
stub.interrupt = function(id: string)
	note(`interrupt:{id}`)
end
-- A background process stopped (noted); `stop_fails` makes the next fail.
local STOP_FAILS = ui.state("fixture_stop_fails", false)
stub.stop_task = function(session: string, task: string): any
	note(`stop_task:{session}:{task}`)
	if STOP_FAILS:peek() then
		STOP_FAILS:set(false)
		return {
			after = function(_self: any, _ok: any, err: (any) -> ())
				err("failed")
			end,
		}
	end
	return settled(true)
end
ui.command {
	id = "stop_fails",
	title = "The next Stop fails",
	run = function()
		STOP_FAILS:set(true)
	end,
}
stub.configure = function(id: string, patch: any)
	local keys = {}
	for k, v in patch do
		table.insert(keys, `{k}={tostring(v)}`)
	end
	table.sort(keys)
	note(`configure:{id}:{table.concat(keys, ";")}`)
	publish(id, patch)
end
stub.approve = function(id: string, request: string, scope: string)
	note(`approve:{id}:{request}:{scope}`)
	publish(id, { ask = ui.null })
end
stub.deny = function(id: string, request: string)
	note(`deny:{id}:{request}`)
end
stub.answer = function() end
stub.queue = function() end
-- One queued message, editable here.
stub.queue_edit = function(id: string, item: string): any
	note(`queue_edit:{id}:{item}`)
	local s = table.clone(META[id] or {})
	local q = table.clone(s.queue or {})
	for i, row in q do
		if row.id == item then
			q[i] = { id = row.id, text = row.text, editing = true }
		end
	end
	publish(id, { queue = q })
	return settled({ text = "Queued words" })
end
stub.queue_finish = function(id: string, item: string, action: string, text: string?): any
	note(`queue_finish:{id}:{item}:{action}:{text or ""}`)
	publish(id, { queue = { { id = item, text = if action == "commit" then text else "Queued words" } } })
	return settled(nil)
end
ui.command {
	id = "queue_one",
	title = "A message waits in the queue",
	run = function()
		if opened then
			publish(opened, { queue = { { id = "q1", text = "Queued words" } } })
		end
	end,
}
stub.load_catalog = function() end
-- (The device whose catalog the composer reads, noted as it changes.)
stub.catalog_on = function(device: string?)
	note(`catalog_on:{device or "here"}`)
end

-- The session's files, searched for `@`.
local FILES = { "src/chat.rs", "src/main.rs", "ui/screens/chat/composer.luau", "README.md" }
local files_stub = files :: any
files_stub.search = function(target: any, query: string): any
	note(`search:{target.chat or target.space or "?"}:{query}`)
	local found = {}
	for _, path in FILES do
		if query == "" or string.find(string.lower(path), string.lower(query), 1, true) then
			table.insert(found, { path = path, dir = false })
		end
	end
	return settled(found)
end

-- The agent's `/` commands and `$` skills, as the host lists them
-- (screens/chat/invocations), each read noted.
local Invocations = require("@app/screens/chat/invocations")
local invocations_stub = Invocations :: any
invocations_stub.read = function(harness: string, target: any): any
	note(`invocations:{harness}:{target.chat or target.space or "?"}`)
	return settled({
		rows = {
			{ kind = "command", name = "review", label = "/review", detail = "Review the diff · <branch>", link = "[/review](zeron-invoke:7b226b696e64223a22636f6d6d616e64222c226e616d65223a22726576696577227d)" },
			{ kind = "command", name = "compact", label = "/compact", detail = "Summarize the conversation", link = "[/compact](zeron-invoke:7b7d)" },
			-- (The agent's own `/new`: Zeron's is `/zeron:new` beside it.)
			{ kind = "command", name = "new", label = "/new", detail = "The agent's new", link = "[/new](zeron-invoke:7b7d)" },
			{ kind = "skill", name = "code-review", label = "Code Review", detail = "Careful review — /s/code-review/SKILL.md", link = "[$code-review](zeron-invoke:7b7d)" },
		},
	})
end

-- The engine's copy of the prompt just sent landing: the same message (its
-- id), a new table, as the host publishes it.
ui.command {
	id = "engine_copy",
	title = "The engine's copy of the prompt lands",
	run = function()
		local all = store.messages:peek()
		local list = table.clone(all)
		local last = list[#list]
		if last and last.role == "user" then
			-- (Still sending: the session's turn hasn't begun.)
			list[#list] = { id = last.id, role = "user", text = last.text, attachments = {}, waiting = "sending" }
			store.messages:set(list)
		end
	end,
}
-- The engine starts the turn: the newest prompt's line turns to working.
ui.command {
	id = "turn_starts",
	title = "The turn begins",
	run = function()
		local n = store.messages:len()
		local last = store.messages:peek()[n]
		if last and last.role == "user" then
			ui.batch(function()
				store.messages:set_key({ n, "waiting" }, "working")
				store.messages:set_key({ n, "since" }, ui.now() / 1000)
			end)
		end
	end,
}

-- The engine's side: a reply streaming into the open session, word by word.
local REPLY = "Here is a longer answer, written a few words at a time so the transcript grows in place while you read it. It keeps going for a while."
local shown = 0
ui.command {
	id = "start_reply",
	title = "A reply starts",
	run = function()
		if not opened then
			return
		end
		shown = 0
		-- The prompt's line gives way to the reply's own.
		local n = store.messages:len()
		local prompt = store.messages:peek()[n]
		if prompt and prompt.role == "user" and prompt.waiting then
			store.messages:set_key({ n, "waiting" }, ui.null)
		end
		store.messages:push {
			id = "live",
			role = "assistant",
			text = "",
			model = "Opus",
			streaming = true,
			started = ui.now() / 1000,
			tools = {},
			-- (A host sending its time early, as an older one did: no lane
			-- shows it until the reply is done.)
			at = "Sep 8, 5:00 PM",
			copy = "early",
		}
		store.draft:set { id = "live", text = "" }
		publish(opened, { running = true })
	end,
}
ui.command {
	id = "stream_words",
	title = "The reply writes a few words",
	run = function()
		local stop = string.find(REPLY, " ", shown + 12) or #REPLY
		shown = stop
		store.draft:set { id = "live", text = string.sub(REPLY, 1, shown) }
	end,
}
ui.command {
	id = "add_tool",
	title = "The reply runs a tool",
	run = function()
		local n = store.messages:len()
		local m = store.messages:peek()[n]
		if not m or m.role ~= "assistant" then
			return
		end
		local tools = table.clone(m.tools or {})
		table.insert(tools, { id = `lt{#tools + 1}`, kind = "exec" :: any, verb = "Ran", target = "cargo test" })
		store.messages:set_key({ n, "tools" }, tools)
	end,
}
ui.command {
	id = "end_reply",
	title = "The turn ends",
	run = function()
		local n = store.messages:len()
		if n == 0 then
			return
		end
		ui.batch(function()
			store.messages:set_key({ n, "text" }, store.draft:peek().text)
			store.messages:set_key({ n, "streaming" }, false)
			store.draft:set { id = "", text = "" }
		end)
		if opened then
			publish(opened, { running = false })
		end
	end,
}
ui.command {
	id = "ask_approval",
	title = "The agent asks to run a command",
	run = function()
		if opened then
			publish(opened, {
				ask = {
					request = "r1",
					kind = "approval",
					approval = { title = "Run this command?", command = "rm -rf build", cwd = "~/atlas" },
				},
			})
		end
	end,
}

-- The theme, for captures in both.
ui.command { id = "theme_light", title = "Light theme", run = function() ui.theme.follow_system(false); ui.theme.current:set("zeron-light") end }
ui.command { id = "theme_dark", title = "Dark theme", run = function() ui.theme.follow_system(false); ui.theme.current:set("zeron-dark") end }
ui.command { id = "opaque", title = "Opaque surfaces", run = function() require("@ds/material").frosted:set(false) end }
ui.command { id = "frosted", title = "Frosted surfaces", run = function() require("@ds/material").frosted:set(true) end }

-- A passage of a reply attached, as the transcript's selection Attach does.
ui.command {
	id = "attach_passage",
	title = "A reply's passage attached",
	run = function()
		chat.shared.attach_quote {
			text = "Layout dominates on both machines, so that is where the work should go first.",
			who = "GPT-5.5",
			from = "Sep 8, 1:43 PM",
		}
	end,
}
ui.command {
	id = "ask_question",
	title = "The agent asks a question",
	run = function()
		if opened then
			publish(opened, {
				ask = {
					request = "q1",
					kind = "question",
					question = {
						header = "Question",
						text = "Which branch should the fix go on?",
						options = {
							{ value = "main", label = "main", detail = "Straight onto the default branch." },
							{ value = "fix", label = "A new branch", detail = "fix/render-loop, opened as a PR." },
						},
					},
				},
			})
		end
	end,
}

-- The same question as the second of two pages (the first answered).
ui.command {
	id = "ask_question_page2",
	title = "The agent's question, on its second page",
	run = function()
		if opened then
			publish(opened, {
				ask = {
					request = "q2",
					kind = "question",
					question = {
						header = "Question",
						text = "And which reviewer?",
						page = 2,
						pages = 2,
						options = {
							{ value = "ada", label = "Ada", detail = "Knows the render loop." },
							{ value = "alan", label = "Alan", detail = "Knows the host." },
						},
					},
				},
			})
		end
	end,
}
-- Back a page (noted): what the first page had picked.
stub.question_back = function(id: string, request: string): any
	note(`question_back:{id}:{request}`)
	-- (The host shows the first page again.)
	if opened then
		publish(opened, {
			ask = {
				request = request,
				kind = "question",
				question = {
					header = "Question",
					text = "Which branch should the fix go on?",
					page = 1,
					pages = 2,
					options = {
						{ value = "main", label = "main", detail = "Straight onto the default branch." },
						{ value = "fix", label = "A new branch", detail = "fix/render-loop, opened as a PR." },
					},
				},
			},
		})
	end
	return settled({ "fix" })
end

-- The registry icons the app has heard of (src/agents.rs `agent_icons`):
-- Factory Droid's, as the registry draws it.
local ICONS = {
	-- (Factory Droid's icon as the registry serves it.)
	["acp.factory-droid"] = [==[<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 900 900" fill="none"><path fill="currentColor" d="M622.037 192.524a10.58 10.58 0 0 1-4.056-2.001 10.573 10.573 0 0 1-3.98-7.89 10.595 10.595 0 0 1 .804-4.452c20.214-49.194 29.134-88.555 14.74-105.033-38.123-43.716-191.004 43.219-239.75 72.663a10.596 10.596 0 0 1-10.841.041 10.576 10.576 0 0 1-4.396-5.07c-20.491-49.089-42.031-83.233-63.865-84.714-57.871-3.96-104.516 165.624-118.17 220.895a10.574 10.574 0 0 1-1.993 4.057 10.572 10.572 0 0 1-7.884 3.977 10.586 10.586 0 0 1-4.447-.802c-49.194-20.215-88.572-29.134-105.033-14.74-43.717 38.123 43.202 191.005 72.644 239.75a10.553 10.553 0 0 1 1.454 4.279 10.573 10.573 0 0 1-6.481 10.958c-49.072 20.491-83.216 42.031-84.715 63.864-3.944 57.871 165.624 104.516 220.913 118.171a10.566 10.566 0 0 1 5.658 3.601 10.57 10.57 0 0 1 2.355 6.281 10.58 10.58 0 0 1-.799 4.442c-20.214 49.194-29.134 88.572-14.739 105.033 38.121 43.717 191.021-43.202 239.767-72.644a10.566 10.566 0 0 1 15.238 5.027c20.489 49.072 42.013 83.216 63.862 84.715 57.871 3.944 104.516-165.624 118.153-220.913a10.603 10.603 0 0 1 9.895-8.021 10.566 10.566 0 0 1 4.449.808c49.193 20.214 88.553 29.116 105.032 14.738 43.718-38.121-43.219-191.022-72.663-239.767a10.602 10.602 0 0 1 1.326-12.655 10.604 10.604 0 0 1 3.703-2.582c49.089-20.49 83.233-42.031 84.714-63.863 3.96-57.871-165.624-104.516-220.895-118.153Zm-66.394-55.478c11.123 19.938-46.196 152.797-88.83 245.724a8.36 8.36 0 0 1-8.239 4.855 8.374 8.374 0 0 1-7.412-6.043c-17.219-60.42-36.899-131.411-57.957-191.675a10.557 10.557 0 0 1 4.924-12.759c52.586-28.721 142.568-66.86 157.514-40.102ZM303.635 153.49c21.953 6.233 75.365 140.709 110.92 236.564a8.364 8.364 0 0 1-2.394 9.249 8.364 8.364 0 0 1-9.504.978c-54.943-30.493-119.013-66.824-176.522-94.546a10.565 10.565 0 0 1-5.528-12.501c16.926-57.44 53.532-148.095 83.028-139.744ZM137.064 343.322c19.921-11.123 152.795 46.197 245.707 88.83a8.369 8.369 0 0 1-1.189 15.652c-60.401 17.219-131.411 36.899-191.675 57.957a10.552 10.552 0 0 1-12.742-4.925c-28.668-52.584-66.876-142.568-40.101-157.514Zm16.443 252.009c6.217-21.953 140.709-75.365 236.564-110.921a8.368 8.368 0 0 1 10.227 11.898c-30.511 54.945-66.842 119.014-94.563 176.507a10.548 10.548 0 0 1-5.229 5.075 10.544 10.544 0 0 1-7.271.468c-57.441-16.822-148.095-53.531-139.728-83.027ZM343.34 761.902c-11.14-19.922 46.197-152.796 88.829-245.707a8.383 8.383 0 0 1 5.713-4.66 8.38 8.38 0 0 1 7.182 1.664 8.371 8.371 0 0 1 2.758 4.184c17.217 60.403 36.898 131.412 57.957 191.675a10.558 10.558 0 0 1-4.942 12.743c-52.568 28.668-142.568 66.875-157.445 40.101h-.052Zm252.009-16.443c-21.971-6.216-75.383-140.709-110.939-236.564a8.37 8.37 0 0 1 11.916-10.228c54.926 30.494 119.014 66.842 176.506 94.563a10.538 10.538 0 0 1 5.527 12.502c-16.909 57.526-53.515 148.094-83.01 139.727Zm166.57-189.834c-19.938 11.141-152.796-46.197-245.724-88.83a8.367 8.367 0 0 1-2.993-12.892 8.378 8.378 0 0 1 4.182-2.759c60.419-17.218 131.41-36.899 191.675-57.958a10.577 10.577 0 0 1 12.758 4.943c28.652 52.568 66.86 142.569 40.102 157.496Zm-16.444-252.009c-6.232 21.971-140.709 75.383-236.562 110.94a8.371 8.371 0 0 1-10.229-11.917c30.495-54.926 66.825-119.012 94.547-176.505a10.555 10.555 0 0 1 12.5-5.528c57.441 16.909 148.096 53.516 139.744 83.01Z"/></svg>]==],
}
local agent_icons = ui.shared("agent_icons", {} :: { [string]: string })
agent_icons:set(ICONS)
-- (Not heard of yet, then heard of.)
ui.command { id = "forget_icons", title = "Forget the registry icons", run = function()
	agent_icons:set({})
end }
ui.command { id = "learn_icons", title = "Hear of the registry icons", run = function()
	agent_icons:set(ICONS)
end }

-- The shell's sessions: one project, two chats, this Mac.
local function seed()
	local chats: { any } = {}
	for id, m in META do
		table.insert(chats, { id = id, title = m.title or id, project = m.project, device = "mac", updated = 0, created = 0, agent = m.agent })
	end
	table.sort(chats, function(a, b)
		return (a.id :: string) < (b.id :: string)
	end)
	sessions.now:set(({
		state = "ready",
		here = "mac",
		-- This Mac; a box that's up (seen a moment ago); one gone quiet.
		devices = {
			{ id = "mac", name = "MacBook Pro", platform = "macos", here = true, online = true },
			-- (Its beat a day ahead: up however long the fixture runs.)
			{ id = "box", name = "Build box", platform = "linux", here = false, online = true, seen = ui.now() / 1000 + 86400 },
			{ id = "old", name = "Old laptop", platform = "macos", here = false, online = false, seen = ui.now() / 1000 - 3 * 3600 },
		},
		projects = {
			{ id = "atlas", name = "Atlas", device = "mac", path = "/tmp/atlas", git = true, github = "acme/atlas" },
			{ id = "notes", name = "Notes", device = "mac", path = "/tmp/notes", git = false },
			-- (Its folder the app's own assets: a picture in it loads.)
			{ id = "kit", name = "Kit", device = "mac", path = "assets", git = false },
			{ id = "borealis", name = "Borealis", device = "box", path = "/srv/borealis", git = true },
			{ id = "attic", name = "Attic", device = "old", path = "/Users/me/attic", git = true },
		},
		chats = chats,
		places = { pinned = {}, sections = {} },
		places_ready = true,
		archived = {},
	}) :: any)
end
seed()
-- This device's connection as the shell hears it (`connection`).
for _, c in { { "connection_reconnecting", "reconnecting" }, { "connection_offline", "offline" }, { "connection_back", nil } } do
	ui.command {
		id = c[1],
		title = `The connection: {c[2] or "back"}`,
		run = function()
			local now = table.clone(sessions.now:peek() :: any)
			now.connection = c[2]
			sessions.now:set(now)
		end,
	}
end
local made_chats = 0
local sessions_stub = sessions :: any
sessions_stub.create = function(req: any)
	made_chats += 1
	local id = `new{made_chats}`
	note(`create:{req.project or "none"}:{req.config and req.config.model or ""}`)
	note(`made:{req.project or "-"}|{req.device or "-"}|{req.branch or "-"}|{req.cwd or "-"}{if req.cloud then "|cloud" else ""}`)
	META[id] = { title = "New chat", project = req.project, model = req.config and req.config.model }
	TRANSCRIPTS[id] = {}
	seed()
	-- On Cloud: made on a machine of its own, its checkout of the repository.
	if req.cloud then
		return settled({ id = id, cwd = "/home/user/atlas" })
	end
	return settled(id)
end
sessions_stub.archive = function(id: string)
	note(`archive:{id}`)
end

-- Where the shell is, moved as the sidebar would.
-- A session whose config has no model (made elsewhere, with none set).
ui.command {
	id = "model_less",
	title = "A session with no model",
	run = function()
		META.s3 = { title = "No model", project = "atlas", access = "full", plan = false }
		TRANSCRIPTS.s3 = { { id = "z1", role = "user", text = "Hi" } }
		seed()
	end,
}
-- A session of long messages (each folds to five lines).
do
	local long_chat = {}
	for n = 1, 4 do
		local lines = {}
		for l = 1, 14 do
			table.insert(lines, `Message {n}, line {l}: a sentence long enough to fill the bubble's width.`)
		end
		table.insert(long_chat, { id = `l{n}`, role = "user", text = table.concat(lines, "\n") })
		table.insert(long_chat, { id = `r{n}`, role = "assistant", text = `Reply {n}.`, model = "Opus" })
	end
	META.s4 = { title = "Long messages", project = "atlas", model = "opus", effort = "high", access = "full", plan = false }
	TRANSCRIPTS.s4 = long_chat
	seed()
end
-- A reply in blocks, as the host lays one out: text, three tools, text,
-- then two tools still running (its open group, at the tail).
local function tool_of(id: string, kind: string, verb: string, target: string): any
	return { id = id, kind = kind, verb = verb, target = target }
end
local GROUPED = {
	{ id = "q1", role = "user", text = "Look into the failing test" },
	{
		id = "w1",
		role = "assistant",
		model = "Opus",
		streaming = true,
		text = "",
		blocks = {
			{ id = "p1", kind = "text", text = "First I'll look around the tests." },
			{ id = "g:x1", kind = "tools", summary = "Ran 2 commands · read 1 file", tools = {
				tool_of("x1", "exec", "Ran", "cargo test"),
				tool_of("x2", "exec", "Ran", "git status"),
				tool_of("x3", "read", "Read", "chat.rs"),
			} },
			{ id = "p2", kind = "text", text = "The second check fails. Reading the code next." },
			{ id = "g:x4", kind = "tools", summary = "Read 2 files", live = true, tools = {
				tool_of("x4", "read", "Read", "lib.rs"),
				tool_of("x5", "read", "Read", "main.rs"),
			} },
		},
	},
}
META.s5 = { title = "Grouped tools", project = "atlas", model = "opus", effort = "high", access = "full", plan = false }
TRANSCRIPTS.s5 = GROUPED
-- Settled replies and a move to another agent, as the shipping app's
-- reply-footer fixture has them: each message's time and Copy under it on
-- hover, the switch seam over the prompt that moved the chat.
local function line(id: string, role: string, minute: number, text: string, extra: any?): any
	local m: any = { id = id, role = role, text = text, at = `Sep 8, 1:{minute + 40} PM`, copy = text, model = "GPT-5.5" }
	if extra then
		for k, v in extra do
			m[k] = v
		end
	end
	return m
end
TRANSCRIPTS.s6 = {
	line("f0", "user", 0, "Profile the render loop and tell me where the frame time goes."),
	line("f1", "assistant", 1, "Layout is 61% of the frame and paint 22%. The rest is present and input.", {
		blocks = {
			{ id = "g:fr", kind = "tools", summary = "Thought process", tools = {
				-- (As the host gives a thought: its words to open to, as prose.)
				{
					id = "fr",
					kind = "thought",
					verb = "Thought process",
					call = "**Reading the frame**\n\nLayout runs twice a frame. Paint is cheap by comparison, and present is a fixed cost whatever the scene holds, so the frame's shape is set by layout alone.\n\n**Where the time goes**\n\nThe second layout pass comes from the sidebar measuring itself after the transcript has settled, so every frame pays for two trees. Moving the sidebar's measure ahead of the transcript's would take one pass out, and the paint cost follows the layout's size, so it would fall too.\n\nI'll profile the present step next, in case the swap chain is waiting on the compositor.",
					prose = true,
				},
			} },
			{ id = "ft", kind = "text", text = "Layout is 61% of the frame and paint 22%. The rest is present and input." },
		},
	}),
	{ id = "switch:1", role = "switch", text = "", from = "Claude Code", to = "Codex", from_mark = "claude", to_mark = "openai" },
	line("f2", "user", 2, "Carry on from here and draft the summary table."),
	line("f3", "assistant", 3, "Here is the summary. Layout dominates on both machines, so that is where the work should go first.\n\nPaint is second, and present is small everywhere."),
	line("f4", "user", 4, "Thanks, ship it."),
	line("f5", "assistant", 5, "Done. The table is in the release notes."),
}
META.s6 = { title = "Switched agents", project = "atlas", model = "sonnet", effort = "medium", access = "full", plan = false }
seed()
ui.command {
	id = "group_grows",
	title = "A third tool joins the open group",
	run = function()
		local n = store.messages:len()
		store.messages:set_key({ n, "blocks", 4, "tools", 3 }, tool_of("x6", "search", "Searched", "fold"))
		store.messages:set_key({ n, "blocks", 4, "summary" }, "Read 2 files · searched 1 time")
	end,
}
-- Tall replies, far taller than the list's estimate for a row not yet
-- measured (as a real agent's are): what a view put back after a reload
-- has to find its place among.
do
	local tall = {}
	for i = 1, 20 do
		local paragraphs = {}
		for k = 1, 10 do
			table.insert(paragraphs, `Reply {i}, paragraph {k}: the layout pass runs first, then paint, then present.`)
		end
		local text = table.concat(paragraphs, "\n\n")
		table.insert(tall, { id = `tu{i}`, role = "user", text = `Turn {i}: tell me more.`, at = `Sep 9, 3:{10 + i} PM`, copy = `Turn {i}: tell me more.` })
		table.insert(tall, {
			id = `ta{i}`,
			role = "assistant",
			text = text,
			model = "Opus",
			at = `Sep 9, 4:{10 + i} PM`,
			copy = text,
			blocks = { { id = "t", kind = "text", text = text } },
		})
	end
	META.s7 = { title = "Tall replies", project = "atlas", model = "opus", effort = "high", access = "full", plan = false }
	TRANSCRIPTS.s7 = tall
	seed()
end
-- The content of the shipping app's compare fixture (a reply's text, a
-- tool group and a code block, then a settled turn), for side by sides.
do
	local fix = "The second check fails: the engine drops before the write lands. The fix:\n\n```rust\nlet ack = engine.write(doc, change).await?;\nack.persisted().await?;\n```\n\nIt passes now, **200 runs** in a row."
	TRANSCRIPTS.s8 = {
		{ id = "cu0", role = "user", text = "Look into the failing test", at = "Sep 8, 1:40 PM", copy = "Look into the failing test" },
		{
			id = "ca0",
			role = "assistant",
			text = "First I'll look around the tests.\n\n" .. fix,
			model = "Opus",
			at = "Sep 8, 1:41 PM",
			copy = "First I'll look around the tests.\n\n" .. fix,
			blocks = {
				{ id = "p1", kind = "text", text = "First I'll look around the tests." },
				{ id = "g:k1", kind = "tools", summary = "Ran 1 command · read 1 file", tools = {
					{ id = "k1", kind = "exec", verb = "Ran", target = "cargo test" },
					{ id = "k2", kind = "read", verb = "Read", target = "chat.rs" },
				} },
				{ id = "p2", kind = "text", text = fix },
			},
			-- Compact (as the host lays it out): its work folded, done in
			-- 12 s, the reply it ends on shown.
			compact = {
				{ id = "work", kind = "work", summary = "Worked for 12s", tools = {
					{ id = "k1", kind = "exec", verb = "Ran", target = "cargo test" },
					{ id = "k2", kind = "read", verb = "Read", target = "chat.rs" },
				}, blocks = {
					{ id = "p1", kind = "text", text = "First I'll look around the tests." },
					{ id = "g:k1", kind = "tools", summary = "Ran 1 command · read 1 file", tools = {
						{ id = "k1", kind = "exec", verb = "Ran", target = "cargo test" },
						{ id = "k2", kind = "read", verb = "Read", target = "chat.rs" },
					} },
				} },
				{ id = "p2", kind = "text", text = fix },
			},
		},
		{ id = "cu1", role = "user", text = "Thanks, ship it.", at = "Sep 8, 1:42 PM", copy = "Thanks, ship it." },
		{ id = "ca1", role = "assistant", text = "Done. The release workflow is running.", model = "Opus", at = "Sep 8, 1:43 PM", copy = "Done. The release workflow is running.", blocks = { { id = "t", kind = "text", text = "Done. The release workflow is running." } } },
	}
	META.s8 = { title = "Fix the failing test", project = "atlas", model = "opus", effort = "high", access = "full", plan = false }
	-- An agent from the ACP registry: a chat moved to it, named by its
	-- harness alone everywhere (its mark found by that: `agent_icons`).
	TRANSCRIPTS.s9 = {
		line("g0", "user", 0, "Look at the flaky upload test."),
		line("g1", "assistant", 1, "It races the progress callback; I'll pin the order."),
		{ id = "switch:9", role = "switch", text = "", from = "Devin", to = "Factory Droid", from_mark = "devin", to_mark = "acp.factory-droid" },
		line("g2", "user", 2, "hello"),
	}
	META.s9 = { title = "Moved to Factory Droid", project = "atlas", model = "droid", effort = "high", access = "full", plan = false, agent = "acp.factory-droid" }
	seed()
end
-- A reply as the engine starts one: no blocks yet (its first part an
-- empty thought), then its text, then done with its time.
ui.command {
	id = "bare_reply",
	title = "A reply starts with nothing to show",
	run = function()
		store.messages:push {
			id = "e1",
			role = "assistant",
			text = "",
			model = "Opus",
			streaming = true,
			started = ui.now() / 1000,
			blocks = {},
		}
	end,
}
-- The files pane's "Add to chat" on two of the project's files.
ui.command {
	id = "add_to_chat",
	title = "Add two project files to the chat",
	run = function()
		require("@app/screens/chat/shared").add_to_chat({ "src/lib.rs", "docs/notes.md" }, "/tmp/atlas")
	end,
}
-- Files dropped from outside: a GIF and a HEIC (no thumbnail zgx can
-- draw yet), and a PNG of the app's own.
ui.command {
	id = "drop_pictures",
	title = "Drop a GIF, a HEIC and a PNG",
	run = function()
		require("@app/screens/chat/shared").attach_files({
			{ name = "spin.gif", path = "assets/spin.gif" },
			{ name = "photo.heic", path = "assets/photo.heic" },
			{ name = "zgx.png", path = "assets/favicons/zgx.png" },
		})
	end,
}
-- Two review comments left on files in the files pane.
ui.command {
	id = "leave_comments",
	title = "Leave two review comments",
	run = function()
		local shared = require("@app/screens/chat/shared")
		local id = shared.attach_comment({ file = "src/app.rs", line = 12, body = "Rename this." })
		shared.attach_comment({ file = "docs/guide.md", line = 3, body = "Typo here.\nAnd here." })
		shared.update_comment(id, "Rename this.\nIt reads badly.")
	end,
}
-- A review comment as the files pane leaves it (screens/files/comments:
-- `{ path, line, body, root }`), then carried two lines down by edits.
ui.command {
	id = "comment_from_files",
	title = "Leave a comment from the files pane",
	run = function()
		local shared = require("@app/screens/chat/shared")
		local id = shared.attach_comment({ path = "/proj/src/app.rs", line = 12, body = "Rename this.", root = "/proj" })
		ui.state("test_comment_id", ""):set(id)
	end,
}
-- Two comments a plugin left on a diff (Git's Changes, through
-- `zeron.compose_comment`; screens/plugins hands them on as this does): a
-- range of the changed file's lines with its code, and an old line.
-- `test_plugin_comments` says which of the plugin's are still carried.
ui.command {
	id = "plugin_diff_comments",
	title = "A plugin leaves two comments on a diff",
	run = function()
		local shared = require("@app/screens/chat/shared")
		shared.attach_comment({ file = "src/graph.rs", line = 10, to = 12, side = "R", snippet = " fn lanes() {}\n-fn rows() {}\n+fn rows(n: usize) {}", body = "Keep the old name.", plugin = "git", key = "k1", session = "s2" })
		shared.attach_comment({ file = "src/old.rs", line = 4, side = "L", body = "Why did this go?", plugin = "git", key = "k2", session = "s2" })
	end,
}
local plugin_comments = ui.state("test_plugin_comments", "")
ui.effect(function()
	local shared = require("@app/screens/chat/shared")
	local keys: { string } = {}
	for key, c in shared.plugin_comments().git or {} do
		table.insert(keys, `{key}@{c.session}`)
	end
	table.sort(keys)
	plugin_comments:set(table.concat(keys, ","))
end)
ui.command {
	id = "comment_moves",
	title = "The comment is carried two lines down",
	run = function()
		local shared = require("@app/screens/chat/shared")
		shared.move_comment(ui.state("test_comment_id", ""):peek(), 14)
	end,
}
-- One long reply (about 400 top-level blocks: paragraphs, lists, code,
-- quotes), as a long agent answer comes: what a message's own windowing is
-- measured on.
local function long_reply_text(): string
	local parts: { string } = {}
	for i = 1, 100 do
		table.insert(parts, `## Part {i}`)
		table.insert(parts, `Part {i} explains how the render loop spends its frame: layout first, then paint, then present. The numbers below come from a trace of section {i}, read over several runs so one slow frame doesn't skew them.`)
		table.insert(parts, `- layout takes **{40 + i % 20}%**\n- paint takes {20 + i % 10}%\n- present takes the rest`)
		table.insert(parts, "```rust\nfn part_" .. i .. "(frame: &Frame) -> Duration {\n    frame.layout + frame.paint\n}\n```")
	end
	return table.concat(parts, "\n\n")
end
ui.command {
	id = "long_reply",
	title = "A long reply arrives",
	run = function()
		local text = long_reply_text()
		store.messages:push {
			id = "long1",
			role = "assistant",
			text = text,
			model = "Opus",
			at = "Sep 8, 2:00 PM",
			copy = text,
			blocks = { { id = "t", kind = "text", text = text } },
		}
	end,
}
-- The shell's ⌘/: the composer's model picker opened.
ui.command {
	id = "open_model_picker",
	title = "Open the model picker",
	run = function()
		note(`model_picker:{tostring(require("@app/screens/chat").open_model_picker())}`)
	end,
}
-- A picture of a project on this computer (Kit's), added to the chat.
ui.command {
	id = "add_picture_to_chat",
	title = "Add a picture of a project here to the chat",
	run = function()
		require("@app/screens/chat/shared").add_to_chat({ "favicons/zgx.png" }, "assets")
	end,
}
-- A picture of a project on another computer (the build box's).
ui.command {
	id = "add_far_picture_to_chat",
	title = "Add a picture of a project elsewhere to the chat",
	run = function()
		require("@app/screens/chat/shared").add_to_chat({ "ui/logo.png" }, "/srv/borealis")
	end,
}
-- A turn with emoji and an inline code span long enough to wrap: what a
-- look at colour glyphs and wrapped code corners wants.
ui.command {
	id = "emoji_turn",
	title = "A turn with emoji and wrapped code",
	run = function()
		local reply = "Shipped 🚀 and the tests are green ✅. Dark text keeps its colours: 👍🏽 ❤️ 🇯🇵 👩‍💻 🌚.\n\n"
			.. "The fix lives in `crates/zgx-script/src/reactive/ownership_of_effect_runs_and_their_timers.rs`, which a narrow column wraps mid-span, and `short` stays one piece."
		store.messages:push { id = "eu1", role = "user", text = "Did it ship? 🤞 Tell me 😅", at = "Sep 8, 1:50 PM", copy = "Did it ship?" }
		store.messages:push {
			id = "ea1",
			role = "assistant",
			text = reply,
			model = "Opus",
			at = "Sep 8, 1:50 PM",
			copy = reply,
			blocks = { { id = "t", kind = "text", text = reply } },
		}
	end,
}
-- A turn whose prompt mentions a file and whose reply links files (one at
-- a line, one outside the chat's folder, one on the web), in a chat whose
-- folder is /work/atlas; what opens a file there noted (`open_file:`).
local panel_stub: any = (require :: any)("@app/screens/panel")
panel_stub.open_file = function(session: any, path: string)
	note(`open_file:{if type(session) == "table" then session.id else session}:{path}`)
end
-- The app taking web links (ds/links, as the shell sets it for "Open links
-- in Zeron"): each link opened is noted, nothing else happens.
ui.command {
	id = "set_opener",
	title = "Take web links",
	run = function()
		local Links = require("@ds/links")
		Links.opener = function(url: string): boolean
			note(`opener:{url}`)
			return true
		end
	end,
}
-- "Open links in" as the shell has it (`test_open_links`: "zeron" or
-- "external"), the panel's browser tab noting what it's sent (`panel:`); a
-- chat's Zeron link and Reveal in Finder noted as asked (ds/links' menus).
local open_links = ui.state("test_open_links", "zeron")
ui.command {
	id = "set_links",
	title = "Links as the shell sets them",
	run = function()
		local Links = require("@ds/links")
		Links.panel = function(url: string): boolean
			note(`panel:{url}`)
			return true
		end
		Links.in_app = function(): boolean
			return open_links:peek() == "zeron"
		end
		Links.opener = function(url: string): boolean
			return open_links:peek() == "zeron" and (Links.panel :: any)(url)
		end
	end,
}
sessions_stub.link = function(id: string): any
	return settled(`zeron://open/chat/{id}?workspace=w1`)
end
local files_stub = (require :: any)("@app/screens/files/api")
files_stub.reveal = function(target: any, path: string): any
	note(`reveal:{target.chat}:{target.root}:{path}`)
	return settled(nil)
end
ui.command {
	id = "link_turn",
	title = "A turn with file links",
	run = function()
		local s = table.clone(store.session:peek() or {})
		s.cwd = "/work/atlas"
		store.session:set(s)
		store.messages:push {
			id = "lu1",
			role = "user",
			text = "Look at [chat.rs](zeron-file:src/chat.rs) please, as in [Long chat](zeron-chat:s1), and [the site](https://example.com/)",
			at = "Sep 8, 1:50 PM",
			copy = "Look at chat.rs please",
		}
		local reply = "The fix is in [the parser](src/parse.rs#L12) and [the host](/work/atlas/src/main.rs:40). "
			.. "Not [this one](/etc/hosts.conf), nor [the docs](https://example.com/a.rs). "
			.. "PR: **https://github.com/zeronsh/example/pull/1**."
		store.messages:push {
			id = "la1",
			role = "assistant",
			text = reply,
			model = "Opus",
			at = "Sep 8, 1:50 PM",
			copy = reply,
			blocks = { { id = "t", kind = "text", text = reply } },
		}
	end,
}
-- A live turn whose tools include a command of five lines (a shell script
-- run as one call), a tool named only, and a thought: as the user's turn
-- had it. `call` is the whole command, as the host gives it.
local SCRIPT = table.concat({
	"echo '--- extension-ish in main'; git grep -n 'extension' -- '*.rs' | head -20",
	"echo '--- plugin dirs'; ls -d crates/*plugin* apps/*/plugins 2>/dev/null",
	"echo '--- branches'; git branch -a | grep -i plugin",
	"echo '--- PRs'; gh pr list --search plugin --state all --limit 10",
	"echo '--- issues'; gh issue list --search plugin --state all --limit 10",
}, "\n")
ui.command {
	id = "script_turn",
	title = "A live turn that ran a five-line command",
	run = function()
		local one = string.gsub(SCRIPT, "%s+", " ")
		store.messages:push {
			id = "st1",
			role = "assistant",
			text = "",
			model = "Opus",
			streaming = true,
			started = ui.now() / 1000,
			blocks = {
				{ id = "g:st", kind = "tools", summary = "Thought process · ran 1 command · called 1 tool", live = true, tools = {
					{ id = "st-x", kind = "exec", verb = "Ran", target = one, call = SCRIPT },
					{ id = "st-t", kind = "exec", verb = "Used", target = "ToolSearch" },
					{ id = "st-r", kind = "thought", verb = "Thought" },
				} },
			},
		}
	end,
}
-- The same, with the command's newlines left in its line (what the host
-- sent before it made each tool's line one line): the row must still hold
-- it to one line.
ui.command {
	id = "script_turn_raw",
	title = "A live turn whose command's line has its newlines",
	run = function()
		store.messages:push {
			id = "st2",
			role = "assistant",
			text = "",
			model = "Opus",
			streaming = true,
			started = ui.now() / 1000,
			blocks = {
				{ id = "g:st2", kind = "tools", summary = "Thought process · ran 1 command · called 1 tool", live = true, tools = {
					{ id = "st2-x", kind = "exec", verb = "Ran", target = SCRIPT },
					{ id = "st2-t", kind = "exec", verb = "Used", target = "ToolSearch" },
					{ id = "st2-r", kind = "thought", verb = "Thought" },
				} },
			},
		}
	end,
}
-- ⌘F's find in the chat shown (what the shell's Find runs).
ui.command {
	id = "find_open",
	title = "Find in the chat",
	run = function()
		require("@app/screens/chat/find").open()
	end,
}
-- A live turn still thinking (its thought being written, as the host gives
-- it: live), then the thought settling.
ui.command {
	id = "thinking_turn",
	title = "A live turn, thinking",
	run = function()
		store.messages:push {
			id = "th1",
			role = "assistant",
			text = "",
			model = "Opus",
			streaming = true,
			started = ui.now() / 1000,
			blocks = {
				{ id = "g:th", kind = "tools", summary = "Thought process", live = true, tools = {
					{ id = "th-r", kind = "thought", verb = "Thought process", call = "Weighing the parser first.", prose = true, live = true },
				} },
			},
		}
	end,
}
-- The thought going on (its words past the fold, the latest still coming).
ui.command {
	id = "thought_grows",
	title = "The thought grows past the fold",
	run = function()
		local n = store.messages:len()
		store.messages:set_key(
			{ n, "blocks", 1, "tools", 1, "call" },
			"Weighing the parser first.\n\n**The parser**\n\nIt reads the whole file on every keystroke, and the file is the transcript, so the cost grows with the chat. A cursor into the last block would read only what changed, which is what the host already keeps.\n\n**The host**\n\nIt sends each block whole as it grows, so the parser's input is the bottleneck twice over. The last line is the freshest thought."
		)
	end,
}
ui.command {
	id = "thought_settles",
	title = "The thought settles",
	run = function()
		local n = store.messages:len()
		store.messages:set_key({ n, "blocks", 1, "tools", 1, "live" }, false)
		store.messages:set_key({ n, "blocks", 1, "live" }, false)
	end,
}
ui.command {
	id = "bare_reply_text",
	title = "Its text arrives",
	run = function()
		local n = store.messages:len()
		store.messages:set_key({ n, "blocks" }, { { id = "t", kind = "text", text = "The moon knocked. I opened the window. It came home." } })
		store.messages:set_key({ n, "text" }, "The moon knocked. I opened the window. It came home.")
	end,
}
ui.command {
	id = "bare_reply_done",
	title = "It is done",
	run = function()
		local n = store.messages:len()
		store.messages:set_key({ n, "streaming" }, false)
		store.messages:set_key({ n, "at" }, "Sep 8, 2:00 PM")
		store.messages:set_key({ n, "copy" }, "The moon knocked. I opened the window. It came home.")
	end,
}
-- (A session dropped from the sidebar on the chat: s1 mentioned.)
ui.command {
	id = "mention_s9",
	title = "Mention s9",
	run = function()
		require("@app/screens/chat/shared").mention_chat("s9")
	end,
}
ui.command {
	id = "mention_s1",
	title = "Mention s1",
	run = function()
		require("@app/screens/chat/shared").mention_chat("s1")
	end,
}
for _, id in { "s1", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9" } do
	ui.command {
		id = `go_{id}`,
		title = `Open {id}`,
		run = function()
			route.open_session(id)
		end,
	}
end
ui.command {
	id = "go_home",
	title = "A new chat",
	run = function()
		route.new_chat("atlas")
	end,
}

-- The catalog changing: an agent arriving with its models, and none at all
-- (signed out of every agent, or still looking).
ui.command {
	id = "catalog_grows",
	title = "Another agent arrives",
	run = function()
		local c = store.catalog:peek()
		local harnesses: { any } = table.clone(c.harnesses)
		local models: { any } = table.clone(c.models)
		table.insert(harnesses, { id = "codex", label = "Codex", mark = "openai" })
		table.insert(models, { value = "gpt", label = "GPT-5.5", harness = "codex", efforts = EFFORT, effort = "medium" })
		store.catalog:set { state = "ready", harnesses = harnesses, models = models }
	end,
}
-- An agent from the ACP registry in the catalog, as the host lists it: its
-- mark its harness (no icon carried: found by that).
ui.command {
	id = "catalog_registry",
	title = "Factory Droid arrives",
	run = function()
		local c = store.catalog:peek()
		local harnesses: { any } = table.clone(c.harnesses)
		local models: { any } = table.clone(c.models)
		table.insert(harnesses, { id = "acp.factory-droid", label = "Factory Droid", mark = "acp.factory-droid" })
		table.insert(models, { value = "droid", label = "Droid Auto", harness = "acp.factory-droid", efforts = EFFORT, effort = "medium" })
		store.catalog:set { state = "ready", harnesses = harnesses, models = models }
	end,
}
ui.command {
	id = "catalog_empty",
	title = "No agents",
	run = function()
		store.catalog:set { state = "ready", harnesses = {}, models = {} }
	end,
}

-- A plugin putting words into the composer (`zeron.compose.insert`).
ui.command {
	id = "insert_words",
	title = "Insert words into the composer",
	run = function()
		store.insert("src/chat.rs")
	end,
}

-- The session page made again, as a hot reload or a saved template does.
local remade = ui.state("remade", 0)
ui.command {
	id = "remake_session",
	title = "Make the session page again",
	run = function()
		remade:add(1)
	end,
}

-- The shell, as small as can hold the page: the route's page, and the
-- composer over it.
return ui.overlay {
	w_full = true,
	h_full = true,
	ui.column {
		w_full = true,
		h_full = true,
		pt = ui.raw(38, "the title bar's height"),
		ui.presence(function(): string
			-- (Keyed by the remake count too: the test's stand-in for a reload.)
			return `{route.current:get().name}:{remade:get()}`
		end, function(key: string): Node
			local name = string.match(key, "^(%a+)")
			if name == "session" then
				return chat.Session()
			end
			return chat.Home()
		end, { w_full = true, grow = 1, min_h = 0 }),
	},
	chat.Composer(),
	-- A link's context menu, as the shell hangs it (ds/menu_at).
	require("@ds/menu_at").Host {},
	-- What's said for a moment, as the shell has it (ds/toast).
	ds.toast.Host {},
	-- In the title bar's strip, what a drag may come from: the files pane's
	-- files (their paths), and something else (a sidebar section).
	ui.row {
		absolute = true,
		left = ui.raw(80, "past the traffic lights"),
		top = ui.raw(6, "in the title bar's strip"),
		gap = "$space.2",
		ui.div {
			id = "fixture-drag-files",
			w = 24,
			h = 24,
			bg = "$hover",
			on_drag = function(): any
				return { tree_files = { 1 }, paths = { "src/lib.rs" }, root = "/tmp/atlas" }
			end,
		},
		ui.div {
			id = "fixture-drag-section",
			w = 24,
			h = 24,
			bg = "$hover",
			on_drag = function(): any
				return { section = "later" }
			end,
		},
	},
}
